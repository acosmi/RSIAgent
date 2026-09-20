//! Resumable cleanup derived from the authoritative tombstone, watermark and typed edges.

use super::{Store, internal};
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use std::path::{Path, PathBuf};

pub const REVOKE_TOMBSTONE_SCHEMA: &str = "rsia.revoke_tombstone.v1";
pub const MAX_CLEANUP_EDGE_PAGE: usize = 1_000;
pub const BACKUP_SCHEMA: &str = "rsia.backup.v2";

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
        let existing_source = session
            .get::<serde_json::Value>(ctx, &source.kind, &source.id)
            .await?;
        let existing_tombstone = session
            .get::<RevokeTombstone>(ctx, "tombstone", &source.id)
            .await?;
        if existing_source.is_none() && existing_tombstone.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(tombstone) = existing_tombstone {
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
            let value = existing_source.ok_or(Error::NotFound)?;
            if value.get("schema_version").and_then(|item| item.as_str())
                != Some("rsia.e16.import_source.v1")
                || validate_e16_envelope(ctx, &source, &value).is_err()
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
                 WHERE namespace=? AND job_id=? AND expanded=0 ORDER BY seq LIMIT 1",
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
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM revoke_cleanup_frontier
             WHERE namespace=? AND job_id=? AND expanded=0",
        )
        .bind(ctx.namespace())
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        if pending == 0 {
            let completed = sqlx::query(
                "UPDATE revoke_cleanup_jobs SET state='complete',updated_at=?
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

pub(crate) async fn backup_consistent(store: &Store, destination: &Path) -> Result<()> {
    if !cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple"
    )) {
        return Err(Error::Invalid(
            "atomic no-overwrite backup publication unsupported platform".into(),
        ));
    }
    if destination.exists() {
        return Err(Error::Conflict("backup destination already exists".into()));
    }
    let parent = destination.parent().unwrap_or(Path::new("."));
    tokio::fs::create_dir_all(parent).await.map_err(internal)?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::Invalid("backup destination needs a UTF-8 name".into()))?;
    let temp = parent.join(format!(".{name}.tmp-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&temp).await.map_err(internal)?;
    let result = backup_into_temp(store, &temp).await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_dir_all(&temp).await;
        return Err(error);
    }
    if destination.exists() {
        let _ = tokio::fs::remove_dir_all(&temp).await;
        return Err(Error::Conflict(
            "backup destination appeared during backup".into(),
        ));
    }
    let source = temp.clone();
    let target = destination.to_path_buf();
    let published = tokio::task::spawn_blocking(move || publish_backup_directory(&source, &target))
        .await
        .map_err(internal)?;
    if let Err(error) = published {
        let _ = tokio::fs::remove_dir_all(&temp).await;
        return Err(error);
    }
    sync_directory(parent.to_path_buf()).await?;
    Ok(())
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

async fn backup_into_temp(store: &Store, temp: &Path) -> Result<()> {
    let _blob_guard = store.blob_lock.write().await;
    let mut connection = store.pool.acquire().await.map_err(internal)?;
    let database_path = temp.join("rsia.sqlite3");
    sqlx::query("VACUUM INTO ?")
        .bind(database_path.to_string_lossy().as_ref())
        .execute(&mut *connection)
        .await
        .map_err(internal)?;
    sync_file(&database_path).await?;
    let database = file_entry(&database_path, "rsia.sqlite3").await?;
    let migrations = load_backup_migrations(&mut connection).await?;
    let watermarks = load_backup_watermarks(&mut connection).await?;
    let mut blobs = copy_backup_blobs(store, temp).await?;
    blobs.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = BackupManifest {
        schema_version: BACKUP_SCHEMA.into(),
        complete: true,
        database,
        blobs,
        migrations,
        watermarks,
        created_at: evo_core::now(),
    };
    let manifest_path = temp.join("backup-manifest.json");
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(internal)?;
    tokio::fs::write(&manifest_path, bytes)
        .await
        .map_err(internal)?;
    sync_file(&manifest_path).await?;
    sync_directory(temp.to_path_buf()).await
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
    if !tokio::fs::symlink_metadata(&source)
        .await
        .map_err(internal)?
        .is_dir()
    {
        return Err(Error::Conflict(
            "linked blob root cannot be backed up".into(),
        ));
    }
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
            tokio::fs::create_dir_all(&target_namespace)
                .await
                .map_err(internal)?;
            let destination = target_namespace.join(&file_name);
            tokio::fs::write(&destination, &bytes)
                .await
                .map_err(internal)?;
            sync_file(&destination).await?;
            entries.push(BackupFileEntry {
                path: relative,
                sha256: evo_core::hash(&bytes),
                bytes: u64::try_from(bytes.len()).map_err(|_| Error::Internal)?,
            });
        }
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
            &node_id,
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
        let value: serde_json::Value = serde_json::from_str(&body).map_err(internal)?;
        if value["schema_version"] == "rsia.redacted.v1" {
            continue;
        }
        if value["schema_version"] != "rsia.replay_world.v2" {
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
        let sources = value["manifest"]["source_closure"]
            .as_array()
            .ok_or_else(|| Error::Invalid("world source closure missing".into()))?;
        if sources
            .iter()
            .any(|v| v["source_id"].as_str() == Some(source_id))
        {
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
    source_id: &str,
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
        if schema == "rsia.redacted.v1" {
            continue;
        }
        if schema != "rsia.model_request.artifact.v1" {
            mark_unknown_scope(
                tx,
                ctx,
                job_id,
                &TypedObjectRef {
                    kind: "reservation".into(),
                    id: call_id,
                },
                &format!("unknown_budget_request_schema:{schema}"),
                now,
            )
            .await?;
            continue;
        }
        let body: String = row.try_get("request_artifact_body").map_err(internal)?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(internal)?;
        let source_ids = value
            .get("source_closure")
            .and_then(|value| value.as_array())
            .ok_or_else(|| Error::Invalid("model request lacks source closure".into()))?
            .iter()
            .map(|value| {
                value
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| Error::Invalid("model request source id missing".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        if source_ids.iter().any(|value| value == source_id) {
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

fn e16_schema_kind(schema: &str) -> Option<&'static str> {
    match schema {
        "rsia.e16.source_selection.v1" => Some("source_selection"),
        "rsia.e16.import_source.v1" => Some("import_source"),
        "rsia.e16.import_result.v1" => Some("import_result"),
        "rsia.e16.staged_asset.v1" => Some("staged_asset"),
        "rsia.e16.seed_install.v1" => Some("seed_install"),
        "rsia.e16.export_attempt.v1" => Some("export_attempt"),
        "rsia.e16.delivery_audit.v1" => Some("delivery_audit"),
        "rsia.e16.export_attempt.v2" => Some("export_attempt"),
        "rsia.e16.delivery_audit.v2" => Some("delivery_audit"),
        _ => None,
    }
}

fn validate_e16_envelope(
    ctx: &Context,
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
    if object["id"].as_str() != Some(node.id.as_str())
        || object["namespace"].as_str() != Some(ctx.namespace())
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
    Ok(schema)
}

fn is_lower_hex_digest(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[allow(clippy::too_many_arguments)]
async fn cleanup_e16_envelope(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    now: i64,
) -> Result<()> {
    let kind = match validate_e16_envelope(ctx, node, value) {
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
    {
        remove_controlled_local_export(store, ctx, node, job_id, tx, now).await?;
    }

    let mut blob_digests = Vec::new();
    let payload = &value["payload"];
    let fields: &[&str] = match kind {
        "import_source" => &["raw_blob_digest"],
        "staged_asset" => &["content_blob_digest"],
        "seed_install" => &[
            "baseline_blob_digest",
            "local_blob_digest",
            "upstream_blob_digest",
        ],
        "export_attempt" => {
            if value["schema_version"].as_str() == Some("rsia.e16.export_attempt.v2") {
                &["projection_blob_digest"]
            } else {
                &["output_blob_digest"]
            }
        }
        _ => &[],
    };
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
    for field in [
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
    ] {
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

async fn remove_controlled_local_export(
    store: &Store,
    ctx: &Context,
    node: &TypedObjectRef,
    job_id: &str,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: i64,
) -> Result<()> {
    let export_root = store.root.join("exports");
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
    let path = namespace_root.join(&node.id);
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
           'rsia.e16.seed_install.v1','rsia.e16.export_attempt.v1',
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
           'rsia.e16.staged_asset.v1','rsia.e16.seed_install.v1','rsia.e16.export_attempt.v1',
           'rsia.e16.export_attempt.v2'
         )
         AND NOT EXISTS (
           SELECT 1 FROM revoke_cleanup_frontier f
           WHERE f.namespace=o.namespace AND f.job_id=?
             AND f.node_kind=o.kind AND f.node_id=o.id
         )",
    )
    .bind(ctx.namespace())
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
        validate_e16_envelope(ctx, &node, &value)?;
        for field in [
            "content_blob_digest",
            "baseline_blob_digest",
            "local_blob_digest",
            "upstream_blob_digest",
            "output_blob_digest",
        ] {
            if value["payload"].get(field).and_then(|item| item.as_str()) == Some(digest) {
                return Ok(true);
            }
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
    if report.schema_version != evo_core::replay::STORED_REPLAY_REPORT_SCHEMA
        || report.replay_engine_version != evo_core::replay::REPLAY_ENGINE_VERSION
        || report.purpose != evo_core::evidence::Purpose::Development
        || report.policy.validate().is_err()
        || report.profile.validate(&report.caps).is_err()
    {
        return false;
    }
    let body_digest = fingerprint(&report.member_reports).ok();
    let semantic_digest =
        evo_core::replay::replay_report_semantic_digest(&report.member_reports).ok();
    let report_id = evo_core::replay::replay_report_id(
        &report.namespace,
        report.partition,
        &report.pool_digest,
        &report.policy_digest,
        &report.profile_digest,
        &report.caps_digest,
    )
    .ok();
    report.policy_digest == fingerprint(&report.policy).unwrap_or_default()
        && report.profile_digest == fingerprint(&report.profile).unwrap_or_default()
        && report.caps_digest == fingerprint(&report.caps).unwrap_or_default()
        && body_digest.as_deref() == Some(report.reports_body_digest.as_str())
        && semantic_digest.as_deref() == Some(report.semantic_reports_digest.as_str())
        && report_id.as_deref() == Some(report.report_id.as_str())
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
        let value: serde_json::Value = serde_json::from_str(&body).map_err(internal)?;
        if value["schema_version"] == "rsia.redacted.v1" {
            return Ok(());
        }
        if value["schema_version"] != "rsia.replay_world.v2" {
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
        let redacted = json!({"schema_version":"rsia.redacted.v1","state":"source_revoked","original_schema":"rsia.replay_world.v2",
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
    let body: Option<String> =
        sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind=? AND id=?")
            .bind(ctx.namespace())
            .bind(&node.kind)
            .bind(&node.id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
    let Some(body) = body else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(&body).map_err(internal)?;
    let schema = value
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let record_kind = value
        .get("record_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if node.kind == "artifact" && e16_schema_kind(schema).is_some() {
        return cleanup_e16_envelope(tx, store, ctx, job_id, node, &body, &value, now).await;
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
    let preserve = match (node.kind.as_str(), schema, record_kind) {
        // These historical accounting facts retain their consumer's schema and spent counters.
        ("artifact", "rsia.budget_call_ref.v1", _) => true,
        (
            "artifact",
            "rsia.typed_artifact_envelope.v1",
            "exposure_ledger_v1"
            | "alpha_claim_reservation_v1"
            | "evaluation_ticket_v2"
            | "evaluation_ticket_issue_receipt_v1"
            | "execution_receipt_v2"
            | "independent_grader_receipt_v2"
            | "formal_evaluation_v2",
        ) => true,
        ("release", "rsia.persistent_release.v1", _)
        | ("pointer", "rsia.profile_pointer.v1", _)
        | ("tombstone", "rsia.revoke_tombstone.v1", _) => true,
        ("evaluation", "", _) => value.as_object().is_some_and(|m| {
            m.len() == 3
                && ["id", "score", "state"]
                    .iter()
                    .all(|key| m.contains_key(*key))
        }),
        _ => false,
    };
    let redact = match (node.kind.as_str(), schema, record_kind) {
        (_, "rsia.redacted.v1", _) => return Ok(()),
        ("artifact", "rsia.optimization.stage_fact.v1", _) => matches!(
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
        ),
        (
            "artifact",
            "rsia.typed_artifact_envelope.v1",
            "registered_evaluation_control_v41"
            | "protected_holdout_v41"
            | "evaluation_execution_output_v1",
        ) => true,
        (
            "artifact",
            "rsia.exploration_artifact_envelope.v1",
            "exploration_world_v1"
            | "exploration_node_v1"
            | "exploration_dispatch_v1"
            | "optimization_history_v1",
        ) => true,
        ("candidate" | "artifact", "rsia.release_candidate.v1", _) => true,
        ("artifact", "rsia.run_application.v1", _)
        | ("receipt", "rsia.host_application.v1", _)
        | ("artifact", "rsia.host_execution_receipt.v1", _) => true,
        ("artifact", "rsia.resolved_bundle.v2", _) => true,
        // E03's persisted SourceSelection predates a schema field; exact fields define its shape.
        ("artifact", "", _) => value.as_object().is_some_and(|m| {
            m.len() == 4
                && ["roots", "run_ids", "purpose", "allow_model_excerpts"]
                    .iter()
                    .all(|key| m.contains_key(*key))
        }),
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
    let mut metadata = minimal_metadata(&value);
    if schema == "rsia.optimization.stage_fact.v1"
        || schema == "rsia.exploration_artifact_envelope.v1"
    {
        metadata["historical_facts"] = historical_stage_facts(&value["payload"]);
    }
    let redacted = json!({
        "id": node.id,
        "schema_version": "rsia.redacted.v1",
        "state": "source_revoked",
        "original_kind": node.kind,
        "original_schema": value.get("schema_version").and_then(|v| v.as_str()),
        "original_digest": evo_core::hash(body.as_bytes()),
        "metadata": metadata,
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
        "content_redacted",
        now,
        json!({"original_digest":evo_core::hash(body.as_bytes())}),
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
         WHERE namespace=? AND job_id=? AND expanded=0",
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
