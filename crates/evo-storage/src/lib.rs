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
                "blob is linked, non-regular, empty, or oversized".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(Error::Conflict("blob is hard-linked".into()));
            }
        }
        let bytes = std::fs::read(&path).map_err(internal)?;
        if bytes.len() > max_bytes || hash(&bytes) != expected {
            return Err(Error::Conflict("blob digest mismatch".into()));
        }
        Ok(())
    })
    .await
    .map_err(internal)??;
    Ok(())
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

async fn publish_or_verify_blob(
    temp: &Path,
    final_path: &Path,
    digest: &str,
    max_bytes: usize,
    existing_preverified: bool,
) -> Result<bool> {
    let source = temp.to_path_buf();
    let destination = final_path.to_path_buf();
    let published =
        tokio::task::spawn_blocking(move || publish_blob_no_replace(&source, &destination))
            .await
            .map_err(internal)??;
    if !published {
        if existing_preverified {
            verify_blob_identity(final_path, digest, max_bytes).await?;
        } else {
            verify_blob_file(final_path, digest, max_bytes).await?;
        }
        match tokio::fs::remove_file(temp).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(internal(error)),
        }
    }
    Ok(published)
}
async fn verify_blob_identity(path: &Path, digest: &str, max_bytes: usize) -> Result<()> {
    if path.file_name().and_then(|n| n.to_str()) != Some(digest) {
        return Err(Error::Conflict("published blob filename mismatch".into()));
    }
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(internal)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > max_bytes as u64 {
        return Err(Error::Conflict(
            "published blob identity is linked, non-regular, empty, or oversized".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(Error::Conflict("published blob is hard-linked".into()));
        }
    }
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || File::open(path).and_then(|dir| dir.sync_all()))
        .await
        .map_err(internal)?
        .map_err(internal)
}

async fn cleanup_private_blob_staging(root: &Path) -> Result<()> {
    let staging = root.join(".blob-staging");
    let metadata = match tokio::fs::symlink_metadata(&staging).await {
        Ok(value) => value,
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
        if !namespace.file_type().await.map_err(internal)?.is_dir() {
            return Err(Error::Conflict(
                "private blob staging namespace is non-directory".into(),
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

async fn verify_import_blob_registration(
    session: &mut Session,
    ctx: &Context,
    artifact_id: &str,
    digest: &str,
) -> Result<()> {
    if session
        .get::<Value>(ctx, "tombstone", artifact_id)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    let artifact: Value = session.need(ctx, "artifact", artifact_id).await?;
    if artifact.get("schema_version").and_then(Value::as_str) != Some("rsia.e16.import_source.v1")
        || artifact.get("id").and_then(Value::as_str) != Some(artifact_id)
        || artifact.get("namespace").and_then(Value::as_str) != Some(ctx.namespace())
        || artifact.get("owner_actor").and_then(Value::as_str) != Some(ctx.actor())
        || artifact.pointer("/payload/status").and_then(Value::as_str) != Some("prepared")
        || artifact
            .pointer("/payload/raw_blob_digest")
            .and_then(Value::as_str)
            != Some(digest)
    {
        return Err(Error::Conflict(
            "registered import blob producer changed or is not Prepared".into(),
        ));
    }
    let expected = artifact
        .get("revoke_watermark")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Conflict("registered import source lacks watermark".into()))?;
    let current = session
        .watermark(ctx)
        .await?
        .and_then(|(seq, _)| u64::try_from(seq).ok())
        .ok_or_else(|| Error::Conflict("current revoke watermark is unavailable".into()))?;
    if expected != current {
        return Err(Error::Conflict(
            "registered import source watermark is stale".into(),
        ));
    }
    let refs = artifact
        .get("source_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Conflict("registered import source lacks refs".into()))?;
    if refs.len() != 1
        || refs[0].get("kind").and_then(Value::as_str) != Some("blob")
        || refs[0].get("id").and_then(Value::as_str) != Some(digest)
        || refs[0].get("content_digest").and_then(Value::as_str) != Some(digest)
    {
        return Err(Error::Conflict("registered import blob ref differs".into()));
    }
    let edge:Option<i64>=sqlx::query_scalar("SELECT 1 FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=? AND dst_kind='blob' AND dst_id=?")
        .bind(ctx.namespace()).bind(artifact_id).bind(digest).fetch_optional(&mut *session.tx).await.map_err(internal)?;
    if edge.is_none() {
        return Err(Error::Conflict(
            "registered import blob edge is absent".into(),
        ));
    }
    Ok(())
}
impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        tokio::fs::create_dir_all(&root).await.map_err(internal)?;
        cleanup_private_blob_staging(&root).await?;
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
        let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut staged = PrivateStagedBlob::new(temp);
        write_synced_temp(staged.path(), bytes).await?;
        let published =
            publish_or_verify_blob(staged.path(), &dir.join(&digest), &digest, max_bytes, false)
                .await?;
        staged.disarm();
        if published {
            sync_directory(&dir).await?;
        }
        Ok(digest)
    }
    pub async fn publish_registered_blob(
        &self,
        ctx: &Context,
        artifact_id: &str,
        expected_schema: &str,
        payload_digest_field: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<String> {
        ctx.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
        identifier(artifact_id)?;
        if expected_schema != "rsia.e16.import_source.v1"
            || payload_digest_field != "raw_blob_digest"
        {
            return Err(Error::Invalid(
                "unsupported import blob schema or digest field".into(),
            ));
        }
        validate_blob_bound(bytes, max_bytes)?;
        let digest = hash(bytes);
        let namespace_hash = hash(ctx.namespace().as_bytes());
        let staging_dir = self.root.join(".blob-staging").join(&namespace_hash);
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(internal)?;
        let mut staged =
            PrivateStagedBlob::new(staging_dir.join(format!("{}.tmp", uuid::Uuid::new_v4())));
        write_synced_temp(staged.path(), bytes).await?;
        verify_blob_file(staged.path(), &digest, max_bytes).await?;
        let final_dir = self.root.join("blobs").join(namespace_hash);
        tokio::fs::create_dir_all(&final_dir)
            .await
            .map_err(internal)?;
        let final_path = final_dir.join(&digest);
        let existing_preverified = match tokio::fs::symlink_metadata(&final_path).await {
            Ok(_) => {
                verify_blob_file(&final_path, &digest, max_bytes).await?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(internal(error)),
        };
        let _guard = self.blob_lock.write().await;
        let mut session = self.session().await?;
        let fenced = sqlx::query(
            "UPDATE objects SET revision=revision WHERE namespace=? AND kind='artifact' AND id=?",
        )
        .bind(ctx.namespace())
        .bind(artifact_id)
        .execute(&mut *session.tx)
        .await
        .map_err(internal)?;
        if fenced.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        verify_import_blob_registration(&mut session, ctx, artifact_id, &digest).await?;
        let published = publish_or_verify_blob(
            staged.path(),
            &final_path,
            &digest,
            max_bytes,
            existing_preverified,
        )
        .await?;
        staged.disarm();
        session.commit().await?;
        drop(_guard);
        if published {
            sync_directory(&final_dir).await?;
        }
        Ok(digest)
    }
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
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
        let expected = digest.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::NotFound
                } else {
                    internal(e)
                }
            })?;
            if !metadata.file_type().is_file()
                || metadata.len() == 0
                || metadata.len() > max_bytes as u64
            {
                return Err(Error::Conflict(
                    "blob is linked, non-regular, empty, or oversized".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(Error::Conflict("blob is hard-linked".into()));
                }
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
            if bytes.is_empty() || bytes.len() > max_bytes || hash(&bytes) != expected {
                return Err(Error::Conflict("blob changed or digest mismatched".into()));
            }
            Ok(bytes)
        })
        .await
        .map_err(internal)?
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
        let invalid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE namespace=? AND kind=? AND json_extract(body,'$.schema_version')=? AND (json_type(body,?) IS NULL OR json_type(body,?)!='integer' OR json_extract(body,?)<0)")
            .bind(ctx.namespace()).bind(kind).bind(schema_version).bind(numeric_path).bind(numeric_path).bind(numeric_path).fetch_one(&mut *self.tx).await.map_err(internal)?;
        if invalid != 0 {
            return Err(Error::Conflict(
                "existing capacity records contain an invalid numeric fact".into(),
            ));
        }
        let used:i64=sqlx::query_scalar("SELECT COALESCE(SUM(json_extract(body,?)),0) FROM objects WHERE namespace=? AND kind=? AND json_extract(body,'$.schema_version')=?")
            .bind(numeric_path).bind(ctx.namespace()).bind(kind).bind(schema_version).fetch_one(&mut *self.tx).await.map_err(internal)?;
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
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
    {
        return Err(Error::Invalid("invalid JSON capacity path".into()));
    }
    Ok(())
}
fn json_u64_at_path(value: &Value, path: &str) -> Option<u64> {
    let mut current = value;
    for segment in path.strip_prefix("$.")?.split('.') {
        current = current.get(segment)?;
    }
    current.as_u64()
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
    async fn cancelled_registered_import_write_removes_private_temp() {
        let (_d, s) = db().await;
        let c = Context::new("n", "admin", Role::Admin).unwrap();
        let bytes = vec![b'z'; 2 * 1024 * 1024];
        let digest = hash(&bytes);
        let mut tx = s.session().await.unwrap();
        tx.bump_watermark(&c, &hash(b"wm")).await.unwrap();
        let record = json!({"schema_version":"rsia.e16.import_source.v1","id":"source","namespace":"n","owner_actor":"admin","request_key":"request","input_digest":hash(b"input"),"created_at":1,"updated_at":1,"source_refs":[{"kind":"blob","id":digest,"content_digest":digest}],"revoke_watermark":1,"payload":{"status":"prepared","raw_blob_digest":digest,"blob_published":false}});
        tx.put(&c, "artifact", "source", "admin", &record)
            .await
            .unwrap();
        tx.put_edge(&c, "artifact", "source", "blob", &digest)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let guard = s.blob_lock.write().await;
        let task_store = s.clone();
        let task_ctx = c.clone();
        let task = tokio::spawn(async move {
            task_store
                .publish_registered_blob(
                    &task_ctx,
                    "source",
                    "rsia.e16.import_source.v1",
                    "raw_blob_digest",
                    &bytes,
                    4 * 1024 * 1024,
                )
                .await
        });
        let staging = s
            .root
            .join(".blob-staging")
            .join(hash(c.namespace().as_bytes()));
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
        assert!(observed);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(guard);
        let mut entries = tokio::fs::read_dir(&staging).await.unwrap();
        assert!(entries.next_entry().await.unwrap().is_none());
        assert!(matches!(
            s.read_blob(&c, &digest, 4 * 1024 * 1024).await,
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
