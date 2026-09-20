//! Transactional typed documents, idempotency, audit and disposable FTS5 indexes.
//! This crate never connects to a model or executes candidate content.
pub mod budget;
pub mod lifecycle;
pub mod replay;

use evo_core::{Context, Error, Result, fingerprint, hash, identifier, now, search_tokens};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{
    ConnectOptions, Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::RwLock};

pub const MVP_MAX_RUNS: u64 = 1_000;
pub const MAX_LOCAL_EXPORT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_LOCAL_EXPORT_FILES: usize = 101;

#[derive(Debug, Clone)]
pub struct LocalExportFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalExportReceipt {
    pub delivery_kind: String,
    pub local_export_ref: String,
    pub package_tree_digest: String,
    pub delivered_bytes: u64,
    pub completion_receipt_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DependencyRecordSnapshot {
    kind: String,
    id: String,
    fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LocalExportDependencyPlan {
    namespace: String,
    export_id: String,
    records: Vec<DependencyRecordSnapshot>,
}

#[derive(Debug, Clone)]
pub struct LocalExportDependencySnapshot {
    plan: LocalExportDependencyPlan,
    blob_identities: Vec<(String, BlobFileIdentity)>,
}

#[derive(Debug)]
pub struct RegisteredBlobPublishPlan {
    namespace: String,
    artifact_id: String,
    expected_schema: String,
    payload_digest_field: String,
    digest: String,
    max_bytes: usize,
    existing_identity: Option<BlobFileIdentity>,
    dependencies: LocalExportDependencySnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlobFileIdentity {
    len: u64,
    readonly: bool,
    #[cfg(unix)]
    links: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    root: PathBuf,
    blob_lock: Arc<RwLock<()>>,
}
pub struct Session {
    tx: Transaction<'static, Sqlite>,
}
fn internal(e: impl std::fmt::Display) -> Error {
    tracing::error!(error=%e,"database operation failed");
    Error::Internal
}

struct PrivateStagedBlob {
    path: Option<PathBuf>,
}

struct PrivateStagedDirectory {
    path: Option<PathBuf>,
}

impl PrivateStagedDirectory {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("private staged directory is armed")
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for PrivateStagedDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

impl PrivateStagedBlob {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn path(&self) -> &Path {
        self.path.as_deref().expect("private staged blob is armed")
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for PrivateStagedBlob {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn validate_blob_bound(bytes: &[u8], max_bytes: usize) -> Result<()> {
    if max_bytes == 0 || max_bytes > 64 * 1024 * 1024 {
        return Err(Error::Invalid(
            "blob write bound must be 1..=67108864 bytes".into(),
        ));
    }
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(Error::Invalid(format!(
            "blob must be 1..={max_bytes} bytes"
        )));
    }
    Ok(())
}

pub fn local_export_tree_digest(files: &[LocalExportFile]) -> Result<(String, u64)> {
    #[derive(Serialize)]
    struct Entry<'a> {
        path: &'a str,
        digest: String,
        bytes: u64,
    }
    if files.is_empty() || files.len() > MAX_LOCAL_EXPORT_FILES {
        return Err(Error::Invalid("local export file count is invalid".into()));
    }
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let mut seen = std::collections::BTreeSet::new();
    let mut entries = Vec::with_capacity(sorted.len());
    let mut total = 0usize;
    let mut previous_path: Option<&str> = None;
    for file in sorted {
        validate_local_export_path(&file.path)?;
        if previous_path.is_some_and(|previous| file.path.starts_with(&format!("{previous}/"))) {
            return Err(Error::Invalid(
                "local export path conflicts with a file parent".into(),
            ));
        }
        if file.bytes.is_empty()
            || file.bytes.len() > 1024 * 1024
            || !seen.insert(file.path.as_str())
        {
            return Err(Error::Invalid(
                "local export member is empty, oversized, or duplicated".into(),
            ));
        }
        std::str::from_utf8(&file.bytes)
            .map_err(|_| Error::Invalid("local export member is not UTF-8".into()))?;
        total = total
            .checked_add(file.bytes.len())
            .ok_or_else(|| Error::Invalid("local export byte total overflow".into()))?;
        if total > MAX_LOCAL_EXPORT_BYTES {
            return Err(Error::Invalid("local export exceeds 10 MiB".into()));
        }
        entries.push(Entry {
            path: &file.path,
            digest: hash(&file.bytes),
            bytes: file.bytes.len() as u64,
        });
        previous_path = Some(&file.path);
    }
    if !seen.contains("manifest.json") {
        return Err(Error::Invalid("local export lacks manifest.json".into()));
    }
    Ok((fingerprint(&entries)?, total as u64))
}

fn validate_local_export_path(path: &str) -> Result<()> {
    if path.is_empty() || path.contains('\0') || path.contains('\\') {
        return Err(Error::Invalid("invalid local export path".into()));
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(Error::Invalid("unsafe local export path".into()));
    }
    Ok(())
}

fn local_export_ref(namespace: &str, export_id: &str) -> String {
    format!("exports/{}/{export_id}", hash(namespace.as_bytes()))
}

pub fn local_export_receipt(
    namespace: &str,
    export_id: &str,
    tree_digest: &str,
    delivered_bytes: u64,
) -> Result<LocalExportReceipt> {
    let local_export_ref = local_export_ref(namespace, export_id);
    let completion_receipt_digest = fingerprint(&(
        "local_directory_v1",
        export_id,
        local_export_ref.as_str(),
        tree_digest,
        delivered_bytes,
    ))?;
    Ok(LocalExportReceipt {
        delivery_kind: "local_directory_v1".into(),
        local_export_ref,
        package_tree_digest: tree_digest.into(),
        delivered_bytes,
        completion_receipt_digest,
    })
}

fn validate_registered_blob_field(schema: &str, field: &str) -> Result<()> {
    let valid = match schema {
        "rsia.e16.import_source.v1" => field == "raw_blob_digest",
        "rsia.e16.staged_asset.v1" => field == "content_blob_digest",
        "rsia.e16.seed_install.v1" => matches!(
            field,
            "baseline_blob_digest" | "local_blob_digest" | "upstream_blob_digest"
        ),
        _ => false,
    };
    if !valid {
        return Err(Error::Invalid(
            "unsupported E16 registered blob schema or digest field".into(),
        ));
    }
    Ok(())
}

async fn write_synced_temp(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .map_err(internal)?;
    file.write_all(bytes).await.map_err(internal)?;
    file.sync_all().await.map_err(internal)
}

async fn verify_blob_file(path: &Path, digest: &str, max_bytes: usize) -> Result<()> {
    let path = path.to_path_buf();
    let expected = digest.to_string();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let metadata = std::fs::symlink_metadata(&path).map_err(internal)?;
        if !metadata.file_type().is_file()
            || metadata.len() == 0
            || metadata.len() > max_bytes as u64
        {
            return Err(Error::Conflict(
                "published blob is linked, non-regular, empty, or oversized".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(Error::Conflict("published blob is hard-linked".into()));
            }
        }
        let bytes = std::fs::read(&path).map_err(internal)?;
        if bytes.len() > max_bytes || hash(&bytes) != expected {
            return Err(Error::Conflict("published blob digest mismatch".into()));
        }
        Ok(())
    })
    .await
    .map_err(internal)??;
    Ok(())
}

async fn blob_file_identity(path: &Path) -> Result<BlobFileIdentity> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<BlobFileIdentity> {
        let metadata = std::fs::symlink_metadata(path).map_err(internal)?;
        if !metadata.file_type().is_file() {
            return Err(Error::Conflict(
                "dependency blob is not a regular file".into(),
            ));
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(BlobFileIdentity {
            len: metadata.len(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            links: metadata.nlink(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            modified_seconds: metadata.mtime(),
            #[cfg(unix)]
            modified_nanoseconds: metadata.mtime_nsec(),
        })
    })
    .await
    .map_err(internal)?
}

async fn publish_or_verify_blob(
    temp: &Path,
    final_path: &Path,
    existing_preverified: Option<&BlobFileIdentity>,
) -> Result<bool> {
    let source = temp.to_path_buf();
    let destination = final_path.to_path_buf();
    let published =
        tokio::task::spawn_blocking(move || publish_blob_no_replace(&source, &destination))
            .await
            .map_err(internal)??;
    if !published {
        let expected_identity = existing_preverified.ok_or_else(|| {
            Error::Conflict("blob target appeared after preflight; retry publication".into())
        })?;
        if blob_file_identity(final_path).await? != *expected_identity {
            return Err(Error::Conflict(
                "preverified blob identity changed before publication".into(),
            ));
        }
        match tokio::fs::remove_file(temp).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(internal(error)),
        }
    }
    Ok(published)
}

async fn sync_directory(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || File::open(path).and_then(|directory| directory.sync_all()))
        .await
        .map_err(internal)?
        .map_err(internal)
}

fn publish_blob_no_replace(source: &Path, destination: &Path) -> Result<bool> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        match renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(true),
            Err(error) if error == rustix::io::Errno::EXIST => Ok(false),
            Err(error)
                if error == rustix::io::Errno::NOSYS || error == rustix::io::Errno::OPNOTSUPP =>
            {
                Err(Error::Invalid(
                    "atomic no-overwrite blob publication unsupported".into(),
                ))
            }
            Err(error) => Err(internal(error)),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        let _ = (source, destination);
        Err(Error::Invalid(
            "atomic no-overwrite blob publication unsupported platform".into(),
        ))
    }
}

async fn cleanup_private_blob_staging(root: &Path) -> Result<()> {
    let staging = root.join(".blob-staging");
    let metadata = match tokio::fs::symlink_metadata(&staging).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(internal(error)),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Conflict(
            "private blob staging root is linked or non-directory".into(),
        ));
    }
    let mut namespaces = tokio::fs::read_dir(&staging).await.map_err(internal)?;
    while let Some(namespace) = namespaces.next_entry().await.map_err(internal)? {
        let metadata = namespace.metadata().await.map_err(internal)?;
        if !metadata.is_dir() || namespace.file_type().await.map_err(internal)?.is_symlink() {
            return Err(Error::Conflict(
                "private blob staging namespace is linked or non-directory".into(),
            ));
        }
        let mut files = tokio::fs::read_dir(namespace.path())
            .await
            .map_err(internal)?;
        while let Some(file) = files.next_entry().await.map_err(internal)? {
            let metadata = file.metadata().await.map_err(internal)?;
            if !metadata.is_file() || file.file_type().await.map_err(internal)?.is_symlink() {
                return Err(Error::Conflict(
                    "private blob staging member is linked or non-regular".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(Error::Conflict(
                        "private blob staging member is hard-linked".into(),
                    ));
                }
            }
            tokio::fs::remove_file(file.path())
                .await
                .map_err(internal)?;
        }
        tokio::fs::remove_dir(namespace.path())
            .await
            .map_err(internal)?;
    }
    tokio::fs::remove_dir(staging).await.map_err(internal)
}

async fn cleanup_private_export_staging(root: &Path) -> Result<()> {
    let staging = root.join(".export-staging");
    let metadata = match tokio::fs::symlink_metadata(&staging).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(internal(error)),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Conflict(
            "private export staging root is linked or non-directory".into(),
        ));
    }
    let mut namespaces = tokio::fs::read_dir(&staging).await.map_err(internal)?;
    while let Some(namespace) = namespaces.next_entry().await.map_err(internal)? {
        let metadata = tokio::fs::symlink_metadata(namespace.path())
            .await
            .map_err(internal)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Conflict(
                "private export staging namespace is linked or non-directory".into(),
            ));
        }
        tokio::fs::remove_dir_all(namespace.path())
            .await
            .map_err(internal)?;
    }
    tokio::fs::remove_dir(staging).await.map_err(internal)
}

async fn verify_registered_source_refs(
    session: &mut Session,
    ctx: &Context,
    schema: &str,
    artifact: &Value,
    publishing_digest: &str,
) -> Result<()> {
    let refs = artifact
        .get("source_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::Conflict("registered blob producer has invalid source_refs".into())
        })?;
    let digest_field = if schema == "rsia.e16.import_source.v1" {
        "content_digest"
    } else {
        "digest"
    };
    for source in refs {
        let source = source
            .as_object()
            .ok_or_else(|| Error::Conflict("registered source_ref is not an object".into()))?;
        if source.len() != 3 {
            return Err(Error::Conflict(
                "registered source_ref has unknown or missing fields".into(),
            ));
        }
        let kind = source
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("registered source_ref lacks kind".into()))?;
        let id = source
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("registered source_ref lacks id".into()))?;
        let expected_digest = source
            .get(digest_field)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("registered source_ref lacks digest".into()))?;
        identifier(id)?;
        if expected_digest.len() != 64
            || !expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Conflict(
                "registered source_ref digest is invalid".into(),
            ));
        }
        if session.get::<Value>(ctx, "tombstone", id).await?.is_some() {
            return Err(Error::Forbidden);
        }
        if kind == "blob" {
            if id != publishing_digest || expected_digest != publishing_digest {
                return Err(Error::Conflict(
                    "registered blob source_ref differs from published content".into(),
                ));
            }
            continue;
        }
        let stored: Value = session.need(ctx, kind, id).await?;
        if fingerprint(&stored)? != expected_digest {
            return Err(Error::Conflict(format!(
                "registered source changed: {kind}:{id}"
            )));
        }
    }
    Ok(())
}

async fn load_dependency_record_snapshots(
    session: &mut Session,
    ctx: &Context,
    artifact_id: &str,
) -> Result<Vec<DependencyRecordSnapshot>> {
    let rows = sqlx::query(
        "WITH RECURSIVE closure(kind,id) AS (
           SELECT dst_kind,dst_id FROM dependencies
             WHERE namespace=? AND src_kind='artifact' AND src_id=?
           UNION
           SELECT d.dst_kind,d.dst_id FROM dependencies d
             JOIN closure c ON d.src_kind=c.kind AND d.src_id=c.id
             WHERE d.namespace=?
         )
         SELECT kind,id FROM closure ORDER BY kind,id LIMIT 10001",
    )
    .bind(ctx.namespace())
    .bind(artifact_id)
    .bind(ctx.namespace())
    .fetch_all(&mut *session.tx)
    .await
    .map_err(internal)?;
    if rows.len() > 10_000 {
        return Err(Error::Conflict(
            "local export dependency closure exceeds bounded verification".into(),
        ));
    }
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind").map_err(internal)?;
        let id: String = row.try_get("id").map_err(internal)?;
        if session.get::<Value>(ctx, "tombstone", &id).await?.is_some() {
            return Err(Error::Forbidden);
        }
        let record_fingerprint = if kind == "blob" {
            None
        } else {
            let stored: Value = session.need(ctx, &kind, &id).await?;
            Some(fingerprint(&stored)?)
        };
        records.push(DependencyRecordSnapshot {
            kind,
            id,
            fingerprint: record_fingerprint,
        });
    }
    Ok(records)
}

async fn verify_dependency_snapshot_in_session(
    session: &mut Session,
    ctx: &Context,
    snapshot: &LocalExportDependencySnapshot,
) -> Result<()> {
    let current = load_dependency_record_snapshots(session, ctx, &snapshot.plan.export_id).await?;
    if current != snapshot.plan.records {
        return Err(Error::Conflict(
            "local export dependency closure changed after preflight".into(),
        ));
    }
    Ok(())
}

async fn write_local_export_staging(root: &Path, files: &[LocalExportFile]) -> Result<()> {
    for member in files {
        let target = root.join(&member.path);
        let parent = target.parent().ok_or(Error::Internal)?;
        tokio::fs::create_dir_all(parent).await.map_err(internal)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)
            .await
            .map_err(internal)?;
        file.write_all(&member.bytes).await.map_err(internal)?;
        file.sync_all().await.map_err(internal)?;
        let mut permissions = file.metadata().await.map_err(internal)?.permissions();
        permissions.set_readonly(true);
        tokio::fs::set_permissions(&target, permissions)
            .await
            .map_err(internal)?;
        sync_directory(parent).await?;
    }
    sync_directory(root).await
}

async fn ensure_unlinked_directory(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::Conflict(
                    "controlled export path is linked or non-directory".into(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir(path).await.map_err(internal)?;
        }
        Err(error) => return Err(internal(error)),
    }
    Ok(())
}

async fn verify_local_export_directory(
    root: &Path,
    expected: &[LocalExportFile],
) -> Result<(String, u64)> {
    let root = root.to_path_buf();
    let mut expected_map = std::collections::BTreeMap::new();
    let mut expected_directories = std::collections::BTreeSet::new();
    for file in expected {
        expected_map.insert(
            file.path.clone(),
            (hash(&file.bytes), file.bytes.len() as u64),
        );
        let mut parent = Path::new(&file.path).parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(
                directory
                    .to_str()
                    .ok_or_else(|| Error::Invalid("local export path is not UTF-8".into()))?
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
            parent = directory.parent();
        }
    }
    tokio::task::spawn_blocking(move || -> Result<(String, u64)> {
        let metadata = std::fs::symlink_metadata(&root).map_err(internal)?;
        if !metadata.file_type().is_dir() {
            return Err(Error::Conflict("local export is not a directory".into()));
        }
        let mut stack = vec![root.clone()];
        let mut actual = Vec::new();
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory).map_err(internal)? {
                let entry = entry.map_err(internal)?;
                let metadata = entry.metadata().map_err(internal)?;
                let file_type = entry.file_type().map_err(internal)?;
                if file_type.is_symlink() {
                    return Err(Error::Conflict("local export contains a symlink".into()));
                }
                if file_type.is_dir() {
                    let relative = entry
                        .path()
                        .strip_prefix(&root)
                        .map_err(internal)?
                        .to_str()
                        .ok_or_else(|| Error::Invalid("local export path is not UTF-8".into()))?
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    if !expected_directories.contains(&relative) {
                        return Err(Error::Conflict(
                            "local export has an extra directory".into(),
                        ));
                    }
                    stack.push(entry.path());
                    continue;
                }
                if !file_type.is_file() {
                    return Err(Error::Conflict(
                        "local export contains a non-regular member".into(),
                    ));
                }
                if !metadata.permissions().readonly() {
                    return Err(Error::Conflict(
                        "local export member is not read-only".into(),
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.nlink() != 1 {
                        return Err(Error::Conflict(
                            "local export contains a hard-linked member".into(),
                        ));
                    }
                }
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(internal)?
                    .to_str()
                    .ok_or_else(|| Error::Invalid("local export path is not UTF-8".into()))?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                validate_local_export_path(&relative)?;
                let bytes = std::fs::read(entry.path()).map_err(internal)?;
                let expected = expected_map
                    .get(&relative)
                    .ok_or_else(|| Error::Conflict("local export has an extra member".into()))?;
                if hash(&bytes) != expected.0 || bytes.len() as u64 != expected.1 {
                    return Err(Error::Conflict("local export member changed".into()));
                }
                actual.push(LocalExportFile {
                    path: relative,
                    bytes,
                });
            }
        }
        if actual.len() != expected_map.len() {
            return Err(Error::Conflict("local export member is missing".into()));
        }
        local_export_tree_digest(&actual)
    })
    .await
    .map_err(internal)?
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalExportIdentityEntry {
    path: String,
    is_dir: bool,
    len: u64,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
}

async fn local_export_identity(root: &Path) -> Result<Vec<LocalExportIdentityEntry>> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<Vec<LocalExportIdentityEntry>> {
        let mut stack = vec![root.clone()];
        let mut identity = Vec::new();
        while let Some(path) = stack.pop() {
            let metadata = std::fs::symlink_metadata(&path).map_err(internal)?;
            if metadata.file_type().is_symlink()
                || (!metadata.file_type().is_dir() && !metadata.file_type().is_file())
            {
                return Err(Error::Conflict(
                    "local export identity contains a linked or special member".into(),
                ));
            }
            let relative = if path == root {
                ".".into()
            } else {
                path.strip_prefix(&root)
                    .map_err(internal)?
                    .to_str()
                    .ok_or_else(|| Error::Invalid("local export path is not UTF-8".into()))?
                    .replace(std::path::MAIN_SEPARATOR, "/")
            };
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            identity.push(LocalExportIdentityEntry {
                path: relative,
                is_dir: metadata.file_type().is_dir(),
                len: metadata.len(),
                readonly: metadata.permissions().readonly(),
                #[cfg(unix)]
                device: metadata.dev(),
                #[cfg(unix)]
                inode: metadata.ino(),
                #[cfg(unix)]
                modified_seconds: metadata.mtime(),
                #[cfg(unix)]
                modified_nanoseconds: metadata.mtime_nsec(),
            });
            if metadata.file_type().is_dir() {
                for entry in std::fs::read_dir(path).map_err(internal)? {
                    stack.push(entry.map_err(internal)?.path());
                }
            }
        }
        identity.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(identity)
    })
    .await
    .map_err(internal)?
}

async fn verify_export_source_refs(
    session: &mut Session,
    ctx: &Context,
    refs: &[Value],
) -> Result<()> {
    for source in refs {
        let source = source
            .as_object()
            .ok_or_else(|| Error::Conflict("export source_ref is not an object".into()))?;
        if source.len() != 3 {
            return Err(Error::Conflict("export source_ref shape is invalid".into()));
        }
        let kind = source
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("export source_ref lacks kind".into()))?;
        let id = source
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("export source_ref lacks id".into()))?;
        let digest = source
            .get("digest")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Conflict("export source_ref lacks digest".into()))?;
        if session.get::<Value>(ctx, "tombstone", id).await?.is_some() {
            return Err(Error::Forbidden);
        }
        let stored: Value = session.need(ctx, kind, id).await?;
        if fingerprint(&stored)? != digest {
            return Err(Error::Conflict("export source changed".into()));
        }
    }
    Ok(())
}

fn validate_local_export_attempt(
    ctx: &Context,
    export_id: &str,
    attempt: &Value,
    receipt: &LocalExportReceipt,
    expected_state: &str,
) -> Result<()> {
    const ENVELOPE_FIELDS: [&str; 11] = [
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
    const PAYLOAD_FIELDS: [&str; 16] = [
        "publisher",
        "asset_id",
        "staged_asset_id",
        "manifest_digest",
        "projection_blob_digest",
        "projection_blob_bytes",
        "package_tree_digest",
        "delivered_bytes",
        "delivery_kind",
        "local_export_ref",
        "state",
        "destination_scope",
        "delivery_audit_id",
        "completion_receipt_digest",
        "budget_ref",
        "error",
    ];
    if !has_exact_fields(attempt, &ENVELOPE_FIELDS)
        || !has_exact_fields(&attempt["payload"], &PAYLOAD_FIELDS)
        || attempt["schema_version"].as_str() != Some("rsia.e16.export_attempt.v2")
        || attempt["id"].as_str() != Some(export_id)
        || attempt["namespace"].as_str() != Some(ctx.namespace())
        || attempt["owner_actor"].as_str() != Some(ctx.actor())
        || attempt["payload"]["state"].as_str() != Some(expected_state)
        || attempt["payload"]["destination_scope"].as_str() != Some("local_namespace")
        || attempt["payload"]["delivery_kind"].as_str() != Some("local_directory_v1")
        || attempt["payload"]["local_export_ref"].as_str()
            != Some(receipt.local_export_ref.as_str())
        || attempt["payload"]["package_tree_digest"].as_str()
            != Some(receipt.package_tree_digest.as_str())
        || attempt["payload"]["delivered_bytes"].as_u64() != Some(receipt.delivered_bytes)
    {
        return Err(Error::Conflict(
            "local export attempt identity mismatch".into(),
        ));
    }
    Ok(())
}

fn validate_local_export_completion(
    ctx: &Context,
    export_id: &str,
    completed: &Value,
    audit: &Value,
    receipt: &LocalExportReceipt,
    watermark: u64,
) -> Result<()> {
    validate_local_export_attempt(ctx, export_id, completed, receipt, "completed")?;
    const ENVELOPE_FIELDS: [&str; 11] = [
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
    const AUDIT_FIELDS: [&str; 14] = [
        "export_attempt_id",
        "package_id",
        "projection_blob_digest",
        "delivery_kind",
        "local_export_ref",
        "package_tree_digest",
        "delivered_bytes",
        "completion_receipt_digest",
        "delivered_to",
        "delivered_at",
        "watermark_at_delivery",
        "revoked",
        "revocation_notice_sent",
        "remote_erasure_disclaimer",
    ];
    let audit_id = audit["id"]
        .as_str()
        .ok_or_else(|| Error::Conflict("delivery audit lacks id".into()))?;
    identifier(audit_id)?;
    if !has_exact_fields(audit, &ENVELOPE_FIELDS)
        || !has_exact_fields(&audit["payload"], &AUDIT_FIELDS)
        || completed["payload"]["delivery_audit_id"].as_str() != Some(audit_id)
        || completed["payload"]["completion_receipt_digest"].as_str()
            != Some(receipt.completion_receipt_digest.as_str())
        || audit["schema_version"].as_str() != Some("rsia.e16.delivery_audit.v2")
        || audit["namespace"].as_str() != Some(ctx.namespace())
        || audit["owner_actor"].as_str() != Some(ctx.actor())
        || audit["payload"]["export_attempt_id"].as_str() != Some(export_id)
        || audit["payload"]["delivery_kind"].as_str() != Some("local_directory_v1")
        || audit["payload"]["local_export_ref"].as_str() != Some(receipt.local_export_ref.as_str())
        || audit["payload"]["package_tree_digest"].as_str()
            != Some(receipt.package_tree_digest.as_str())
        || audit["payload"]["delivered_bytes"].as_u64() != Some(receipt.delivered_bytes)
        || audit["payload"]["completion_receipt_digest"].as_str()
            != Some(receipt.completion_receipt_digest.as_str())
        || audit["payload"]["watermark_at_delivery"].as_u64() != Some(watermark)
    {
        return Err(Error::Conflict("local delivery completion differs".into()));
    }
    Ok(())
}

fn has_exact_fields(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        tokio::fs::create_dir_all(&root).await.map_err(internal)?;
        cleanup_private_blob_staging(&root).await?;
        cleanup_private_export_staging(&root).await?;
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5))
            .disable_statement_logging();
        // One connection serializes state transitions, including read/modify/write budget transactions.
        // The CLI also takes an OS file lock: separate daemons must not share a data directory.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(internal)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(internal)?;
        Ok(Self {
            pool,
            root,
            blob_lock: Arc::new(RwLock::new(())),
        })
    }
    pub async fn session(&self) -> Result<Session> {
        Ok(Session {
            tx: self.pool.begin().await.map_err(internal)?,
        })
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
    pub async fn integrity(&self) -> Result<String> {
        sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(internal)
    }
    pub async fn backup(&self, destination: &Path) -> Result<()> {
        crate::lifecycle::backup_consistent(self, destination).await
    }
    /// Caller must register the returned digest and owner through a trusted host operation.
    pub async fn put_blob(&self, ctx: &Context, bytes: &[u8]) -> Result<String> {
        self.put_blob_bounded(ctx, bytes, 1024 * 1024).await
    }
    /// Store a bounded content blob atomically. E16 producers must prefer
    /// `publish_registered_blob`, which closes the revoke-before-publication race.
    pub async fn put_blob_bounded(
        &self,
        ctx: &Context,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<String> {
        ctx.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
        validate_blob_bound(bytes, max_bytes)?;
        let _guard = self.blob_lock.write().await;
        let digest = hash(bytes);
        let dir = self
            .root
            .join("blobs")
            .join(hash(ctx.namespace().as_bytes()));
        tokio::fs::create_dir_all(&dir).await.map_err(internal)?;
        let final_path = dir.join(&digest);
        let existing_preverified = match tokio::fs::symlink_metadata(&final_path).await {
            Ok(_) => {
                verify_blob_file(&final_path, &digest, max_bytes).await?;
                Some(blob_file_identity(&final_path).await?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(internal(error)),
        };
        let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut staged = PrivateStagedBlob::new(temp);
        write_synced_temp(staged.path(), bytes).await?;
        let published =
            publish_or_verify_blob(staged.path(), &final_path, existing_preverified.as_ref())
                .await?;
        staged.disarm();
        if published {
            sync_directory(&dir).await?;
        }
        Ok(digest)
    }
    /// Publish bytes only while a strict E16 Prepared artifact still registers
    /// their digest. The private file write and hash happen before the DB lock;
    /// publication takes the same blob-lock -> SQLite-lock order as cleanup.
    pub async fn publish_registered_blob(
        &self,
        ctx: &Context,
        artifact_id: &str,
        expected_schema: &str,
        payload_digest_field: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<String> {
        let plan = self
            .preflight_registered_blob_publish(
                ctx,
                artifact_id,
                expected_schema,
                payload_digest_field,
                bytes,
                max_bytes,
            )
            .await?;
        match self
            .publish_preflighted_registered_blob(ctx, plan, bytes)
            .await
        {
            Err(Error::Conflict(message))
                if message == "blob target appeared after preflight; retry publication" =>
            {
                let retry = self
                    .preflight_registered_blob_publish(
                        ctx,
                        artifact_id,
                        expected_schema,
                        payload_digest_field,
                        bytes,
                        max_bytes,
                    )
                    .await?;
                self.publish_preflighted_registered_blob(ctx, retry, bytes)
                    .await
            }
            result => result,
        }
    }

    #[doc(hidden)]
    pub async fn preflight_registered_blob_publish(
        &self,
        ctx: &Context,
        artifact_id: &str,
        expected_schema: &str,
        payload_digest_field: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<RegisteredBlobPublishPlan> {
        ctx.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
        identifier(artifact_id)?;
        validate_registered_blob_field(expected_schema, payload_digest_field)?;
        validate_blob_bound(bytes, max_bytes)?;
        let digest = hash(bytes);
        let dependency_plan = self
            .snapshot_registered_dependencies(ctx, artifact_id)
            .await?;
        let dependencies = self
            .prevalidate_registered_dependency_blobs(ctx, dependency_plan, Some(digest.as_str()))
            .await?;
        let namespace_hash = hash(ctx.namespace().as_bytes());
        let final_path = self.root.join("blobs").join(namespace_hash).join(&digest);
        let existing_identity = match tokio::fs::symlink_metadata(&final_path).await {
            Ok(_) => {
                verify_blob_file(&final_path, &digest, max_bytes).await?;
                Some(blob_file_identity(&final_path).await?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(internal(error)),
        };
        Ok(RegisteredBlobPublishPlan {
            namespace: ctx.namespace().into(),
            artifact_id: artifact_id.into(),
            expected_schema: expected_schema.into(),
            payload_digest_field: payload_digest_field.into(),
            digest,
            max_bytes,
            existing_identity,
            dependencies,
        })
    }

    #[doc(hidden)]
    pub async fn publish_preflighted_registered_blob(
        &self,
        ctx: &Context,
        plan: RegisteredBlobPublishPlan,
        bytes: &[u8],
    ) -> Result<String> {
        ctx.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
        if plan.namespace != ctx.namespace() {
            return Err(Error::Forbidden);
        }
        validate_blob_bound(bytes, plan.max_bytes)?;
        if hash(bytes) != plan.digest {
            return Err(Error::Conflict(
                "registered blob bytes changed after preflight".into(),
            ));
        }
        let artifact_id = plan.artifact_id;
        let expected_schema = plan.expected_schema;
        let payload_digest_field = plan.payload_digest_field;
        let digest = plan.digest;
        let max_bytes = plan.max_bytes;
        let existing_preverified = plan.existing_identity;
        let dependencies = plan.dependencies;
        let namespace_hash = hash(ctx.namespace().as_bytes());
        let staging_dir = self.root.join(".blob-staging").join(&namespace_hash);
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(internal)?;
        let temp_path = staging_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut staged = PrivateStagedBlob::new(temp_path);
        write_synced_temp(staged.path(), bytes).await?;

        verify_blob_file(staged.path(), &digest, max_bytes).await?;
        let final_dir = self.root.join("blobs").join(namespace_hash);
        tokio::fs::create_dir_all(&final_dir)
            .await
            .map_err(internal)?;
        let final_path = final_dir.join(&digest);

        let _guard = self.blob_lock.write().await;
        self.verify_prevalidated_dependency_identities(ctx, &dependencies)
            .await?;
        let mut session = self.session().await?;
        let fenced = sqlx::query(
            "UPDATE objects SET revision=revision
             WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(ctx.namespace())
        .bind(&artifact_id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        if fenced.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        if session
            .get::<Value>(ctx, "tombstone", &artifact_id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let artifact: Value = session.need(ctx, "artifact", &artifact_id).await?;
        let state_field = if matches!(
            expected_schema.as_str(),
            "rsia.e16.seed_install.v1" | "rsia.e16.import_source.v1"
        ) {
            "status"
        } else {
            "state"
        };
        if artifact.get("schema_version").and_then(Value::as_str) != Some(expected_schema.as_str())
            || artifact.get("id").and_then(Value::as_str) != Some(artifact_id.as_str())
            || artifact.get("namespace").and_then(Value::as_str) != Some(ctx.namespace())
            || artifact.get("owner_actor").and_then(Value::as_str) != Some(ctx.actor())
            || artifact
                .get("payload")
                .and_then(|payload| payload.get(state_field))
                .and_then(Value::as_str)
                != Some("prepared")
            || artifact
                .get("payload")
                .and_then(|payload| payload.get(&payload_digest_field))
                .and_then(Value::as_str)
                != Some(digest.as_str())
        {
            return Err(Error::Conflict(
                "registered blob producer is missing, revoked, changed, or no longer Prepared"
                    .into(),
            ));
        }
        let expected_watermark = artifact
            .get("revoke_watermark")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Conflict("registered blob producer lacks watermark".into()))?;
        let current_watermark = session
            .watermark(ctx)
            .await?
            .and_then(|(seq, _)| u64::try_from(seq).ok())
            .ok_or_else(|| Error::Conflict("current revoke watermark is unavailable".into()))?;
        if expected_watermark != current_watermark {
            return Err(Error::Conflict(
                "registered blob producer watermark is stale".into(),
            ));
        }
        verify_registered_source_refs(&mut session, ctx, &expected_schema, &artifact, &digest)
            .await?;
        verify_dependency_snapshot_in_session(&mut session, ctx, &dependencies).await?;
        let published =
            publish_or_verify_blob(staged.path(), &final_path, existing_preverified.as_ref())
                .await?;
        staged.disarm();
        session.commit().await?;
        drop(_guard);
        if published {
            sync_directory(&final_dir).await?;
        }
        Ok(digest)
    }

    pub fn local_export_directory(&self, ctx: &Context, export_id: &str) -> Result<PathBuf> {
        ctx.require(&[evo_core::Role::Admin])?;
        identifier(export_id)?;
        Ok(self
            .root
            .join("exports")
            .join(hash(ctx.namespace().as_bytes()))
            .join(export_id))
    }

    pub async fn snapshot_local_export_dependencies(
        &self,
        ctx: &Context,
        export_id: &str,
    ) -> Result<LocalExportDependencyPlan> {
        ctx.require(&[evo_core::Role::Admin])?;
        self.snapshot_registered_dependencies(ctx, export_id).await
    }

    async fn snapshot_registered_dependencies(
        &self,
        ctx: &Context,
        artifact_id: &str,
    ) -> Result<LocalExportDependencyPlan> {
        identifier(artifact_id)?;
        let mut session = self.session().await?;
        let records = load_dependency_record_snapshots(&mut session, ctx, artifact_id).await?;
        session.commit().await?;
        Ok(LocalExportDependencyPlan {
            namespace: ctx.namespace().into(),
            export_id: artifact_id.into(),
            records,
        })
    }

    pub async fn prevalidate_local_export_dependency_blobs(
        &self,
        ctx: &Context,
        plan: LocalExportDependencyPlan,
    ) -> Result<LocalExportDependencySnapshot> {
        ctx.require(&[evo_core::Role::Admin])?;
        self.prevalidate_registered_dependency_blobs(ctx, plan, None)
            .await
    }

    async fn prevalidate_registered_dependency_blobs(
        &self,
        ctx: &Context,
        plan: LocalExportDependencyPlan,
        excluded_digest: Option<&str>,
    ) -> Result<LocalExportDependencySnapshot> {
        if plan.namespace != ctx.namespace() {
            return Err(Error::Forbidden);
        }
        let mut blob_identities = Vec::new();
        for record in &plan.records {
            if record.kind != "blob" || excluded_digest == Some(record.id.as_str()) {
                continue;
            }
            let path = self
                .root
                .join("blobs")
                .join(hash(ctx.namespace().as_bytes()))
                .join(&record.id);
            verify_blob_file(&path, &record.id, 64 * 1024 * 1024).await?;
            blob_identities.push((record.id.clone(), blob_file_identity(&path).await?));
        }
        Ok(LocalExportDependencySnapshot {
            plan,
            blob_identities,
        })
    }

    async fn verify_prevalidated_dependency_identities(
        &self,
        ctx: &Context,
        dependencies: &LocalExportDependencySnapshot,
    ) -> Result<()> {
        for (digest, expected_identity) in &dependencies.blob_identities {
            let path = self
                .root
                .join("blobs")
                .join(hash(ctx.namespace().as_bytes()))
                .join(digest);
            if blob_file_identity(&path).await? != *expected_identity {
                return Err(Error::Conflict(
                    "registered dependency blob identity changed after preflight".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn publish_registered_local_export(
        &self,
        ctx: &Context,
        export_id: &str,
        files: &[LocalExportFile],
        completed_attempt: &Value,
        delivery_audit: &Value,
        dependencies: &LocalExportDependencySnapshot,
    ) -> Result<LocalExportReceipt> {
        ctx.require(&[evo_core::Role::Admin])?;
        identifier(export_id)?;
        if dependencies.plan.namespace != ctx.namespace()
            || dependencies.plan.export_id != export_id
        {
            return Err(Error::Forbidden);
        }
        let (tree_digest, delivered_bytes) = local_export_tree_digest(files)?;
        let receipt =
            local_export_receipt(ctx.namespace(), export_id, &tree_digest, delivered_bytes)?;
        let namespace_hash = hash(ctx.namespace().as_bytes());
        let staging_root = self.root.join(".export-staging");
        ensure_unlinked_directory(&staging_root).await?;
        let staging_parent = staging_root.join(&namespace_hash);
        ensure_unlinked_directory(&staging_parent).await?;
        let staging_path = staging_parent.join(uuid::Uuid::new_v4().to_string());
        let mut staging = PrivateStagedDirectory::new(staging_path.clone());
        tokio::fs::create_dir(&staging_path)
            .await
            .map_err(internal)?;
        write_local_export_staging(staging.path(), files).await?;
        if verify_local_export_directory(staging.path(), files).await?
            != (tree_digest.clone(), delivered_bytes)
        {
            return Err(Error::Conflict("staged local export tree changed".into()));
        }
        let staging_identity = local_export_identity(staging.path()).await?;

        let final_path = self.local_export_directory(ctx, export_id)?;
        let final_parent = final_path.parent().ok_or(Error::Internal)?;
        let export_root = self.root.join("exports");
        ensure_unlinked_directory(&export_root).await?;
        ensure_unlinked_directory(final_parent).await?;
        let preverified_final = if tokio::fs::try_exists(&final_path).await.map_err(internal)? {
            if verify_local_export_directory(&final_path, files).await?
                != (tree_digest.clone(), delivered_bytes)
            {
                return Err(Error::Conflict("existing local export differs".into()));
            }
            Some(local_export_identity(&final_path).await?)
        } else {
            None
        };

        let _guard = self.blob_lock.write().await;
        if local_export_identity(staging.path()).await? != staging_identity {
            return Err(Error::Conflict(
                "staged local export identity changed".into(),
            ));
        }
        match &preverified_final {
            Some(identity) => {
                if local_export_identity(&final_path).await? != *identity {
                    return Err(Error::Conflict(
                        "existing local export identity changed".into(),
                    ));
                }
            }
            None => {
                if tokio::fs::try_exists(&final_path).await.map_err(internal)? {
                    return Err(Error::Conflict(
                        "local export target appeared after preflight".into(),
                    ));
                }
            }
        }
        self.verify_prevalidated_dependency_identities(ctx, dependencies)
            .await?;
        let mut session = self.session().await?;
        let fenced = sqlx::query(
            "UPDATE objects SET revision=revision WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(ctx.namespace())
        .bind(export_id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        if fenced.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        if session
            .get::<Value>(ctx, "tombstone", export_id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let attempt: Value = session.need(ctx, "artifact", export_id).await?;
        validate_local_export_attempt(ctx, export_id, &attempt, &receipt, "prepared")?;
        let expected_watermark = attempt["revoke_watermark"]
            .as_u64()
            .ok_or_else(|| Error::Conflict("export attempt lacks watermark".into()))?;
        let actual_watermark = session
            .watermark(ctx)
            .await?
            .and_then(|(sequence, _)| u64::try_from(sequence).ok())
            .ok_or_else(|| Error::Conflict("current watermark is unavailable".into()))?;
        if actual_watermark != expected_watermark {
            return Err(Error::Conflict("export watermark changed".into()));
        }
        let refs = attempt["source_refs"]
            .as_array()
            .ok_or_else(|| Error::Conflict("export attempt lacks source_refs".into()))?;
        verify_export_source_refs(&mut session, ctx, refs).await?;
        verify_dependency_snapshot_in_session(&mut session, ctx, dependencies).await?;
        validate_local_export_completion(
            ctx,
            export_id,
            completed_attempt,
            delivery_audit,
            &receipt,
            actual_watermark,
        )?;
        if completed_attempt["input_digest"] != attempt["input_digest"]
            || completed_attempt["source_refs"] != attempt["source_refs"]
            || completed_attempt["revoke_watermark"] != attempt["revoke_watermark"]
            || delivery_audit["source_refs"] != attempt["source_refs"]
            || delivery_audit["revoke_watermark"] != attempt["revoke_watermark"]
        {
            return Err(Error::Conflict(
                "local export completion changed its source closure".into(),
            ));
        }
        let mut expected_completed = attempt.clone();
        expected_completed["updated_at"] = completed_attempt["updated_at"].clone();
        expected_completed["payload"]["state"] = Value::String("completed".into());
        expected_completed["payload"]["delivery_audit_id"] =
            completed_attempt["payload"]["delivery_audit_id"].clone();
        expected_completed["payload"]["completion_receipt_digest"] =
            completed_attempt["payload"]["completion_receipt_digest"].clone();
        if expected_completed != *completed_attempt {
            return Err(Error::Conflict("completed export content differs".into()));
        }

        if preverified_final.is_none() {
            if !publish_blob_no_replace(staging.path(), &final_path)? {
                return Err(Error::Conflict(
                    "local export target raced after write fence".into(),
                ));
            }
            staging.disarm();
            sync_directory(final_parent).await?;
            if local_export_identity(&final_path).await? != staging_identity {
                return Err(Error::Conflict(
                    "published local export identity changed".into(),
                ));
            }
        }

        let audit_id = delivery_audit["id"]
            .as_str()
            .ok_or_else(|| Error::Conflict("delivery audit lacks id".into()))?;
        if let Some(existing) = session.get::<Value>(ctx, "artifact", audit_id).await? {
            if existing != *delivery_audit {
                return Err(Error::Conflict("delivery audit differs".into()));
            }
        } else {
            session
                .put(ctx, "artifact", audit_id, ctx.actor(), delivery_audit)
                .await?;
        }
        session
            .put(ctx, "artifact", export_id, ctx.actor(), completed_attempt)
            .await?;
        session
            .put_edge(ctx, "artifact", audit_id, "artifact", export_id)
            .await?;
        session
            .put_edge(ctx, "artifact", export_id, "artifact", audit_id)
            .await?;
        session.audit(ctx, "e16.export.complete", export_id).await?;
        session.commit().await?;
        Ok(receipt)
    }

    pub async fn verify_registered_local_export(
        &self,
        ctx: &Context,
        export_id: &str,
        files: &[LocalExportFile],
    ) -> Result<LocalExportReceipt> {
        ctx.require(&[evo_core::Role::Admin])?;
        identifier(export_id)?;
        let dependency_plan = self
            .snapshot_local_export_dependencies(ctx, export_id)
            .await?;
        let dependencies = self
            .prevalidate_local_export_dependency_blobs(ctx, dependency_plan)
            .await?;
        let (tree_digest, delivered_bytes) = local_export_tree_digest(files)?;
        let receipt =
            local_export_receipt(ctx.namespace(), export_id, &tree_digest, delivered_bytes)?;
        let path = self.local_export_directory(ctx, export_id)?;
        if verify_local_export_directory(&path, files).await? != (tree_digest, delivered_bytes) {
            return Err(Error::Conflict(
                "completed local export is unavailable".into(),
            ));
        }
        let identity = local_export_identity(&path).await?;
        let _guard = self.blob_lock.read().await;
        if local_export_identity(&path).await? != identity {
            return Err(Error::Conflict(
                "completed local export identity changed".into(),
            ));
        }
        self.verify_prevalidated_dependency_identities(ctx, &dependencies)
            .await?;
        let mut session = self.session().await?;
        let fenced = sqlx::query(
            "UPDATE objects SET revision=revision WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(ctx.namespace())
        .bind(export_id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        if fenced.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        if session
            .get::<Value>(ctx, "tombstone", export_id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let attempt: Value = session.need(ctx, "artifact", export_id).await?;
        validate_local_export_attempt(ctx, export_id, &attempt, &receipt, "completed")?;
        let watermark = session
            .watermark(ctx)
            .await?
            .and_then(|(sequence, _)| u64::try_from(sequence).ok())
            .ok_or_else(|| Error::Conflict("current watermark is unavailable".into()))?;
        if attempt["revoke_watermark"].as_u64() != Some(watermark) {
            return Err(Error::Conflict("completed export sources are stale".into()));
        }
        let refs = attempt["source_refs"]
            .as_array()
            .ok_or_else(|| Error::Conflict("completed export lacks source_refs".into()))?;
        verify_export_source_refs(&mut session, ctx, refs).await?;
        verify_dependency_snapshot_in_session(&mut session, ctx, &dependencies).await?;
        let audit_id = attempt["payload"]["delivery_audit_id"]
            .as_str()
            .ok_or_else(|| Error::Conflict("completed export lacks delivery audit".into()))?;
        let audit: Value = session.need(ctx, "artifact", audit_id).await?;
        validate_local_export_completion(ctx, export_id, &attempt, &audit, &receipt, watermark)?;
        session.commit().await?;
        Ok(receipt)
    }

    pub async fn abort_registered_local_export(
        &self,
        ctx: &Context,
        export_id: &str,
        aborted_attempt: &Value,
    ) -> Result<()> {
        ctx.require(&[evo_core::Role::Admin])?;
        identifier(export_id)?;
        let _guard = self.blob_lock.write().await;
        let mut session = self.session().await?;
        let fenced = sqlx::query(
            "UPDATE objects SET revision=revision WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(ctx.namespace())
        .bind(export_id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        if fenced.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        let current: Value = session.need(ctx, "artifact", export_id).await?;
        if current["schema_version"].as_str() != Some("rsia.e16.export_attempt.v2")
            || current["owner_actor"].as_str() != Some(ctx.actor())
            || current["payload"]["state"].as_str() != Some("prepared")
            || aborted_attempt["schema_version"].as_str() != Some("rsia.e16.export_attempt.v2")
            || aborted_attempt["id"].as_str() != Some(export_id)
            || aborted_attempt["payload"]["state"].as_str() != Some("aborted")
        {
            return Err(Error::Conflict("export cannot be aborted".into()));
        }
        let mut expected_abort = current.clone();
        expected_abort["updated_at"] = aborted_attempt["updated_at"].clone();
        expected_abort["payload"]["state"] = Value::String("aborted".into());
        if expected_abort != *aborted_attempt {
            return Err(Error::Conflict("aborted export content differs".into()));
        }
        let path = self.local_export_directory(ctx, export_id)?;
        if tokio::fs::try_exists(path).await.map_err(internal)? {
            return Err(Error::Conflict(
                "published local export must be recovered, not aborted".into(),
            ));
        }
        session
            .put(ctx, "artifact", export_id, ctx.actor(), aborted_attempt)
            .await?;
        session.audit(ctx, "e16.export.abort", export_id).await?;
        session.commit().await
    }
    /// Read a namespace-scoped content blob for a trusted management consumer.
    /// The path is derived from the authenticated context, and bytes are
    /// re-hashed so a replaced file cannot inherit authority from its name.
    pub async fn read_blob(
        &self,
        ctx: &Context,
        digest: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        ctx.require(&[
            evo_core::Role::Admin,
            evo_core::Role::Host,
            evo_core::Role::Worker,
        ])?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Invalid("invalid lowercase content digest".into()));
        }
        if max_bytes == 0 || max_bytes > 64 * 1024 * 1024 {
            return Err(Error::Invalid(
                "blob read bound must be 1..=67108864 bytes".into(),
            ));
        }
        let _guard = self.blob_lock.read().await;
        let path = self
            .root
            .join("blobs")
            .join(hash(ctx.namespace().as_bytes()))
            .join(digest);
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::NotFound
                } else {
                    internal(error)
                }
            })?;
            if !metadata.file_type().is_file() {
                return Err(Error::Conflict("blob is not a regular file".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(Error::Conflict("linked blob is not trusted content".into()));
                }
            }
            if metadata.len() == 0 || metadata.len() > max_bytes as u64 {
                return Err(Error::Conflict(
                    "blob exceeds the authorized read bound".into(),
                ));
            }
            let file = File::open(&path).map_err(internal)?;
            let opened = file.metadata().map_err(internal)?;
            if !opened.is_file() || opened.len() != metadata.len() {
                return Err(Error::Conflict("blob changed while being opened".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if opened.nlink() != 1
                    || opened.dev() != metadata.dev()
                    || opened.ino() != metadata.ino()
                {
                    return Err(Error::Conflict("blob changed while being opened".into()));
                }
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            file.take(max_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(internal)?;
            if bytes.is_empty() || bytes.len() > max_bytes {
                return Err(Error::Conflict("blob changed while being read".into()));
            }
            Ok(bytes)
        })
        .await
        .map_err(internal)??;
        if hash(&bytes) != digest {
            return Err(Error::Conflict("blob content digest mismatch".into()));
        }
        Ok(bytes)
    }
    pub async fn delete_blob(&self, ctx: &Context, digest: &str) -> Result<()> {
        ctx.require(&[evo_core::Role::Admin])?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("invalid content digest".into()));
        }
        let _guard = self.blob_lock.write().await;
        let path = self
            .root
            .join("blobs")
            .join(hash(ctx.namespace().as_bytes()))
            .join(digest);
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }
    pub async fn verify_audit(&self, ctx: &Context) -> Result<usize> {
        ctx.require(&[evo_core::Role::Admin, evo_core::Role::Evaluator])?;
        let rows = sqlx::query(
            "SELECT payload,previous_hash,digest FROM audit WHERE namespace=? ORDER BY seq",
        )
        .bind(ctx.namespace())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut previous = String::new();
        for row in &rows {
            let payload: String = row.try_get("payload").map_err(internal)?;
            let prev: String = row.try_get("previous_hash").map_err(internal)?;
            let digest: String = row.try_get("digest").map_err(internal)?;
            if prev != previous || digest != hash(format!("{prev}\n{payload}").as_bytes()) {
                return Err(Error::Conflict("audit chain mismatch".into()));
            }
            previous = digest;
        }
        Ok(rows.len())
    }
}
impl Session {
    /// Count persisted objects in the authenticated namespace inside this
    /// transaction. This is an authority-side measurement, never caller usage.
    pub async fn namespace_object_count(&mut self, ctx: &Context, kind: &str) -> Result<u64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE namespace=? AND kind=?")
                .bind(ctx.namespace())
                .bind(kind)
                .fetch_one(&mut *self.tx)
                .await
                .map_err(internal)?;
        u64::try_from(count).map_err(|_| Error::Internal)
    }

    /// Insert a new object only when the current namespace count is below the
    /// bound. Count and insert share this transaction and the Store serializes
    /// writers, so concurrent callers cannot both cross the limit.
    pub async fn put_new_below_capacity<T: Serialize>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
        owner: &str,
        body: &T,
        limit: u64,
    ) -> Result<()> {
        if limit == 0 {
            return Err(Error::Invalid("capacity limit must be positive".into()));
        }
        if self.get::<Value>(ctx, kind, id).await?.is_some() {
            return Err(Error::Conflict(
                "bounded creation requires a new object identity".into(),
            ));
        }
        let count = self.namespace_object_count(ctx, kind).await?;
        if count >= limit {
            return Err(Error::Conflict(format!(
                "namespace {kind} capacity exceeded: {count} >= {limit}"
            )));
        }
        self.put(ctx, kind, id, owner, body).await
    }

    /// Atomically sum a nonnegative integer from strict versioned objects and
    /// create a new object only when its authoritative delta fits the bound.
    #[allow(clippy::too_many_arguments)]
    pub async fn put_new_below_json_sum_capacity<T: Serialize>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
        owner: &str,
        body: &T,
        schema_version: &str,
        numeric_path: &str,
        delta: u64,
        limit: u64,
    ) -> Result<()> {
        validate_json_path(numeric_path)?;
        if limit == 0 || delta > limit {
            return Err(Error::Invalid("invalid capacity limit or delta".into()));
        }
        let value = serde_json::to_value(body).map_err(internal)?;
        if value.get("schema_version").and_then(Value::as_str) != Some(schema_version)
            || json_u64_at_path(&value, numeric_path) != Some(delta)
        {
            return Err(Error::Invalid(
                "created object does not match its capacity schema or delta".into(),
            ));
        }
        if self.get::<Value>(ctx, kind, id).await?.is_some() {
            return Err(Error::Conflict(
                "bounded creation requires a new object identity".into(),
            ));
        }
        let invalid: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objects WHERE namespace=? AND kind=?
             AND json_extract(body,'$.schema_version')=?
             AND (json_type(body,?) IS NULL OR json_type(body,?)!='integer'
                  OR json_extract(body,?)<0)",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .bind(schema_version)
        .bind(numeric_path)
        .bind(numeric_path)
        .bind(numeric_path)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        if invalid != 0 {
            return Err(Error::Conflict(
                "existing capacity records contain an invalid numeric fact".into(),
            ));
        }
        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(json_extract(body,?)),0) FROM objects
             WHERE namespace=? AND kind=? AND json_extract(body,'$.schema_version')=?",
        )
        .bind(numeric_path)
        .bind(ctx.namespace())
        .bind(kind)
        .bind(schema_version)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        let used = u64::try_from(used).map_err(|_| Error::Internal)?;
        if used.checked_add(delta).is_none_or(|total| total > limit) {
            return Err(Error::Conflict(format!(
                "namespace event capacity exceeded: {used} + {delta} > {limit}"
            )));
        }
        self.put(ctx, kind, id, owner, body).await
    }

    pub async fn get<T: DeserializeOwned>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<Option<T>> {
        identifier(id)?;
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind=? AND id=?")
                .bind(ctx.namespace())
                .bind(kind)
                .bind(id)
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(internal)?;
        body.map(|b| serde_json::from_str(&b).map_err(internal))
            .transpose()
    }
    pub async fn need<T: DeserializeOwned>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<T> {
        self.get(ctx, kind, id).await?.ok_or(Error::NotFound)
    }
    pub async fn list<T: DeserializeOwned>(&mut self, ctx: &Context, kind: &str) -> Result<Vec<T>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM objects WHERE namespace=? AND kind=? ORDER BY rowid LIMIT 10001",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(internal)?;
        if rows.len() > 10000 {
            return Err(Error::Conflict("scope collection exceeds bounded scan; archive or add paginated query before continuing".into()));
        }
        rows.into_iter()
            .map(|r| serde_json::from_str(&r).map_err(internal))
            .collect()
    }
    pub async fn put<T: Serialize>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
        owner: &str,
        body: &T,
    ) -> Result<()> {
        identifier(id)?;
        identifier(owner)?;
        if kind == "run" {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM objects WHERE namespace=? AND kind='run' AND id=?",
            )
            .bind(ctx.namespace())
            .bind(id)
            .fetch_one(&mut *self.tx)
            .await
            .map_err(internal)?;
            if exists == 0 {
                let used: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM objects WHERE namespace=? AND (
                       kind='run' OR
                       (kind='artifact' AND json_extract(body,'$.source')='trusted_host_snapshot')
                     )",
                )
                .bind(ctx.namespace())
                .fetch_one(&mut *self.tx)
                .await
                .map_err(internal)?;
                if u64::try_from(used).map_err(|_| Error::Internal)? >= MVP_MAX_RUNS {
                    return Err(Error::Conflict(format!(
                        "namespace run capacity exceeded: {used} >= {MVP_MAX_RUNS}"
                    )));
                }
            }
        }
        let body = serde_json::to_string(body).map_err(internal)?;
        if body.len() > 4 * 1024 * 1024 {
            return Err(Error::Invalid("object exceeds 4 MiB".into()));
        }
        sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?) ON CONFLICT(namespace,kind,id) DO UPDATE SET owner=excluded.owner,body=excluded.body,revision=objects.revision+1")
   .bind(ctx.namespace()).bind(kind).bind(id).bind(owner).bind(body).execute(&mut *self.tx).await.map_err(internal)?;
        Ok(())
    }
    pub async fn delete(&mut self, ctx: &Context, kind: &str, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM objects WHERE namespace=? AND kind=? AND id=?")
            .bind(ctx.namespace())
            .bind(kind)
            .bind(id)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }
    pub async fn cached<T: DeserializeOwned, A: Serialize>(
        &mut self,
        ctx: &Context,
        op: &str,
        key: &str,
        args: &A,
    ) -> Result<Option<T>> {
        identifier(key)?;
        let row=sqlx::query("SELECT payload_hash,response,redacted FROM idempotency WHERE namespace=? AND actor=? AND operation=? AND request_key=?")
   .bind(ctx.namespace()).bind(ctx.actor()).bind(op).bind(key).fetch_optional(&mut *self.tx).await.map_err(internal)?;
        let Some(row) = row else { return Ok(None) };
        let expected: String = row.try_get("payload_hash").map_err(internal)?;
        if expected != fingerprint(args)? {
            return Err(Error::Conflict(
                "idempotency key reused with different content".into(),
            ));
        }
        if row.try_get::<i64, _>("redacted").map_err(internal)? != 0 {
            return Err(Error::Conflict(
                "subject was deleted; request cannot be replayed".into(),
            ));
        }
        let body: String = row.try_get("response").map_err(internal)?;
        Ok(Some(serde_json::from_str(&body).map_err(internal)?))
    }
    pub async fn cache<A: Serialize, T: Serialize>(
        &mut self,
        ctx: &Context,
        op: &str,
        key: &str,
        args: &A,
        subject: &str,
        result: &T,
    ) -> Result<()> {
        sqlx::query("INSERT INTO idempotency(namespace,actor,operation,request_key,payload_hash,subject_id,response) VALUES(?,?,?,?,?,?,?)")
   .bind(ctx.namespace()).bind(ctx.actor()).bind(op).bind(key).bind(fingerprint(args)?).bind(subject).bind(serde_json::to_string(result).map_err(internal)?)
   .execute(&mut *self.tx).await.map_err(internal)?;
        Ok(())
    }
    pub async fn redact_cache(&mut self, ctx: &Context, subject: &str) -> Result<()> {
        sqlx::query(
            "UPDATE idempotency SET response='null',redacted=1 WHERE namespace=? AND subject_id=?",
        )
        .bind(ctx.namespace())
        .bind(subject)
        .execute(&mut *self.tx)
        .await
        .map_err(internal)?;
        Ok(())
    }
    pub async fn audit(&mut self, ctx: &Context, operation: &str, entity: &str) -> Result<()> {
        let prev: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM audit WHERE namespace=? ORDER BY seq DESC LIMIT 1",
        )
        .bind(ctx.namespace())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        let prev = prev.unwrap_or_default();
        let payload=json!({"actor":ctx.actor(),"role":ctx.role(),"operation":operation,"entity":entity,"timestamp":now()}).to_string();
        let digest = hash(format!("{prev}\n{payload}").as_bytes());
        sqlx::query("INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?)")
            .bind(ctx.namespace())
            .bind(payload)
            .bind(prev)
            .bind(digest)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }
    pub async fn replace_search_index(
        &mut self,
        ctx: &Context,
        entries: &[(String, String)],
    ) -> Result<()> {
        sqlx::query("DELETE FROM skill_fts WHERE namespace=?")
            .bind(ctx.namespace())
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        for (id, content) in entries {
            let tokens = search_tokens(content).join(" ");
            sqlx::query("INSERT INTO skill_fts(namespace,candidate_id,tokens) VALUES(?,?,?)")
                .bind(ctx.namespace())
                .bind(id)
                .bind(tokens)
                .execute(&mut *self.tx)
                .await
                .map_err(internal)?;
        }
        Ok(())
    }
    pub async fn search(&mut self, ctx: &Context, query: &str) -> Result<Vec<String>> {
        let tokens = search_tokens(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let expression = tokens
            .iter()
            .take(32)
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        sqlx::query_scalar("SELECT candidate_id FROM skill_fts WHERE skill_fts MATCH ? AND namespace=? ORDER BY bm25(skill_fts),candidate_id LIMIT 64").bind(expression).bind(ctx.namespace()).fetch_all(&mut *self.tx).await.map_err(internal)
    }
    pub async fn put_edge(
        &mut self,
        ctx: &Context,
        src_kind: &str,
        src_id: &str,
        dst_kind: &str,
        dst_id: &str,
    ) -> Result<()> {
        identifier(src_id)?;
        identifier(dst_id)?;
        sqlx::query("INSERT OR IGNORE INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)")
            .bind(ctx.namespace())
            .bind(src_kind)
            .bind(src_id)
            .bind(dst_kind)
            .bind(dst_id)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn dependents(
        &mut self,
        ctx: &Context,
        dst_kind: &str,
        dst_id: &str,
    ) -> Result<Vec<(String, String)>> {
        identifier(dst_id)?;
        let rows = sqlx::query("SELECT src_kind,src_id FROM dependencies WHERE namespace=? AND dst_kind=? AND dst_id=?")
            .bind(ctx.namespace())
            .bind(dst_kind)
            .bind(dst_id)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(internal)?;
        let mut out = Vec::new();
        for row in rows {
            out.push((
                row.try_get("src_kind").map_err(internal)?,
                row.try_get("src_id").map_err(internal)?,
            ));
        }
        Ok(out)
    }

    pub async fn bump_watermark(&mut self, ctx: &Context, digest: &str) -> Result<i64> {
        identifier(digest)?;
        let prev: i64 = sqlx::query_scalar("SELECT seq FROM revoke_watermark WHERE namespace=?")
            .bind(ctx.namespace())
            .fetch_optional(&mut *self.tx)
            .await
            .map_err(internal)?
            .unwrap_or(0);
        let seq = prev + 1;
        sqlx::query("INSERT INTO revoke_watermark(namespace,seq,digest) VALUES(?,?,?) ON CONFLICT(namespace) DO UPDATE SET seq=excluded.seq,digest=excluded.digest")
            .bind(ctx.namespace())
            .bind(seq)
            .bind(digest)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(seq)
    }

    pub async fn watermark(&mut self, ctx: &Context) -> Result<Option<(i64, String)>> {
        let row = sqlx::query("SELECT seq,digest FROM revoke_watermark WHERE namespace=?")
            .bind(ctx.namespace())
            .fetch_optional(&mut *self.tx)
            .await
            .map_err(internal)?;
        match row {
            Some(r) => Ok(Some((
                r.try_get("seq").map_err(internal)?,
                r.try_get("digest").map_err(internal)?,
            ))),
            None => Ok(None),
        }
    }

    pub async fn put_world(
        &mut self,
        ctx: &Context,
        id: &str,
        sealed: bool,
        manifest: &Value,
    ) -> Result<()> {
        identifier(id)?;
        if let Some((existing_sealed, existing_manifest)) = self.get_world(ctx, id).await? {
            if existing_sealed {
                if sealed && existing_manifest == *manifest {
                    return Ok(());
                }
                return Err(Error::Conflict(
                    "sealed replay world cannot be changed or reopened".into(),
                ));
            }
            if !sealed && existing_manifest == *manifest {
                return Ok(());
            }
        }
        sqlx::query("INSERT INTO replay_worlds(namespace,id,sealed,manifest) VALUES(?,?,?,?) ON CONFLICT(namespace,id) DO UPDATE SET sealed=excluded.sealed,manifest=excluded.manifest")
            .bind(ctx.namespace())
            .bind(id)
            .bind(i64::from(sealed))
            .bind(manifest.to_string())
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn get_world(&mut self, ctx: &Context, id: &str) -> Result<Option<(bool, Value)>> {
        identifier(id)?;
        let row =
            sqlx::query("SELECT sealed,manifest FROM replay_worlds WHERE namespace=? AND id=?")
                .bind(ctx.namespace())
                .bind(id)
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(internal)?;
        match row {
            Some(r) => {
                let sealed: i64 = r.try_get("sealed").map_err(internal)?;
                let manifest: String = r.try_get("manifest").map_err(internal)?;
                Ok(Some((
                    sealed != 0,
                    serde_json::from_str(&manifest).map_err(internal)?,
                )))
            }
            None => Ok(None),
        }
    }

    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await.map_err(internal)
    }
    pub async fn raw_object(&mut self, ctx: &Context, kind: &str, id: &str) -> Result<Value> {
        self.need(ctx, kind, id).await
    }
}

fn validate_json_path(path: &str) -> Result<()> {
    let Some(rest) = path.strip_prefix("$.") else {
        return Err(Error::Invalid("invalid JSON capacity path".into()));
    };
    if rest.is_empty()
        || rest.split('.').any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(Error::Invalid("invalid JSON capacity path".into()));
    }
    Ok(())
}

fn json_u64_at_path(value: &Value, path: &str) -> Option<u64> {
    json_value_at_path(value, path)?.as_u64()
}

fn json_value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.strip_prefix("$.")?.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::Role;
    async fn db() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("db.sqlite3")).await.unwrap();
        (d, s)
    }
    #[tokio::test]
    async fn bounded_blob_read_is_role_namespace_digest_and_size_scoped() {
        let (dir, store) = db().await;
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let other = Context::new("other", "admin", Role::Admin).unwrap();
        let agent = Context::new("n", "agent", Role::Agent).unwrap();
        let digest = store.put_blob(&admin, b"trusted bytes").await.unwrap();
        assert_eq!(
            store.read_blob(&admin, &digest, 64).await.unwrap(),
            b"trusted bytes"
        );
        assert!(matches!(
            store.read_blob(&agent, &digest, 64).await,
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            store.read_blob(&other, &digest, 64).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.read_blob(&admin, &digest, 4).await,
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            store.read_blob(&admin, &digest.to_uppercase(), 64).await,
            Err(Error::Invalid(_))
        ));
        #[cfg(unix)]
        {
            let blob_path = store
                .root
                .join("blobs")
                .join(hash(admin.namespace().as_bytes()))
                .join(&digest);
            let linked = dir.path().join("linked-blob");
            std::fs::hard_link(&blob_path, &linked).unwrap();
            assert!(matches!(
                store.read_blob(&admin, &digest, 64).await,
                Err(Error::Conflict(_))
            ));
            std::fs::remove_file(linked).unwrap();
        }
        store.close().await;
        let reopened = Store::open(&dir.path().join("db.sqlite3")).await.unwrap();
        assert_eq!(
            reopened.read_blob(&admin, &digest, 64).await.unwrap(),
            b"trusted bytes"
        );
    }
    #[tokio::test]
    async fn bounded_blob_write_preserves_legacy_limit_and_accepts_registered_projection_size() {
        let (_dir, store) = db().await;
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let bytes = vec![b'x'; 1024 * 1024 + 1];
        assert!(matches!(
            store.put_blob(&admin, &bytes).await,
            Err(Error::Invalid(_))
        ));
        let digest = store
            .put_blob_bounded(&admin, &bytes, 2 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            store
                .read_blob(&admin, &digest, 2 * 1024 * 1024)
                .await
                .unwrap(),
            bytes
        );
        assert!(matches!(
            store
                .put_blob_bounded(&admin, b"x", 64 * 1024 * 1024 + 1)
                .await,
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn registered_blob_publish_is_fenced_by_prepared_owner_sources_and_watermark() {
        let (_dir, store) = db().await;
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let source = json!({"id":"source","value":"authority"});
        let nested = json!({"id":"nested","value":"dependency"});
        let source_digest = fingerprint(&source).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "run", "source", admin.actor(), &source)
            .await
            .unwrap();
        session
            .put(&admin, "run", "nested", admin.actor(), &nested)
            .await
            .unwrap();
        session
            .put_edge(&admin, "run", "source", "run", "nested")
            .await
            .unwrap();
        session
            .bump_watermark(&admin, &hash(b"watermark-1"))
            .await
            .unwrap();
        let bytes = b"registered package projection";
        let digest = hash(bytes);
        let artifact = json!({
            "schema_version":"rsia.e16.staged_asset.v1",
            "id":"prepared-package",
            "namespace":"n",
            "owner_actor":"admin",
            "request_key":"request",
            "input_digest":hash(b"input"),
            "created_at":1,
            "updated_at":1,
            "source_refs":[{"kind":"run","id":"source","digest":source_digest}],
            "revoke_watermark":1,
            "payload":{"state":"prepared","content_blob_digest":digest}
        });
        session
            .put(
                &admin,
                "artifact",
                "prepared-package",
                admin.actor(),
                &artifact,
            )
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", "prepared-package", "run", "source")
            .await
            .unwrap();
        session.commit().await.unwrap();

        let other = Context::new("n", "other-admin", Role::Admin).unwrap();
        assert!(matches!(
            store
                .publish_registered_blob(
                    &other,
                    "prepared-package",
                    "rsia.e16.staged_asset.v1",
                    "content_blob_digest",
                    bytes,
                    64 * 1024 * 1024,
                )
                .await,
            Err(Error::Conflict(_))
        ));

        let first = store
            .publish_registered_blob(
                &admin,
                "prepared-package",
                "rsia.e16.staged_asset.v1",
                "content_blob_digest",
                bytes,
                64 * 1024 * 1024,
            )
            .await
            .unwrap();
        let retry = store
            .publish_registered_blob(
                &admin,
                "prepared-package",
                "rsia.e16.staged_asset.v1",
                "content_blob_digest",
                bytes,
                64 * 1024 * 1024,
            )
            .await
            .unwrap();
        assert_eq!(first, retry);

        let mut session = store.session().await.unwrap();
        session
            .put(
                &admin,
                "tombstone",
                "nested",
                admin.actor(),
                &json!({"id":"nested"}),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(
            store
                .publish_registered_blob(
                    &admin,
                    "prepared-package",
                    "rsia.e16.staged_asset.v1",
                    "content_blob_digest",
                    bytes,
                    64 * 1024 * 1024,
                )
                .await,
            Err(Error::Forbidden)
        ));

        let mut session = store.session().await.unwrap();
        session.delete(&admin, "tombstone", "nested").await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"watermark-2"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(
            store
                .publish_registered_blob(
                    &admin,
                    "prepared-package",
                    "rsia.e16.staged_asset.v1",
                    "content_blob_digest",
                    bytes,
                    64 * 1024 * 1024,
                )
                .await,
            Err(Error::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn restart_cleans_unpublished_private_blob_staging() {
        let (dir, store) = db().await;
        let staging = dir.path().join(".blob-staging").join(hash(b"n"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("orphan.tmp"), b"secret").unwrap();
        store.close().await;
        let reopened = Store::open(&dir.path().join("db.sqlite3")).await.unwrap();
        assert!(!dir.path().join(".blob-staging").exists());
        reopened.close().await;
    }

    #[tokio::test]
    async fn cancelled_registered_write_removes_partial_private_temp() {
        let (_dir, store) = db().await;
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let bytes = vec![b'z'; 2 * 1024 * 1024];
        let digest = hash(&bytes);
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"cancel-watermark"))
            .await
            .unwrap();
        session
            .put(
                &admin,
                "artifact",
                "cancelled-producer",
                admin.actor(),
                &json!({
                    "schema_version":"rsia.e16.staged_asset.v1",
                    "id":"cancelled-producer",
                    "namespace":"n",
                    "owner_actor":"admin",
                    "request_key":"cancel-request",
                    "input_digest":hash(b"cancel-input"),
                    "created_at":1,
                    "updated_at":1,
                    "source_refs":[],
                    "revoke_watermark":1,
                    "payload":{"state":"prepared","content_blob_digest":digest}
                }),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();

        let guard = store.blob_lock.write().await;
        let task_store = store.clone();
        let task_admin = admin.clone();
        let task = tokio::spawn(async move {
            task_store
                .publish_registered_blob(
                    &task_admin,
                    "cancelled-producer",
                    "rsia.e16.staged_asset.v1",
                    "content_blob_digest",
                    &bytes,
                    4 * 1024 * 1024,
                )
                .await
        });
        let staging = store
            .root
            .join(".blob-staging")
            .join(hash(admin.namespace().as_bytes()));
        let mut observed = false;
        for _ in 0..200 {
            if let Ok(mut entries) = tokio::fs::read_dir(&staging).await
                && entries.next_entry().await.unwrap().is_some()
            {
                observed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(observed, "registered write never reached private staging");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(guard);
        let mut entries = tokio::fs::read_dir(&staging).await.unwrap();
        assert!(entries.next_entry().await.unwrap().is_none());
        assert!(matches!(
            store.read_blob(&admin, &digest, 4 * 1024 * 1024).await,
            Err(Error::NotFound)
        ));
    }
    #[tokio::test]
    async fn rollback_is_atomic() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        {
            let mut t = s.session().await.unwrap();
            t.put(&c, "run", "r", "a", &json!({"id":"r"}))
                .await
                .unwrap();
        }
        let mut t = s.session().await.unwrap();
        assert!(t.get::<Value>(&c, "run", "r").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn namespaces_never_cross() {
        let (_d, s) = db().await;
        let a = Context::new("a", "u", Role::Admin).unwrap();
        let b = Context::new("b", "u", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put(&a, "run", "r", "u", &json!({"id":"r"}))
            .await
            .unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        assert!(t.get::<Value>(&b, "run", "r").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn caches_detect_conflict_and_redaction() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.cache(&c, "op", "k", &1, "r", &2).await.unwrap();
        assert_eq!(
            t.cached::<i32, _>(&c, "op", "k", &1).await.unwrap(),
            Some(2)
        );
        assert!(t.cached::<i32, _>(&c, "op", "k", &3).await.is_err());
        t.redact_cache(&c, "r").await.unwrap();
        assert!(t.cached::<i32, _>(&c, "op", "k", &1).await.is_err());
    }
    #[tokio::test]
    async fn fts_chinese_and_injection() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.replace_search_index(&c, &[("x".into(), "检查配置路径 config_file".into())])
            .await
            .unwrap();
        assert_eq!(t.search(&c, "配置").await.unwrap(), vec!["x"]);
        assert!(t.search(&c, "\" OR * ; DROP TABLE objects").await.is_ok());
    }
    #[tokio::test]
    async fn audit_and_backup_survive_restart() {
        let (d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.audit(&c, "test", "x").await.unwrap();
        t.bump_watermark(&c, &hash(b"initial-watermark"))
            .await
            .unwrap();
        t.commit().await.unwrap();
        assert_eq!(s.verify_audit(&c).await.unwrap(), 1);
        let backup = d.path().join("backup");
        s.backup(&backup).await.unwrap();
        let restored = Store::open(&backup.join("rsia.sqlite3")).await.unwrap();
        assert_eq!(restored.verify_audit(&c).await.unwrap(), 1);
        assert_eq!(restored.integrity().await.unwrap(), "ok");
    }
    #[tokio::test]
    async fn revoke_edge_blocks_dependents_and_watermark_is_required() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put_edge(&c, "job", "j1", "run", "run2").await.unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        let deps = t.dependents(&c, "run", "run2").await.unwrap();
        assert_eq!(deps, vec![("job".into(), "j1".into())]);
        let seq = t.bump_watermark(&c, "wm1").await.unwrap();
        assert_eq!(seq, 1);
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        assert!(t.watermark(&c).await.unwrap().is_some());
        t.commit().await.unwrap();
        let mut other = s.session().await.unwrap();
        let b = Context::new("other", "a", Role::Admin).unwrap();
        assert!(other.watermark(&b).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn sealed_world_roundtrip() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put_world(&c, "w1", true, &json!({"k":"v"}))
            .await
            .unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        let got = t.get_world(&c, "w1").await.unwrap().unwrap();
        assert!(got.0);
        assert_eq!(got.1["k"], "v");
    }
}
