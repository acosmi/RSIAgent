//! Resumable cleanup derived from the authoritative tombstone, watermark and typed edges.

use super::{Store, internal};
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{ConnectOptions, Connection, Row, sqlite::SqliteConnectOptions};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::OwnedRwLockWriteGuard;

pub const REVOKE_TOMBSTONE_SCHEMA: &str = "rsia.revoke_tombstone.v1";
pub const MAX_CLEANUP_EDGE_PAGE: usize = 1_000;
pub const BACKUP_SCHEMA: &str = "rsia.backup.v2";

/// A single validated component beneath this Store's fixed backup root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BackupName(String);

impl BackupName {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty()
            || value.len() > 64
            || !value.as_bytes()[0].is_ascii_alphanumeric()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::Invalid("invalid backup relative name".into()));
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BackupName {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupPhase {
    Preparing,
    Prepared,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupDurability {
    Verified,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupPhysicalIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
}

/// Private physical provenance; public status projects only phase and result references.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupProgressV2 {
    pub phase: BackupPhase,
    pub durability: BackupDurability,
    snapshot_generation: u64,
    snapshot_lease_token: Option<String>,
    source: BackupPhysicalIdentity,
    database: BackupPhysicalIdentity,
    parent: BackupPhysicalIdentity,
    temporary: BackupPhysicalIdentity,
    temporary_name: String,
    manifest_utf8: Option<String>,
    manifest_sha256: Option<String>,
}

impl BackupProgressV2 {
    pub fn manifest_sha256(&self) -> Option<&str> {
        self.manifest_sha256.as_deref()
    }

    pub fn published(&self, durability: BackupDurability) -> Box<Self> {
        let mut progress = self.clone();
        progress.phase = BackupPhase::Published;
        progress.durability = durability;
        Box::new(progress)
    }

    pub(crate) fn valid_wire(&self) -> bool {
        let owned_name = self.temporary_name.starts_with('.')
            && !self.temporary_name.contains('/')
            && !self.temporary_name.contains('\\');
        owned_name
            && self.snapshot_generation > 0
            && self.snapshot_lease_token.is_some()
            && match self.phase {
                BackupPhase::Preparing => {
                    self.manifest_utf8.is_none()
                        && self.manifest_sha256.is_none()
                        && self.durability == BackupDurability::Unverified
                }
                BackupPhase::Prepared | BackupPhase::Published => {
                    self.manifest_utf8
                        .as_ref()
                        .zip(self.manifest_sha256.as_ref())
                        .is_some_and(|(raw, digest)| evo_core::hash(raw.as_bytes()) == *digest)
                        && (self.phase != BackupPhase::Prepared
                            || self.durability == BackupDurability::Unverified)
                }
            }
    }
}

/// An authorized operation with no filesystem effects yet.
pub struct BackupPreparation {
    store: Store,
    name: BackupName,
}

/// Owns the temporary directory and the existing blob exclusion lock.
pub struct PreparingBackup {
    store: Store,
    name: BackupName,
    root: PathBuf,
    parent: PathBuf,
    temp: PathBuf,
    progress: BackupProgressV2,
    guard: Option<OwnedRwLockWriteGuard<()>>,
    #[cfg(test)]
    pause_before_copy: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    #[cfg(test)]
    fail_parent_sync: bool,
}

/// A complete snapshot whose preparation lock is retained until its receipt commits.
pub struct PreparedBackup {
    owned: PreparingBackup,
}

/// Publication does not borrow the source pool; the caller retains its new Session.
pub struct BackupPublicationTask {
    owned: PreparingBackup,
}

#[derive(Debug, Clone)]
pub struct BackupPublication {
    pub relative_name: BackupName,
    pub manifest_sha256: String,
    pub durability: BackupDurability,
    directory: PathBuf,
}

impl BackupPublication {
    /// Physical output for trusted local callers; never included in public status.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl BackupPreparation {
    pub(crate) fn new(store: Store, name: BackupName) -> Self {
        Self { store, name }
    }

    /// Direct Store backups create no management job, input, or audit row.
    pub async fn complete(self) -> Result<BackupPublication> {
        let store = self.store.clone();
        let prepared = self
            .begin(0, None)
            .await?
            .prepare(
                Instant::now() + Duration::from_secs(30),
                Arc::new(AtomicBool::new(false)),
            )
            .await?;
        let session = match store.session().await {
            Ok(session) => session,
            Err(error) => {
                prepared.discard().await?;
                return Err(error);
            }
        };
        let result = prepared.release().publish().await;
        match (result, session.commit().await) {
            (Ok(mut publication), Err(_)) => {
                publication.durability = BackupDurability::Unverified;
                Ok(publication)
            }
            (result, _) => result,
        }
    }

    pub async fn begin(
        self,
        generation: u64,
        lease_token: Option<String>,
    ) -> Result<PreparingBackup> {
        let guard = self.store.blob_lock.clone().write_owned().await;
        // A successful pool borrow precedes even the first filesystem operation.
        let connection = self.store.pool.acquire().await.map_err(internal)?;
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            return Err(Error::Invalid("backup unsupported platform".into()));
        }
        let root = backup_source_root(&self.store)?;
        let source = backup_identity(&root, true)?;
        let database_path = self
            .store
            .pool
            .connect_options()
            .get_filename()
            .to_path_buf();
        let database = backup_identity(&database_path, false)?;
        let parent = root.join("backups");
        match std::fs::symlink_metadata(&parent) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_backup_dir(&parent)?;
                sync_directory(root.clone()).await?;
            }
            Err(error) => return Err(internal(error)),
        }
        let parent_identity = backup_identity(&parent, true)?;
        check_backup_ancestors(&parent)?;
        let destination = parent.join(self.name.as_str());
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(Error::Conflict("backup destination already exists".into()));
        }
        evo_core::now()?;
        let temporary_name = format!(".{}.tmp-{}", self.name.as_str(), uuid::Uuid::new_v4());
        let temp = parent.join(&temporary_name);
        create_backup_dir(&temp)?;
        let temporary = match backup_identity(&temp, true) {
            Ok(identity) => identity,
            Err(error) => {
                std::fs::remove_dir_all(&temp).map_err(internal)?;
                return Err(error);
            }
        };
        drop(connection);
        Ok(PreparingBackup {
            store: self.store,
            name: self.name,
            root,
            parent,
            temp,
            progress: BackupProgressV2 {
                phase: BackupPhase::Preparing,
                durability: BackupDurability::Unverified,
                snapshot_generation: generation,
                snapshot_lease_token: lease_token,
                source,
                database,
                parent: parent_identity,
                temporary,
                temporary_name,
                manifest_utf8: None,
                manifest_sha256: None,
            },
            guard: Some(guard),
            #[cfg(test)]
            pause_before_copy: None,
            #[cfg(test)]
            fail_parent_sync: false,
        })
    }

    /// Read-only recovery. Snapshot claims belong to the old Running image,
    /// which necessarily predates the later Prepared receipt.
    pub async fn verify(
        &self,
        progress: &BackupProgressV2,
        ctx: &Context,
        job: &super::ManagementJobV2,
        private_input_utf8: &str,
    ) -> Result<Option<BackupPublication>> {
        let root = backup_source_root(&self.store)?;
        let parent = root.join("backups");
        if backup_identity(&root, true)? != progress.source
            || backup_identity(self.store.pool.connect_options().get_filename(), false)?
                != progress.database
            || backup_identity(&parent, true)? != progress.parent
        {
            return Err(Error::Conflict("backup source identity differs".into()));
        }
        check_backup_ancestors(&parent)?;
        let destination = parent.join(self.name.as_str());
        if !destination.try_exists().map_err(internal)? {
            return Ok(None);
        }
        if backup_identity(&destination, true)? != progress.temporary {
            return Err(Error::Conflict(
                "backup publication ownership differs".into(),
            ));
        }
        let digest = progress
            .manifest_sha256
            .as_ref()
            .ok_or_else(|| Error::Conflict("backup manifest digest missing".into()))?;
        let proof = async {
            let raw = progress
                .manifest_utf8
                .as_ref()
                .ok_or_else(|| Error::Conflict("backup manifest proof missing".into()))?;
            backup_identity(&destination.join("backup-manifest.json"), false)?;
            let actual = tokio::fs::read(destination.join("backup-manifest.json"))
                .await
                .map_err(internal)?;
            if actual != raw.as_bytes() || evo_core::hash(&actual) != *digest {
                return Err(Error::Conflict("backup manifest proof differs".into()));
            }
            let manifest: BackupManifest =
                serde_json::from_value(parse_strict_json(raw)?).map_err(internal)?;
            verify_backup_members(&destination, &manifest).await?;
            let options = SqliteConnectOptions::new()
                .filename(destination.join("rsia.sqlite3"))
                .read_only(true)
                .disable_statement_logging();
            let mut connection = sqlx::SqliteConnection::connect_with(&options)
                .await
                .map_err(internal)?;
            let result = async {
                sqlx::query("BEGIN")
                    .execute(&mut connection)
                    .await
                    .map_err(internal)?;
                let saved = sqlx::query(
                    "SELECT owner,body FROM objects
                     WHERE namespace=? AND kind='job' AND id=?",
                )
                .bind(ctx.namespace())
                .bind(&job.id)
                .fetch_one(&mut connection)
                .await
                .map_err(internal)?;
                let saved_job = super::decode_management_job(
                    saved.try_get::<&str, _>("body").map_err(internal)?,
                )?;
                let super::VersionedManagementJob::V2(saved_job) = saved_job else {
                    return Err(Error::Conflict(
                        "backup snapshot job version differs".into(),
                    ));
                };
                if saved.try_get::<String, _>("owner").map_err(internal)? != ctx.actor()
                    || saved_job.id != job.id
                    || saved_job.operation != "backup.create"
                    || saved_job.owner_actor != job.owner_actor
                    || saved_job.owner_role != job.owner_role
                    || saved_job.request_key != job.request_key
                    || saved_job.payload_digest != job.payload_digest
                    || saved_job.private_input_ref != job.private_input_ref
                    || saved_job.state != super::ManagementJobState::Running
                    || saved_job.generation != progress.snapshot_generation
                    || saved_job.lease_token != progress.snapshot_lease_token
                    || saved_job.backup.as_ref().is_none_or(|saved| {
                        saved.phase != BackupPhase::Preparing
                            || saved.temporary != progress.temporary
                            || saved.source != progress.source
                            || saved.parent != progress.parent
                            || saved.database != progress.database
                    })
                {
                    return Err(Error::Conflict("backup snapshot claim differs".into()));
                }
                let saved_input: String = sqlx::query_scalar(
                    "SELECT body FROM objects
                     WHERE namespace=? AND kind='artifact' AND id=? AND owner=?",
                )
                .bind(ctx.namespace())
                .bind(&job.private_input_ref)
                .bind(ctx.actor())
                .fetch_one(&mut connection)
                .await
                .map_err(internal)?;
                if saved_input != private_input_utf8 {
                    return Err(Error::Conflict("backup snapshot input differs".into()));
                }
                if fingerprint(&load_backup_migrations(&mut connection).await?)?
                    != fingerprint(&manifest.migrations)?
                    || fingerprint(&load_backup_watermarks(&mut connection).await?)?
                        != fingerprint(&manifest.watermarks)?
                {
                    return Err(Error::Conflict("backup snapshot metadata differs".into()));
                }
                Ok(())
            }
            .await;
            let closed = connection.close().await.map_err(internal);
            result?;
            closed
        }
        .await;
        Ok(Some(BackupPublication {
            relative_name: self.name.clone(),
            manifest_sha256: digest.clone(),
            directory: destination,
            durability: if proof.is_ok() && progress.phase == BackupPhase::Published {
                progress.durability
            } else {
                BackupDurability::Unverified
            },
        }))
    }
}

impl PreparingBackup {
    pub fn progress(&self) -> Box<BackupProgressV2> {
        Box::new(self.progress.clone())
    }

    pub async fn discard(self) -> Result<()> {
        if backup_identity(&self.temp, true)? != self.progress.temporary {
            return Err(Error::Conflict("backup temporary ownership differs".into()));
        }
        tokio::fs::remove_dir_all(&self.temp)
            .await
            .map_err(internal)
    }

    pub async fn prepare(
        mut self,
        deadline: Instant,
        stopped: Arc<AtomicBool>,
    ) -> Result<PreparedBackup> {
        let result = self.snapshot(deadline, stopped).await;
        match result {
            Ok(()) => Ok(PreparedBackup { owned: self }),
            Err(error) => {
                self.discard().await?;
                Err(error)
            }
        }
    }

    async fn snapshot(&mut self, deadline: Instant, stopped: Arc<AtomicBool>) -> Result<()> {
        let mut connection = self.store.pool.acquire().await.map_err(internal)?;
        if backup_identity(&self.root, true)? != self.progress.source
            || backup_identity(&self.parent, true)? != self.progress.parent
            || backup_identity(&self.temp, true)? != self.progress.temporary
            || backup_identity(self.store.pool.connect_options().get_filename(), false)?
                != self.progress.database
        {
            return Err(Error::Conflict(
                "backup preparation identity differs".into(),
            ));
        }
        let database_path = self.temp.join("rsia.sqlite3");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&database_path).map_err(internal)?;
        {
            let mut handle = connection.lock_handle().await.map_err(internal)?;
            handle.set_progress_handler(1_000, move || {
                Instant::now() < deadline && !stopped.load(Ordering::Acquire)
            });
        }
        let vacuum = sqlx::query("VACUUM INTO ?")
            .bind(database_path.to_string_lossy().as_ref())
            .execute(&mut *connection)
            .await;
        // Await the actual SQLite result and clear the handler on every result.
        connection
            .lock_handle()
            .await
            .map_err(internal)?
            .remove_progress_handler();
        vacuum.map_err(internal)?;
        let migrations = load_backup_migrations(&mut connection).await?;
        let watermarks = load_backup_watermarks(&mut connection).await?;
        drop(connection);
        #[cfg(test)]
        if let Some((reached, resume)) = &self.pause_before_copy {
            reached.notify_one();
            resume.notified().await;
        }
        sync_file(&database_path).await?;
        let database = file_entry(&database_path, "rsia.sqlite3").await?;
        let mut blobs = copy_backup_blobs(&self.store, &self.temp).await?;
        blobs.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = BackupManifest {
            schema_version: BACKUP_SCHEMA.into(),
            complete: true,
            database,
            blobs,
            migrations,
            watermarks,
            created_at: evo_core::now()?,
        };
        let raw = serde_json::to_string_pretty(&manifest).map_err(internal)?;
        write_backup_member(&self.temp.join("backup-manifest.json"), raw.as_bytes()).await?;
        sync_directory(self.temp.clone()).await?;
        self.progress.phase = BackupPhase::Prepared;
        self.progress.manifest_sha256 = Some(evo_core::hash(raw.as_bytes()));
        self.progress.manifest_utf8 = Some(raw);
        Ok(())
    }
}

impl PreparedBackup {
    pub fn progress(&self) -> Box<BackupProgressV2> {
        self.owned.progress()
    }
    pub async fn discard(self) -> Result<()> {
        self.owned.discard().await
    }
    pub fn release(mut self) -> BackupPublicationTask {
        self.owned.guard.take();
        BackupPublicationTask { owned: self.owned }
    }
}

impl BackupPublicationTask {
    pub fn progress(&self) -> Box<BackupProgressV2> {
        self.owned.progress()
    }
    pub async fn discard(self) -> Result<()> {
        self.owned.discard().await
    }

    /// The caller keeps its publication Session alive across this entire future.
    pub async fn publish(self) -> Result<BackupPublication> {
        let owned = self.owned;
        let destination = owned.parent.join(owned.name.as_str());
        let checked = async {
            if backup_source_root(&owned.store)? != owned.root {
                return Err(Error::Conflict("backup source identity differs".into()));
            }
            check_backup_ancestors(&owned.parent)?;
            if backup_identity(&owned.root, true)? != owned.progress.source
                || backup_identity(owned.store.pool.connect_options().get_filename(), false)?
                    != owned.progress.database
                || backup_identity(&owned.parent, true)? != owned.progress.parent
                || backup_identity(&owned.temp, true)? != owned.progress.temporary
            {
                return Err(Error::Conflict(
                    "backup publication identity differs".into(),
                ));
            }
            backup_identity(&owned.temp.join("backup-manifest.json"), false)?;
            let raw = tokio::fs::read(owned.temp.join("backup-manifest.json"))
                .await
                .map_err(internal)?;
            if owned.progress.manifest_utf8.as_deref().map(str::as_bytes) != Some(raw.as_slice())
                || owned.progress.manifest_sha256.as_deref() != Some(evo_core::hash(&raw).as_str())
            {
                return Err(Error::Conflict("backup prepared manifest differs".into()));
            }
            publish_backup_directory(&owned.temp, &destination)
        }
        .await;
        if let Err(error) = checked {
            owned.discard().await?;
            return Err(error);
        }
        #[cfg(test)]
        let synced = if owned.fail_parent_sync {
            Err(Error::Internal)
        } else {
            sync_directory(owned.parent.clone()).await
        };
        #[cfg(not(test))]
        let synced = sync_directory(owned.parent.clone()).await;
        let durability = if synced.is_ok() {
            BackupDurability::Verified
        } else {
            BackupDurability::Unverified
        };
        Ok(BackupPublication {
            relative_name: owned.name.clone(),
            manifest_sha256: owned
                .progress
                .manifest_sha256
                .clone()
                .ok_or(Error::Internal)?,
            durability,
            directory: destination,
        })
    }
}

fn backup_source_root(store: &Store) -> Result<PathBuf> {
    let supplied = if store.root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &store.root
    };
    let absolute = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir().map_err(internal)?.join(supplied)
    };
    // Inspect the original spelling before canonicalization can erase an
    // alias, including one followed by `..`.
    let mut prefix = PathBuf::new();
    for component in absolute.components() {
        prefix.push(component.as_os_str());
        if std::fs::symlink_metadata(&prefix)
            .map_err(internal)?
            .file_type()
            .is_symlink()
        {
            return Err(Error::Conflict("data_directory_insecure".into()));
        }
    }
    let root = absolute.canonicalize().map_err(internal)?;
    check_backup_ancestors(&root)?;
    Ok(root)
}

fn backup_identity(path: &Path, directory: bool) -> Result<BackupPhysicalIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path).map_err(internal)?;
        if meta.file_type().is_symlink()
            || (directory && !meta.is_dir())
            || (!directory && (!meta.is_file() || meta.nlink() != 1))
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
            return Err(Error::Conflict("data_directory_insecure".into()));
        }
        Ok(BackupPhysicalIdentity {
            device: meta.dev(),
            inode: meta.ino(),
            uid: meta.uid(),
            mode: meta.mode() & 0o7777,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, directory);
        Err(Error::Invalid("backup unsupported platform".into()))
    }
}

fn check_backup_ancestors(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for ancestor in path.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor).map_err(internal)?;
            let uid = meta.uid();
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || (uid != 0 && uid != rustix::process::geteuid().as_raw())
                || (meta.mode() & 0o022 != 0 && !(uid == 0 && meta.mode() & 0o1000 != 0))
            {
                return Err(Error::Conflict("data_directory_insecure".into()));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::Invalid("backup unsupported platform".into()))
    }
}

fn create_backup_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(internal)
}

async fn write_backup_member(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(path).await.map_err(internal)?;
    file.write_all(bytes).await.map_err(internal)?;
    file.sync_all().await.map_err(internal)
}

async fn verify_backup_members(root: &Path, manifest: &BackupManifest) -> Result<()> {
    if manifest.schema_version != BACKUP_SCHEMA
        || !manifest.complete
        || manifest.database.path != "rsia.sqlite3"
    {
        return Err(Error::Conflict("backup manifest invalid".into()));
    }
    let mut expected = std::collections::BTreeSet::from(["backup-manifest.json".to_owned()]);
    for member in std::iter::once(&manifest.database).chain(&manifest.blobs) {
        let path = Path::new(&member.path);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || !expected.insert(member.path.clone())
        {
            return Err(Error::Conflict("backup member path invalid".into()));
        }
        let full = root.join(path);
        check_backup_ancestors(full.parent().ok_or(Error::Internal)?)?;
        backup_identity(&full, false)?;
        let actual = file_entry(&full, &member.path).await?;
        if actual.sha256 != member.sha256 || actual.bytes != member.bytes {
            return Err(Error::Conflict("backup member proof differs".into()));
        }
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        backup_identity(&directory, true)?;
        let mut entries = tokio::fs::read_dir(&directory).await.map_err(internal)?;
        while let Some(entry) = entries.next_entry().await.map_err(internal)? {
            let kind = entry.file_type().await.map_err(internal)?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(internal)?
                    .to_string_lossy()
                    .into_owned();
                if !expected.remove(&relative) {
                    return Err(Error::Conflict("backup has unexpected member".into()));
                }
            } else {
                return Err(Error::Conflict("backup has linked member".into()));
            }
        }
    }
    if !expected.is_empty() {
        return Err(Error::Conflict("backup member missing".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedObjectRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeTombstone {
    pub id: String,
    pub schema_version: String,
    pub source_kind: String,
    pub reason: String,
    pub watermark_seq: u64,
    pub watermark_digest: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupState {
    Pending,
    Running,
    Complete,
    Failed,
}

impl CleanupState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "complete" => Ok(Self::Complete),
            "failed" => Ok(Self::Failed),
            _ => Err(Error::Internal),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupStatus {
    pub job_id: String,
    pub source: TypedObjectRef,
    pub watermark_seq: u64,
    pub watermark_digest: String,
    pub state: CleanupState,
    pub processed_nodes: u64,
    pub pending_nodes: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupFileEntry {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupMigrationEntry {
    pub version: i64,
    pub description: String,
    pub checksum_hex: String,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupWatermarkEntry {
    pub namespace: String,
    pub seq: i64,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub schema_version: String,
    pub complete: bool,
    pub database: BackupFileEntry,
    pub blobs: Vec<BackupFileEntry>,
    pub migrations: Vec<BackupMigrationEntry>,
    pub watermarks: Vec<BackupWatermarkEntry>,
    pub created_at: i64,
}

pub struct LifecycleStore;

struct BudgetScanCursor {
    frontier_seq: i64,
    scope: Option<String>,
    call: Option<String>,
}

// Read-only compatibility wire used only while destroying replay content.
// E08 must compile without later executable E09/E10 replay types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoricalUsageWire {
    input_tokens: u64,
    output_tokens: u64,
    cost_micros: Option<u64>,
    latency_millis: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceEvidenceEnvelopeWire {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: ResourceEvidenceWire,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceEvidenceWire {
    schema_version: String,
    ticket_id: String,
    billing_scope: String,
    cost_scope: String,
    calls: Vec<crate::budget::BudgetCallRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayCleanupEnvelope<T> {
    schema_version: String,
    id: String,
    namespace: String,
    record_kind: String,
    payload: T,
}

// Read-only Host authority shape; storage cannot depend on engine's writer type.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeTraceAuthorityWire {
    schema_version: String,
    record: NativeRunRecordWire,
    trace: evo_core::optimization::OptimizationTrace,
    excerpt_start: usize,
    excerpt_end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRunRecordWire {
    id: String,
    body: Vec<u8>,
    parent_family: String,
    task_origin: evo_core::evidence::TaskOrigin,
    execution_attestation: evo_core::evidence::ExecutionAttestation,
    purpose: evo_core::evidence::Purpose,
}

pub(crate) fn valid_native_trace_authority(
    node: &TypedObjectRef,
    value: &serde_json::Value,
) -> bool {
    let Ok(authority) = serde_json::from_value::<NativeTraceAuthorityWire>(value.clone()) else {
        return false;
    };
    let record = &authority.record;
    let trace = &authority.trace;
    authority.schema_version == "rsia.optimization.source.v1"
        && record.id == node.id
        && record.task_origin == evo_core::evidence::TaskOrigin::TrustedRun
        && record.execution_attestation == evo_core::evidence::ExecutionAttestation::TrustedHost
        && record.purpose == evo_core::evidence::Purpose::Development
        && trace.purpose == record.purpose
        && trace.run_id == record.id
        && trace.parent_family == record.parent_family
        && trace.source_digest == evo_core::hash(&record.body)
        && record
            .body
            .get(authority.excerpt_start..authority.excerpt_end)
            == Some(trace.excerpt.as_bytes())
        && identifier(&record.id).is_ok()
        && identifier(&record.parent_family).is_ok()
}

pub(crate) fn registered_import_source(
    namespace: &str,
    source: &TypedObjectRef,
    value: &serde_json::Value,
) -> bool {
    matches!(
        validate_e16_envelope(namespace, source, value),
        Ok("import_source")
    )
}

impl LifecycleStore {
    pub async fn begin_revoke(
        ctx: &Context,
        store: &Store,
        source: TypedObjectRef,
        reason: &str,
        now: i64,
    ) -> Result<CleanupStatus> {
        ctx.require(&[Role::Admin])?;
        validate_source(&source)?;
        evo_core::text(reason, "revoke reason", 512)?;
        valid_time(now)?;
        let mut session = store.session().await?;
        let existing_source = session.object_row(ctx, &source.kind, &source.id).await?;
        let existing_tombstone = session
            .object_row(ctx, "tombstone", &source.id)
            .await?
            .map(|row| {
                parse_strict_json(&row.body)
                    .map_err(internal)
                    .and_then(|value| {
                        serde_json::from_value::<RevokeTombstone>(value).map_err(internal)
                    })
            })
            .transpose()?;
        if existing_source.is_none() && existing_tombstone.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(tombstone) = existing_tombstone {
            // A tombstone is keyed by the id of its source alone, so a run and an
            // artifact that share an id share one tombstone, written for whichever
            // was revoked first. A request for the other kind finds no job of its
            // own under that tombstone: it is a name conflict, not corruption (this
            // used to fall through to `Internal` below). Nothing is written for it:
            // falling through to the writes further down would overwrite the first
            // tombstone (`put` is an upsert) and the restore replay would lose its
            // watermark sequence. Keying the tombstone by kind as well (§11.1) is a
            // separate change.
            if tombstone.source_kind != source.kind {
                if existing_source.is_none() {
                    return Err(Error::NotFound);
                }
                let revoked_as = match tombstone.source_kind.as_str() {
                    kind @ ("run" | "artifact") => kind,
                    // Not a kind `begin_revoke` writes; do not echo a stored string.
                    _ => "an unrecognised source kind",
                };
                return Err(Error::Conflict(format!(
                    "cannot revoke {} {}: the id is already revoked as {revoked_as} \
                     (a revocation tombstone is keyed by id)",
                    source.kind, source.id
                )));
            }
            if !crate::revocation::valid_tombstone(&tombstone, &source.id) {
                return Err(Error::Internal);
            }
            let status = load_status_by_source(
                &mut session.tx,
                ctx.namespace(),
                &source,
                tombstone.watermark_seq,
            )
            .await?;
            session.commit().await?;
            return status.ok_or(Error::Internal);
        }
        if source.kind == "artifact" {
            let row = existing_source.ok_or(Error::NotFound)?;
            if !parse_strict_json(&row.body)
                .ok()
                .as_ref()
                .is_some_and(|value| registered_import_source(ctx.namespace(), &source, value))
            {
                return Err(Error::Invalid(
                    "only a strict E16 import_source artifact is a revocable source".into(),
                ));
            }
        }
        let watermark_digest = fingerprint(&(
            "rsia.revoke_watermark.v1",
            ctx.namespace(),
            &source.kind,
            &source.id,
            reason,
            now,
        ))?;
        let watermark_seq = session.bump_watermark(ctx, &watermark_digest).await?;
        let watermark_seq = u64::try_from(watermark_seq).map_err(|_| Error::Internal)?;
        let tombstone = RevokeTombstone {
            id: source.id.clone(),
            schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
            source_kind: source.kind.clone(),
            reason: reason.into(),
            watermark_seq,
            watermark_digest: watermark_digest.clone(),
            created_at: now,
        };
        session
            .put(ctx, "tombstone", &source.id, ctx.actor(), &tombstone)
            .await?;
        let job_hash = fingerprint(&(
            ctx.namespace(),
            &source.kind,
            &source.id,
            watermark_seq,
            &watermark_digest,
        ))?;
        let job_id = format!("revoke-{}", &job_hash[..32]);
        sqlx::query(
            "INSERT INTO revoke_cleanup_jobs(
               namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,
               state,created_at,updated_at
             ) VALUES(?,?,?,?,?,?,'pending',?,?)",
        )
        .bind(ctx.namespace())
        .bind(&job_id)
        .bind(&source.kind)
        .bind(&source.id)
        .bind(i64::try_from(watermark_seq).map_err(|_| Error::Internal)?)
        .bind(&watermark_digest)
        .bind(now)
        .bind(now)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
             VALUES(?,?,'__budget_scan',?)",
        )
        .bind(ctx.namespace())
        .bind(&job_id)
        .bind(&source.id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        sqlx::query("INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id) VALUES(?,?,'__world_scan',?)")
            .bind(ctx.namespace()).bind(&job_id).bind(&source.id).execute(&mut *session.tx).await.map_err(internal)?;
        sqlx::query(
            "INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
             VALUES(?,?,?,?)",
        )
        .bind(ctx.namespace())
        .bind(&job_id)
        .bind(&source.kind)
        .bind(&source.id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        insert_cleanup_event(
            &mut session.tx,
            ctx.namespace(),
            &job_id,
            Some(&source),
            "logical_block_committed",
            now,
            json!({"watermark_seq":watermark_seq,"watermark_digest":watermark_digest}),
        )
        .await?;
        let status = load_status(&mut session.tx, ctx.namespace(), &job_id)
            .await?
            .ok_or(Error::Internal)?;
        session.commit().await?;
        Ok(status)
    }

    pub async fn cleanup_step(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        edge_page_limit: usize,
        now: i64,
    ) -> Result<CleanupStatus> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        identifier(job_id)?;
        valid_time(now)?;
        if edge_page_limit == 0 || edge_page_limit > MAX_CLEANUP_EDGE_PAGE {
            return Err(Error::Invalid(format!(
                "edge page limit must be 1..={MAX_CLEANUP_EDGE_PAGE}"
            )));
        }
        let _blob_guard = store.blob_lock.write().await;
        let mut tx = store.pool.begin().await.map_err(internal)?;
        let status = load_status(&mut tx, ctx.namespace(), job_id)
            .await?
            .ok_or(Error::NotFound)?;
        if status.state == CleanupState::Complete {
            tx.commit().await.map_err(internal)?;
            return Ok(status);
        }
        ensure_logical_block(&mut tx, ctx, &status).await?;
        if status.state == CleanupState::Failed && ctx.role() == Role::Admin {
            prepare_failed_cleanup_retry(&mut tx, ctx, &status, now).await?;
        }
        sqlx::query(
            "UPDATE revoke_cleanup_jobs SET state='running',updated_at=?
             WHERE namespace=? AND job_id=? AND state IN ('pending','running')",
        )
        .bind(now)
        .bind(ctx.namespace())
        .bind(job_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        for _ in 0..edge_page_limit.min(128) {
            let frontier = sqlx::query(
                "SELECT seq,node_kind,node_id,cursor_src_kind,cursor_src_id
                 FROM revoke_cleanup_frontier
                 WHERE namespace=? AND job_id=? AND expanded=0 AND node_kind!='__formal_ticket_scan' ORDER BY seq LIMIT 1",
            )
            .bind(ctx.namespace())
            .bind(job_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
            let Some(frontier) = frontier else { break };
            expand_frontier(
                &mut tx,
                store,
                ctx,
                job_id,
                &status.source,
                &frontier,
                edge_page_limit,
                now,
            )
            .await?;
        }
        let formal_caught_up =
            catch_up_old_formal_cleanup(&mut tx, ctx, job_id, edge_page_limit, now).await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM revoke_cleanup_frontier
             WHERE namespace=? AND job_id=? AND expanded=0 AND node_kind!='__formal_ticket_scan'",
        )
        .bind(ctx.namespace())
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        // An empty queue is not yet a closed closure. A node's dependents are read
        // through a forward key-set cursor and the node is expanded once, so an
        // edge written after `begin_revoke` can be missed: its key sorts before
        // the cursor of a node that is still being paged, or the node was already
        // expanded. `closure_is_open` rereads, in this transaction, what the queue
        // is derived from (the dependency table, and the budget calls that
        // `__budget_scan` reads once), queues or redacts what the queue does not
        // hold, and the job completes only when it finds nothing.
        //
        // Termination: the frontier only grows, and every positive answer either
        // adds a node to it or redacts a budget call that no later recheck selects
        // again; the nodes and the calls are finite. A writer that never stops
        // postpones `Complete` but cannot slip a dependent past it, and once the
        // writer stops the job finishes. A late node of a schema the cleanup does
        // not classify is expanded like any other and fails the job
        // (`blocked_unknown_scope`) instead of completing it.
        //
        // The store has one connection, so this recheck and the state change below
        // are atomic with respect to every writer. Only the tables are read, so the
        // argument holds after a restart.
        if pending == 0
            && formal_caught_up
            && !closure_is_open(&mut tx, ctx, job_id, &status.source, edge_page_limit, now).await?
        {
            let completed = sqlx::query(
                "UPDATE revoke_cleanup_jobs SET state='complete',last_error=NULL,updated_at=?
                 WHERE namespace=? AND job_id=? AND state='running'",
            )
            .bind(now)
            .bind(ctx.namespace())
            .bind(job_id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            if completed.rows_affected() == 1 {
                insert_cleanup_event(
                    &mut tx,
                    ctx.namespace(),
                    job_id,
                    None,
                    "cleanup_complete",
                    now,
                    json!({}),
                )
                .await?;
            }
        }
        let status = load_status(&mut tx, ctx.namespace(), job_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(status)
    }

    pub async fn cleanup_status(
        ctx: &Context,
        store: &Store,
        job_id: &str,
    ) -> Result<CleanupStatus> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        identifier(job_id)?;
        let mut tx = store.pool.begin().await.map_err(internal)?;
        let status = load_status(&mut tx, ctx.namespace(), job_id)
            .await?
            .ok_or(Error::NotFound)?;
        tx.commit().await.map_err(internal)?;
        Ok(status)
    }
}

/// An explicit Admin retry is a new verification round over the same frontier.
/// Keep old diagnostics until completion, with the old progress in the audit.
/// Both scan cursors are reset: historical failures do not identify every world
/// or a budget call's billing scope. No execution/accounting facts are changed.
async fn prepare_failed_cleanup_retry(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    status: &CleanupStatus,
    now: i64,
) -> Result<()> {
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        &status.job_id,
        Some(&status.source),
        "cleanup_retry_started",
        now,
        json!({"previous_processed_nodes":status.processed_nodes,
            "previous_pending_nodes":status.pending_nodes,"previous_last_error":status.last_error}),
    )
    .await?;
    sqlx::query(
        "UPDATE revoke_cleanup_frontier
         SET expanded=0,cursor_src_kind=NULL,cursor_src_id=NULL
         WHERE namespace=? AND job_id=?",
    )
    .bind(ctx.namespace())
    .bind(&status.job_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE revoke_cleanup_jobs SET state='running',processed_nodes=0,updated_at=?
         WHERE namespace=? AND job_id=? AND state='failed'",
    )
    .bind(now)
    .bind(ctx.namespace())
    .bind(&status.job_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Whether the closure of a job whose queue is empty is still open: a dependent
/// or a budget call written after the queue passed it was found (and, for a
/// dependent, queued; for a call, redacted), so the job must stay `Running` and
/// be stepped again. `false` is the fixpoint: the dependency table holds no
/// dependent of an expanded node that the frontier lacks, and no budget call that
/// names the source keeps its request. A job that is not running (a node of
/// unknown scope failed it) cannot complete whatever the closure holds, so there
/// is nothing to recheck.
///
/// At most `limit` nodes are queued, or calls redacted, per call, like the edge
/// pages of the expansion; the dependency recheck goes first and a positive
/// answer skips the budget recheck, which reads every unredacted request of the
/// namespace.
async fn closure_is_open(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    limit: usize,
    now: i64,
) -> Result<bool> {
    let running: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM revoke_cleanup_jobs WHERE namespace=? AND job_id=? AND state='running'",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if running.is_none() {
        return Ok(false);
    }
    if queue_late_dependents(tx, ctx, job_id, limit, now).await? > 0 {
        return Ok(true);
    }
    redact_late_budget_calls(tx, ctx, job_id, source, limit, now).await
}

/// Queues up to `limit` dependents of an expanded node of this job that the
/// job's frontier does not hold, as unexpanded nodes (the ordinary expansion then
/// reads their own dependents and cleans their content). Returns how many.
/// A frontier is the closure of one job, so membership is per `job_id`.
async fn queue_late_dependents(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    limit: usize,
    now: i64,
) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT DISTINCT d.src_kind,d.src_id
         FROM revoke_cleanup_frontier f
         JOIN dependencies d
           ON d.namespace=f.namespace AND d.dst_kind=f.node_kind AND d.dst_id=f.node_id
         WHERE f.namespace=? AND f.job_id=? AND f.expanded=1
           AND NOT EXISTS (
             SELECT 1 FROM revoke_cleanup_frontier g
             WHERE g.namespace=f.namespace AND g.job_id=f.job_id
               AND g.node_kind=d.src_kind AND g.node_id=d.src_id)
         ORDER BY d.src_kind,d.src_id LIMIT ?",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(i64::try_from(limit).map_err(|_| Error::Internal)?)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    for row in &rows {
        let kind: String = row.try_get("src_kind").map_err(internal)?;
        let id: String = row.try_get("src_id").map_err(internal)?;
        let queued = sqlx::query(
            "INSERT OR IGNORE INTO revoke_cleanup_frontier(
               namespace,job_id,node_kind,node_id
             ) VALUES(?,?,?,?)",
        )
        .bind(ctx.namespace())
        .bind(job_id)
        .bind(kind)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        // The query excluded every node the frontier holds, under the key the
        // insert is unique on; an ignored insert would report progress forever.
        if queued.rows_affected() != 1 {
            return Err(Error::Internal);
        }
    }
    if !rows.is_empty() {
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            None,
            "late_dependents_queued",
            now,
            json!({"count":rows.len()}),
        )
        .await?;
    }
    Ok(rows.len())
}

/// New-mode history validation is shared by the initial scan and the late
/// fixpoint. Its index repair precedes direct-source filtering so ref -> S -> R
/// remains reachable when R is not a declared direct material.
async fn scan_e16_budget_call(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    row: &sqlx::sqlite::SqliteRow,
    now: i64,
) -> Result<Option<bool>> {
    let scope: String = row.try_get("billing_scope").map_err(internal)?;
    let call_id: String = row.try_get("call_id").map_err(internal)?;
    let schema: String = row.try_get("request_artifact_schema").map_err(internal)?;
    let body: String = row.try_get("request_artifact_body").map_err(internal)?;
    if !crate::typed_budget::request_body_marks_e16(&schema, &body)
        && !crate::typed_budget::new_ref_exists_in_tx(tx, ctx.namespace(), &scope, &call_id).await?
    {
        return Ok(None);
    }
    let call = crate::budget::load_call(tx, &scope, &call_id)
        .await?
        .ok_or(Error::Internal)?;
    let history = crate::typed_budget::maintain_history_in_tx(tx, ctx, &call).await?;
    let (reference, index_added) = match history {
        crate::typed_budget::E16History::Bound {
            reference,
            index_added,
        } => (reference, index_added),
        crate::typed_budget::E16History::Legacy | crate::typed_budget::E16History::Unavailable => {
            mark_unknown_scope(
                tx,
                ctx,
                job_id,
                &TypedObjectRef {
                    kind: "reservation".into(),
                    id: call_id,
                },
                "blocked_unknown_scope:e16_budget_binding",
                now,
            )
            .await?;
            return Ok(Some(true));
        }
    };
    if index_added > 0 {
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(&TypedObjectRef {
                kind: "artifact".into(),
                id: reference.id.clone(),
            }),
            "historical_budget_index_repaired",
            now,
            json!({"added_rows":index_added}),
        )
        .await?;
    }
    let direct = reference
        .object_refs
        .iter()
        .any(|r| r.kind == source.kind && r.id == source.id);
    let in_frontier: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM revoke_cleanup_frontier
         WHERE namespace=? AND job_id=? AND node_kind='artifact' AND node_id=?)",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(&reference.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let mut progress = index_added > 0;
    if direct || in_frontier != 0 {
        progress |=
            crate::budget::redact_e16_budget_call_content_in_tx(tx, ctx, &scope, &call_id).await?;
        let queued = sqlx::query(
            "INSERT OR IGNORE INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
             VALUES(?,?,'artifact',?)",
        )
        .bind(ctx.namespace())
        .bind(job_id)
        .bind(&reference.id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        progress |= queued.rows_affected() > 0;
    }
    Ok(Some(progress))
}

/// The `__budget_scan` of a job reads `root_budget_calls` once; a call reserved
/// after it passed holds its request body (and the model input in it) although it
/// names the revoked source. Same criterion as `expand_budget_scan`: redact the
/// request of every unredacted call of the namespace whose source closure
/// (`budget_call_source_closure`) holds the source, index its budget call
/// reference and queue that reference; a call whose closure cannot be read (a
/// reference that does not decode or is not the call's own, or no reference and a
/// request whose schema or body cannot be read) is isolated (`mark_unknown_scope`,
/// which fails the job) and left as it is. Redacts at most `limit` calls; returns
/// whether it redacted or isolated any.
async fn redact_late_budget_calls(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    limit: usize,
    now: i64,
) -> Result<bool> {
    const PAGE: i64 = 64;
    let mut redacted = 0usize;
    let mut progress_calls = 0usize;
    let mut open = false;
    let mut cursor: Option<(String, String)> = None;
    loop {
        let (after_scope, after_call) = match &cursor {
            Some((scope, call)) => (Some(scope.as_str()), Some(call.as_str())),
            None => (None, None),
        };
        let rows = sqlx::query(
            "SELECT billing_scope,call_id,request_artifact_schema,request_artifact_body
             FROM root_budget_calls
             WHERE namespace=? AND request_artifact_body IS NOT NULL
               AND
               (? IS NULL OR billing_scope>? OR (billing_scope=? AND call_id>?))
             ORDER BY billing_scope,call_id LIMIT ?",
        )
        .bind(ctx.namespace())
        .bind(after_scope)
        .bind(after_scope)
        .bind(after_scope)
        .bind(after_call)
        .bind(PAGE)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        for row in &rows {
            let billing_scope: String = row.try_get("billing_scope").map_err(internal)?;
            let call_id: String = row.try_get("call_id").map_err(internal)?;
            let schema: String = row.try_get("request_artifact_schema").map_err(internal)?;
            let body: String = row.try_get("request_artifact_body").map_err(internal)?;
            cursor = Some((billing_scope.clone(), call_id.clone()));
            if let Some(progress) = scan_e16_budget_call(tx, ctx, job_id, source, row, now).await? {
                if progress {
                    open = true;
                    progress_calls += 1;
                }
                if progress_calls >= limit {
                    break;
                }
                continue;
            }
            if schema == "rsia.redacted.v1" {
                continue;
            }
            let closure =
                budget_call_source_closure(tx, ctx, &billing_scope, &call_id, &schema, &body)
                    .await?;
            let source_ids = match closure {
                BudgetRequestSources::Named(source_ids) => source_ids,
                BudgetRequestSources::Unknown(error) => {
                    mark_unknown_scope(
                        tx,
                        ctx,
                        job_id,
                        &TypedObjectRef {
                            kind: "reservation".into(),
                            id: call_id,
                        },
                        &error,
                        now,
                    )
                    .await?;
                    open = true;
                    continue;
                }
            };
            if !source_ids.contains(&source.id) {
                continue;
            }
            let reference = crate::budget::index_budget_call_ref_in_tx(
                tx,
                ctx,
                &billing_scope,
                &call_id,
                &source_ids,
            )
            .await?;
            crate::budget::redact_budget_call_content_in_tx(
                tx,
                ctx,
                &billing_scope,
                &call_id,
                "source_revoked",
            )
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO revoke_cleanup_frontier(
                   namespace,job_id,node_kind,node_id
                 ) VALUES(?,?,'artifact',?)",
            )
            .bind(ctx.namespace())
            .bind(job_id)
            .bind(reference.id)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
            redacted += 1;
            progress_calls += 1;
            open = true;
            if progress_calls >= limit {
                break;
            }
        }
        if progress_calls >= limit || rows.len() < PAGE as usize {
            break;
        }
    }
    if redacted > 0 {
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            None,
            "late_budget_calls_redacted",
            now,
            json!({"count":redacted}),
        )
        .await?;
    }
    Ok(open)
}

/// What the stored request of a budget call says about the sources it was built
/// from, as far as the revocation cleanup can read it.
pub(crate) enum BudgetRequestSources {
    /// The ids of the sources the request names.
    Named(Vec<String>),
    /// Whether the request names a revoked source cannot be told: the diagnostic
    /// the call is isolated under (`mark_unknown_scope`).
    Unknown(String),
}

/// The source closure the revocation cleanup judges an unredacted legacy budget
/// call by, in `expand_budget_scan` and in its recheck `redact_late_budget_calls`,
/// in the order the legacy dispatch and settlement gates read it
/// (`legacy_call_sources`): the closure registered in the call's budget call
/// reference when it has one (`registered_call_source_ids`), whatever its request
/// says; otherwise the closure its stored request names
/// (`budget_request_source_ids`). A request of a schema that names no closure,
/// such as the formal task request the broker stores, is judged by its registered
/// closure: a call unrelated to the revoked source is left as it is, a call that
/// depends on it is redacted. A call without a reference whose request cannot be
/// read is `Unknown`, and isolated (plan §11, §11.3, §11.5; E08).
///
/// So is a call whose reference is stored but is not trusted
/// (`unreadable_budget_call_ref:<schema>`): it does not decode, or it decodes and
/// is not the call's own reference (`RegisteredCallSources::Unqualified`: another
/// schema, namespace, billing scope or call, a source id that is not an
/// identifier, or `source_ids` not in ascending order without repeats). Its
/// closure is neither trusted nor guessed at (whether it holds the revoked source
/// is not asked) and the request is not read in its place (plan §11.5: isolate
/// what lacks the facts it is judged by; E16.2: a dependency source that cannot be
/// resolved is isolated); the reference is not sorted, repaired or indexed. The
/// legacy gates refuse a reference that does not decode with `Internal` and write
/// nothing, and read the `source_ids` of an unqualified one as before
/// (`registered_source_ids`); the cleanup isolates both, records it
/// (`mark_unknown_scope`: a `Failed` job an Admin can retry) and goes on with the
/// rest of its namespace, because an error here would roll the whole step back and
/// leave the job running with no diagnostic, unreachable by a retry (AG-096). The
/// answers differ on purpose.
async fn budget_call_source_closure(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
    schema: &str,
    body: &str,
) -> Result<BudgetRequestSources> {
    match crate::budget::registered_call_source_ids(tx, ctx.namespace(), billing_scope, call_id)
        .await?
    {
        crate::budget::RegisteredCallSources::Named(source_ids) => {
            Ok(BudgetRequestSources::Named(source_ids))
        }
        crate::budget::RegisteredCallSources::Missing => {
            Ok(budget_request_source_ids(schema, body))
        }
        crate::budget::RegisteredCallSources::Unqualified(_)
        | crate::budget::RegisteredCallSources::Undecodable(_) => Ok(
            BudgetRequestSources::Unknown(format!("unreadable_budget_call_ref:{schema}")),
        ),
    }
}

/// The sources a budget call's stored request names. This is the one reader of
/// request bodies for the revocation cleanup (`budget_call_source_closure`, in
/// `expand_budget_scan` and in its recheck `redact_late_budget_calls`) and for the
/// legacy gates (`legacy_call_sources`), and both read a request only for a call
/// without a budget call reference: a registered closure comes first. It reads the
/// `source_closure[*].id` of a model request (for a bound request, of its
/// `request`) and the `source_ids` of a registered execution.
/// Nothing else in a request is read: its input, its digests and a bound
/// request's compiler binding are checked on the use path, so a request that
/// names a revoked source is cleaned whether or not it would still be accepted
/// for dispatch, and a field these readers do not name is ignored. Reading a
/// request grants no compiler authority and dispatches no model.
///
/// A request of a schema the cleanup does not read cannot be told free of a
/// source; neither can a body of a known schema whose closure cannot be read: not
/// strict JSON (a duplicate key could hide a closure), a bound request that does
/// not declare the wire its column names, a closure or an id that is missing or of
/// another JSON type, or an id that is not an identifier. Either is `Unknown`,
/// never an error: the call is isolated and left as it is, and the cleanup goes on
/// with the rest of its namespace (plan §11: the revocation is written first, then
/// cleaned page by page and nothing is skipped silently; §11.5, E08: what cannot
/// be classified is isolated).
pub(crate) fn budget_request_source_ids(schema: &str, body: &str) -> BudgetRequestSources {
    let read = || parse_strict_json(body).ok();
    let ids = match schema {
        "rsia.bound_model_request.v1" | "rsia.bound_model_request.v2" => read()
            .as_ref()
            .and_then(|bound| bound_request_closure_ids(bound, schema)),
        "rsia.model_request.artifact.v1" => read().as_ref().and_then(closure_ids),
        crate::budget::REGISTERED_EXECUTION_REQUEST_SCHEMA => {
            read().as_ref().and_then(registered_execution_source_ids)
        }
        _ => {
            return BudgetRequestSources::Unknown(format!(
                "unknown_budget_request_schema:{schema}"
            ));
        }
    };
    match ids.filter(|ids| ids.iter().all(|id| identifier(id).is_ok())) {
        Some(ids) => BudgetRequestSources::Named(ids),
        None => BudgetRequestSources::Unknown(format!("unreadable_budget_request:{schema}")),
    }
}

/// The closure of the model request a bound request carries. A bound request
/// declares its wire in the body as well; a body that declares another wire is
/// not read as this one.
fn bound_request_closure_ids(bound: &serde_json::Value, schema: &str) -> Option<Vec<String>> {
    if !matches!(
        schema,
        "rsia.bound_model_request.v1" | "rsia.bound_model_request.v2"
    ) || bound.get("schema_version")?.as_str()? != schema
    {
        return None;
    }
    closure_ids(bound.get("request")?)
}

/// `source_closure[*].id` of a model request.
fn closure_ids(request: &serde_json::Value) -> Option<Vec<String>> {
    request
        .get("source_closure")?
        .as_array()?
        .iter()
        .map(|source| source.get("id")?.as_str().map(str::to_owned))
        .collect()
}

fn registered_execution_source_ids(request: &serde_json::Value) -> Option<Vec<String>> {
    request
        .get("source_ids")?
        .as_array()?
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect()
}

fn publish_backup_directory(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(|error| {
            if error == rustix::io::Errno::EXIST || error == rustix::io::Errno::NOTEMPTY {
                Error::Conflict("backup destination appeared during atomic publication".into())
            } else if error == rustix::io::Errno::NOSYS || error == rustix::io::Errno::OPNOTSUPP {
                Error::Invalid("atomic no-overwrite backup publication unsupported".into())
            } else {
                internal(error)
            }
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        let _ = (source, destination);
        Err(Error::Invalid(
            "atomic no-overwrite backup publication unsupported platform".into(),
        ))
    }
}

async fn load_backup_migrations(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<BackupMigrationEntry>> {
    let rows = sqlx::query(
        "SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(internal)?;
    let mut migrations = Vec::with_capacity(rows.len());
    for row in rows {
        let checksum: Vec<u8> = row.try_get("checksum").map_err(internal)?;
        migrations.push(BackupMigrationEntry {
            version: row.try_get("version").map_err(internal)?,
            description: row.try_get("description").map_err(internal)?,
            checksum_hex: hex(&checksum),
            success: row.try_get::<i64, _>("success").map_err(internal)? != 0,
        });
    }
    if migrations.is_empty() || migrations.iter().any(|migration| !migration.success) {
        return Err(Error::Conflict(
            "backup requires successful schema migrations".into(),
        ));
    }
    let compiled = sqlx::migrate!("./migrations");
    if migrations.len() != compiled.iter().count()
        || migrations
            .iter()
            .zip(compiled.iter())
            .any(|(saved, current)| {
                saved.version != current.version
                    || saved.checksum_hex != hex(current.checksum.as_ref())
            })
    {
        return Err(Error::Conflict(
            "backup migration checksums differ from current code".into(),
        ));
    }
    Ok(migrations)
}

async fn load_backup_watermarks(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<BackupWatermarkEntry>> {
    let namespaces: Vec<String> = sqlx::query_scalar(
        "SELECT namespace FROM objects
         UNION SELECT namespace FROM audit
         UNION SELECT namespace FROM root_budget_namespaces
         ORDER BY namespace",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(internal)?;
    if namespaces.is_empty() {
        return Err(Error::Conflict(
            "backup has no namespace with a verifiable revoke watermark".into(),
        ));
    }
    let mut output = Vec::with_capacity(namespaces.len());
    for namespace in namespaces {
        let row = sqlx::query("SELECT seq,digest FROM revoke_watermark WHERE namespace=?")
            .bind(&namespace)
            .fetch_optional(&mut *connection)
            .await
            .map_err(internal)?
            .ok_or_else(|| Error::Conflict("namespace lacks revoke watermark".into()))?;
        output.push(BackupWatermarkEntry {
            namespace,
            seq: row.try_get("seq").map_err(internal)?,
            digest: row.try_get("digest").map_err(internal)?,
        });
    }
    Ok(output)
}

async fn copy_backup_blobs(store: &Store, temp: &Path) -> Result<Vec<BackupFileEntry>> {
    let source = store.root.join("blobs");
    if !source.exists() {
        return Ok(Vec::new());
    }
    backup_identity(&source, true)?;
    let target_root = temp.join("blobs");
    let mut entries = Vec::new();
    let mut namespaces = tokio::fs::read_dir(&source).await.map_err(internal)?;
    while let Some(namespace) = namespaces.next_entry().await.map_err(internal)? {
        if !namespace.file_type().await.map_err(internal)?.is_dir() {
            return Err(Error::Conflict(
                "unexpected non-directory in blob root".into(),
            ));
        }
        let namespace_name = namespace.file_name();
        let namespace_text = namespace_name
            .to_str()
            .ok_or_else(|| Error::Invalid("blob namespace path is not UTF-8".into()))?;
        let target_namespace = target_root.join(&namespace_name);
        backup_identity(&namespace.path(), true)?;
        let mut files = tokio::fs::read_dir(namespace.path())
            .await
            .map_err(internal)?;
        while let Some(file) = files.next_entry().await.map_err(internal)? {
            if !file.file_type().await.map_err(internal)?.is_file() {
                return Err(Error::Conflict("blob entry is not a regular file".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if file.metadata().await.map_err(internal)?.nlink() != 1 {
                    return Err(Error::Conflict(
                        "hard-linked blob is not a backup member".into(),
                    ));
                }
            }
            let file_name = file.file_name();
            let file_text = file_name
                .to_str()
                .ok_or_else(|| Error::Invalid("blob path is not UTF-8".into()))?;
            let relative = format!("blobs/{namespace_text}/{file_text}");
            let bytes = tokio::fs::read(file.path()).await.map_err(internal)?;
            if evo_core::hash(&bytes) != file_text {
                return Err(Error::Conflict("blob filename digest mismatch".into()));
            }
            if !target_root.exists() {
                create_backup_dir(&target_root)?;
            }
            if !target_namespace.exists() {
                create_backup_dir(&target_namespace)?;
            }
            let destination = target_namespace.join(&file_name);
            backup_identity(&file.path(), false)?;
            write_backup_member(&destination, &bytes).await?;
            entries.push(BackupFileEntry {
                path: relative,
                sha256: evo_core::hash(&bytes),
                bytes: u64::try_from(bytes.len()).map_err(|_| Error::Internal)?,
            });
        }
    }
    if target_root.exists() {
        let mut directories = tokio::fs::read_dir(&target_root).await.map_err(internal)?;
        while let Some(directory) = directories.next_entry().await.map_err(internal)? {
            sync_directory(directory.path()).await?;
        }
        sync_directory(target_root).await?;
    }
    Ok(entries)
}

async fn file_entry(path: &Path, relative: &str) -> Result<BackupFileEntry> {
    let bytes = tokio::fs::read(path).await.map_err(internal)?;
    Ok(BackupFileEntry {
        path: relative.into(),
        sha256: evo_core::hash(&bytes),
        bytes: u64::try_from(bytes.len()).map_err(|_| Error::Internal)?,
    })
}

async fn sync_file(path: &Path) -> Result<()> {
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .map_err(internal)?;
    file.sync_all().await.map_err(internal)
}

async fn sync_directory(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all())
        .await
        .map_err(internal)?
        .map_err(internal)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(clippy::too_many_arguments)]
async fn expand_frontier(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    frontier: &sqlx::sqlite::SqliteRow,
    limit: usize,
    now: i64,
) -> Result<()> {
    let seq: i64 = frontier.try_get("seq").map_err(internal)?;
    let node_kind: String = frontier.try_get("node_kind").map_err(internal)?;
    let node_id: String = frontier.try_get("node_id").map_err(internal)?;
    let cursor_kind: Option<String> = frontier.try_get("cursor_src_kind").map_err(internal)?;
    let cursor_id: Option<String> = frontier.try_get("cursor_src_id").map_err(internal)?;
    if node_kind == "__world_scan" {
        return expand_world_scan(
            tx,
            ctx,
            job_id,
            &node_id,
            seq,
            cursor_id.as_deref(),
            limit,
            now,
        )
        .await;
    }
    if node_kind == "__budget_scan" {
        return expand_budget_scan(
            tx,
            ctx,
            job_id,
            source,
            &BudgetScanCursor {
                frontier_seq: seq,
                scope: cursor_kind,
                call: cursor_id,
            },
            limit,
            now,
        )
        .await;
    }
    let query_limit =
        i64::try_from(limit.checked_add(1).ok_or(Error::Internal)?).map_err(|_| Error::Internal)?;
    let rows = sqlx::query(
        "SELECT src_kind,src_id FROM dependencies
         WHERE namespace=? AND dst_kind=? AND dst_id=? AND
           (? IS NULL OR src_kind>? OR (src_kind=? AND src_id>?))
         ORDER BY src_kind,src_id LIMIT ?",
    )
    .bind(ctx.namespace())
    .bind(&node_kind)
    .bind(&node_id)
    .bind(&cursor_kind)
    .bind(&cursor_kind)
    .bind(&cursor_kind)
    .bind(&cursor_id)
    .bind(query_limit)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let has_more = rows.len() > limit;
    let accepted = rows.iter().take(limit).collect::<Vec<_>>();
    for row in &accepted {
        let child_kind: String = row.try_get("src_kind").map_err(internal)?;
        let child_id: String = row.try_get("src_id").map_err(internal)?;
        sqlx::query(
            "INSERT OR IGNORE INTO revoke_cleanup_frontier(
               namespace,job_id,node_kind,node_id
             ) VALUES(?,?,?,?)",
        )
        .bind(ctx.namespace())
        .bind(job_id)
        .bind(child_kind)
        .bind(child_id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    if has_more {
        let last = accepted.last().ok_or(Error::Internal)?;
        let last_kind: String = last.try_get("src_kind").map_err(internal)?;
        let last_id: String = last.try_get("src_id").map_err(internal)?;
        sqlx::query(
            "UPDATE revoke_cleanup_frontier SET cursor_src_kind=?,cursor_src_id=? WHERE seq=?",
        )
        .bind(last_kind)
        .bind(last_id)
        .bind(seq)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    } else {
        sqlx::query("UPDATE revoke_cleanup_frontier SET expanded=1 WHERE seq=?")
            .bind(seq)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        sqlx::query(
            "UPDATE idempotency SET response='null',redacted=1
             WHERE namespace=? AND subject_id=?",
        )
        .bind(ctx.namespace())
        .bind(&node_id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        cleanup_node_content(
            tx,
            store,
            ctx,
            job_id,
            source,
            &TypedObjectRef {
                kind: node_kind.clone(),
                id: node_id.clone(),
            },
            now,
        )
        .await?;
        sqlx::query(
            "UPDATE revoke_cleanup_jobs SET processed_nodes=processed_nodes+1,updated_at=?
             WHERE namespace=? AND job_id=?",
        )
        .bind(now)
        .bind(ctx.namespace())
        .bind(job_id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(&TypedObjectRef {
                kind: node_kind,
                id: node_id,
            }),
            "node_expanded",
            now,
            json!({"historical_facts_preserved":true}),
        )
        .await?;
    }
    Ok(())
}

/// A private per-job frontier cursor, not a DataObject kind or a new source.
/// One bounded page runs beside ordinary node work, including after upgrade
/// when genuine old Running tickets already have expanded=1. No node resets.
async fn catch_up_old_formal_cleanup(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    limit: usize,
    now: i64,
) -> Result<bool> {
    let running: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM revoke_cleanup_jobs WHERE namespace=? AND job_id=? AND state='running'",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if running.is_none() {
        return Ok(true);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
         VALUES(?,?,'__formal_ticket_scan',?)",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(job_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    let cursor = sqlx::query(
        "SELECT seq,cursor_src_kind,cursor_src_id FROM revoke_cleanup_frontier
         WHERE namespace=? AND job_id=? AND node_kind='__formal_ticket_scan' AND node_id=?",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(job_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let marker_seq: i64 = cursor.try_get("seq").map_err(internal)?;
    let kind: Option<String> = cursor.try_get("cursor_src_kind").map_err(internal)?;
    let id: Option<String> = cursor.try_get("cursor_src_id").map_err(internal)?;
    let last = match (kind.as_deref(), id.as_deref()) {
        (None, None) => 0,
        (Some("frontier_seq"), Some(value)) => value.parse::<i64>().map_err(|_| Error::Internal)?,
        _ => return Err(Error::Internal),
    };
    if last < 0 {
        return Err(Error::Internal);
    }
    let rows = sqlx::query(
        "SELECT seq,node_id FROM revoke_cleanup_frontier
         WHERE namespace=? AND job_id=? AND node_kind='artifact' AND seq>?
         ORDER BY seq LIMIT ?",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(last)
    .bind(i64::try_from(limit.checked_add(1).ok_or(Error::Internal)?).map_err(|_| Error::Internal)?)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let more = rows.len() > limit;
    let mut next = last;
    for row in rows.iter().take(limit) {
        let ticket_id: String = row.try_get("node_id").map_err(internal)?;
        discover_old_formal_cleanup_relation(tx, ctx, job_id, &ticket_id, now).await?;
        next = row.try_get("seq").map_err(internal)?;
    }
    sqlx::query(
        "UPDATE revoke_cleanup_frontier SET cursor_src_kind='frontier_seq',cursor_src_id=?,expanded=?
         WHERE namespace=? AND job_id=? AND seq=? AND node_kind='__formal_ticket_scan'",
    ).bind(next.to_string()).bind(if more { 0_i64 } else { 1_i64 })
        .bind(ctx.namespace()).bind(job_id).bind(marker_seq).execute(&mut **tx).await.map_err(internal)?;
    Ok(!more)
}

/// Cleanup-only identity discovery; it neither authenticates ticket_digest
/// for new use nor rewrites old objects, witnesses or dependency edges.
async fn discover_old_formal_cleanup_relation(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    ticket_physical_id: &str,
    now: i64,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT o.owner,o.body FROM objects o WHERE namespace=? AND kind='artifact' AND id=?
         AND CASE WHEN json_valid(o.body) THEN EXISTS (
             SELECT 1 FROM json_each(o.body) AS member
             WHERE member.key='record_kind' AND member.type='text'
               AND member.value='evaluation_ticket_v2'
         ) ELSE 0 END",
    )
    .bind(ctx.namespace())
    .bind(ticket_physical_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else {
        return Ok(());
    };
    let body: String = row.try_get("body").map_err(internal)?;
    let ticket = parse_strict_json(&body)?;
    if ticket["record_kind"] != "evaluation_ticket_v2" {
        return Ok(());
    }
    let owner: String = row.try_get("owner").map_err(internal)?;
    let logical_id = ticket["payload"]["id"].as_str().unwrap_or("");
    let evaluator = ticket["payload"]["evaluator_actor"].as_str().unwrap_or("");
    let envelope_matches = |value: &serde_json::Value, kind: &str, id: &str| {
        value.as_object().is_some_and(|outer| {
            outer.len() == 4
                && value["schema_version"] == "rsia.typed_artifact_envelope.v1"
                && value["record_kind"] == kind
                && value["id"] == id
                && outer
                    .get("payload")
                    .is_some_and(serde_json::Value::is_object)
        })
    };
    let ticket_node = TypedObjectRef {
        kind: "artifact".into(),
        id: ticket_physical_id.into(),
    };
    if !envelope_matches(&ticket, "evaluation_ticket_v2", ticket_physical_id)
        || identifier(logical_id).is_err()
        || identifier(evaluator).is_err()
        || owner != evaluator
        || ticket_physical_id
            != format!(
                "e05-{}",
                fingerprint(&("evaluation_ticket_v2", logical_id))?
            )
    {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            &ticket_node,
            "blocked_unknown_scope:formal_legacy_cleanup_identity",
            now,
        )
        .await;
    }
    let report_id = format!(
        "e05-{}",
        fingerprint(&("formal_evaluation_v2", logical_id))?
    );
    // One indexed lookup of this ticket's original same-logical-ID slot.
    // New derived IDs are not enumerated or inferred from source_ticket_id.
    let report_row = sqlx::query(
        "SELECT owner,body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(ctx.namespace())
    .bind(&report_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(report_row) = report_row else {
        return Ok(());
    };
    let report_body: String = report_row.try_get("body").map_err(internal)?;
    let report = parse_strict_json(&report_body)?;
    let report_owner: String = report_row.try_get("owner").map_err(internal)?;
    let report_node = TypedObjectRef {
        kind: "artifact".into(),
        id: report_id,
    };
    // The ordinary native wire keeps its real ticket in result.ticket_id; only
    // its exact schema and decision select that member. Every other payload
    // keeps the original top-level ticket identity; a missing one fails closed.
    let payload = &report["payload"];
    let report_ticket = if payload["schema_version"] == "rsia.formal_evaluation.v3"
        && payload["decision_version"] == "rsia.fixed_sample_regression.v1"
    {
        &payload["result"]["ticket_id"]
    } else {
        &payload["ticket_id"]
    };
    if !envelope_matches(&report, "formal_evaluation_v2", &report_node.id)
        || report_owner != owner
        || report_ticket.as_str() != Some(logical_id)
    {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            &report_node,
            "blocked_unknown_scope:formal_legacy_cleanup_identity",
            now,
        )
        .await;
    }
    // Unknown inner versions, including a schema tag injected into an old
    // identity, reach the one existing cleanup classifier without prefilter.
    sqlx::query(
        "INSERT OR IGNORE INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
         VALUES(?,?,'artifact',?)",
    )
    .bind(ctx.namespace())
    .bind(job_id)
    .bind(&report_node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn expand_world_scan(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    source_id: &str,
    seq: i64,
    cursor: Option<&str>,
    limit: usize,
    now: i64,
) -> Result<()> {
    let rows=sqlx::query("SELECT id,manifest FROM replay_worlds WHERE namespace=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
        .bind(ctx.namespace()).bind(cursor).bind(cursor).bind(i64::try_from(limit+1).map_err(internal)?)
        .fetch_all(&mut **tx).await.map_err(internal)?;
    for row in rows.iter().take(limit) {
        let id: String = row.try_get("id").map_err(internal)?;
        let body: String = row.try_get("manifest").map_err(internal)?;
        let value = parse_strict_json(&body)?;
        if value["schema_version"] == "rsia.redacted.v1" {
            continue;
        }
        if !matches!(
            value["schema_version"].as_str(),
            Some("rsia.replay_world.v2" | "rsia.replay_world.v3")
        ) {
            mark_unknown_scope(
                tx,
                ctx,
                job_id,
                &TypedObjectRef {
                    kind: "replay_world".into(),
                    id,
                },
                "blocked_unknown_scope:world_scan",
                now,
            )
            .await?;
            continue;
        }
        let uses_source = if value["schema_version"] == "rsia.replay_world.v3" {
            match replay_world_v3_cleanup_source_ids(ctx, &id, &value) {
                Ok(ids) => ids.iter().any(|candidate| candidate == source_id),
                Err(_) => {
                    mark_unknown_scope(
                        tx,
                        ctx,
                        job_id,
                        &TypedObjectRef {
                            kind: "replay_world".into(),
                            id,
                        },
                        "blocked_unknown_scope:current_world_scan",
                        now,
                    )
                    .await?;
                    continue;
                }
            }
        } else {
            let sources = value["manifest"]["source_closure"]
                .as_array()
                .ok_or_else(|| Error::Invalid("world source closure missing".into()))?;
            sources
                .iter()
                .any(|v| v["source_id"].as_str() == Some(source_id))
        };
        if uses_source {
            // Draft worlds also hold source content before E10 seals their ordinary edges.
            sqlx::query("INSERT OR IGNORE INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,'replay_world',?,'run',?)")
                .bind(ctx.namespace()).bind(&id).bind(source_id).execute(&mut **tx).await.map_err(internal)?;
            sqlx::query("INSERT OR IGNORE INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id) VALUES(?,?,'replay_world',?)")
                .bind(ctx.namespace()).bind(job_id).bind(id).execute(&mut **tx).await.map_err(internal)?;
        }
    }
    if rows.len() > limit {
        let id: String = rows[limit - 1].try_get("id").map_err(internal)?;
        sqlx::query("UPDATE revoke_cleanup_frontier SET cursor_src_kind='replay_world',cursor_src_id=? WHERE seq=?")
            .bind(id).bind(seq).execute(&mut **tx).await.map_err(internal)?;
    } else {
        sqlx::query("UPDATE revoke_cleanup_frontier SET expanded=1 WHERE seq=?")
            .bind(seq)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        sqlx::query("UPDATE revoke_cleanup_jobs SET processed_nodes=processed_nodes+1,updated_at=? WHERE namespace=? AND job_id=?")
            .bind(now).bind(ctx.namespace()).bind(job_id).execute(&mut **tx).await.map_err(internal)?;
    }
    Ok(())
}

async fn expand_budget_scan(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    cursor: &BudgetScanCursor,
    limit: usize,
    now: i64,
) -> Result<()> {
    let query_limit =
        i64::try_from(limit.checked_add(1).ok_or(Error::Internal)?).map_err(|_| Error::Internal)?;
    let rows = sqlx::query(
        "SELECT billing_scope,call_id,request_artifact_schema,request_artifact_body
         FROM root_budget_calls WHERE namespace=? AND request_artifact_body IS NOT NULL AND
           (? IS NULL OR billing_scope>? OR (billing_scope=? AND call_id>?))
         ORDER BY billing_scope,call_id LIMIT ?",
    )
    .bind(ctx.namespace())
    .bind(&cursor.scope)
    .bind(&cursor.scope)
    .bind(&cursor.scope)
    .bind(&cursor.call)
    .bind(query_limit)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let has_more = rows.len() > limit;
    let accepted = rows.iter().take(limit).collect::<Vec<_>>();
    for row in &accepted {
        let billing_scope: String = row.try_get("billing_scope").map_err(internal)?;
        let call_id: String = row.try_get("call_id").map_err(internal)?;
        let schema: String = row.try_get("request_artifact_schema").map_err(internal)?;
        if scan_e16_budget_call(tx, ctx, job_id, source, row, now)
            .await?
            .is_some()
        {
            continue;
        }
        if schema == "rsia.redacted.v1" {
            continue;
        }
        let body: String = row.try_get("request_artifact_body").map_err(internal)?;
        let closure =
            budget_call_source_closure(tx, ctx, &billing_scope, &call_id, &schema, &body).await?;
        let source_ids = match closure {
            BudgetRequestSources::Named(source_ids) => source_ids,
            BudgetRequestSources::Unknown(error) => {
                mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    &TypedObjectRef {
                        kind: "reservation".into(),
                        id: call_id,
                    },
                    &error,
                    now,
                )
                .await?;
                continue;
            }
        };
        if source_ids.iter().any(|value| value == &source.id) {
            let reference = crate::budget::index_budget_call_ref_in_tx(
                tx,
                ctx,
                &billing_scope,
                &call_id,
                &source_ids,
            )
            .await?;
            crate::budget::redact_budget_call_content_in_tx(
                tx,
                ctx,
                &billing_scope,
                &call_id,
                "source_revoked",
            )
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO revoke_cleanup_frontier(
                   namespace,job_id,node_kind,node_id
                 ) VALUES(?,?,'artifact',?)",
            )
            .bind(ctx.namespace())
            .bind(job_id)
            .bind(reference.id)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
    }
    if has_more {
        let last = accepted.last().ok_or(Error::Internal)?;
        let scope: String = last.try_get("billing_scope").map_err(internal)?;
        let call: String = last.try_get("call_id").map_err(internal)?;
        sqlx::query(
            "UPDATE revoke_cleanup_frontier SET cursor_src_kind=?,cursor_src_id=? WHERE seq=?",
        )
        .bind(scope)
        .bind(call)
        .bind(cursor.frontier_seq)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    } else {
        sqlx::query("UPDATE revoke_cleanup_frontier SET expanded=1 WHERE seq=?")
            .bind(cursor.frontier_seq)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        sqlx::query(
            "UPDATE revoke_cleanup_jobs SET processed_nodes=processed_nodes+1,updated_at=?
             WHERE namespace=? AND job_id=?",
        )
        .bind(now)
        .bind(ctx.namespace())
        .bind(job_id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    Ok(())
}

fn validate_observed_status_wire(value: &serde_json::Value) -> Result<()> {
    let object = value.as_object().ok_or(Error::Internal)?;
    let status = object
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    let exact_keys = |expected: &[&str]| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    };
    match status {
        "valid" if exact_keys(&["status", "quality_micros"]) => {
            let quality = object["quality_micros"].as_u64().ok_or(Error::Internal)?;
            if quality > 1_000_000 {
                return Err(Error::Internal);
            }
        }
        "repairable_failure"
            if exact_keys(&[
                "status",
                "episode_id",
                "failure_kind",
                "repair_template_digest",
                "environment_reset",
                "dispatched_repairs",
            ]) =>
        {
            identifier(object["episode_id"].as_str().ok_or(Error::Internal)?)?;
            validate_hex_digest(
                object["repair_template_digest"]
                    .as_str()
                    .ok_or(Error::Internal)?,
            )?;
            if !matches!(
                object["failure_kind"].as_str(),
                Some(
                    "compile"
                        | "implementation"
                        | "type"
                        | "output_shape"
                        | "environment"
                        | "resource"
                        | "safety"
                        | "unknown"
                )
            ) || !object["environment_reset"].is_boolean()
                || object["dispatched_repairs"]
                    .as_u64()
                    .is_none_or(|value| value > u64::from(u8::MAX))
            {
                return Err(Error::Internal);
            }
        }
        "environment_failure"
        | "hard_failure"
        | "safety_rejected"
        | "cancelled"
        | "usage_uncertain"
            if exact_keys(&["status"]) => {}
        _ => return Err(Error::Internal),
    }
    Ok(())
}

fn validate_hex_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Internal);
    }
    Ok(())
}

async fn redact_evaluation_resource_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    now: i64,
) -> Result<()> {
    let wire: ResourceEvidenceEnvelopeWire = match serde_json::from_value(value.clone()) {
        Ok(wire) => wire,
        Err(_) => {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:evaluation_resource_evidence_wire",
                now,
            )
            .await;
        }
    };
    if wire.schema_version != "rsia.typed_artifact_envelope.v1"
        || wire.record_kind != "evaluation_resource_evidence_v1"
        || wire.payload.schema_version != "rsia.evaluation_resource_evidence.v1"
        || wire.payload.cost_scope != "ticket_execution_only"
        || wire.id != node.id
    {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            node,
            "blocked_unknown_scope:evaluation_resource_evidence_identity",
            now,
        )
        .await;
    }
    identifier(&wire.payload.ticket_id)?;
    identifier(&wire.payload.billing_scope)?;
    let calls = wire
        .payload
        .calls
        .iter()
        .map(minimal_budget_call_fact)
        .collect::<Vec<_>>();
    let redacted = json!({
        "id": node.id,
        "schema_version": "rsia.redacted.v1",
        "state": "source_revoked",
        "original_kind": node.kind,
        "original_schema": wire.schema_version,
        "original_digest": evo_core::hash(body.as_bytes()),
        "metadata": {
            "record_kind": wire.record_kind,
            "ticket_id": wire.payload.ticket_id,
            "billing_scope": wire.payload.billing_scope,
            "cost_scope": wire.payload.cost_scope,
            "calls": calls,
            "formal_closure_invalidated": true,
        },
    });
    sqlx::query(
        "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind=? AND id=?",
    )
    .bind(redacted.to_string())
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "resource_evidence_content_redacted",
        now,
        json!({"calls_preserved":wire.payload.calls.len()}),
    )
    .await
}

fn minimal_budget_call_fact(call: &crate::budget::BudgetCallRecord) -> serde_json::Value {
    json!({
        "billing_scope": call.billing_scope,
        "call_id": call.call_id,
        "dispatch_group_id": call.dispatch_group_id,
        "namespace": call.namespace,
        "stage": call.stage,
        "actual_input_digest": call.actual_input_digest,
        "reserved_micros": call.reserved_micros,
        "state": call.state,
        "lease_epoch": call.lease_epoch,
        "lease_until": call.lease_until,
        "dispatch_id": call.dispatch_id,
        "provider_request_id": call.provider_request_id,
        "usage_record_id": call.usage_record_id,
        "output_digest": call.output_digest,
        "actual_cost_micros": call.actual_cost_micros,
        "actual_currency": call.actual_currency,
        "actual_pricing_version": call.actual_pricing_version,
        "execution_provenance": call.execution_provenance,
        "actual_model_digest": call.actual_model_digest,
        "request_artifact": call.request_artifact.as_ref().map(minimal_budget_artifact),
        "transport_artifact": call.transport_artifact.as_ref().map(minimal_budget_artifact),
        "response_artifact": call.response_artifact.as_ref().map(minimal_budget_artifact),
        "response_usable": call.response_usable,
        "response_block_reason_digest": call.response_block_reason.as_ref().map(|value| evo_core::hash(value.as_bytes())),
        "execution_closed": call.execution_closed,
        "execution_closed_at": call.execution_closed_at,
        "execution_close_reason_digest": call.execution_close_reason.as_ref().map(|value| evo_core::hash(value.as_bytes())),
        "terminal_reason_digest": call.terminal_reason.as_ref().map(|value| evo_core::hash(value.as_bytes())),
        "created_at": call.created_at,
        "dispatched_at": call.dispatched_at,
        "finalized_at": call.finalized_at,
    })
}

fn minimal_budget_artifact(artifact: &crate::budget::BudgetArtifact) -> serde_json::Value {
    json!({
        "schema_version": artifact.schema_version,
        "digest": artifact.digest,
        "body_redacted": true,
    })
}

async fn redact_replay_store_envelope(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    now: i64,
) -> Result<()> {
    let record_kind = value
        .get("record_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let metadata = match record_kind {
        crate::replay::REPLAY_POOL_RECORD_KIND => {
            let wire: ReplayCleanupEnvelope<evo_core::replay::ReplayPoolManifestV1> =
                match serde_json::from_value(value.clone()) {
                    Ok(wire) => wire,
                    Err(_) => {
                        return mark_unknown_scope(
                            tx,
                            ctx,
                            job_id,
                            node,
                            "blocked_unknown_scope:replay_pool_wire",
                            now,
                        )
                        .await;
                    }
                };
            if !valid_replay_cleanup_envelope(
                ctx,
                node,
                &wire.schema_version,
                &wire.id,
                &wire.namespace,
                &wire.record_kind,
                crate::replay::REPLAY_POOL_RECORD_KIND,
            ) || wire.payload.validate_identity().is_err()
                || wire.id != format!("replay-pool-{}", wire.payload.pool_digest)
            {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:replay_pool_identity",
                    now,
                )
                .await;
            }
            json!({
                "record_kind": wire.record_kind,
                "pool_digest": wire.payload.pool_digest,
                "compatibility": wire.payload.compatibility,
                "members": wire.payload.members,
            })
        }
        crate::replay::REPLAY_REPORT_RECORD_KIND => {
            let wire: ReplayCleanupEnvelope<evo_core::replay::StoredReplayReportV1> =
                match serde_json::from_value(value.clone()) {
                    Ok(wire) => wire,
                    Err(_) => {
                        return mark_unknown_scope(
                            tx,
                            ctx,
                            job_id,
                            node,
                            "blocked_unknown_scope:stored_replay_report_wire",
                            now,
                        )
                        .await;
                    }
                };
            if !valid_replay_cleanup_envelope(
                ctx,
                node,
                &wire.schema_version,
                &wire.id,
                &wire.namespace,
                &wire.record_kind,
                crate::replay::REPLAY_REPORT_RECORD_KIND,
            ) || !valid_stored_report_without_pool(&wire.payload)
                || wire.id != wire.payload.report_id
            {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_identity",
                    now,
                )
                .await;
            }
            let pool_id = format!("replay-pool-{}", wire.payload.pool_digest);
            let pool_body: Option<String> = sqlx::query_scalar(
                "SELECT body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
            )
            .bind(ctx.namespace())
            .bind(&pool_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
            let Some(pool_body) = pool_body else {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_pool_missing",
                    now,
                )
                .await;
            };
            let pool_manifest = replay_pool_from_cleanup_body(ctx, &pool_id, &pool_body);
            if !matches!(
                pool_manifest.as_ref(),
                Ok(pool) if wire.payload.validate_static(pool).is_ok()
            ) {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_pool_binding",
                    now,
                )
                .await;
            }
            let usage = wire
                .payload
                .member_reports
                .iter()
                .map(|member| {
                    json!({
                        "world_id":member.world_id,
                        "world_digest":member.world_digest,
                        "historical_usage":member.report.historical_usage,
                        "probes":member.report.probes,
                        "simulated_rounds":member.report.simulated_rounds,
                        "replay_cpu_nanos":member.report.replay_cpu_nanos,
                        "coverage":member.report.coverage,
                        "terminal":member.report.terminal,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "record_kind": wire.record_kind,
                "report_id": wire.payload.report_id,
                "pool_digest": wire.payload.pool_digest,
                "partition": wire.payload.partition,
                "policy_digest": wire.payload.policy_digest,
                "profile_digest": wire.payload.profile_digest,
                "caps_digest": wire.payload.caps_digest,
                "reports_body_digest": wire.payload.reports_body_digest,
                "semantic_reports_digest": wire.payload.semantic_reports_digest,
                "irreversible_usage": usage,
                "report_invalidated": true,
            })
        }
        crate::replay::CURRENT_REPLAY_REPORT_RECORD_KIND => {
            let wire: ReplayCleanupEnvelope<evo_core::replay::StoredReplayReportV2> =
                match parse_strict_json(body).and_then(|strict| {
                    serde_json::from_value(strict)
                        .map_err(|_| Error::Invalid("invalid current replay report wire".into()))
                }) {
                    Ok(wire) => wire,
                    Err(_) => {
                        return mark_unknown_scope(
                            tx,
                            ctx,
                            job_id,
                            node,
                            "blocked_unknown_scope:stored_replay_report_wire",
                            now,
                        )
                        .await;
                    }
                };
            if !valid_replay_cleanup_envelope(
                ctx,
                node,
                &wire.schema_version,
                &wire.id,
                &wire.namespace,
                &wire.record_kind,
                crate::replay::CURRENT_REPLAY_REPORT_RECORD_KIND,
            ) || evo_core::replay::validate_current_v3_report_without_pool(&wire.payload)
                .is_err()
                || wire.id != wire.payload.report_id
            {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_identity",
                    now,
                )
                .await;
            }
            let pool_id = format!("replay-pool-{}", wire.payload.pool_digest);
            let pool_body: Option<String> = sqlx::query_scalar(
                "SELECT body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
            )
            .bind(ctx.namespace())
            .bind(&pool_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
            let Some(pool_body) = pool_body else {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_pool_missing",
                    now,
                )
                .await;
            };
            let pool_manifest = replay_pool_from_cleanup_body(ctx, &pool_id, &pool_body);
            if !matches!(
                pool_manifest.as_ref(),
                Ok(pool) if wire.payload.validate_static(pool).is_ok()
            ) {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:stored_replay_report_pool_binding",
                    now,
                )
                .await;
            }
            let usage = wire
                .payload
                .member_reports
                .iter()
                .map(|member| {
                    json!({
                        "world_id":member.world_id,
                        "world_digest":member.world_digest,
                        "historical_usage":member.report.historical_usage,
                        "probes":member.report.probes,
                        "simulated_rounds":member.report.simulated_rounds,
                        "replay_cpu_nanos":member.report.replay_cpu_nanos,
                        "coverage":member.report.coverage,
                        "terminal":member.report.terminal,
                        "root_contract_state":member.report.root_contract_state,
                        "policy_stop_exclusions":member.report.policy_stop_exclusions,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "record_kind": wire.record_kind,
                "report_id": wire.payload.report_id,
                "pool_digest": wire.payload.pool_digest,
                "partition": wire.payload.partition,
                "policy_digest": wire.payload.policy_digest,
                "profile_digest": wire.payload.profile_digest,
                "caps_digest": wire.payload.caps_digest,
                "reports_body_digest": wire.payload.reports_body_digest,
                "semantic_reports_digest": wire.payload.semantic_reports_digest,
                "irreversible_usage": usage,
                "replay_engine_version": wire.payload.replay_engine_version,
                "attempted_worlds": wire.payload.attempted_worlds,
                "failed_worlds": wire.payload.failed_worlds,
                "report_invalidated": true,
            })
        }
        _ => {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:replay_store_record_kind",
                now,
            )
            .await;
        }
    };
    let redacted = json!({
        "id":node.id,
        "schema_version":"rsia.redacted.v1",
        "state":"source_revoked",
        "original_kind":node.kind,
        "original_schema":crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA,
        "original_digest":evo_core::hash(body.as_bytes()),
        "metadata":metadata,
    });
    sqlx::query(
        "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind=? AND id=?",
    )
    .bind(redacted.to_string())
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "replay_content_redacted",
        now,
        json!({"record_kind":record_kind}),
    )
    .await
}

pub(crate) fn e16_schema_kind(schema: &str) -> Option<&'static str> {
    match schema {
        "rsia.e16.source_selection.v1" => Some("source_selection"),
        "rsia.e16.import_source.v1" => Some("import_source"),
        // Both known result versions are cleaned alike; a v1 result is legacy
        // history, never re-derived.
        "rsia.e16.import_result.v1" | "rsia.e16.import_result.v2" => Some("import_result"),
        "rsia.e16.staged_asset.v1" => Some("staged_asset"),
        "rsia.e16.seed_install.v1" | "rsia.e16.seed_install.v2" => Some("seed_install"),
        "rsia.e16.export_attempt.v1" => Some("export_attempt"),
        "rsia.e16.delivery_audit.v1" => Some("delivery_audit"),
        "rsia.e16.export_attempt.v2" => Some("export_attempt"),
        "rsia.e16.delivery_audit.v2" => Some("delivery_audit"),
        _ => None,
    }
}

pub(crate) fn validate_e16_envelope(
    namespace: &str,
    node: &TypedObjectRef,
    value: &serde_json::Value,
) -> Result<&'static str> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::Conflict("E16 envelope is not an object".into()))?;
    const FIELDS: [&str; 11] = [
        "schema_version",
        "id",
        "namespace",
        "owner_actor",
        "request_key",
        "input_digest",
        "created_at",
        "updated_at",
        "source_refs",
        "revoke_watermark",
        "payload",
    ];
    if object.len() != FIELDS.len() || FIELDS.iter().any(|field| !object.contains_key(*field)) {
        return Err(Error::Conflict(
            "unknown or missing E16 envelope field".into(),
        ));
    }
    let schema = object["schema_version"]
        .as_str()
        .and_then(e16_schema_kind)
        .ok_or_else(|| Error::Conflict("unknown E16 schema".into()))?;
    if object["id"].as_str() != Some(node.id.as_str()) {
        return Err(Error::Conflict("invalid E16 envelope identity".into()));
    }
    validate_e16_metadata(namespace, object, schema)?;
    Ok(schema)
}

/// Authority fields retained by canonical E16 cleanup, shared with the live envelope.
pub(crate) fn validate_e16_metadata(
    namespace: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    schema: &str,
) -> Result<()> {
    if [
        "namespace",
        "owner_actor",
        "request_key",
        "input_digest",
        "created_at",
        "updated_at",
        "source_refs",
        "revoke_watermark",
        "payload",
    ]
    .iter()
    .any(|key| !object.contains_key(*key))
    {
        return Err(Error::Conflict("missing E16 authority field".into()));
    }
    if object["namespace"].as_str() != Some(namespace)
        || object["owner_actor"].as_str().is_none()
        || object["request_key"].as_str().is_none()
        || !is_lower_hex_digest(&object["input_digest"])
        || object["created_at"].as_i64().is_none()
        || object["updated_at"].as_i64().is_none()
        || object["revoke_watermark"].as_u64().is_none()
        || !object["payload"].is_object()
    {
        return Err(Error::Conflict(
            "invalid E16 envelope authority fields".into(),
        ));
    }
    let refs = object["source_refs"]
        .as_array()
        .ok_or_else(|| Error::Conflict("invalid E16 source_refs".into()))?;
    let digest_field = if matches!(
        schema,
        "source_selection" | "import_source" | "import_result"
    ) {
        "content_digest"
    } else {
        "digest"
    };
    for source in refs {
        let source = source
            .as_object()
            .ok_or_else(|| Error::Conflict("invalid E16 source_ref".into()))?;
        if source.len() != 3
            || !source.contains_key("kind")
            || !source.contains_key("id")
            || !source.contains_key(digest_field)
            || source["kind"].as_str().is_none()
            || source["id"].as_str().is_none()
            || !is_lower_hex_digest(&source[digest_field])
        {
            return Err(Error::Conflict(
                "unknown or invalid E16 source_ref field".into(),
            ));
        }
    }
    Ok(())
}

fn is_lower_hex_digest(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(crate) const E16_RETAINED_PAYLOAD_FIELDS: &[&str] = &[
    "publisher",
    "asset_id",
    "package_kind",
    "kind",
    "manifest_digest",
    "content_blob_digest",
    "content_bytes",
    "baseline_blob_digest",
    "baseline_digest",
    "local_blob_digest",
    "local_digest",
    "upstream_blob_digest",
    "upstream_digest",
    "environment_digest",
    "package_schema_version",
    "compiler_version",
    "candidate_ref",
    "final_projection_digest",
    "output_blob_digest",
    "output_bytes",
    "projection_blob_digest",
    "projection_blob_bytes",
    "package_tree_digest",
    "delivered_bytes",
    "delivery_kind",
    "local_export_ref",
    "completion_receipt_digest",
    "destination_scope",
    "delivery_audit_id",
    "budget_ref",
    "root_budget_id",
    "billing_scope",
    "payment_subject",
];

#[allow(clippy::too_many_arguments)]
async fn cleanup_e16_envelope(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    export_directory_rechecked: bool,
    now: i64,
) -> Result<()> {
    let kind = match validate_e16_envelope(ctx.namespace(), node, value) {
        Ok(kind) => kind,
        Err(_) => {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:invalid_e16_envelope",
                now,
            )
            .await;
        }
    };
    if kind == "delivery_audit" {
        let mut preserved = value.clone();
        preserved["updated_at"] = json!(now);
        preserved["payload"]["revoked"] = json!(true);
        sqlx::query(
            "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(preserved.to_string())
        .bind(ctx.namespace())
        .bind(&node.id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(node),
            "delivery_fact_preserved",
            now,
            json!({"remote_erasure_not_claimed":true,"revoked":true,"revocation_notice_sent":false}),
        )
        .await?;
        return Ok(());
    }

    if kind == "export_attempt"
        && value["schema_version"].as_str() == Some("rsia.e16.export_attempt.v2")
        && !export_directory_rechecked
    {
        remove_controlled_local_export(store, ctx, node, job_id, tx, now, "exports", &node.id)
            .await?;
    }

    let mut blob_digests = Vec::new();
    let payload = &value["payload"];
    let fields = e16_blob_digest_fields(value["schema_version"].as_str().unwrap_or_default());
    for field in fields {
        if let Some(digest) = payload.get(*field).and_then(|value| value.as_str()) {
            if !is_lower_hex_digest(&serde_json::Value::String(digest.into())) {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:invalid_e16_blob_digest",
                    now,
                )
                .await;
            }
            blob_digests.push(digest.to_string());
        }
    }

    let mut retained_payload = serde_json::Map::new();
    for &field in E16_RETAINED_PAYLOAD_FIELDS {
        if let Some(item) = payload.get(field) {
            retained_payload.insert(field.into(), item.clone());
        }
    }
    let redacted = json!({
        "id": node.id,
        "schema_version": "rsia.redacted.v1",
        "state": if kind == "export_attempt" { "revoked" } else { "source_revoked" },
        "original_kind": kind,
        "original_schema": value["schema_version"],
        "original_digest": evo_core::hash(body.as_bytes()),
        "metadata": {
            "namespace": value["namespace"],
            "owner_actor": value["owner_actor"],
            "request_key": value["request_key"],
            "input_digest": value["input_digest"],
            "created_at": value["created_at"],
            "updated_at": value["updated_at"],
            "source_refs": value["source_refs"],
            "revoke_watermark": value["revoke_watermark"],
            "payload": retained_payload,
        },
    });
    sqlx::query(
        "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(redacted.to_string())
    .bind(ctx.namespace())
    .bind(&node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    for digest in blob_digests {
        remove_e16_blob_if_unreferenced(tx, store, ctx, job_id, &digest).await?;
    }
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "e16_content_redacted",
        now,
        json!({"record_kind":kind,"blob_candidates":fields.len()}),
    )
    .await
}

/// The caller's typed node is a member of this job's persisted frontier. A
/// directory failure from another job in the same namespace still belongs to
/// that node. EXISTS avoids loading the history; its scan cost is not constant.
async fn has_failed_local_export(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    node: &TypedObjectRef,
) -> Result<bool> {
    if node.kind != "artifact" {
        return Ok(false);
    }
    let failed: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM revoke_cleanup_events
           WHERE namespace=? AND node_kind=? AND node_id=?
             AND event_kind='blocked_unknown_scope'
             AND json_extract(details,'$.error') IN (
               'blocked_unknown_scope:invalid_local_export_id',
               'blocked_unknown_scope:linked_local_export_ancestor',
               'blocked_unknown_scope:invalid_local_export_directory'))",
    )
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(failed == 1)
}

/// Remove one controlled local export directory, `<root>/<namespace-hash>/<directory>`
/// under the store root (`exports` for E16 package exports, `trace-exports` for
/// trace exports), recording that remote erasure is not claimed.
#[allow(clippy::too_many_arguments)]
async fn remove_controlled_local_export(
    store: &Store,
    ctx: &Context,
    node: &TypedObjectRef,
    job_id: &str,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: i64,
    root: &str,
    directory: &str,
) -> Result<()> {
    // The directory id names the directory under the namespace export root;
    // an id such as ".." would resolve outside it, so refuse to touch the
    // filesystem for any id the export entry points would not accept.
    if crate::validate_export_id(directory).is_err() {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            node,
            "blocked_unknown_scope:invalid_local_export_id",
            now,
        )
        .await;
    }
    let export_root = store.root.join(root);
    let namespace_root = export_root.join(evo_core::hash(ctx.namespace().as_bytes()));
    for ancestor in [&export_root, &namespace_root] {
        match tokio::fs::symlink_metadata(ancestor).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:linked_local_export_ancestor",
                    now,
                )
                .await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                insert_cleanup_event(
                    tx,
                    ctx.namespace(),
                    job_id,
                    Some(node),
                    "local_export_directory_absent",
                    now,
                    json!({}),
                )
                .await?;
                return Ok(());
            }
            Err(error) => return Err(internal(error)),
        }
    }
    let path = namespace_root.join(directory);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:invalid_local_export_directory",
                    now,
                )
                .await;
            }
            tokio::fs::remove_dir_all(&path).await.map_err(internal)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent.to_path_buf()).await?;
            }
            insert_cleanup_event(
                tx,
                ctx.namespace(),
                job_id,
                Some(node),
                "local_export_directory_deleted",
                now,
                json!({"remote_erasure_not_claimed":true}),
            )
            .await?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            insert_cleanup_event(
                tx,
                ctx.namespace(),
                job_id,
                Some(node),
                "local_export_directory_absent",
                now,
                json!({}),
            )
            .await?;
        }
        Err(error) => return Err(internal(error)),
    }
    Ok(())
}

async fn cleanup_e16_blob(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    now: i64,
) -> Result<()> {
    if !is_lower_hex_digest(&serde_json::Value::String(node.id.clone())) {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            node,
            "blocked_unknown_scope:invalid_blob_digest",
            now,
        )
        .await;
    }
    let removed = remove_e16_blob_if_unreferenced(tx, store, ctx, job_id, &node.id).await?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        if removed {
            "blob_content_deleted"
        } else {
            "shared_blob_preserved"
        },
        now,
        json!({}),
    )
    .await
}

async fn remove_e16_blob_if_unreferenced(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    digest: &str,
) -> Result<bool> {
    let live_edges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dependencies d
         JOIN objects o ON o.namespace=d.namespace AND o.kind=d.src_kind AND o.id=d.src_id
         WHERE d.namespace=? AND d.dst_kind='blob' AND d.dst_id=?
         AND json_extract(o.body,'$.schema_version') IN (
           'rsia.e16.import_source.v1','rsia.e16.staged_asset.v1',
           'rsia.e16.seed_install.v1','rsia.e16.seed_install.v2','rsia.e16.export_attempt.v1',
           'rsia.e16.export_attempt.v2'
         )
         AND NOT EXISTS (
           SELECT 1 FROM revoke_cleanup_frontier f
           WHERE f.namespace=d.namespace AND f.job_id=?
             AND f.node_kind=d.src_kind AND f.node_id=d.src_id
         )",
    )
    .bind(ctx.namespace())
    .bind(digest)
    .bind(job_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    if live_edges != 0 || e16_payload_has_live_blob_reference(tx, ctx, job_id, digest).await? {
        return Ok(false);
    }
    let path = store
        .root
        .join("blobs")
        .join(evo_core::hash(ctx.namespace().as_bytes()))
        .join(digest);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(Error::Conflict("refusing to remove linked E16 blob".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(Error::Conflict(
                        "refusing to remove hard-linked E16 blob".into(),
                    ));
                }
            }
            tokio::fs::remove_file(path).await.map_err(internal)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(internal(error)),
    }
}

/// Every payload field of an E16 record schema that can carry a registered
/// blob digest. This is the single source of truth for both content
/// redaction (which blobs a revoked record may release) and shared-blob
/// liveness (which blobs a surviving record may keep).
fn e16_blob_digest_fields(schema: &str) -> &'static [&'static str] {
    match schema {
        "rsia.e16.import_source.v1" => &["raw_blob_digest"],
        "rsia.e16.staged_asset.v1" => &["content_blob_digest"],
        "rsia.e16.seed_install.v1" | "rsia.e16.seed_install.v2" => &[
            "baseline_blob_digest",
            "local_blob_digest",
            "upstream_blob_digest",
        ],
        "rsia.e16.export_attempt.v1" => &["output_blob_digest"],
        "rsia.e16.export_attempt.v2" => &["projection_blob_digest"],
        _ => &[],
    }
}

/// Whether an E16 record is in a state that keeps the blobs it references
/// alive. Shared blob removal counts only live references, so a record that
/// has been aborted, failed, quarantined, or revoked (redacted records no
/// longer carry an E16 schema and are never consulted) must not preserve a
/// blob once its last live referrer is revoked.
///
/// The live sets are derived from each record kind's state enum:
/// - import_source (`payload.status`, engine `ImportSourceReadStatus`):
///   `prepared` (registration in flight) and `ready`; the read-failure
///   statuses (`missing`, `permission_denied`, `source_changed`,
///   `zero_records`, `invalid_file`, `too_large`) are not live.
/// - staged_asset (`payload.state`, engine `StagedAssetState`): `prepared`
///   (registration in flight) and `staged`; `quarantined`, `aborted` and
///   `failed` are not live.
/// - seed_install (`payload.status`, engine `SeedStatus`): `prepared`
///   (registration in flight, same protocol as a prepared staged asset) and
///   `installed`; `staged`, `quarantined` and `unknown` are not live.
/// - export_attempt v1/v2 (`payload.state`, engine `ExportAttemptState`):
///   `prepared` and `completed`; `aborted`, `revoked` and `failed` are not
///   live.
///
/// A record with a missing or unrecognised state is never live.
fn e16_record_keeps_blobs_alive(schema: &str, payload: &serde_json::Value) -> bool {
    let (field, live): (&str, &[&str]) = match schema {
        "rsia.e16.import_source.v1" => ("status", &["prepared", "ready"]),
        "rsia.e16.staged_asset.v1" => ("state", &["prepared", "staged"]),
        "rsia.e16.seed_install.v1" | "rsia.e16.seed_install.v2" => {
            ("status", &["prepared", "installed"])
        }
        "rsia.e16.export_attempt.v1" | "rsia.e16.export_attempt.v2" => {
            ("state", &["prepared", "completed"])
        }
        _ => return false,
    };
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|state| live.contains(&state))
}

async fn e16_payload_has_live_blob_reference(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    digest: &str,
) -> Result<bool> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT o.body FROM objects o
         WHERE o.namespace=? AND o.kind='artifact'
         AND json_extract(o.body,'$.schema_version') IN (
           'rsia.e16.import_source.v1','rsia.e16.staged_asset.v1',
           'rsia.e16.seed_install.v1','rsia.e16.seed_install.v2','rsia.e16.export_attempt.v1',
           'rsia.e16.export_attempt.v2'
         )
         AND (
           json_extract(o.body,'$.payload.raw_blob_digest')=?
           OR json_extract(o.body,'$.payload.content_blob_digest')=?
           OR json_extract(o.body,'$.payload.baseline_blob_digest')=?
           OR json_extract(o.body,'$.payload.local_blob_digest')=?
           OR json_extract(o.body,'$.payload.upstream_blob_digest')=?
           OR json_extract(o.body,'$.payload.output_blob_digest')=?
           OR json_extract(o.body,'$.payload.projection_blob_digest')=?
         )
         AND NOT EXISTS (
           SELECT 1 FROM revoke_cleanup_frontier f
           WHERE f.namespace=o.namespace AND f.job_id=?
             AND f.node_kind=o.kind AND f.node_id=o.id
         )",
    )
    .bind(ctx.namespace())
    .bind(digest)
    .bind(digest)
    .bind(digest)
    .bind(digest)
    .bind(digest)
    .bind(digest)
    .bind(digest)
    .bind(job_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    for body in rows {
        let value: serde_json::Value = serde_json::from_str(&body).map_err(internal)?;
        let node = TypedObjectRef {
            kind: "artifact".into(),
            id: value["id"].as_str().unwrap_or_default().into(),
        };
        validate_e16_envelope(ctx.namespace(), &node, &value)?;
        let schema = value["schema_version"].as_str().unwrap_or_default();
        let payload = &value["payload"];
        if !e16_record_keeps_blobs_alive(schema, payload) {
            continue;
        }
        if e16_blob_digest_fields(schema)
            .iter()
            .any(|field| payload.get(*field).and_then(serde_json::Value::as_str) == Some(digest))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn replay_pool_from_cleanup_body(
    ctx: &Context,
    pool_id: &str,
    body: &str,
) -> Result<evo_core::replay::ReplayPoolManifestV1> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(internal)?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA)
    {
        let wire: ReplayCleanupEnvelope<evo_core::replay::ReplayPoolManifestV1> =
            serde_json::from_value(value).map_err(internal)?;
        if !valid_replay_cleanup_envelope(
            ctx,
            &TypedObjectRef {
                kind: "artifact".into(),
                id: pool_id.into(),
            },
            &wire.schema_version,
            &wire.id,
            &wire.namespace,
            &wire.record_kind,
            crate::replay::REPLAY_POOL_RECORD_KIND,
        ) {
            return Err(Error::Conflict("invalid replay pool envelope".into()));
        }
        wire.payload.validate_identity()?;
        return Ok(wire.payload);
    }
    let object = value.as_object().ok_or(Error::Internal)?;
    let expected = [
        "id",
        "schema_version",
        "state",
        "original_kind",
        "original_schema",
        "original_digest",
        "metadata",
    ];
    if object.len() != expected.len()
        || !expected.iter().all(|key| object.contains_key(*key))
        || value["id"] != pool_id
        || value["schema_version"] != "rsia.redacted.v1"
        || value["state"] != "source_revoked"
        || value["original_kind"] != "artifact"
        || value["original_schema"] != crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA
    {
        return Err(Error::Conflict("invalid redacted replay pool".into()));
    }
    let metadata = value["metadata"].as_object().ok_or(Error::Internal)?;
    let metadata_keys = ["record_kind", "pool_digest", "compatibility", "members"];
    if metadata.len() != metadata_keys.len()
        || !metadata_keys.iter().all(|key| metadata.contains_key(*key))
        || value["metadata"]["record_kind"] != crate::replay::REPLAY_POOL_RECORD_KIND
    {
        return Err(Error::Conflict(
            "invalid redacted replay pool metadata".into(),
        ));
    }
    let manifest = evo_core::replay::ReplayPoolManifestV1 {
        schema_version: evo_core::replay::REPLAY_POOL_SCHEMA.into(),
        compiler_version: evo_core::replay::REPLAY_POOL_COMPILER_VERSION.into(),
        pool_digest: serde_json::from_value(value["metadata"]["pool_digest"].clone())
            .map_err(internal)?,
        compatibility: serde_json::from_value(value["metadata"]["compatibility"].clone())
            .map_err(internal)?,
        members: serde_json::from_value(value["metadata"]["members"].clone()).map_err(internal)?,
    };
    manifest.validate_identity()?;
    if manifest.pool_digest != pool_id.strip_prefix("replay-pool-").unwrap_or("") {
        return Err(Error::Conflict("redacted replay pool id mismatch".into()));
    }
    Ok(manifest)
}

fn valid_replay_cleanup_envelope(
    ctx: &Context,
    node: &TypedObjectRef,
    schema_version: &str,
    id: &str,
    namespace: &str,
    record_kind: &str,
    expected_kind: &str,
) -> bool {
    schema_version == crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA
        && id == node.id
        && namespace == ctx.namespace()
        && record_kind == expected_kind
}

fn valid_stored_report_without_pool(report: &evo_core::replay::StoredReplayReportV1) -> bool {
    // Retirement never revokes historical content or changes its old identity.
    evo_core::replay::validate_historical_v2_report_without_pool(report).is_ok()
}

fn replay_world_v3_cleanup_source_ids(
    ctx: &Context,
    id: &str,
    value: &serde_json::Value,
) -> Result<Vec<String>> {
    let world: evo_core::replay::ReplayWorldV3 = serde_json::from_value(value.clone())
        .map_err(|_| Error::Invalid("invalid current replay cleanup world".into()))?;
    if world.manifest.world_id != id || world.manifest.root_source.namespace != ctx.namespace() {
        return Err(Error::Invalid(
            "current replay cleanup identity differs".into(),
        ));
    }
    if world.sealed_digest.is_some() {
        world.validate_sealed()?;
    } else {
        world.validate_draft()?;
    }
    let image = evo_core::replay::ReplayRegistrationImageV1::parse_for_source(
        &world.registration_image_utf8,
        &world.manifest.root_source,
    )?;
    let mut sources: Vec<_> = world
        .manifest
        .source_closure
        .iter()
        .map(|s| s.source_id.clone())
        .collect();
    sources.extend(
        image
            .source_closure
            .into_iter()
            .filter_map(|(kind, id, _)| (kind == "run").then_some(id)),
    );
    sources.sort();
    sources.dedup();
    Ok(sources)
}

async fn cleanup_node_content(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    source: &TypedObjectRef,
    node: &TypedObjectRef,
    now: i64,
) -> Result<()> {
    if node.kind == "run" && node.kind == source.kind && node.id == source.id {
        sqlx::query("DELETE FROM objects WHERE namespace=? AND kind='run' AND id=?")
            .bind(ctx.namespace())
            .bind(&node.id)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(node),
            "source_content_deleted",
            now,
            json!({}),
        )
        .await?;
        return Ok(());
    }
    if node.kind == "replay_world" {
        let body: Option<String> =
            sqlx::query_scalar("SELECT manifest FROM replay_worlds WHERE namespace=? AND id=?")
                .bind(ctx.namespace())
                .bind(&node.id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(internal)?;
        let Some(body) = body else {
            return Ok(());
        };
        let value = parse_strict_json(&body)?;
        if value["schema_version"] == "rsia.redacted.v1" {
            return Ok(());
        }
        if !matches!(
            value["schema_version"].as_str(),
            Some("rsia.replay_world.v2" | "rsia.replay_world.v3")
        ) {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:replay_world",
                now,
            )
            .await;
        }
        if value["schema_version"] == "rsia.replay_world.v3"
            && replay_world_v3_cleanup_source_ids(ctx, &node.id, &value).is_err()
        {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:current_replay_world",
                now,
            )
            .await;
        }
        let Some(items) = value["transitions"].as_array() else {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:world_transitions",
                now,
            )
            .await;
        };
        let transitions=items.iter().map(|v| {
            let usage:HistoricalUsageWire=serde_json::from_value(v["actual_usage"].clone()).map_err(internal)?;
            let status=if v["outcome"]["outcome"]=="observed" {
                validate_observed_status_wire(&v["outcome"]["status"])?;
                v["outcome"]["status"].clone()
            } else {serde_json::Value::Null};
            Ok(json!({"record_id":v["record_id"],"record_seq":v["record_seq"],"actual_usage":usage,"outcome":v["outcome"]["outcome"],"observed_status":status}))
        }).collect::<Result<Vec<_>>>();
        let Ok(transitions) = transitions else {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:world_historical_facts",
                now,
            )
            .await;
        };
        let redacted = json!({"schema_version":"rsia.redacted.v1","state":"source_revoked","original_schema":value["schema_version"],
            "original_digest":evo_core::hash(body.as_bytes()),"sealed_digest":value["sealed_digest"],
            "metadata":minimal_metadata(&value["manifest"]),"historical_transitions":transitions});
        sqlx::query("UPDATE replay_worlds SET manifest=? WHERE namespace=? AND id=?")
            .bind(redacted.to_string())
            .bind(ctx.namespace())
            .bind(&node.id)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        return insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(node),
            "world_content_redacted",
            now,
            json!({"historical_usage_preserved":true}),
        )
        .await;
    }
    if node.kind == "blob" {
        return cleanup_e16_blob(tx, store, ctx, job_id, node, now).await;
    }
    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT body, CASE WHEN objects.kind='artifact' AND json_valid(objects.body)
         THEN EXISTS (
             SELECT 1 FROM json_each(objects.body) AS member
             WHERE member.key='schema_version' AND member.type='text'
               AND member.value='rsia.host_surface_record.v2'
         ) ELSE 0 END, owner
         FROM objects WHERE namespace=? AND kind=? AND id=?",
    )
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let (body, value, owner) = match row {
        Some((body, current_host_surface, owner)) => {
            let value =
                crate::decode_object_row_body::<serde_json::Value>(&body, current_host_surface)?;
            (Some(body), Some(value), Some(owner))
        }
        None => (None, None, None),
    };
    let redacted_export = node.kind == "artifact"
        && value.as_ref().is_some_and(|value| {
            value["schema_version"] == "rsia.redacted.v1"
                && value["original_schema"] == "rsia.e16.export_attempt.v2"
        });
    let export_directory_rechecked =
        has_failed_local_export(tx, ctx, node).await? || redacted_export;
    if export_directory_rechecked {
        if let Some(value) = &value {
            // Bind an extant export record to this namespace and typed identity
            // before touching its controlled directory. Absent historical nodes
            // still require the path check; no user-supplied path is consulted.
            if (value["schema_version"] == "rsia.e16.export_attempt.v2"
                && validate_e16_envelope(ctx.namespace(), node, value).is_err())
                || (redacted_export
                    && (value["id"].as_str() != Some(node.id.as_str())
                        || value["original_kind"] != "export_attempt"
                        || value["metadata"]["namespace"].as_str() != Some(ctx.namespace())))
            {
                return mark_unknown_scope(
                    tx,
                    ctx,
                    job_id,
                    node,
                    "blocked_unknown_scope:invalid_e16_envelope",
                    now,
                )
                .await;
            }
        }
        remove_controlled_local_export(store, ctx, node, job_id, tx, now, "exports", &node.id)
            .await?;
    }
    let (Some(body), Some(value), Some(owner)) = (body, value, owner) else {
        return Ok(());
    };
    let schema = value
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let record_kind = value
        .get("record_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if node.kind == "artifact" && e16_schema_kind(schema).is_some() {
        return cleanup_e16_envelope(
            tx,
            store,
            ctx,
            job_id,
            node,
            &body,
            &value,
            export_directory_rechecked,
            now,
        )
        .await;
    }
    if node.kind == "artifact" && schema == crate::TRACE_EXPORT_SCHEMA {
        // Controlled local cleanup of the export's own published tree, if any;
        // the record is then redacted below, keeping its delivery facts and
        // its statement that copies taken before this cleanup are not erased.
        let Some(export_id) = value["export_id"].as_str() else {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:invalid_trace_export_record",
                now,
            )
            .await;
        };
        remove_controlled_local_export(
            store,
            ctx,
            node,
            job_id,
            tx,
            now,
            crate::TRACE_EXPORT_ROOT,
            export_id,
        )
        .await?;
    }
    if node.kind == "artifact" && schema == crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA {
        return redact_replay_store_envelope(tx, ctx, job_id, node, &body, &value, now).await;
    }
    if matches!(
        (node.kind.as_str(), schema, record_kind),
        (
            "artifact",
            "rsia.typed_artifact_envelope.v1",
            "evaluation_resource_evidence_v1"
        )
    ) {
        return redact_evaluation_resource_evidence(tx, ctx, job_id, node, &body, &value, now)
            .await;
    }
    if node.kind == "artifact" && schema == crate::typed_budget::E16_BUDGET_REF_SCHEMA {
        let Some(call) =
            crate::typed_budget::history_call_for_ref_in_tx(tx, ctx, node, &body).await?
        else {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:e16_budget_ref_binding",
                now,
            )
            .await;
        };
        crate::budget::redact_e16_budget_call_content_in_tx(
            tx,
            ctx,
            &call.billing_scope,
            &call.call_id,
        )
        .await?;
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(node),
            "historical_fact_preserved",
            now,
            json!({"kind":node.kind}),
        )
        .await?;
        return Ok(());
    }
    if node.kind == "artifact"
        && schema == "rsia.typed_artifact_envelope.v1"
        && record_kind == "formal_evaluation_v2"
    {
        // Exact payload-schema classes of the one catalog, never a shape guess:
        // registered ordinary reports (no schema, or the native wrapper) stay
        // under the original preservation policy below; only the exact derived
        // name reaches the derived redactor; any other name is outside the table.
        let payload = &value["payload"];
        let error = match payload.get("schema_version").map(serde_json::Value::as_str) {
            None if formal_ordinary_inner_version(None, payload).is_some() => None,
            None => Some("blocked_unknown_scope:formal_judgment_old_wire_or_domain"),
            Some(Some("rsia.formal_judgment.v1")) => {
                return redact_formal_judgment_derived(tx, ctx, job_id, node, &body, &value, now)
                    .await;
            }
            Some(Some("rsia.formal_evaluation.v3"))
                if formal_native_wire_known(tx, ctx, node, &owner, &value).await? =>
            {
                None
            }
            Some(Some("rsia.formal_evaluation.v3")) => {
                Some("blocked_unknown_scope:formal_judgment_native_wire_or_domain")
            }
            Some(_) => Some("blocked_unknown_scope:formal_judgment_version_outside_table"),
        };
        if let Some(error) = error {
            return mark_unknown_scope(tx, ctx, job_id, node, error, now).await;
        }
    }
    let preserve = match (node.kind.as_str(), schema, record_kind) {
        // These historical accounting facts retain their consumer's schema and spent counters.
        ("artifact", "rsia.budget_call_ref.v1", _) => true,
        // Compiler bindings retain only ids, digests, purposes, ordering and
        // typed dependency metadata; they contain no original input or P/B body.
        (
            "artifact",
            "rsia.persisted_compiler_binding.v1" | "rsia.persisted_compiler_binding.v2",
            _,
        ) => true,
        (
            "artifact",
            "rsia.typed_artifact_envelope.v1",
            "exposure_ledger_v1"
            | "alpha_claim_reservation_v1"
            | "evaluation_ticket_v2"
            | "evaluation_ticket_issue_receipt_v1"
            | "execution_receipt_v2"
            | "independent_grader_receipt_v2"
            | "formal_evaluation_v2"
            | "development_control_v1"
            | "development_execution_receipt_v1"
            | "development_grader_receipt_v1"
            | "development_run_receipt_v1",
        ) => true,
        ("release", "rsia.persistent_release.v1", _)
        | ("pointer", "rsia.profile_pointer.v1", _)
        | ("tombstone", "rsia.revoke_tombstone.v1", _) => true,
        // Monitoring facts that are scope management, accounting or results: the
        // consolidation scope index, the terminal run record (outcome and the
        // budget call ids it reconciles) and an environment-drift invalidation
        // survive a source revocation; revocation must not erase spend or history.
        (
            "artifact",
            "rsia.monitoring.consolidation_scope.v1"
            | "rsia.monitoring.consolidation_run.v1"
            | "rsia.monitoring.environment_drift.v1",
            _,
        ) => true,
        // E12 curriculum facts that are control registration, scheduling or quota:
        // the administrator's profile, the coverage probe job (limits, attempt
        // outcomes, terminal) and the idempotent schedule receipt survive a source
        // revocation; revocation must not erase the control record or the
        // scheduling history. They hold ids, digests and counters only.
        (
            "artifact",
            "rsia.curriculum_artifact_envelope.v1",
            "curriculum_profile_v1" | "coverage_probe_job_v1" | "probe_schedule_receipt_v1",
        ) => true,
        // E09 practice: the Admin registration that authorizes a K=3 comparison
        // is a management fact, not source-derived content; it survives a revocation.
        ("artifact", "rsia.practice_registration.v1", _) => true,
        // E09 practice: the unique registration-to-set binding records that a K=3
        // registration was spent; a revocation never reopens that authorization.
        ("artifact", "rsia.practice_registration_binding.v1", _) => true,
        // E10/E11 replay-economic facts: the preregistered experiment, the job
        // started over it, the per-component cost receipts and the blocked report
        // are the record of an experiment that was run and of what it was billed.
        // A revocation must neither make that experiment look as if it never
        // happened nor reverse an amount (plan §11: a diagnostic experiment that
        // happened is kept as a fact, and amounts and history are not reversed by
        // deletion). They hold identifiers, digests, enums, counters and amounts,
        // plus the reason strings of a job, a cost receipt and a report: a fixed
        // vocabulary generated by the code, or the sentence the Evaluator or Admin
        // typed when cancelling the job or measuring a cost. None of them carries
        // source content: no excerpt, model output or answer. What the experiment
        // selected (world, pool, replay report) is redacted by the entries of
        // those records, and the experiment and job edges (experiment -> sources
        // and selection report, job -> experiment) are how a revoked source
        // reaches them. The record kinds are listed one by one: any other record
        // kind or schema version of this envelope still stops the job at
        // `blocked_unknown_scope`.
        (
            "artifact",
            "rsia.replay_economic_artifact_envelope.v1",
            "replay_economic_experiment_v1"
            | "replay_economic_job_v1"
            | "replay_economic_cost_receipt_v1"
            | "replay_economic_report_v1"
            | "replay_economic_report_v2",
        ) => true,
        ("evaluation", "", _) => value.as_object().is_some_and(|m| {
            m.len() == 3
                && ["id", "score", "state"]
                    .iter()
                    .all(|key| m.contains_key(*key))
        }),
        // E07 management jobs are the audit record of an action that already
        // happened: who asked for which operation, how far it got and how it
        // ended. `ManagementJob` and its `ManagementResult` hold ids, digests,
        // enums, counters, booleans and fixed-vocabulary strings only (no
        // source-derived text; the request payload lives in the private input,
        // redacted below), so a job survives a source revocation. Revocation must
        // not erase a performed action.
        ("job", "rsia.management_job.v1" | "rsia.management_job.v2", _) => true,
        _ => false,
    };
    let redact = match (node.kind.as_str(), schema, record_kind) {
        (_, "rsia.redacted.v1", _) => return Ok(()),
        // Only complete native Host authorities qualify, bound to this closure node.
        ("run", "rsia.optimization.source.v1", _) => valid_native_trace_authority(node, &value),
        ("artifact", "rsia.optimization.stage_fact.v1" | "rsia.optimization.stage_fact.v2", _) => {
            matches!(
                value["kind"].as_str(),
                Some(
                    "request_prepared"
                        | "response_observed"
                        | "edit_compiled"
                        | "development_request_prepared"
                        | "development_observed"
                        | "terminal_no_change"
                        | "terminal_rejected"
                        | "terminal_candidate"
                        | "step_prepared"
                        | "step_completed"
                        | "dispatch_prepared"
                        | "dispatch_observed"
                        | "terminal_uncertain"
                )
            )
        }
        (
            "artifact",
            "rsia.typed_artifact_envelope.v1",
            "registered_evaluation_control_v41"
            | "protected_holdout_v41"
            | "evaluation_execution_output_v1"
            | "development_execution_output_v1",
        ) => true,
        (
            "artifact",
            "rsia.exploration_artifact_envelope.v1",
            "exploration_world_v1"
            | "exploration_node_v1"
            | "exploration_dispatch_v1"
            | "optimization_history_v1",
        ) => true,
        // E14.2a MetaTrial: the trial record carries the two improver contents,
        // the S0 digests and the source closure it forks. That is derived from the
        // trusted runs it names, so it is redacted like the two worlds it forks.
        // Its dependency edges (trial -> source runs and trial -> both worlds,
        // written in the transaction that writes the record) are what carry a
        // revoked run to it.
        ("artifact", "rsia.exploration_artifact_envelope.v1", "meta_trial_v1") => true,
        ("candidate" | "artifact", "rsia.release_candidate.v1", _) => true,
        ("artifact", "rsia.release_candidate.v2", _) => true,
        ("artifact", "rsia.run_application.v1", _)
        | ("artifact", "rsia.run_application.v2", _)
        | ("artifact", "rsia.run_application.v3", _)
        | ("receipt", "rsia.host_application.v1", _)
        | ("artifact", "rsia.host_execution_receipt.v1", _) => true,
        // Both historical and Current compiler-use/bundle records carry full
        // source-derived inputs; the Current bundle also holds original P/B.
        (
            "artifact",
            "rsia.optimization.compiler_use.v1"
            | "rsia.optimization.compiler_use.v2"
            | "rsia.authenticated_bundle.v1"
            | "rsia.authenticated_bundle.v2"
            | "rsia.authenticated_bundle.v3"
            | "rsia.registered_formal_host.v2"
            | "rsia.frozen_compiler_profile.v2",
            _,
        ) => true,
        ("artifact", "rsia.resolved_bundle.v2", _) => true,
        // A trace export record names its captured source runs and is
        // redacted like the content derived from them; its published tree was
        // removed just above.
        ("artifact", crate::TRACE_EXPORT_SCHEMA, _) => true,
        // Monitoring content derived from source runs (environment and observation
        // identities, per-task pairings, the claim, the consolidation proposal with
        // its candidate bundle and the binding that reserves its candidate id) is
        // redacted; the preserved scope, run and drift facts above keep the ids
        // and counts needed for reconciliation.
        (
            "artifact",
            "rsia.monitoring.environment.v1"
            | "rsia.monitoring.environment.v3"
            | "rsia.monitoring.observation.v1"
            | "rsia.monitoring.development_cycle.v1"
            | "rsia.monitoring.development_cycle.v2"
            | "rsia.monitoring.development_report_binding.v1"
            | "rsia.monitoring.consolidation_claim.v1"
            | "rsia.monitoring.consolidation_proposal.v1"
            | "rsia.monitoring.consolidation_proposal.v2"
            | "rsia.monitoring.consolidation_staging.v1",
            _,
        ) => true,
        // E12 curriculum content derived from source runs (the learner state with
        // its completed cycles and failure clusters, the cycle receipts, proposal
        // attempts, structured test proposals, validity reports and task
        // selections) is redacted; the preserved profile, probe job and schedule
        // receipt above keep the control, limit and idempotency facts.
        (
            "artifact",
            "rsia.curriculum_artifact_envelope.v1",
            "learner_state_v2"
            | "development_cycle_receipt_v1"
            | "curriculum_proposal_attempt_v1"
            | "structured_test_proposal_v1"
            | "curriculum_validity_report_v1"
            | "curriculum_selection_v1",
        ) => true,
        // E09 practice attempt sets carry source-derived scores and output digests.
        ("artifact", "rsia.practice_attempt_set.v1" | "rsia.practice_attempt_set.v2", _) => true,
        // Service Run is bare content; exact keys and its declared type bound this case.
        ("artifact", "", _)
            if value.as_object().is_some_and(|m| {
                m.len() == 9
                    && [
                        "id",
                        "owner",
                        "goal",
                        "snapshot_id",
                        "improver_version",
                        "capabilities",
                        "status",
                        "source",
                        "created_at",
                    ]
                    .iter()
                    .all(|key| m.contains_key(*key))
                    && serde_json::from_value::<evo_core::Run>(value.clone())
                        .is_ok_and(|run| run.source == "trusted_host_snapshot")
            }) =>
        {
            true
        }
        // E03's persisted SourceSelection predates a schema field; exact fields define its shape.
        ("artifact", "", _) => value.as_object().is_some_and(|m| {
            m.len() == 4
                && ["roots", "run_ids", "purpose", "allow_model_excerpts"]
                    .iter()
                    .all(|key| m.contains_key(*key))
        }),
        // E07 management private input: the complete request payload of a
        // management job (for exploration.start the whole world and its run
        // closure). It is content derived from sources and is redacted; the
        // preserved job above keeps the id of the redacted input.
        ("artifact", "rsia.management_private_input.v1", _) => true,
        ("artifact", "rsia.monitoring.environment.v2", "tool_only_diagnostic") => true,
        _ => false,
    };
    if !redact && !preserve {
        return mark_unknown_scope(
            tx,
            ctx,
            job_id,
            node,
            &format!("blocked_unknown_scope:{}:{schema}:{record_kind}", node.kind),
            now,
        )
        .await;
    }
    if preserve {
        insert_cleanup_event(
            tx,
            ctx.namespace(),
            job_id,
            Some(node),
            "historical_fact_preserved",
            now,
            json!({"kind":node.kind}),
        )
        .await?;
        return Ok(());
    }
    let redacted = redacted_object_body(&node.kind, &node.id, &value, &body);
    let mut details = json!({"original_digest":evo_core::hash(body.as_bytes())});
    if schema == "rsia.optimization.stage_fact.v2" {
        details["job_scope_digest"] = json!(fingerprint(&value["payload"]["job_scope"])?);
    }
    sqlx::query(
        "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind=? AND id=?",
    )
    .bind(redacted.to_string())
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "content_redacted",
        now,
        details,
    )
    .await
}

async fn mark_unknown_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    error: &str,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE revoke_cleanup_jobs SET state='failed',last_error=?,updated_at=?
         WHERE namespace=? AND job_id=?",
    )
    .bind(error)
    .bind(now)
    .bind(ctx.namespace())
    .bind(job_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "blocked_unknown_scope",
        now,
        json!({"error":error}),
    )
    .await
}

/// The `rsia.redacted.v1` object that replaces the object `kind`/`id`, whose stored
/// body was `body` and whose parsed form is `value`, once a revoked source reaches
/// it: its identity, the schema and the digest of the content it replaces, and the
/// minimal metadata kept for reconciliation (`minimal_metadata`, and for a stage fact
/// or an exploration envelope the history of its payload, `historical_stage_facts`).
/// No other content survives.
///
/// This is the one construction of the redaction. The cleanup replaces what it
/// finds with it, and a writer that is about to store content whose source is
/// already revoked can store it directly, so the object it stores is exactly the
/// one the cleanup would have made of it. `body` must be the text the object would
/// have been stored as (the digest of that text is the digest of the replaced
/// content).
pub fn redacted_object_body(
    kind: &str,
    id: &str,
    value: &serde_json::Value,
    body: &str,
) -> serde_json::Value {
    let schema = value
        .get("schema_version")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let mut metadata = minimal_metadata(value);
    if schema == crate::TRACE_EXPORT_SCHEMA {
        // Ids, digests, sizes, the delivery receipt with its scope and
        // residual statement, and the observed cost: delivery and cost facts
        // are never reversed by a revocation, and none of them is content.
        for key in [
            "export_id",
            "phase",
            "prepare_job_id",
            "publish_job_id",
            "candidate_digest",
            "members",
            "delivery",
            "local_cost",
            "source_snapshots",
            "owner_actor",
        ] {
            if let Some(kept) = value.get(key) {
                metadata[key] = kept.clone();
            }
        }
    }
    if schema == "rsia.optimization.stage_fact.v1"
        || schema == "rsia.optimization.stage_fact.v2"
        || schema == "rsia.exploration_artifact_envelope.v1"
    {
        metadata["historical_facts"] = historical_stage_facts(&value["payload"]);
    }
    if schema == "rsia.optimization.stage_fact.v2" {
        metadata["job_scope"] = value["payload"]["job_scope"].clone();
    }
    json!({
        "id": id,
        "schema_version": "rsia.redacted.v1",
        "state": "source_revoked",
        "original_kind": kind,
        "original_schema": value.get("schema_version").and_then(|v| v.as_str()),
        "original_digest": evo_core::hash(body.as_bytes()),
        "metadata": metadata,
    })
}

fn historical_stage_facts(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(historical_stage_facts).collect())
        }
        serde_json::Value::Object(fields) => {
            let mut result = minimal_metadata(value)
                .as_object()
                .cloned()
                .unwrap_or_default();
            for (key, item) in fields {
                if item.is_object() || item.is_array() {
                    let kept = historical_stage_facts(item);
                    if kept.as_object().is_none_or(|m| !m.is_empty()) {
                        result.insert(key.clone(), kept);
                    }
                }
            }
            serde_json::Value::Object(result)
        }
        _ => serde_json::Value::Null,
    }
}

fn minimal_metadata(value: &serde_json::Value) -> serde_json::Value {
    let mut output = serde_json::Map::new();
    let Some(object) = value.as_object() else {
        return serde_json::Value::Object(output);
    };
    for key in [
        "artifact_id",
        "kind",
        "state",
        "stage",
        "episode_id",
        "step",
        "attempt",
        "request_id",
        "call_id",
        "dispatch_id",
        "root_budget_id",
        "provider_request_id",
        "usage_record_id",
        "ticket_id",
        "report_id",
        "run_id",
        "release_id",
        "candidate_id",
        "profile_id",
        "bundle_digest",
        "parent_digest",
        "pointer_epoch",
        "input_digest",
        "output_digest",
        "request_digest",
        "actual_request_digest",
        "environment_digest",
        "host_surface_digest",
        "host_capabilities_digest",
        "revoke_watermark",
        "namespace",
        "purpose",
        "failure_kind",
        "quality_micros",
        "dispatched_repairs",
        "environment_reset",
        "world_id",
        "sealed_digest",
        "status",
        "decision",
        "record_kind",
        "provenance",
        "actual_cost_micros",
        "cost_micros",
        "input_tokens",
        "output_tokens",
        "latency_millis",
        "usage_record_ids",
        "parent_total_micros",
        "candidate_total_micros",
        "execution_receipt_id",
        "used_ids",
        "capability_level",
        "evolution_enabled",
        "lease_epoch",
        "dispatch_state",
        "opportunity_consumed",
        "remaining_root_micros",
        "remaining_recovery_dispatches",
        "dispatch_ids",
        "node_ids",
        "decision_round",
        "source_watermark",
        "root_slot",
        "branch_seq",
        "action_seq",
        "estimated_cost_upper_micros",
        "claim_id",
        "proposal_id",
    ] {
        if let Some(item) = object.get(key) {
            output.insert(key.into(), item.clone());
        }
    }
    serde_json::Value::Object(output)
}

async fn ensure_logical_block(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    status: &CleanupStatus,
) -> Result<()> {
    let tombstone: Option<String> = sqlx::query_scalar(
        "SELECT body FROM objects WHERE namespace=? AND kind='tombstone' AND id=?",
    )
    .bind(ctx.namespace())
    .bind(&status.source.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let watermark: Option<(i64, String)> =
        sqlx::query_as("SELECT seq,digest FROM revoke_watermark WHERE namespace=?")
            .bind(ctx.namespace())
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
    let expected_seq = i64::try_from(status.watermark_seq).map_err(|_| Error::Internal)?;
    let valid_watermark = watermark.as_ref().is_some_and(|value| {
        value.0 > expected_seq || (value.0 == expected_seq && value.1 == status.watermark_digest)
    });
    if tombstone.is_none() || !valid_watermark {
        return Err(Error::Conflict(
            "authoritative tombstone or revoke watermark is missing".into(),
        ));
    }
    Ok(())
}

async fn load_status_by_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    namespace: &str,
    source: &TypedObjectRef,
    watermark_seq: u64,
) -> Result<Option<CleanupStatus>> {
    let row = sqlx::query(
        "SELECT * FROM revoke_cleanup_jobs
         WHERE namespace=? AND source_kind=? AND source_id=? AND watermark_seq=?",
    )
    .bind(namespace)
    .bind(&source.kind)
    .bind(&source.id)
    .bind(i64::try_from(watermark_seq).map_err(|_| Error::Internal)?)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    match row {
        Some(row) => Ok(Some(status_from_row(tx, namespace, row).await?)),
        None => Ok(None),
    }
}

async fn load_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    namespace: &str,
    job_id: &str,
) -> Result<Option<CleanupStatus>> {
    let row = sqlx::query("SELECT * FROM revoke_cleanup_jobs WHERE namespace=? AND job_id=?")
        .bind(namespace)
        .bind(job_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?;
    match row {
        Some(row) => Ok(Some(status_from_row(tx, namespace, row).await?)),
        None => Ok(None),
    }
}

async fn status_from_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    namespace: &str,
    row: sqlx::sqlite::SqliteRow,
) -> Result<CleanupStatus> {
    let job_id: String = row.try_get("job_id").map_err(internal)?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM revoke_cleanup_frontier
         WHERE namespace=? AND job_id=? AND expanded=0 AND node_kind!='__formal_ticket_scan'",
    )
    .bind(namespace)
    .bind(&job_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(CleanupStatus {
        job_id,
        source: TypedObjectRef {
            kind: row.try_get("source_kind").map_err(internal)?,
            id: row.try_get("source_id").map_err(internal)?,
        },
        watermark_seq: u64::try_from(row.try_get::<i64, _>("watermark_seq").map_err(internal)?)
            .map_err(|_| Error::Internal)?,
        watermark_digest: row.try_get("watermark_digest").map_err(internal)?,
        state: CleanupState::parse(&row.try_get::<String, _>("state").map_err(internal)?)?,
        processed_nodes: u64::try_from(row.try_get::<i64, _>("processed_nodes").map_err(internal)?)
            .map_err(|_| Error::Internal)?,
        pending_nodes: u64::try_from(pending).map_err(|_| Error::Internal)?,
        last_error: row.try_get("last_error").map_err(internal)?,
    })
}

async fn insert_cleanup_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    namespace: &str,
    job_id: &str,
    node: Option<&TypedObjectRef>,
    event_kind: &str,
    event_at: i64,
    details: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO revoke_cleanup_events(
           namespace,job_id,node_kind,node_id,event_kind,event_at,details
         ) VALUES(?,?,?,?,?,?,?)",
    )
    .bind(namespace)
    .bind(job_id)
    .bind(node.map(|value| value.kind.as_str()))
    .bind(node.map(|value| value.id.as_str()))
    .bind(event_kind)
    .bind(event_at)
    .bind(details.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

fn validate_source(source: &TypedObjectRef) -> Result<()> {
    if source.kind != "run" && source.kind != "artifact" {
        return Err(Error::Invalid(
            "initial E08 revoke source must be a run or strict E16 import_source artifact".into(),
        ));
    }
    identifier(&source.id)
}

fn valid_time(value: i64) -> Result<()> {
    if value < 0 {
        return Err(Error::Invalid("time must be nonnegative".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// E16.5 startup recovery gate: read-only control-plane facts.
//
// `Store::open` always writes (create_if_missing, migrations, staging cleanup),
// so the recovery gate needs a reader that never goes through it. The facts read
// here are exactly the ones `scripts/restore_backup.py` requires to be equal
// between the trusted anchor and the restored database (`protected_facts`), plus
// the revoke watermarks and tombstones it replays. The gate in
// `evo-engine::startup_gate` only compares the returned structures.
// ---------------------------------------------------------------------------

/// `objects.kind` values `restore_backup.py::protected_facts` always treats as
/// consumed accounting. `crates/evo-engine/tests/startup_gate_v42.rs` parses the
/// script source and asserts this list and the schema lists
/// ([`PROTECTED_SCHEMA_VERSIONS`] plus [`PROTECTED_ACTION_SCHEMA_VERSIONS`]) equal
/// the script's, so the two implementations cannot drift apart silently.
pub const PROTECTED_OBJECT_KINDS: [&str; 4] = ["budget", "reservation", "evaluation", "receipt"];

/// Body `schema_version` values `restore_backup.py::protected_facts` treats as
/// consumed accounting regardless of the object kind.
pub const PROTECTED_SCHEMA_VERSIONS: [&str; 11] = [
    "rsia.typed_artifact_envelope.v1",
    "rsia.exploration_artifact_envelope.v1",
    "rsia.optimization.stage_fact.v1",
    "rsia.budget_call_ref.v1",
    "rsia.budget_call_e16_ref.v1",
    "rsia.e16.export_attempt.v1",
    "rsia.e16.delivery_audit.v1",
    "rsia.e16.export_attempt.v2",
    "rsia.e16.delivery_audit.v2",
    "rsia.trace_export.v1",
    "rsia.optimization.stage_fact.v2",
];

/// Body `schema_version` values of the durable facts of an action that already
/// happened, an idempotency binding, a spend or an invalidation, introduced by the
/// v4.2 monitoring, practice, curriculum, management and replay-economic work
/// (E16.5, E08, plan §11.3, §11.5). They are selected exactly like
/// [`PROTECTED_SCHEMA_VERSIONS`] (any object kind, inside a watermarked namespace,
/// compared for equality between the anchor and the restored database), so a backup
/// that is older than the anchor on any of them is isolated and a restored
/// directory the anchor has moved past on any of them is quarantined; nothing in
/// the decision rule changes.
///
/// - monitoring: the consolidation claim (generation occupancy), the terminal run
///   record (outcome and budget call ids), the proposal, the unique
///   proposal-to-candidate staging binding and the environment drift record (the
///   fact that an environment change invalidated a consolidation scope);
/// - practice: the K=3 registration, the registration-to-set binding (what the
///   registration was spent on) and the attempt set;
/// - curriculum: the artifact envelope, whose `coverage_probe_job_v1` and
///   `probe_schedule_receipt_v1` records are the scheduling idempotency and quota
///   facts;
/// - management: the job (kind `job`; matched by schema because that kind also
///   holds the pre-management `evo_core::Job` records);
/// - replay economics: the artifact envelope of the preregistered experiment, the
///   job started over it (its start key and state), the cost receipts (the amounts
///   they book) and the blocked report. The schema covers all four record kinds,
///   so each is compared whole. The revocation cleanup keeps them as they were (an
///   amount or a history that happened is not reversed by a deletion), so a backup
///   that predates one of them is behind the anchor.
///
/// `rsia.management_private_input.v1` is deliberately absent: it is the request
/// payload, not an action or an account. So is the mutable scope index
/// `rsia.monitoring.consolidation_scope.v1`: each of its `claim_ids` is written in
/// the transaction of its (protected) claim and its `invalidated_by` only in the
/// transaction of the (protected) drift record, so what it decides is compared
/// through those records; the rest of it is cycle bookkeeping. A record that
/// source revocation has redacted carries `rsia.redacted.v1` and no longer matches
/// any of these.
///
/// Kept apart from [`PROTECTED_SCHEMA_VERSIONS`], the accounting set whose size
/// `tests/control_plane_facts.rs` pins. The script lists both groups in one tuple.
pub const PROTECTED_ACTION_SCHEMA_VERSIONS: [&str; 14] = [
    "rsia.monitoring.consolidation_claim.v1",
    "rsia.monitoring.consolidation_run.v1",
    "rsia.monitoring.consolidation_staging.v1",
    "rsia.monitoring.consolidation_proposal.v1",
    "rsia.monitoring.environment_drift.v1",
    "rsia.practice_registration.v1",
    "rsia.practice_registration_binding.v1",
    "rsia.practice_attempt_set.v1",
    "rsia.curriculum_artifact_envelope.v1",
    "rsia.management_job.v1",
    "rsia.replay_economic_artifact_envelope.v1",
    "rsia.monitoring.consolidation_proposal.v2",
    "rsia.management_job.v2",
    "rsia.practice_attempt_set.v2",
];

fn is_protected_schema(schema: &str) -> bool {
    PROTECTED_SCHEMA_VERSIONS.contains(&schema)
        || PROTECTED_ACTION_SCHEMA_VERSIONS.contains(&schema)
}

/// The exact table selector `restore_backup.py::protected_facts` uses. SQLite
/// `LIKE` treats `_` as a single-character wildcard; the same quirk is kept so
/// both implementations select the same tables.
pub const ROOT_BUDGET_TABLE_QUERY: &str =
    "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'root_budget%'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatermarkFact {
    pub seq: i64,
    pub digest: String,
}

/// One database's recovery-relevant facts, read through a read-only connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneFacts {
    /// `revoke_watermark` rows by namespace.
    pub watermarks: BTreeMap<String, WatermarkFact>,
    /// `kind = 'tombstone'` objects by `(namespace, id)` with the parsed body.
    /// Owner and revision are deliberately not part of the fact: a restore
    /// replays tombstones under `owner = 'restore'`.
    pub tombstones: BTreeMap<(String, String), serde_json::Value>,
    /// Consumed-accounting objects by `(namespace, kind, id)` with the parsed
    /// body, selected like `restore_backup.py::protected_facts`: only namespaces
    /// that have a revoke watermark, and only [`PROTECTED_OBJECT_KINDS`] or a
    /// body whose `schema_version` is in [`PROTECTED_SCHEMA_VERSIONS`] or
    /// [`PROTECTED_ACTION_SCHEMA_VERSIONS`].
    pub protected_objects: BTreeMap<(String, String, String), serde_json::Value>,
    /// Every `root_budget%` table: sorted canonical rows. Each row is the
    /// comma-joined SQLite `quote()` of its columns, so value types are part of
    /// the fact (integer 1 and text '1' differ).
    pub root_budget_tables: BTreeMap<String, Vec<String>>,
}

/// Read the recovery facts of `path` without `Store::open`.
///
/// The database is opened `SQLITE_OPEN_READONLY` without `create_if_missing`,
/// `PRAGMA query_only=ON` is set, and every read happens in one transaction.
/// Before any fact is read the database must pass `PRAGMA integrity_check` and
/// its `_sqlx_migrations` must equal the compiled-in migration set exactly
/// (the same comparison a backup uses). Any failure is an error; nothing here
/// repairs, migrates or creates anything.
///
/// One case would still leave files behind: a WAL-mode database that nobody has
/// open has neither `-wal` nor `-shm`, and even a read-only connection creates
/// both. That database cannot be mid-write (a live or crashed writer leaves the
/// side files), so it is read as `immutable` and nothing is created. Every other
/// database is read through a normal read-only connection so a WAL that a writer
/// left behind is honoured.
pub async fn read_control_plane_facts(path: &Path) -> Result<ControlPlaneFacts> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(is_unopened_wal_database(path))
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5))
        .disable_statement_logging();
    let mut connection = options
        .connect()
        .await
        .map_err(|error| Error::Invalid(format!("cannot open read-only: {error}")))?;
    let facts = read_facts_from(&mut connection).await;
    let _ = connection.close().await;
    facts
}

/// A database file whose header says WAL mode (file format write/read version
/// bytes 18 and 19 are both 2) while neither `<path>-wal` nor `<path>-shm` exists.
fn is_unopened_wal_database(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 20];
    let header_says_wal = std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && header.starts_with(b"SQLite format 3\0")
        && header[18] == 2
        && header[19] == 2;
    let absent = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        matches!(
            std::fs::symlink_metadata(PathBuf::from(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    };
    header_says_wal && absent("-wal") && absent("-shm")
}

async fn read_facts_from(connection: &mut sqlx::SqliteConnection) -> Result<ControlPlaneFacts> {
    fn unreadable(context: &'static str) -> impl Fn(sqlx::Error) -> Error {
        move |error| Error::Invalid(format!("{context}: {error}"))
    }
    sqlx::query("PRAGMA query_only = ON")
        .execute(&mut *connection)
        .await
        .map_err(unreadable("cannot enforce query_only"))?;
    let mut tx = connection
        .begin()
        .await
        .map_err(unreadable("cannot begin read transaction"))?;

    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut *tx)
        .await
        .map_err(unreadable("integrity check could not run"))?;
    if integrity.len() != 1 || integrity[0] != "ok" {
        let first = integrity.first().map(String::as_str).unwrap_or("no result");
        return Err(Error::Conflict(format!(
            "database integrity check failed: {}",
            first.chars().take(200).collect::<String>()
        )));
    }
    // One compiled-set comparison for backups and for the recovery gate.
    if let Err(error) = load_backup_migrations(&mut tx).await {
        return Err(Error::Conflict(format!(
            "migration set is not the compiled-in set ({})",
            match error {
                Error::Conflict(message) | Error::Invalid(message) => message,
                other => other.to_string(),
            }
        )));
    }

    let mut watermarks = BTreeMap::new();
    let rows = sqlx::query("SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace")
        .fetch_all(&mut *tx)
        .await
        .map_err(unreadable("revoke watermarks unreadable"))?;
    for row in rows {
        let namespace: String = row
            .try_get("namespace")
            .map_err(unreadable("revoke watermark namespace"))?;
        watermarks.insert(
            namespace,
            WatermarkFact {
                seq: row
                    .try_get("seq")
                    .map_err(unreadable("revoke watermark seq"))?,
                digest: row
                    .try_get("digest")
                    .map_err(unreadable("revoke watermark digest"))?,
            },
        );
    }

    let mut tombstones = BTreeMap::new();
    let mut protected_objects = BTreeMap::new();
    let rows = sqlx::query(
        "SELECT namespace,kind,id,body,
         CASE WHEN objects.kind='artifact' AND json_valid(objects.body)
         THEN EXISTS (
             SELECT 1 FROM json_each(objects.body) AS member
             WHERE member.key='schema_version' AND member.type='text'
               AND member.value='rsia.host_surface_record.v2'
         ) ELSE 0 END AS current_host_surface
         FROM objects ORDER BY namespace,kind,id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(unreadable("objects unreadable"))?;
    for row in rows {
        let namespace: String = row
            .try_get("namespace")
            .map_err(unreadable("object namespace"))?;
        let kind: String = row.try_get("kind").map_err(unreadable("object kind"))?;
        let id: String = row.try_get("id").map_err(unreadable("object id"))?;
        let in_scope = watermarks.contains_key(&namespace);
        if !in_scope && kind != "tombstone" {
            continue;
        }
        let body: String = row.try_get("body").map_err(unreadable("object body"))?;
        let current_host_surface: i64 = row
            .try_get("current_host_surface")
            .map_err(unreadable("object current schema selector"))?;
        let value = crate::current_host_surface_object_value(&body, current_host_surface)
            .and_then(|value| match value {
                Some(value) => Ok(value),
                None => parse_strict_json(&body),
            })
            .map_err(|error| {
                Error::Invalid(format!(
                    "object {namespace}/{kind}/{id} has an unreadable body: {}",
                    match error {
                        Error::Invalid(message) => message,
                        other => other.to_string(),
                    }
                ))
            })?;
        let schema = match &value {
            serde_json::Value::Object(object) => object
                .get("schema_version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            _ => {
                return Err(Error::Invalid(format!(
                    "object {namespace}/{kind}/{id} body is not a JSON object"
                )));
            }
        };
        if schema.as_deref() == Some("rsia.host_surface_record.v2") {
            identifier(&namespace)?;
            if kind != "artifact"
                || value.get("id").and_then(serde_json::Value::as_str) != Some(id.as_str())
            {
                return Err(Error::Invalid(format!(
                    "object {namespace}/{kind}/{id} current host physical identity differs"
                )));
            }
        }
        if kind == "tombstone" {
            tombstones.insert((namespace.clone(), id.clone()), value.clone());
        }
        if in_scope
            && (PROTECTED_OBJECT_KINDS.contains(&kind.as_str())
                || schema.as_deref().is_some_and(is_protected_schema)
                || (kind == "artifact" && crate::is_trace_export_record(&id, &value)?))
        {
            protected_objects.insert((namespace, kind, id), value);
        }
    }

    let mut tables: Vec<String> = sqlx::query_scalar(ROOT_BUDGET_TABLE_QUERY)
        .fetch_all(&mut *tx)
        .await
        .map_err(unreadable("root budget tables unreadable"))?;
    tables.sort();
    let mut root_budget_tables = BTreeMap::new();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&table)
                .fetch_all(&mut *tx)
                .await
                .map_err(unreadable("root budget columns unreadable"))?;
        if columns.is_empty() {
            return Err(Error::Invalid(format!(
                "root budget table {table} has no columns"
            )));
        }
        let quoted = columns
            .iter()
            .map(|column| format!("quote({})", quote_identifier(column)))
            .collect::<Vec<_>>()
            .join("||','||");
        let mut rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT {quoted} FROM {}",
            quote_identifier(&table)
        ))
        .fetch_all(&mut *tx)
        .await
        .map_err(unreadable("root budget rows unreadable"))?;
        rows.sort();
        root_budget_tables.insert(table, rows);
    }
    tx.rollback()
        .await
        .map_err(unreadable("cannot end read transaction"))?;
    Ok(ControlPlaneFacts {
        watermarks,
        tombstones,
        protected_objects,
        root_budget_tables,
    })
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Parse JSON that must not contain a duplicate object key at any depth
/// (`restore_backup.py::read_json`). `serde_json::Value` alone silently keeps
/// the last duplicate, which would let a receipt or fact hide a value.
pub fn parse_strict_json(text: &str) -> Result<serde_json::Value> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let parsed = StrictJson::deserialize(&mut deserializer)
        .map_err(|error| Error::Invalid(format!("invalid JSON: {error}")))?;
    deserializer
        .end()
        .map_err(|error| Error::Invalid(format!("invalid JSON: {error}")))?;
    Ok(parsed.0)
}

struct StrictJson(serde_json::Value);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> serde::de::Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(|number| StrictJson(serde_json::Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJson>()? {
            values.push(value.0);
        }
        Ok(StrictJson(serde_json::Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON key: {key}"
                )));
            }
            let value = object.next_value::<StrictJson>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJson(serde_json::Value::Object(values)))
    }
}

#[cfg(test)]
#[cfg(unix)]
mod backup_lifecycle_tests {
    use super::*;
    use std::os::unix::fs::OpenOptionsExt;

    async fn fixture() -> (tempfile::TempDir, Store, Context) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("rsia.sqlite3");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let store = Store::open(&path).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &evo_core::hash(b"watermark"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        (directory, store, admin)
    }

    #[tokio::test]
    async fn backup_preparation_releases_pool_after_snapshot_for_lease_progress() {
        let (directory, store, admin) = fixture().await;
        let mut preparing = store
            .backup(&admin, BackupName::parse("pool").unwrap())
            .unwrap()
            .begin(0, None)
            .await
            .unwrap();
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        preparing.pause_before_copy = Some((reached.clone(), resume.clone()));
        let worker = tokio::spawn(async move {
            preparing
                .prepare(
                    Instant::now() + Duration::from_secs(30),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        });
        reached.notified().await;
        let mut session = tokio::time::timeout(Duration::from_secs(2), store.session())
            .await
            .unwrap()
            .unwrap();
        session
            .audit(&admin, "lease_progress_after_snapshot", "pool")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let closing_store = store.clone();
        let closed = tokio::spawn(async move { closing_store.close().await });
        while !store.pool.is_closed() {
            tokio::task::yield_now().await;
        }
        assert!(
            !closed.is_finished(),
            "close must await the actual copy/cleanup lock"
        );
        resume.notify_one();
        let prepared = worker.await.unwrap().unwrap();
        assert!(
            !closed.is_finished(),
            "Prepared still owns its unrecorded temporary directory"
        );
        prepared.discard().await.unwrap();
        closed.await.unwrap();
        assert_eq!(
            std::fs::read_dir(directory.path().join("backups"))
                .unwrap()
                .count(),
            0
        );
        assert!(store.session().await.is_err());
    }

    #[tokio::test]
    async fn backup_parent_fsync_failure_reports_real_publication_reference() {
        let (_directory, store, admin) = fixture().await;
        let mut prepared = store
            .backup(&admin, BackupName::parse("unverified").unwrap())
            .unwrap()
            .begin(0, None)
            .await
            .unwrap()
            .prepare(
                Instant::now() + Duration::from_secs(30),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        prepared.owned.fail_parent_sync = true;
        let progress = prepared.progress();
        let session = store.session().await.unwrap();
        let publication = prepared.release().publish().await.unwrap();
        assert_eq!(publication.durability, BackupDurability::Unverified);
        assert_eq!(
            Some(publication.manifest_sha256.as_str()),
            progress.manifest_sha256()
        );
        assert!(publication.directory().join("rsia.sqlite3").is_file());
        let raw = std::fs::read(publication.directory().join("backup-manifest.json")).unwrap();
        assert_eq!(evo_core::hash(&raw), publication.manifest_sha256);
        session.commit().await.unwrap();
        store.close().await;
        assert!(publication.directory().exists());
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
mod publication_tests {
    use super::*;
    #[test]
    fn atomic_backup_publication_rejects_existing_empty_directory() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(source.join("complete"), b"source").unwrap();
        assert!(matches!(
            publish_backup_directory(&source, &target),
            Err(Error::Conflict(_))
        ));
        assert_eq!(std::fs::read(source.join("complete")).unwrap(), b"source");
        assert!(!target.join("complete").exists());
    }
    #[test]
    fn atomic_backup_directory_race_has_exactly_one_winner() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let left = root.path().join("left");
        let right = root.path().join("right");
        for (path, body) in [(&left, b"left".as_slice()), (&right, b"right".as_slice())] {
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("complete"), body).unwrap();
        }
        let barrier = std::sync::Barrier::new(3);
        let (a, b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                publish_backup_directory(&left, &target)
            });
            let b = scope.spawn(|| {
                barrier.wait();
                publish_backup_directory(&right, &target)
            });
            barrier.wait();
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        if a.is_ok() {
            assert_eq!(std::fs::read(target.join("complete")).unwrap(), b"left");
            assert!(right.join("complete").exists());
            assert!(matches!(b, Err(Error::Conflict(_))));
        } else {
            assert_eq!(std::fs::read(target.join("complete")).unwrap(), b"right");
            assert!(left.join("complete").exists());
            assert!(matches!(a, Err(Error::Conflict(_))));
        }
    }
}

#[cfg(test)]
mod current_bound_request_cleanup_source_tests {
    use super::{BudgetRequestSources, budget_request_source_ids};

    // This cleanup reader extracts source IDs only. The deliberately minimal
    // input is not a complete model request, compiler proof or dispatch grant.
    fn body(schema: &str) -> String {
        serde_json::json!({
            "schema_version": schema,
            "request": {"source_closure": [{"id": "run-b"}, {"id": "run-a"}]}
        })
        .to_string()
    }

    #[test]
    fn both_bound_request_versions_require_the_exact_column_body_pair() {
        let schemas = ["rsia.bound_model_request.v1", "rsia.bound_model_request.v2"];
        for column in schemas {
            for wire in schemas {
                let actual = budget_request_source_ids(column, &body(wire));
                if column == wire {
                    assert!(matches!(actual, BudgetRequestSources::Named(ids)
                        if ids == vec!["run-b".to_string(), "run-a".to_string()]));
                } else {
                    assert!(matches!(actual, BudgetRequestSources::Unknown(reason)
                        if reason == format!("unreadable_budget_request:{column}")));
                }
            }
        }
        let unknown = "rsia.bound_model_request.v3";
        assert!(matches!(budget_request_source_ids(unknown, &body(unknown)),
            BudgetRequestSources::Unknown(reason)
                if reason == format!("unknown_budget_request_schema:{unknown}")));
    }

    #[test]
    fn bound_request_cleanup_keeps_ambiguous_and_unreadable_sources_unknown() {
        for schema in ["rsia.bound_model_request.v1", "rsia.bound_model_request.v2"] {
            // Each healthy version must work before its single-fault cases.
            assert!(matches!(budget_request_source_ids(schema, &body(schema)),
                BudgetRequestSources::Named(ids) if ids.len() == 2));
            let duplicate_schema = format!(
                r#"{{"schema_version":"{schema}","schema_version":"{schema}","request":{{"source_closure":[{{"id":"run-a"}}]}}}}"#
            );
            let duplicate_closure = format!(
                r#"{{"schema_version":"{schema}","request":{{"source_closure":[{{"id":"run-a"}}],"source_closure":[]}}}}"#
            );
            let invalid_id = format!(
                r#"{{"schema_version":"{schema}","request":{{"source_closure":[{{"id":"bad id"}}]}}}}"#
            );
            let missing = format!(r#"{{"schema_version":"{schema}","request":{{}}}}"#);
            let wrong_type = format!(
                r#"{{"schema_version":"{schema}","request":{{"source_closure":[{{"id":7}}]}}}}"#
            );
            for bad in [
                duplicate_schema,
                duplicate_closure,
                invalid_id,
                missing,
                wrong_type,
            ] {
                assert!(matches!(budget_request_source_ids(schema, &bad),
                    BudgetRequestSources::Unknown(reason)
                        if reason == format!("unreadable_budget_request:{schema}")));
            }
        }
    }
}

#[cfg(test)]
mod current_consolidation_proposal_schema_protection_tests {
    use super::{PROTECTED_ACTION_SCHEMA_VERSIONS, is_protected_schema};

    #[test]
    fn both_known_consolidation_versions_keep_restore_protection() {
        // Schema protection is conservative metadata handling, not authority to
        // consume a proposal or a replacement for authenticating its full body.
        for schema in [
            "rsia.monitoring.consolidation_proposal.v1",
            "rsia.monitoring.consolidation_proposal.v2",
        ] {
            assert!(PROTECTED_ACTION_SCHEMA_VERSIONS.contains(&schema));
            assert!(is_protected_schema(schema));
        }
        assert!(!is_protected_schema(
            "rsia.monitoring.consolidation_proposal.v3"
        ));
    }
}

// AG163: one closed metadata-only catalog. A row is not authentication and
// cannot itself grant Current use or write authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormalEarlyPlanDomain(
    pub std::borrow::Cow<'static, str>,
    pub std::borrow::Cow<'static, str>,
    pub std::borrow::Cow<'static, str>,
);

/// Ordered 18-position domain; tuple serialization is part of its version key.
/// Cow owns deserialized input; constants borrow literals. No credential lives here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormalJudgmentDomain(
    pub std::borrow::Cow<'static, str>,         // 0 record_kind
    pub std::borrow::Cow<'static, str>,         // 1 envelope_schema
    pub Option<std::borrow::Cow<'static, str>>, // 2 payload_schema
    pub std::borrow::Cow<'static, str>,         // 3 variant
    pub std::borrow::Cow<'static, str>,         // 4 evidence_scope
    pub std::borrow::Cow<'static, str>,         // 5 control_schema
    pub std::borrow::Cow<'static, str>,         // 6 ticket_schema
    pub std::borrow::Cow<'static, str>,         // 7 experiment_plan_schema
    pub std::borrow::Cow<'static, str>,         // 8 formal_plan_schema
    pub std::borrow::Cow<'static, str>,         // 9 alpha_plan_schema
    pub std::borrow::Cow<'static, str>,         // 10 comparison_schema
    pub std::borrow::Cow<'static, str>,         // 11 optimization_budget_schema
    pub std::borrow::Cow<'static, str>,         // 12 holdout_schema
    pub Option<std::borrow::Cow<'static, str>>, // 13 stats_schema
    pub Option<std::borrow::Cow<'static, str>>, // 14 certificate_schema
    pub Option<FormalEarlyPlanDomain>,          // 15 early_plan_domain
    pub Option<std::borrow::Cow<'static, str>>, // 16 stop_route
    pub std::borrow::Cow<'static, str>,         // 17 rule_tag
);
impl FormalJudgmentDomain {
    pub fn version_key(&self) -> Result<String> {
        Ok(format!("e05dv-{}", evo_core::canonical_fingerprint(self)?))
    }
    /// Material compatibility only; result schemas/rules may differ.
    pub fn same_input_family(&self, other: &Self) -> bool {
        self.0 == other.0
            && self.1 == other.1
            && self.3 == other.3
            && self.4 == other.4
            && self.5 == other.5
            && self.6 == other.6
            && self.7 == other.7
            && self.8 == other.8
            && self.9 == other.9
            && self.10 == other.10
            && self.11 == other.11
            && self.12 == other.12
            && self.15 == other.15
            && self.16 == other.16
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormalJudgmentIdStrategy {
    SourceTicket,
    VersionedInput,
}
#[derive(Debug, Clone)]
pub struct FormalJudgmentVersion {
    pub domain: FormalJudgmentDomain,
    pub introduced_change: &'static str,
    pub verifier_retained: bool,
    pub id_strategy: FormalJudgmentIdStrategy,
}
macro_rules! formal_judgment_some_text {
    ($text:literal) => {
        Some(std::borrow::Cow::Borrowed($text))
    };
}
macro_rules! formal_judgment_early_domain {
    () => {
        Some(FormalEarlyPlanDomain(
            std::borrow::Cow::Borrowed("rsia.early_stop_plan.v1"),
            std::borrow::Cow::Borrowed("rsia.sequential_reject_only.v1"),
            std::borrow::Cow::Borrowed("rsia.normal_mixture_bounded.v1"),
        ))
    };
}
macro_rules! formal_judgment_version {
    ($control:literal, $ticket:literal, $plan:literal, $scope:literal,
     $payload:expr, $variant:literal, $early:expr, $stats:expr, $certificate:expr,
     $route:expr, $rule:literal, $change:literal, $retained:expr, $id:expr) => {
        formal_judgment_version!(
            $control,
            $ticket,
            $plan,
            $scope,
            $payload,
            $variant,
            $early,
            $stats,
            $certificate,
            $route,
            $rule,
            $change,
            $retained,
            $id,
            "rsia.protected_holdout_record.v1"
        )
    };
    ($control:literal, $ticket:literal, $plan:literal, $scope:literal,
     $payload:expr, $variant:literal, $early:expr, $stats:expr, $certificate:expr,
     $route:expr, $rule:literal, $change:literal, $retained:expr, $id:expr,
     $holdout:literal) => {
        FormalJudgmentVersion {
            domain: FormalJudgmentDomain(
                std::borrow::Cow::Borrowed("formal_evaluation_v2"),
                std::borrow::Cow::Borrowed("rsia.typed_artifact_envelope.v1"),
                $payload,
                std::borrow::Cow::Borrowed($variant),
                std::borrow::Cow::Borrowed($scope),
                std::borrow::Cow::Borrowed($control),
                std::borrow::Cow::Borrowed($ticket),
                std::borrow::Cow::Borrowed($plan),
                std::borrow::Cow::Borrowed("rsia.formal_experiment_plan.v4_1"),
                std::borrow::Cow::Borrowed("rsia.research_family_alpha_plan.v1"),
                std::borrow::Cow::Borrowed("rsia.optimizer_comparison_contract.v1"),
                std::borrow::Cow::Borrowed("rsia.optimization_budget_plan.v1"),
                std::borrow::Cow::Borrowed($holdout),
                $stats,
                $certificate,
                $early,
                $route,
                std::borrow::Cow::Borrowed($rule),
            ),
            introduced_change: $change,
            verifier_retained: $retained,
            id_strategy: $id,
        }
    };
}
/// Explicit rows, in lineage order. No runtime schema Cartesian product or
/// current=true shortcut. Engine must authenticate the full original closure
/// in its Session before matching. Derived Complete has a retained source and
/// derived verifier; derived Early remains unavailable without the original
/// stop snapshot. Native rows preserve the actual Control3/Ticket4/Plan3 and
/// Holdout2 domain introduced by the fixed-sample regression decision.
/// One reviewed row per registered domain; kept unwrapped for row-by-row review.
#[rustfmt::skip]
pub const FORMAL_JUDGMENT_VERSIONS: &[FormalJudgmentVersion] = &[
    // Original writer: rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v2, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v2, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v2, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v2, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v3, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v3, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "original control-v1 frozen postprocessing and first-stop semantics", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v3, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    // Original writer: rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v3, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        None, "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "control-v2 preserves profile verdict; original early/invalid semantics retained", true, FormalJudgmentIdStrategy::SourceTicket),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v2, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v2, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v2, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v2, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v2", "rsia.experiment_plan.v1", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v3, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v1, rsia.evaluation_ticket.v3, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v1_frozen",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v1", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v3, program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // Explicit derived target (rsia.formal_judgment.v1): rsia.registered_evaluation_control.v2, rsia.evaluation_ticket.v3, registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v2_preserve_ucb0",
        "explicit canonical original-input snapshot and versioned judgment identity", true, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    formal_judgment_version!("rsia.registered_evaluation_control.v2", "rsia.evaluation_ticket.v3", "rsia.experiment_plan.v2", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "explicit canonical original-input snapshot and versioned judgment identity", false, FormalJudgmentIdStrategy::VersionedInput),
    // AG192 native fixed-sample domain: program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    // AG192 native fixed-sample domain: registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", None, None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("critical_k0"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "early_stopped", formal_judgment_early_domain!(), None, formal_judgment_some_text!("rsia.early_stop_certificate.v1"), formal_judgment_some_text!("sequential_reject_only"), "early_certificate_v1_first_stop",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "invalid", None, None, None, None, "invalid_actual_reason_order_v1",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_evaluation.v3"), "invalid", formal_judgment_early_domain!(), None, None, None, "invalid_actual_reason_order_v1",
        "frozen max-regression decision and complete native wire", true, FormalJudgmentIdStrategy::SourceTicket, "rsia.protected_holdout_record.v2"),
    // Explicit derived complete target over the same native inputs: program_fixture.
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "explicit judgment over frozen native max-regression inputs", true, FormalJudgmentIdStrategy::VersionedInput, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "program_fixture",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "explicit judgment over frozen native max-regression inputs", true, FormalJudgmentIdStrategy::VersionedInput, "rsia.protected_holdout_record.v2"),
    // Explicit derived complete target over the same native inputs: registered_external_host.
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", None, formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "explicit judgment over frozen native max-regression inputs", true, FormalJudgmentIdStrategy::VersionedInput, "rsia.protected_holdout_record.v2"),
    formal_judgment_version!("rsia.registered_evaluation_control.v3", "rsia.evaluation_ticket.v4", "rsia.experiment_plan.v3", "registered_external_host",
        formal_judgment_some_text!("rsia.formal_judgment.v1"), "complete_batch", formal_judgment_early_domain!(), formal_judgment_some_text!("rsia.empirical_bernstein.v2"), None, None, "complete_control_v3_fixed_sample_regression",
        "explicit judgment over frozen native max-regression inputs", true, FormalJudgmentIdStrategy::VersionedInput, "rsia.protected_holdout_record.v2"),
];
/// Semantic lookup only. Read state, authentication and key existence stay
/// separate engine checks and are never inferred from membership.
pub fn formal_judgment_version(
    domain: &FormalJudgmentDomain,
) -> Result<&'static FormalJudgmentVersion> {
    FORMAL_JUDGMENT_VERSIONS
        .iter()
        .find(|entry| entry.domain == *domain)
        .ok_or_else(|| Error::Invalid("formal_judgment_version_outside_table".into()))
}

/// Closed-table lookup only; this is not a source or Current authentication gate.
pub fn formal_judgment_version_by_key(key: &str) -> Result<&'static FormalJudgmentVersion> {
    for entry in FORMAL_JUDGMENT_VERSIONS {
        if entry.domain.version_key()? == key {
            return Ok(entry);
        }
    }
    Err(Error::Invalid(
        "formal_judgment_version_outside_table".into(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormalCleanupDerivedWire {
    schema_version: String,
    report_id: String,
    source_ticket_id: String,
    input_snapshot: serde_json::Value,
    input_digest: String,
    judgment_version: String,
    source_report_ref: [String; 5],
    judgment: serde_json::Value,
}

/// Structural cleanup classification only, never source authentication.
fn formal_derived_cleanup_metadata(
    ctx: &Context,
    node: &TypedObjectRef,
    value: &serde_json::Value,
) -> Result<serde_json::Value> {
    let outer = value.as_object().ok_or(Error::Internal)?;
    let outer_keys = ["schema_version", "id", "record_kind", "payload"];
    if outer.len() != outer_keys.len()
        || outer_keys.iter().any(|key| !outer.contains_key(*key))
        || ["schema_version", "id", "record_kind"]
            .iter()
            .any(|key| !outer[*key].is_string())
        || !outer["payload"].is_object()
    {
        return Err(Error::Internal);
    }
    let payload: FormalCleanupDerivedWire =
        serde_json::from_value(outer["payload"].clone()).map_err(internal)?;
    let version = formal_judgment_version_by_key(&payload.judgment_version)?;
    if payload.schema_version != "rsia.formal_judgment.v1"
        || value["schema_version"] != "rsia.typed_artifact_envelope.v1"
        || value["record_kind"] != "formal_evaluation_v2"
        || value["id"] != node.id
        || node.id
            != format!(
                "e05-{}",
                fingerprint(&("formal_evaluation_v2", &payload.report_id))?
            )
        || version.domain.2.as_deref() != Some("rsia.formal_judgment.v1")
        || version.domain.3.as_ref() != "complete_batch"
    {
        return Err(Error::Internal);
    }
    identifier(&payload.report_id)?;
    identifier(&payload.source_ticket_id)?;
    validate_hex_digest(&payload.input_digest)?;
    let snapshot = payload.input_snapshot.as_object().ok_or(Error::Internal)?;
    if snapshot.len() != 2 || !snapshot.contains_key("common") || !snapshot.contains_key("variant")
    {
        return Err(Error::Internal);
    }
    let common = snapshot["common"].as_array().ok_or(Error::Internal)?;
    let variant = snapshot["variant"].as_object().ok_or(Error::Internal)?;
    if common.len() != 8
        || variant.len() != 2
        || variant.get("variant").and_then(|v| v.as_str()) != Some("complete_batch")
        || common[0].as_str() != Some(ctx.namespace())
        || common[1].as_str() != Some(payload.source_ticket_id.as_str())
        || common[2].as_str() != Some(version.domain.6.as_ref())
    {
        return Err(Error::Internal);
    }
    formal_cleanup_candidate_binding_shape(&common[6])?;
    formal_cleanup_ticket_plan_binding_shape(&common[7])?;
    let input = variant.get("input").ok_or(Error::Internal)?;
    let parts = input.as_array().ok_or(Error::Internal)?;
    if parts.len() != 3 {
        return Err(Error::Internal);
    }
    let reference = |value: &serde_json::Value| -> Result<()> {
        let values = value.as_array().ok_or(Error::Internal)?;
        if values.len() != 5 || values.iter().any(|v| !v.is_string()) || values[0] != "artifact" {
            return Err(Error::Internal);
        }
        let kind = values[1].as_str().ok_or(Error::Internal)?;
        let id = values[2].as_str().ok_or(Error::Internal)?;
        identifier(kind)?;
        identifier(id)?;
        if values[3] != format!("e05-{}", fingerprint(&(kind, id))?) {
            return Err(Error::Internal);
        }
        validate_hex_digest(values[4].as_str().ok_or(Error::Internal)?)
    };
    for index in [3, 4, 5] {
        reference(&common[index])?;
    }
    for group in [&parts[0], &parts[1]] {
        for unit in group.as_array().ok_or(Error::Internal)? {
            let unit = unit.as_array().ok_or(Error::Internal)?;
            if unit.len() != 5 {
                return Err(Error::Internal);
            }
            identifier(unit[0].as_str().ok_or(Error::Internal)?)?;
            identifier(unit[1].as_str().ok_or(Error::Internal)?)?;
            for index in [2, 3, 4] {
                reference(&unit[index])?;
            }
        }
    }
    reference(&parts[2])?;
    if evo_core::canonical_fingerprint(&(
        "rsia.formal_judgment_input.v1",
        &snapshot["common"],
        "complete_batch",
        input,
    ))? != payload.input_digest
    {
        return Err(Error::Internal);
    }
    if payload.report_id
        != format!(
            "e05j-{}",
            &evo_core::canonical_fingerprint(&(
                ctx.namespace(),
                &payload.source_ticket_id,
                &payload.input_digest,
                &payload.judgment_version,
            ))?[..32]
        )
    {
        return Err(Error::Internal);
    }
    reference(&serde_json::to_value(&payload.source_report_ref).map_err(internal)?)?;
    if payload.source_report_ref[1] != "formal_evaluation_v2"
        || payload.source_report_ref[2] != payload.source_ticket_id
    {
        return Err(Error::Internal);
    }
    let judgment = payload.judgment.as_object().ok_or(Error::Internal)?;
    let keys = [
        "variant",
        "evidence_scope",
        "cost_evidence_scope",
        "ticket_id",
        "ticket_digest",
        "manifest_digest",
        "candidate_bundle_digest",
        "baseline_bundle_digest",
        "environment_digest",
        "verdict",
        "report",
        "anchor_evidence_digest",
        "resource_evidence_digest",
    ];
    if keys.iter().any(|key| !judgment.contains_key(*key))
        || judgment
            .keys()
            .any(|key| !keys.contains(&key.as_str()) && key != "stored_dependency_binding")
        || judgment["variant"] != "complete_batch"
        || judgment["ticket_id"] != payload.source_ticket_id
        || judgment["evidence_scope"].as_str() != Some(version.domain.4.as_ref())
        || judgment["cost_evidence_scope"] != "ticket_execution_only"
    {
        return Err(Error::Internal);
    }
    for key in [
        "ticket_digest",
        "manifest_digest",
        "candidate_bundle_digest",
        "baseline_bundle_digest",
        "environment_digest",
        "anchor_evidence_digest",
        "resource_evidence_digest",
    ] {
        validate_hex_digest(judgment[key].as_str().ok_or(Error::Internal)?)?;
    }
    let report = judgment["report"].as_object().ok_or(Error::Internal)?;
    let keys = [
        "stats_version",
        "verdict",
        "n",
        "mean",
        "variance",
        "radius",
        "lcb",
        "alpha_i",
        "reasons",
    ];
    if report.len() != keys.len()
        || keys.iter().any(|key| !report.contains_key(*key))
        || report["stats_version"].as_str() != version.domain.13.as_deref()
        || report["verdict"] != judgment["verdict"]
        || report["n"].as_u64().is_none()
        || !matches!(
            report["verdict"].as_str(),
            Some("improved" | "noninferior" | "inconclusive" | "regressed" | "invalid")
        )
    {
        return Err(Error::Internal);
    }
    for key in ["mean", "variance", "radius", "lcb", "alpha_i"] {
        let value = report[key]
            .as_str()
            .ok_or(Error::Internal)?
            .parse::<f64>()
            .map_err(internal)?;
        if !value.is_finite() {
            return Err(Error::Internal);
        }
    }
    for reason in report["reasons"].as_array().ok_or(Error::Internal)? {
        identifier(reason.as_str().ok_or(Error::Internal)?)?;
    }
    let mut retained = judgment.clone();
    retained.remove("stored_dependency_binding");
    Ok(
        json!({"record_kind":"formal_evaluation_v2","report_id":payload.report_id,
        "source_ticket_id":payload.source_ticket_id,"input_digest":payload.input_digest,
        "judgment_version":payload.judgment_version,"source_report_ref":payload.source_report_ref,
        "judgment":retained,"formal_closure_invalidated":true}),
    )
}

async fn redact_formal_judgment_derived(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    now: i64,
) -> Result<()> {
    let metadata = match formal_derived_cleanup_metadata(ctx, node, value) {
        Ok(metadata) => metadata,
        Err(_) => {
            return mark_unknown_scope(
                tx,
                ctx,
                job_id,
                node,
                "blocked_unknown_scope:formal_judgment_derived_wire_or_domain",
                now,
            )
            .await;
        }
    };
    let redacted = json!({"id":node.id,"schema_version":"rsia.redacted.v1","state":"source_revoked",
        "original_kind":node.kind,"original_schema":"rsia.typed_artifact_envelope.v1",
        "original_digest":evo_core::hash(body.as_bytes()),"metadata":metadata});
    sqlx::query(
        "UPDATE objects SET body=?,revision=revision+1 WHERE namespace=? AND kind=? AND id=?",
    )
    .bind(serde_json::to_string(&redacted).map_err(internal)?)
    .bind(ctx.namespace())
    .bind(&node.kind)
    .bind(&node.id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_cleanup_event(
        tx,
        ctx.namespace(),
        job_id,
        Some(node),
        "node_redacted",
        now,
        json!({"known_inner_schema":"rsia.formal_judgment.v1","historical_facts_preserved":true}),
    )
    .await
}

// AG163 R6: mirror only the existing Serde shapes of the two optional Common
// bindings. No schema-value, source, role, digest or statistical authentication.
fn formal_cleanup_candidate_binding_shape(value: &serde_json::Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let binding = value.as_object().ok_or(Error::Internal)?;
    let fields = [
        "schema_version",
        "namespace",
        "candidate_id",
        "candidate_record_digest",
        "bundle_digest",
        "parent_digest",
        "profile_id",
        "environment_digest",
        "proposer_actor",
        "sources",
        "revoke_watermark_seq",
        "revoke_watermark_digest",
        "nodes",
    ];
    if binding.len() != fields.len() || fields.iter().any(|key| !binding.contains_key(*key)) {
        return Err(Error::Internal);
    }
    for key in [
        "schema_version",
        "namespace",
        "candidate_id",
        "candidate_record_digest",
        "bundle_digest",
        "parent_digest",
        "profile_id",
        "environment_digest",
        "proposer_actor",
        "revoke_watermark_digest",
    ] {
        if !binding[key].is_string() {
            return Err(Error::Internal);
        }
    }
    if binding["revoke_watermark_seq"].as_i64().is_none() {
        return Err(Error::Internal);
    }
    for source in binding["sources"].as_array().ok_or(Error::Internal)? {
        let source = source.as_object().ok_or(Error::Internal)?;
        let fields = ["kind", "id", "content_digest"];
        if source.len() != fields.len()
            || fields
                .iter()
                .any(|key| !source.get(*key).is_some_and(|value| value.is_string()))
        {
            return Err(Error::Internal);
        }
    }
    for node in binding["nodes"].as_array().ok_or(Error::Internal)? {
        let node = node.as_object().ok_or(Error::Internal)?;
        let required = [
            "kind",
            "id",
            "schema_version",
            "logical_id",
            "content_digest",
            "outgoing_edge_digest",
            "outgoing_edge_count",
        ];
        if required.iter().any(|key| !node.contains_key(*key))
            || node
                .keys()
                .any(|key| !required.contains(&key.as_str()) && key != "record_kind")
        {
            return Err(Error::Internal);
        }
        for key in [
            "kind",
            "id",
            "schema_version",
            "logical_id",
            "content_digest",
            "outgoing_edge_digest",
        ] {
            if !node[key].is_string() {
                return Err(Error::Internal);
            }
        }
        // Option<String> in the existing node decoder accepts absent or null.
        if let Some(kind) = node.get("record_kind")
            && !kind.is_null()
            && !kind.is_string()
        {
            return Err(Error::Internal);
        }
        let count = node["outgoing_edge_count"]
            .as_u64()
            .ok_or(Error::Internal)?;
        usize::try_from(count).map_err(internal)?;
    }
    Ok(())
}

fn formal_cleanup_ticket_plan_binding_shape(value: &serde_json::Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let binding = value.as_object().ok_or(Error::Internal)?;
    let fields = [
        "schema_version",
        "namespace",
        "logical_ticket_id",
        "plan_id",
        "plan_digest",
        "candidate_record_digest",
    ];
    if binding.len() != fields.len()
        || fields
            .iter()
            .any(|key| !binding.get(*key).is_some_and(|value| value.is_string()))
    {
        return Err(Error::Internal);
    }
    Ok(())
}

/// The ordinary native wire is exactly schema/decision/result. Its result is
/// classified against the registered native payload domain without changing
/// the frozen result or granting authority from a cleanup-only check.
async fn formal_native_wire_known(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ctx: &Context,
    node: &TypedObjectRef,
    report_owner: &str,
    value: &serde_json::Value,
) -> Result<bool> {
    let payload = &value["payload"];
    if identifier(report_owner).is_err()
        || !value.as_object().is_some_and(|wire| wire.len() == 4)
        || value["id"] != node.id
        || !payload.as_object().is_some_and(|wire| wire.len() == 3)
        || payload["decision_version"] != "rsia.fixed_sample_regression.v1"
    {
        return Ok(false);
    }
    let Some(version) =
        formal_ordinary_inner_version(Some("rsia.formal_evaluation.v3"), &payload["result"])
    else {
        return Ok(false);
    };
    let Some(ticket_id) = payload["result"]["ticket_id"].as_str() else {
        return Ok(false);
    };
    if identifier(ticket_id).is_err()
        || node.id != format!("e05-{}", fingerprint(&("formal_evaluation_v2", ticket_id))?)
    {
        return Ok(false);
    }
    let ticket_physical = format!("e05-{}", fingerprint(&("evaluation_ticket_v2", ticket_id))?);
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT owner,body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(ctx.namespace())
    .bind(&ticket_physical)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some((ticket_owner, body)) = row else {
        return Ok(false);
    };
    let Ok(ticket) = parse_strict_json(&body) else {
        return Ok(false);
    };
    // Cleanup identity only: a native label on an old ticket cannot reclassify
    // its history, including when the report reached the frontier directly.
    Ok(ticket.as_object().is_some_and(|wire| wire.len() == 4)
        && ticket["schema_version"] == "rsia.typed_artifact_envelope.v1"
        && ticket["record_kind"] == "evaluation_ticket_v2"
        && ticket["id"] == ticket_physical
        && ticket["payload"]["id"] == ticket_id
        && ticket["payload"]["schema_version"].as_str() == Some(version.domain.6.as_ref())
        && ticket["payload"]["evaluator_actor"].as_str() == Some(ticket_owner.as_str())
        && ticket_owner == report_owner)
}

/// Ordinary report judgment (the no-schema wire, or the native wrapper's result).
/// Classify only its actual inner version combination for its exact payload
/// schema against the one catalog; do not reinterpret/rejudge history or infer
/// Current from this cleanup-only structural registration check.
fn formal_ordinary_inner_version(
    payload_schema: Option<&str>,
    payload: &serde_json::Value,
) -> Option<&'static FormalJudgmentVersion> {
    let variant = payload.get("variant").and_then(serde_json::Value::as_str)?;
    let scope = payload
        .get("evidence_scope")
        .and_then(serde_json::Value::as_str)?;
    let (stats, certificate, route) = match variant {
        "complete_batch" => {
            let stats = payload
                .get("report")
                .and_then(|v| v.get("stats_version"))
                .and_then(serde_json::Value::as_str)?;
            (Some(stats), None, None)
        }
        "early_stopped" => {
            let certificate = payload.get("certificate")?;
            let schema = certificate
                .get("schema_version")
                .and_then(serde_json::Value::as_str)?;
            let k = certificate.get("k").and_then(serde_json::Value::as_u64)?;
            (
                None,
                Some(schema),
                Some(if k == 0 {
                    "critical_k0"
                } else {
                    "sequential_reject_only"
                }),
            )
        }
        "invalid" => (None, None, None),
        _ => return None,
    };
    FORMAL_JUDGMENT_VERSIONS.iter().find(|entry| {
        entry.domain.2.as_deref() == payload_schema
            && entry.domain.3.as_ref() == variant
            && entry.domain.4.as_ref() == scope
            && entry.domain.13.as_deref() == stats
            && entry.domain.14.as_deref() == certificate
            && entry.domain.16.as_deref() == route
    })
}
