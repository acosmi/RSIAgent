//! Transactional typed documents, idempotency, audit and disposable FTS5 indexes.
//! This crate never connects to a model or executes candidate content.
pub mod budget;
pub mod dependency_read;
pub mod lifecycle;
pub mod replay;
pub mod typed_budget;

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
/// Storage-side mirrors of the E16.5 MVP capacity bounds enforced inside this
/// crate (`Session::put_world`, `Store::reserve_budget_call_with_sources`).
/// The engine's `capacity` module pins the same values and asserts equality.
pub const MVP_MAX_REPLAY_WORLDS: u64 = 100;
pub const MVP_MAX_ACTIVE_LEASES: u64 = 10;
/// Schema identifiers the capacity measurement recognises. They mirror the
/// engine's persisted schema versions; the engine asserts they stay equal.
pub const CAPACITY_MANAGEMENT_JOB_SCHEMA: &str = "rsia.management_job.v1";
pub const CAPACITY_IMPORT_RESULT_SCHEMA: &str = "rsia.e16.import_result.v1";
pub const CAPACITY_EXPLORATION_ENVELOPE_SCHEMA: &str = "rsia.exploration_artifact_envelope.v1";
pub const CAPACITY_EXPLORATION_NODE_RECORD_KIND: &str = "exploration_node_v1";
pub const CAPACITY_STAGED_ASSET_SCHEMA: &str = "rsia.e16.staged_asset.v1";
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

/// A physical object row from the caller's current transaction. This is a
/// read-only snapshot, not a new persistent record or an authority token.
#[derive(Debug, Clone)]
pub struct ObjectRowSnapshot {
    pub rowid: i64,
    pub id: String,
    pub owner: String,
    /// Original JSON bytes, so consumers can reject duplicate keys before decoding.
    pub body: String,
}

pub struct Session {
    tx: Transaction<'static, Sqlite>,
}

/// Schema of the tombstone the revocation cleanup leaves in place of an artifact
/// whose source was revoked (plan §11.5, E08).
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// A `run` or `artifact` node of an upstream closure ([`Session::upstream_closure`])
/// with what a revocation gate needs to judge it.
#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamNode {
    /// `run` or `artifact`.
    pub kind: String,
    pub id: String,
    /// The body stored under the `tombstone` kind and this id, as stored (the
    /// caller decodes it), or `None` when the id has none. A tombstone is keyed by
    /// the id of its source alone, so a run and an artifact of one id share it: it
    /// was written for one of them, and the `source_kind` in its body says which.
    pub tombstone: Option<Value>,
    /// Whether the node is an artifact whose stored body carries the
    /// `rsia.redacted.v1` schema, that is, one the revocation cleanup already
    /// replaced. Always `false` for a run: the cleanup deletes a run instead of
    /// redacting it, and a run body can be large, so it is never read here.
    pub redacted: bool,
}

fn internal(e: impl std::fmt::Display) -> Error {
    tracing::error!(error=%e,"database operation failed");
    Error::Internal
}

/// Authority-side measurement of one namespace against the E16.5 MVP capacity
/// bounds. Every field is counted inside the calling session, so a caller that
/// measures and then inserts within the same session cannot race another
/// writer across a limit (the Store serializes writers on one connection).
///
/// `inflight_prepare` is process-local and never persisted; the engine fills it
/// in after conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct CapacityUsageV41 {
    /// `run` objects plus service runs (artifacts whose `$.source` is
    /// `trusted_host_snapshot`), the same formula `Session::put` enforces.
    pub runs: u64,
    /// Sum of `$.payload.aggregate_summary.total_events` over live import
    /// results plus any `event` objects.
    pub events: u64,
    /// Stored Skill candidates (`kind='candidate'`). This is the one Skill asset
    /// table: release candidates are frozen artifacts derived from these rows
    /// and the disposable `skill_fts` index is keyed by candidate id.
    pub skills: u64,
    /// Exploration node envelopes (`rsia.exploration_artifact_envelope.v1`
    /// with `record_kind = exploration_node_v1`).
    pub exploration_nodes: u64,
    /// Rows in `replay_worlds` (drafts and sealed worlds alike).
    pub replay_worlds: u64,
    /// Live leases: root budget calls that are reserved, dispatched, uncertain
    /// or finalized-but-not-closed with `lease_until > now`, plus running
    /// management jobs whose lease is still live.
    pub active_leases: u64,
    /// Staged assets in `prepared`, `staged` or `quarantined` state.
    pub staged_packages: u64,
    /// Running management jobs whose `lease_until > now`.
    pub concurrent_dispatches: u64,
}

/// Build the typed refusal every MVP capacity gate reports. The message names
/// the exhausted counter and both numbers; it never truncates anything.
pub fn mvp_capacity_exceeded(label: &str, used: u64, limit: u64) -> Error {
    Error::Conflict(format!(
        "MVP capacity exceeded: {label} {used} >= limit {limit}"
    ))
}

fn count_to_u64(count: i64) -> Result<u64> {
    u64::try_from(count).map_err(|_| Error::Internal)
}

/// Live leases across the whole instance at `now`: root budget calls holding a
/// lease plus running management jobs holding a lease, in every namespace.
/// Shared by the session measurement and the budget reservation gate so both
/// count the same facts. Counting is instance-wide (F24): the MVP load
/// contract is per deployment and a shared root cannot be split by claiming
/// another namespace lane.
pub(crate) async fn count_active_leases(
    conn: &mut sqlx::SqliteConnection,
    now: i64,
) -> Result<u64> {
    let budget: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_budget_calls
         WHERE lease_until>? AND (
           state IN ('reserved','dispatched','uncertain')
           OR (state='finalized' AND execution_closed=0)
         )",
    )
    .bind(now)
    .fetch_one(&mut *conn)
    .await
    .map_err(internal)?;
    let jobs = count_running_management_jobs(conn, now).await?;
    count_to_u64(budget)?
        .checked_add(jobs)
        .ok_or(Error::Internal)
}

async fn count_running_management_jobs(conn: &mut sqlx::SqliteConnection, now: i64) -> Result<u64> {
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects
         WHERE kind='job'
           AND json_extract(body,'$.schema_version')=?
           AND json_extract(body,'$.state')='running'
           AND json_extract(body,'$.lease_until')>?",
    )
    .bind(CAPACITY_MANAGEMENT_JOB_SCHEMA)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
    .map_err(internal)?;
    count_to_u64(running)
}

/// Objects of `kind` across every namespace (a capacity count, never content).
async fn count_objects_all_namespaces(
    conn: &mut sqlx::SqliteConnection,
    kind: &str,
) -> Result<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE kind=?")
        .bind(kind)
        .fetch_one(&mut *conn)
        .await
        .map_err(internal)?;
    count_to_u64(count)
}

/// Runs across every namespace: `run` objects plus service runs (artifacts
/// whose `$.source` is `trusted_host_snapshot`).
async fn count_runs_all_namespaces(conn: &mut sqlx::SqliteConnection) -> Result<u64> {
    let runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects WHERE
           kind='run' OR
           (kind='artifact' AND json_extract(body,'$.source')='trusted_host_snapshot')",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(internal)?;
    count_to_u64(runs)
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
    // Validate the raw string rather than `Path::components()`: component
    // iteration normalises away `.` segments and repeated or trailing
    // separators, which would admit non-canonical member paths such as
    // `a/./b`, `a//b` or `a/` that alias other members on disk. Every
    // `/`-separated segment must be a plain, non-empty name; an empty first
    // segment is an absolute path.
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(Error::Invalid("unsafe local export path".into()));
    }
    Ok(())
}

/// Export ids name a directory directly under `<root>/exports/<namespace>/`.
/// `evo_core::identifier` admits dots, so beyond being an identifier an export
/// id must never be a dot-only name (`.` or `..` resolve to the namespace root
/// or its parent) and must not carry a path separator or NUL. Every public
/// local export entry point and the cleanup path validate ids with this.
pub(crate) fn validate_export_id(export_id: &str) -> Result<()> {
    if export_id.is_empty()
        || export_id.bytes().all(|byte| byte == b'.')
        || export_id
            .chars()
            .any(|character| matches!(character, '/' | '\\' | '\0'))
    {
        return Err(Error::Invalid("invalid export id".into()));
    }
    identifier(export_id)
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
        "rsia.e16.seed_install.v1" | "rsia.e16.seed_install.v2" => matches!(
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
    // Bound recursive results before sorting; keep the closure as the outer
    // loop so each node looks up its edges by (namespace, src_kind, src_id).
    let rows = sqlx::query(
        "WITH RECURSIVE closure(kind,id) AS (
           SELECT dst_kind,dst_id FROM dependencies
             WHERE namespace=? AND src_kind='artifact' AND src_id=?
           UNION
           SELECT d.dst_kind,d.dst_id FROM closure c
             CROSS JOIN dependencies d
               ON d.namespace=? AND d.src_kind=c.kind AND d.src_id=c.id
           LIMIT 10001
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

/// Private raw walk. `None` is a measured node-limit excess, never a SQL error.
/// All node kinds count toward the bound; only run/artifact facts are returned.
/// Run-body redaction is read only when the typed caller requests it.
/// Session::upstream_closure keeps its historical decode, ordering and errors.
pub(crate) struct RawUpstreamNode {
    pub kind: String,
    pub id: String,
    pub tombstone: Option<String>,
    pub redacted: bool,
    pub exists: bool,
}

pub(crate) async fn upstream_rows_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    roots: &[(&str, &str)],
    limit: usize,
    inspect_run_redaction: bool,
) -> Result<Option<Vec<RawUpstreamNode>>> {
    if roots.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let seeds = serde_json::to_string(roots).map_err(internal)?;
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    // LIMIT inside the recursive CTE bounds the walk itself. CROSS JOIN keeps
    // its queue outside the dependency lookup (the existing measured algorithm).
    let rows = sqlx::query(
        "WITH RECURSIVE closure(kind, id) AS (
           SELECT json_extract(r.value, '$[0]'), json_extract(r.value, '$[1]')
             FROM json_each(?1) AS r
           UNION
           SELECT d.dst_kind, d.dst_id
             FROM closure AS c
             CROSS JOIN dependencies AS d
               ON d.namespace = ?2 AND d.src_kind = c.kind AND d.src_id = c.id
           LIMIT ?3
         )
         SELECT c.kind AS kind, c.id AS id, t.body AS tombstone,
                CASE WHEN c.kind = 'artifact' OR (?5 AND c.kind = 'run')
                     THEN COALESCE(json_extract(o.body, '$.schema_version') = ?4, 0)
                     ELSE 0 END AS redacted,
                o.id IS NOT NULL AS present
           FROM closure AS c
           LEFT JOIN objects AS t
             ON c.kind IN ('run', 'artifact')
            AND t.namespace = ?2 AND t.kind = 'tombstone' AND t.id = c.id
           LEFT JOIN objects AS o
             ON c.kind IN ('run', 'artifact')
            AND o.namespace = ?2 AND o.kind = c.kind AND o.id = c.id",
    )
    .bind(seeds)
    .bind(namespace)
    .bind(fetch)
    .bind(REDACTED_SCHEMA)
    .bind(inspect_run_redaction)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    if rows.len() > limit {
        return Ok(None);
    }
    let mut nodes = Vec::new();
    for row in rows {
        let kind: String = row.try_get("kind").map_err(internal)?;
        if !matches!(kind.as_str(), "run" | "artifact") {
            continue;
        }
        nodes.push(RawUpstreamNode {
            kind,
            id: row.try_get("id").map_err(internal)?,
            tombstone: row.try_get("tombstone").map_err(internal)?,
            redacted: row.try_get::<i64, _>("redacted").map_err(internal)? != 0,
            exists: row.try_get::<i64, _>("present").map_err(internal)? != 0,
        });
    }
    Ok(Some(nodes))
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
        // Production entry points hold a shared data-directory OS lock until close.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(opts);
        // Retain the pool even when connecting/migrating fails. Dropping its
        // last handle does not await background connection shutdown, so it
        // must be explicitly closed before the caller releases its OS guard.
        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            pool.close().await;
            return Err(internal(error));
        }
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
            "rsia.e16.seed_install.v1" | "rsia.e16.seed_install.v2" | "rsia.e16.import_source.v1"
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
        validate_export_id(export_id)?;
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
        validate_export_id(export_id)?;
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
        validate_export_id(&plan.export_id)?;
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
        validate_export_id(export_id)?;
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
        validate_export_id(export_id)?;
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
        validate_export_id(export_id)?;
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
    /// Measure the whole instance against the E16.5 MVP capacity bounds inside
    /// this transaction. Every counter spans all namespaces (F24): the MVP load
    /// contract is per deployment, so a cap cannot be split by deriving from
    /// another namespace. Counts are not content; object reads stay
    /// namespace-authorized. `now_secs` is the lease clock (unix seconds) used
    /// for `active_leases` and `concurrent_dispatches`.
    pub async fn capacity_usage_v41(&mut self, now_secs: u64) -> Result<CapacityUsageV41> {
        let now = i64::try_from(now_secs)
            .map_err(|_| Error::Invalid("capacity clock is out of range".into()))?;
        let runs = count_runs_all_namespaces(&mut self.tx).await?;
        let imported_events: i64 = sqlx::query_scalar(
            "SELECT CAST(COALESCE(SUM(json_extract(body,'$.payload.aggregate_summary.total_events')),0) AS INTEGER)
             FROM objects WHERE kind='artifact'
               AND json_extract(body,'$.schema_version')=?",
        )
        .bind(CAPACITY_IMPORT_RESULT_SCHEMA)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        let event_objects = count_objects_all_namespaces(&mut self.tx, "event").await?;
        let skills = count_objects_all_namespaces(&mut self.tx, "candidate").await?;
        let exploration_nodes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objects WHERE kind='artifact'
               AND json_extract(body,'$.schema_version')=?
               AND json_extract(body,'$.record_kind')=?",
        )
        .bind(CAPACITY_EXPLORATION_ENVELOPE_SCHEMA)
        .bind(CAPACITY_EXPLORATION_NODE_RECORD_KIND)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        let replay_worlds = self.replay_world_count().await?;
        let active_leases = count_active_leases(&mut self.tx, now).await?;
        let staged_packages: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objects WHERE kind='artifact'
               AND json_extract(body,'$.schema_version')=?
               AND json_extract(body,'$.payload.state') IN ('prepared','staged','quarantined')",
        )
        .bind(CAPACITY_STAGED_ASSET_SCHEMA)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        let concurrent_dispatches = count_running_management_jobs(&mut self.tx, now).await?;
        Ok(CapacityUsageV41 {
            runs,
            events: count_to_u64(imported_events)?
                .checked_add(event_objects)
                .ok_or(Error::Internal)?,
            skills,
            exploration_nodes: count_to_u64(exploration_nodes)?,
            replay_worlds,
            active_leases,
            staged_packages: count_to_u64(staged_packages)?,
            concurrent_dispatches,
        })
    }

    /// Count replay worlds (drafts and sealed) across every namespace.
    pub async fn replay_world_count(&mut self) -> Result<u64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM replay_worlds")
            .fetch_one(&mut *self.tx)
            .await
            .map_err(internal)?;
        count_to_u64(count)
    }

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
        // The exact marker and original body come from this one namespace/kind/id
        // read. json_each sees every top-level member, including duplicate versions.
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT body, CASE WHEN objects.kind='artifact' AND EXISTS (
                 SELECT 1 FROM json_each(objects.body) AS member
                 WHERE member.key='schema_version' AND member.type='text'
                   AND member.value='rsia.host_surface_record.v2'
             ) THEN 1 ELSE 0 END
             FROM objects WHERE namespace=? AND kind=? AND id=?",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        row.map(|(body, current_host_surface)| decode_object_row_body(&body, current_host_surface))
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
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT body, CASE WHEN objects.kind='artifact' AND json_valid(objects.body)
             THEN EXISTS (
                 SELECT 1 FROM json_each(objects.body) AS member
                 WHERE member.key='schema_version' AND member.type='text'
                   AND member.value='rsia.host_surface_record.v2'
             ) ELSE 0 END
             FROM objects WHERE namespace=? AND kind=? ORDER BY rowid LIMIT 10001",
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
            .map(|(body, current_host_surface)| decode_object_row_body(&body, current_host_surface))
            .collect()
    }

    /// Read a bounded page without truncating the collection. Keep this Session
    /// open and use the last rowid as the next cursor until an empty page.
    /// Namespace and kind are always SQL predicates, including the first page.
    pub async fn object_rows_page(
        &mut self,
        ctx: &Context,
        kind: &str,
        after_rowid: Option<i64>,
    ) -> Result<Vec<ObjectRowSnapshot>> {
        let rows = sqlx::query(
            "SELECT rowid,id,owner,body FROM objects
             WHERE namespace=? AND kind=? AND (? IS NULL OR rowid>?)
             ORDER BY rowid LIMIT 64",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .bind(after_rowid)
        .bind(after_rowid)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(internal)?;
        rows.into_iter()
            .map(|row| {
                Ok(ObjectRowSnapshot {
                    rowid: row.try_get("rowid").map_err(internal)?,
                    id: row.try_get("id").map_err(internal)?,
                    owner: row.try_get("owner").map_err(internal)?,
                    body: row.try_get("body").map_err(internal)?,
                })
            })
            .collect()
    }

    /// The physical identity and untouched body at one exact object key.
    pub async fn object_row(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<Option<ObjectRowSnapshot>> {
        identifier(id)?;
        let row = sqlx::query(
            "SELECT rowid,id,owner,body FROM objects WHERE namespace=? AND kind=? AND id=?",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        row.map(|row| {
            Ok(ObjectRowSnapshot {
                rowid: row.try_get("rowid").map_err(internal)?,
                id: row.try_get("id").map_err(internal)?,
                owner: row.try_get("owner").map_err(internal)?,
                body: row.try_get("body").map_err(internal)?,
            })
        })
        .transpose()
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
                // E16.5 / F24: the run bound is instance-wide, like every other
                // MVP counter; a new run id anywhere counts against it.
                let used = count_runs_all_namespaces(&mut self.tx).await?;
                if used >= MVP_MAX_RUNS {
                    return Err(mvp_capacity_exceeded("runs", used, MVP_MAX_RUNS));
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
    /// Checks a content witness against the entire private namespace audit
    /// chain in the caller's transaction. The operation must identify a logical
    /// subject (including schema/version and namespace/kind/id); the entity is
    /// the caller's digest of that subject's complete current content.
    ///
    /// Only the latest event for this exact role and operation can match. A
    /// different role cannot mint or supersede its witness. Public objects and
    /// self-reported issuer fields are never consulted. This returns a scoped
    /// fact, without changing the caller's role or committing its Session.
    pub async fn has_role_audit_witness(
        &mut self,
        ctx: &Context,
        role: evo_core::Role,
        operation: &str,
        entity: &str,
    ) -> Result<bool> {
        if operation.is_empty() {
            return Err(Error::Invalid(
                "audit witness operation must identify a logical subject".into(),
            ));
        }
        if entity.len() != 64
            || !entity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Invalid(
                "audit witness entity must be a content digest".into(),
            ));
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct AuditPayload<'a> {
            #[serde(borrow)]
            actor: std::borrow::Cow<'a, str>,
            role: evo_core::Role,
            #[serde(borrow)]
            operation: std::borrow::Cow<'a, str>,
            #[serde(borrow)]
            entity: std::borrow::Cow<'a, str>,
            timestamp: i64,
        }
        let invalid_chain = || Error::Conflict("audit chain or payload mismatch".into());
        // A finite snapshot, not a new audit-history capacity. Legitimate
        // histories may exceed the 10000 business-event collection bound.
        let snapshot = sqlx::query(
            "SELECT COUNT(*) AS row_count,COALESCE(MAX(seq),0) AS watermark FROM audit WHERE namespace=?",
        )
        .bind(ctx.namespace())
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        let count: i64 = snapshot.try_get("row_count").map_err(internal)?;
        let watermark: i64 = snapshot.try_get("watermark").map_err(internal)?;
        if count < 0 || watermark < 0 {
            return Err(invalid_chain());
        }
        let mut scanned = 0_i64;
        let mut after = 0_i64;
        let mut previous = String::new();
        let mut latest_matches = false;
        loop {
            // Only bounded metadata is fetched in a page. Payload allocation
            // is checked against the actual row's byte length; chunks prevent
            // the driver from first allocating an unchecked whole payload.
            let rows = sqlx::query(
                "SELECT seq,typeof(payload) AS payload_type,length(CAST(payload AS BLOB)) AS payload_bytes,substr(previous_hash,1,65) AS previous_hash,length(CAST(previous_hash AS BLOB)) AS previous_bytes,substr(digest,1,65) AS digest,length(CAST(digest AS BLOB)) AS digest_bytes FROM audit WHERE namespace=? AND seq>? AND seq<=? ORDER BY seq LIMIT 64",
            )
            .bind(ctx.namespace())
            .bind(after)
            .bind(watermark)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(internal)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let sequence: i64 = row.try_get("seq").map_err(internal)?;
                let payload_type: String = row.try_get("payload_type").map_err(internal)?;
                let payload_bytes: i64 = row.try_get("payload_bytes").map_err(internal)?;
                let prev: String = row.try_get("previous_hash").map_err(internal)?;
                let prev_bytes: i64 = row.try_get("previous_bytes").map_err(internal)?;
                let digest: String = row.try_get("digest").map_err(internal)?;
                let digest_bytes: i64 = row.try_get("digest_bytes").map_err(internal)?;
                if sequence <= after
                    || sequence > watermark
                    || scanned >= count
                    || payload_type != "text"
                    || payload_bytes <= 0
                    || prev_bytes != i64::try_from(prev.len()).map_err(internal)?
                    || prev != previous
                    || digest_bytes != 64
                    || digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(invalid_chain());
                }
                let payload_len = usize::try_from(payload_bytes).map_err(|_| invalid_chain())?;
                let prefix_len = previous.len().checked_add(1).ok_or_else(invalid_chain)?;
                let capacity = prefix_len
                    .checked_add(payload_len)
                    .ok_or_else(invalid_chain)?;
                let mut authenticated = Vec::new();
                // This bound is the actual finite payload plus its hash prefix,
                // not an invented maximum number of historic audit events.
                authenticated.try_reserve_exact(capacity).map_err(|_| {
                    Error::Conflict("audit payload cannot fit available addressable memory".into())
                })?;
                authenticated.extend_from_slice(previous.as_bytes());
                authenticated.push(b'\n');
                let mut offset = 0_usize;
                while offset < payload_len {
                    let length = (payload_len - offset).min(8192);
                    let chunk: Vec<u8> = sqlx::query_scalar(
                        "SELECT substr(CAST(payload AS BLOB),?,?) FROM audit WHERE namespace=? AND seq=?",
                    )
                    .bind(i64::try_from(offset).map_err(internal)?.checked_add(1).ok_or_else(invalid_chain)?)
                    .bind(i64::try_from(length).map_err(internal)?)
                    .bind(ctx.namespace())
                    .bind(sequence)
                    .fetch_optional(&mut *self.tx)
                    .await
                    .map_err(internal)?
                    .ok_or_else(invalid_chain)?;
                    if chunk.len() != length {
                        return Err(invalid_chain());
                    }
                    authenticated.extend_from_slice(&chunk);
                    offset = offset.checked_add(length).ok_or_else(invalid_chain)?;
                }
                if authenticated.len() != capacity || hash(&authenticated) != digest {
                    return Err(invalid_chain());
                }
                let raw_payload = &authenticated[prefix_len..];
                if raw_payload
                    .iter()
                    .copied()
                    .find(|byte| !matches!(*byte, b' ' | b'\t' | b'\n' | b'\r'))
                    != Some(b'{')
                {
                    return Err(invalid_chain());
                }
                let payload: AuditPayload<'_> =
                    serde_json::from_slice(raw_payload).map_err(|_| invalid_chain())?;
                if payload.timestamp < 0 || identifier(payload.actor.as_ref()).is_err() {
                    return Err(invalid_chain());
                }
                if payload.role == role && payload.operation.as_ref() == operation {
                    latest_matches = payload.entity.as_ref() == entity;
                }
                previous = digest;
                after = sequence;
                scanned = scanned.checked_add(1).ok_or_else(invalid_chain)?;
            }
        }
        if scanned != count || after != watermark {
            return Err(invalid_chain());
        }
        Ok(latest_matches)
    }

    pub async fn audit(&mut self, ctx: &Context, operation: &str, entity: &str) -> Result<()> {
        let timestamp = now()?;
        let prev: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM audit WHERE namespace=? ORDER BY seq DESC LIMIT 1",
        )
        .bind(ctx.namespace())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        let prev = prev.unwrap_or_default();
        let payload=json!({"actor":ctx.actor(),"role":ctx.role(),"operation":operation,"entity":entity,"timestamp":timestamp}).to_string();
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

    /// Complete outgoing content dependencies of one physical typed object, in
    /// this namespace and transaction. LIMIT+1 detects overflow before returning
    /// anything: a revocation reader must never mistake a prefix for the closure.
    pub async fn outgoing_edges_bounded(
        &mut self,
        ctx: &Context,
        src_kind: &str,
        src_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        identifier(src_kind)?;
        identifier(src_id)?;
        if !(1..=10_000).contains(&limit) {
            return Err(Error::Invalid(
                "outgoing edge limit must be 1..=10000".into(),
            ));
        }
        let rows = sqlx::query(
            "SELECT dst_kind,dst_id FROM dependencies
             WHERE namespace=? AND src_kind=? AND src_id=?
             ORDER BY dst_kind COLLATE BINARY,dst_id COLLATE BINARY LIMIT ?",
        )
        .bind(ctx.namespace())
        .bind(src_kind)
        .bind(src_id)
        .bind(i64::try_from(limit + 1).map_err(|_| Error::Internal)?)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(internal)?;
        if rows.len() > limit {
            return Err(Error::Conflict(format!(
                "outgoing dependency edges exceed {limit}; refused, not truncated"
            )));
        }
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("dst_kind").map_err(internal)?,
                    row.try_get("dst_id").map_err(internal)?,
                ))
            })
            .collect()
    }

    /// Actual outbound typed edges in the authenticated namespace. A caller
    /// receives the complete bounded set or an error, never a truncated closure.
    /// These rows do not attest the existence, schema or authority of either node.
    pub async fn dependency_edges(
        &mut self,
        ctx: &Context,
        src_kind: &str,
        src_id: &str,
    ) -> Result<Vec<(String, String)>> {
        identifier(src_kind)?;
        identifier(src_id)?;
        let rows = sqlx::query(
            "SELECT dst_kind,dst_id FROM dependencies WHERE namespace=? AND src_kind=? AND src_id=? ORDER BY dst_kind,dst_id LIMIT 10001",
        )
        .bind(ctx.namespace())
        .bind(src_kind)
        .bind(src_id)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(internal)?;
        if rows.len() > 10_000 {
            return Err(Error::Conflict("dependency edge limit exceeded".into()));
        }
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("dst_kind").map_err(internal)?,
                    row.try_get("dst_id").map_err(internal)?,
                ))
            })
            .collect()
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

    /// The `run` and `artifact` nodes of the upstream closure of `roots`: the
    /// roots themselves and everything they depend on, directly or transitively.
    /// The walk follows every dependency edge from its dependent to what it
    /// depends on (the direction `dependents` reads backwards), inside this
    /// session's namespace and transaction, so edges the session has written
    /// are walked too. It is read-only.
    ///
    /// The walk passes through nodes of every kind (a release or a candidate can
    /// sit between an artifact and a run), and the closure is counted over all of
    /// them: `limit` bounds the closure, every root together, each node counted
    /// once however many edges reach it (a cycle or a diamond adds nothing).
    /// A closure of more than `limit` nodes is a `Conflict`, never a truncated
    /// list: a caller that judged only the nodes it was handed could take a
    /// revoked source it was never shown for a live one, which is the capacity
    /// rule of E16.5 (a derivation beyond the scale is refused, not silently cut
    /// short). The walk itself stops after `limit + 1` nodes.
    ///
    /// Each node comes with what a revocation gate judges it by, read in the same
    /// statement: the tombstone stored under its id and, for an artifact, whether
    /// its body is already redacted. A node nothing is stored for (a deleted run,
    /// an id that was never written) is still a node of the closure; it is
    /// listed, with no tombstone unless one exists. The result is ordered by
    /// kind, then id; a node of another kind is walked through but not listed.
    pub async fn upstream_closure<K: AsRef<str>, I: AsRef<str>>(
        &mut self,
        ctx: &Context,
        roots: &[(K, I)],
        limit: usize,
    ) -> Result<Vec<UpstreamNode>> {
        let mut seeds = Vec::with_capacity(roots.len());
        for (kind, id) in roots {
            identifier(id.as_ref())?;
            seeds.push([kind.as_ref(), id.as_ref()]);
        }
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let roots: Vec<(&str, &str)> = seeds.iter().map(|pair| (pair[0], pair[1])).collect();
        let rows = upstream_rows_in_tx(&mut self.tx, ctx.namespace(), &roots, limit, false).await?
            .ok_or_else(|| Error::Conflict(format!(
                "upstream dependency closure exceeds {limit} nodes; it is refused, not truncated"
            )))?;
        let mut nodes = Vec::new();
        for row in rows {
            // The public legacy API has always reported redaction only for
            // artifacts; the private facts also cover runs for typed gates.
            let redacted = row.kind == "artifact" && row.redacted;
            nodes.push(UpstreamNode {
                kind: row.kind,
                id: row.id,
                tombstone: row
                    .tombstone
                    .map(|body| serde_json::from_str(&body).map_err(internal))
                    .transpose()?,
                redacted,
            });
        }
        nodes.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        Ok(nodes)
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
        } else {
            // E16.5: a new world id is a new derivation. Sealing or updating an
            // existing draft creates no world and is never refused here.
            let worlds = self.replay_world_count().await?;
            if worlds >= MVP_MAX_REPLAY_WORLDS {
                return Err(mvp_capacity_exceeded(
                    "replay worlds",
                    worlds,
                    MVP_MAX_REPLAY_WORLDS,
                ));
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentHostRestoreContent {
    id: String,
    schema_version: String,
    manifest: evo_core::contract::HostSurfaceManifestV2,
    manifest_digest: String,
    source: evo_core::contract::ReviewedHostSourceEnvelopeV1,
    extracted: Vec<String>,
    extracted_typed: Vec<evo_core::contract::HostSurfaceDeclarationV2>,
    predecessor_surface_id: Option<String>,
    upgrade_diff: evo_core::contract::HostSurfaceUpgradeDiffV2,
}
fn validate_current_host_restore_content(value: &Value) -> Result<()> {
    let record: CurrentHostRestoreContent =
        serde_json::from_value(value.clone()).map_err(internal)?;
    identifier(&record.id)?;
    if record.schema_version != "rsia.host_surface_record.v2"
        || record.predecessor_surface_id.as_deref() == Some(record.id.as_str())
        || record.manifest_digest != fingerprint(&record.manifest)?
    {
        return Err(Error::Invalid(
            "invalid reviewed host source/default".into(),
        ));
    }
    if let Some(previous) = &record.predecessor_surface_id {
        identifier(previous)?;
    }
    // The source string is still the original UTF-8. This parser checks its
    // numeric tokens, decoded duplicate keys and limits before typed values.
    evo_core::contract::validate_host_surface_v2_source(
        &record.manifest,
        &record.source,
        &record.extracted,
        &record.extracted_typed,
    )?;
    if record.predecessor_surface_id.is_none()
        && fingerprint(&record.upgrade_diff)?
            != fingerprint(&evo_core::contract::host_surface_upgrade_diff_v2(
                &record.manifest,
                None,
            )?)?
    {
        return Err(Error::Invalid(
            "invalid reviewed host source/default".into(),
        ));
    }
    Ok(())
}

/// Data dispatch for the one current host-record wire, never authorization.
/// The marker is read with the body by each caller's fixed SQL selector.
pub(crate) fn current_host_surface_object_value(
    body: &str,
    current_host_surface: i64,
) -> Result<Option<Value>> {
    match current_host_surface {
        1 => {
            let value = evo_core::contract::decode_host_surface_record_v2_json(body)?;
            validate_current_host_restore_content(&value)?;
            Ok(Some(value))
        }
        0 => Ok(None),
        _ => Err(Error::Internal),
    }
}

/// Ordinary object readers retain their original serde path for every other
/// body. Recovery facts instead keep their existing strict legacy parser.
pub(crate) fn decode_object_row_body<T: DeserializeOwned>(
    body: &str,
    current_host_surface: i64,
) -> Result<T> {
    match current_host_surface_object_value(body, current_host_surface)? {
        Some(value) => serde_json::from_value(value).map_err(internal),
        None => serde_json::from_str(body).map_err(internal),
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

    // Graph construction stays outside the progress handler's measured region.
    async fn snapshot_blob_chain(session: &mut Session, ctx: &Context, root: &str, nodes: i64) {
        sqlx::query(
            "WITH RECURSIVE seq(i) AS (
               SELECT 1 UNION ALL SELECT i+1 FROM seq WHERE i < ?1
             )
             INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id)
             SELECT ?2, CASE WHEN i=1 THEN 'artifact' ELSE 'blob' END,
                    CASE WHEN i=1 THEN ?3 ELSE ?3 || '-' || (i-1) END,
                    'blob', ?3 || '-' || i FROM seq",
        )
        .bind(nodes)
        .bind(ctx.namespace())
        .bind(root)
        .execute(&mut *session.tx)
        .await
        .unwrap();
    }

    async fn snapshot_vm_steps(
        session: &mut Session,
        ctx: &Context,
        root: &str,
    ) -> (Result<Vec<DependencyRecordSnapshot>>, u64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        let steps = std::sync::Arc::new(AtomicU64::new(0));
        let counter = steps.clone();
        session
            .tx
            .lock_handle()
            .await
            .unwrap()
            .set_progress_handler(1, move || {
                counter.fetch_add(1, Ordering::Relaxed);
                true
            });
        let result = load_dependency_record_snapshots(session, ctx, root).await;
        session
            .tx
            .lock_handle()
            .await
            .unwrap()
            .remove_progress_handler();
        (result, steps.load(Ordering::Relaxed))
    }

    fn assert_snapshot_capacity_conflict(result: Result<Vec<DependencyRecordSnapshot>>) {
        assert!(matches!(result, Err(Error::Conflict(message))
            if message == "local export dependency closure exceeds bounded verification"));
    }

    #[tokio::test]
    async fn dependency_snapshots_accept_exact_limit_and_refuse_one_more() {
        let (_dir, store) = db().await;
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        snapshot_blob_chain(&mut session, &ctx, "root", 10_000).await;
        // The root has no object: it must not be implicitly included.
        let records = load_dependency_record_snapshots(&mut session, &ctx, "root")
            .await
            .unwrap();
        assert_eq!(records.len(), 10_000);
        assert!(
            records
                .iter()
                .all(|record| record.kind == "blob" && record.fingerprint.is_none())
        );
        assert!(records.windows(2).all(|pair| pair[0].id < pair[1].id));
        session
            .put_edge(&ctx, "blob", "root-10000", "candidate", "overflow")
            .await
            .unwrap();
        // Every kind counts, and capacity refusal precedes missing-object reads.
        assert_snapshot_capacity_conflict(
            load_dependency_record_snapshots(&mut session, &ctx, "root").await,
        );
    }

    #[tokio::test]
    async fn dependency_snapshots_preserve_typed_cycles_diamonds_sort_and_fingerprints() {
        let (_dir, store) = db().await;
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        assert!(
            load_dependency_record_snapshots(&mut session, &ctx, "root")
                .await
                .unwrap()
                .is_empty()
        );
        let objects = [
            ("artifact", "root", json!({"root":true})),
            ("candidate", "shared", json!({"candidate":1})),
            ("release", "shared", json!({"release":2})),
            ("run", "z", json!({"run":3})),
        ];
        for (kind, id, body) in &objects {
            session
                .put(&ctx, kind, id, ctx.actor(), body)
                .await
                .unwrap();
        }
        for (src_kind, src_id, dst_kind, dst_id) in [
            ("artifact", "root", "run", "z"),
            ("artifact", "root", "release", "shared"),
            ("run", "z", "candidate", "shared"),
            ("release", "shared", "candidate", "shared"),
            ("candidate", "shared", "blob", "b"),
            ("candidate", "shared", "candidate", "shared"),
            ("blob", "b", "artifact", "root"),
            ("artifact", "root", "artifact", "root"),
        ] {
            session
                .put_edge(&ctx, src_kind, src_id, dst_kind, dst_id)
                .await
                .unwrap();
        }
        let mut expected: Vec<_> = objects
            .iter()
            .map(|(kind, id, body)| DependencyRecordSnapshot {
                kind: (*kind).into(),
                id: (*id).into(),
                fingerprint: Some(fingerprint(body).unwrap()),
            })
            .collect();
        expected.push(DependencyRecordSnapshot {
            kind: "blob".into(),
            id: "b".into(),
            fingerprint: None,
        });
        expected.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        assert_eq!(
            load_dependency_record_snapshots(&mut session, &ctx, "root")
                .await
                .unwrap(),
            expected
        );
        let changed = json!({"candidate":4});
        session
            .put(&ctx, "candidate", "shared", ctx.actor(), &changed)
            .await
            .unwrap();
        expected[2].fingerprint = Some(fingerprint(&changed).unwrap());
        assert_eq!(
            load_dependency_record_snapshots(&mut session, &ctx, "root")
                .await
                .unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn dependency_snapshots_isolate_namespace_edges_objects_and_tombstones() {
        let (_dir, store) = db().await;
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let other = Context::new("other", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        let body = json!({"namespace":"n"});
        for scope in [&ctx, &other] {
            session
                .put_edge(scope, "artifact", "root", "run", "shared")
                .await
                .unwrap();
            session
                .put(
                    scope,
                    "run",
                    "shared",
                    scope.actor(),
                    &json!({"namespace":scope.namespace()}),
                )
                .await
                .unwrap();
        }
        session
            .put_edge(&other, "run", "shared", "candidate", "missing")
            .await
            .unwrap();
        session
            .put(&other, "tombstone", "shared", other.actor(), &json!({}))
            .await
            .unwrap();
        assert_eq!(
            load_dependency_record_snapshots(&mut session, &ctx, "root")
                .await
                .unwrap(),
            vec![DependencyRecordSnapshot {
                kind: "run".into(),
                id: "shared".into(),
                fingerprint: Some(fingerprint(&body).unwrap()),
            }]
        );
        assert!(matches!(
            load_dependency_record_snapshots(&mut session, &other, "root").await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn dependency_snapshots_refuse_missing_nonblob_and_tombstoned_ids() {
        let (_dir, store) = db().await;
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        for kind in ["blob", "run", "artifact", "candidate", "release", "custom"] {
            session
                .put_edge(&ctx, "artifact", kind, kind, "missing")
                .await
                .unwrap();
            let result = load_dependency_record_snapshots(&mut session, &ctx, kind).await;
            if kind == "blob" {
                assert_eq!(result.unwrap()[0].fingerprint, None);
            } else {
                assert!(matches!(result, Err(Error::NotFound)));
            }
            session
                .put(&ctx, "tombstone", "missing", ctx.actor(), &json!({}))
                .await
                .unwrap();
            assert!(matches!(
                load_dependency_record_snapshots(&mut session, &ctx, kind).await,
                Err(Error::Forbidden)
            ));
            session.delete(&ctx, "tombstone", "missing").await.unwrap();
        }
    }

    #[tokio::test]
    async fn dependency_snapshots_vm_rejection_stops_before_long_chain_tail() {
        let (_dir, store) = db().await;
        let short_ctx = Context::new("short", "admin", Role::Admin).unwrap();
        let long_ctx = Context::new("long", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        snapshot_blob_chain(&mut session, &short_ctx, "root", 20_000).await;
        snapshot_blob_chain(&mut session, &long_ctx, "root", 80_000).await;
        // Isolate recursive result bounding from join-order cost. Both graph
        // sizes use the same initialized statistics; the unrelated-edge test
        // below separately checks lookup behavior without ANALYZE.
        sqlx::query("ANALYZE dependencies")
            .execute(&mut *session.tx)
            .await
            .unwrap();
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&mut *session.tx)
            .await
            .unwrap();
        let (short_result, short_steps) = snapshot_vm_steps(&mut session, &short_ctx, "root").await;
        let (long_result, long_steps) = snapshot_vm_steps(&mut session, &long_ctx, "root").await;
        eprintln!("snapshot VM SQLite {version}: 20000={short_steps}, 80000={long_steps}");
        assert_snapshot_capacity_conflict(short_result);
        assert_snapshot_capacity_conflict(long_result);
        assert!(short_steps > 0);
        assert!(
            long_steps <= short_steps * 2,
            "long chain must take <= twice the VM steps"
        );
    }

    #[tokio::test]
    async fn dependency_snapshots_vm_lookup_ignores_unrelated_edges() {
        let (_dir, store) = db().await;
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        snapshot_blob_chain(&mut session, &ctx, "root", 20).await;
        let (before, before_steps) = snapshot_vm_steps(&mut session, &ctx, "root").await;
        snapshot_blob_chain(&mut session, &ctx, "unrelated", 80_000).await;
        let (after, after_steps) = snapshot_vm_steps(&mut session, &ctx, "root").await;
        eprintln!("snapshot VM unrelated edges: 0={before_steps}, 80000={after_steps}");
        assert_eq!(before.unwrap(), after.unwrap());
        assert!(before_steps > 0);
        assert!(
            after_steps <= before_steps * 2,
            "unrelated edges must take <= twice the VM steps"
        );
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

/// Reviewed wire history. These states classify data; they grant no source or
/// execution authority. New consumers must additionally authenticate originals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentContractState {
    CurrentVerified,
    Legacy,
    Invalid,
}
#[derive(Debug, Clone, Copy)]
pub struct PersistentContractVersion {
    pub schema: &'static str,
    pub introduced_change: &'static str,
    pub verifier_retained: bool,
    pub current: bool,
}
macro_rules! contract_versions {
    ($(($old:literal, $new:literal, $change:literal)),+ $(,)?) => {
        pub const PERSISTENT_CONTRACT_VERSIONS: &[PersistentContractVersion] = &[
            $(PersistentContractVersion { schema: $old, introduced_change: "frozen historical wire", verifier_retained: true, current: false },
              PersistentContractVersion { schema: $new, introduced_change: $change, verifier_retained: true, current: true }),+
        ];
    };
}
contract_versions!(
    (
        "rsia.resolved_bundle.v2",
        "rsia.resolved_bundle.v3",
        "complete P/B original content"
    ),
    (
        "rsia.authenticated_bundle.v1",
        "rsia.authenticated_bundle.v2",
        "complete Current bundle and originals"
    ),
    (
        "rsia.formal_compiler_input_binding.v2",
        "rsia.formal_compiler_input_binding.v3",
        "typed Current input closure"
    ),
    (
        "rsia.persisted_compiler_binding.v1",
        "rsia.persisted_compiler_binding.v2",
        "Current compiler binding"
    ),
    (
        "rsia.release_candidate.v1",
        "rsia.release_candidate.v2",
        "complete Current bundle"
    ),
    (
        "rsia.host_surface_record.v1",
        "rsia.host_surface_record.v2",
        "typed raw source and defaults"
    ),
    (
        "rsia.run_application.v1",
        "rsia.run_application.v2",
        "frozen typed Host input"
    ),
    (
        "rsia.bound_model_request.v1",
        "rsia.bound_model_request.v2",
        "Current compiler use"
    ),
    (
        "rsia.optimization.compiler_use.v1",
        "rsia.optimization.compiler_use.v2",
        "Current input originals"
    ),
    (
        "rsia.monitoring.consolidation_proposal.v1",
        "rsia.monitoring.consolidation_proposal.v2",
        "Current candidate and terminal identity"
    ),
    (
        "rsia.registered_external_provider.v1",
        "rsia.registered_external_provider.v3",
        "owned source authority and exact model material"
    ),
    (
        "rsia.issued_formal_authority.v2",
        "rsia.issued_formal_authority.v3",
        "Current candidate binding"
    ),
    (
        "rsia.experiment_plan.v1",
        "rsia.experiment_plan.v2",
        "complete typed candidate pin"
    ),
    (
        "rsia.evaluation_ticket.v2",
        "rsia.evaluation_ticket.v3",
        "source plan and issuance identity"
    ),
);
pub fn persistent_contract_state(
    schema: &str,
    identity_matches: bool,
) -> Result<PersistentContractState> {
    if schema == "rsia.registered_external_provider.v2" {
        return Ok(if identity_matches {
            PersistentContractState::Legacy
        } else {
            PersistentContractState::Invalid
        });
    }
    let version = PERSISTENT_CONTRACT_VERSIONS
        .iter()
        .find(|version| version.schema == schema)
        .ok_or_else(|| Error::Invalid("unknown persistent contract version; isolate".into()))?;
    Ok(if !identity_matches {
        PersistentContractState::Invalid
    } else if version.current {
        PersistentContractState::CurrentVerified
    } else {
        PersistentContractState::Legacy
    })
}

#[cfg(test)]
mod current_host_raw_tests {
    use super::*;
    use evo_core::Role;
    use evo_core::contract::{
        HostSurfaceClassificationV2, HostSurfaceDeclarationV2, HostSurfaceManifestV2,
        HostSurfaceRegistrationV2, HostSurfaceUpgradeDiffV2, ReviewedHostSourceEnvelopeV1,
        SurfaceCoverage, host_surface_upgrade_diff_v2, host_surface_v2_from_source,
    };
    use serde::Deserialize;
    use sqlx::Connection;
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct HostRecordWire {
        id: String,
        schema_version: String,
        manifest: HostSurfaceManifestV2,
        manifest_digest: String,
        source: ReviewedHostSourceEnvelopeV1,
        extracted: Vec<String>,
        extracted_typed: Vec<HostSurfaceDeclarationV2>,
        predecessor_surface_id: Option<String>,
        upgrade_diff: HostSurfaceUpgradeDiffV2,
    }
    fn complete_record() -> HostRecordWire {
        let source = ReviewedHostSourceEnvelopeV1 {
            schema_version: "rsia.host_source_envelope.v1".into(), source_kind: "reviewed_fixture".into(),
            source_schema: "rsia.host_reviewed_declarations.v1".into(),
            extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
            original_utf8: r#"{"schema_version":"rsia.host_reviewed_declarations.v1","host":"reference","host_version":"1","declarations":[{"name":"option","value_type":"object","enum_values":[],"default":{"kind":"value","value":{"decimal":0.1}}}]}"#.into(),
        };
        let registration = HostSurfaceRegistrationV2 {
            adapter_version: "1".into(),
            retired: vec![],
            classifications: vec![HostSurfaceClassificationV2 {
                name: "option".into(),
                coverage: SurfaceCoverage::Unsupported,
                mapped_field: None,
                consumer: None,
                reason: "raw storage control only".into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            }],
        };
        let (manifest, extracted_typed) =
            host_surface_v2_from_source(&source, &registration).unwrap();
        HostRecordWire {
            id: "host-record".into(),
            schema_version: "rsia.host_surface_record.v2".into(),
            manifest_digest: fingerprint(&manifest).unwrap(),
            upgrade_diff: host_surface_upgrade_diff_v2(&manifest, None).unwrap(),
            extracted: extracted_typed
                .iter()
                .map(|item| item.name.clone())
                .collect(),
            manifest,
            source,
            extracted_typed,
            predecessor_surface_id: None,
        }
    }
    #[tokio::test]
    async fn current_host_raw_decoder_preserves_namespace_kind_id_and_exact_dto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host-raw.sqlite3");
        let store = Store::open(&path).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let record = complete_record();
        let raw = serde_json::to_string(&record).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "artifact", &record.id, admin.actor(), &record)
            .await
            .unwrap();
        let read: HostRecordWire = session.need(&admin, "artifact", &record.id).await.unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), raw);
        let listed: Vec<HostRecordWire> = session.list(&admin, "artifact").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(serde_json::to_string(&listed[0]).unwrap(), raw);
        assert_eq!(
            session
                .object_row(&admin, "artifact", &record.id)
                .await
                .unwrap()
                .unwrap()
                .body,
            raw
        );
        let other = Context::new("other", "admin", Role::Admin).unwrap();
        assert!(
            session
                .get::<HostRecordWire>(&other, "artifact", &record.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .get::<HostRecordWire>(&admin, "run", &record.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .get::<HostRecordWire>(&admin, "artifact", "other-id")
                .await
                .unwrap()
                .is_none()
        );
        session.commit().await.unwrap();
        store.close().await;
        let damaged = raw.replacen("\"decimal\":0.1", "\"decimal\":0.100000000000000001", 1);
        assert_ne!(damaged, raw);
        let mut connection = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false)
            .disable_statement_logging()
            .connect()
            .await
            .unwrap();
        sqlx::query("UPDATE objects SET body=? WHERE namespace='n' AND kind='artifact' AND id='host-record'")
            .bind(&damaged).execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
        let store = Store::open(&path).await.unwrap();
        let mut session = store.session().await.unwrap();
        assert!(
            matches!(session.get::<HostRecordWire>(&admin,"artifact",&record.id).await,
            Err(Error::Invalid(message)) if message == "invalid reviewed host source/default")
        );
        assert!(
            session
                .get::<Value>(&other, "artifact", &record.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            session
                .object_row(&admin, "artifact", &record.id)
                .await
                .unwrap()
                .unwrap()
                .body,
            damaged
        );
        session.commit().await.unwrap();
        store.close().await;
        let mut unknown = serde_json::to_value(&record).unwrap();
        unknown["unknown_field"] = serde_json::json!(true);
        let unknown = serde_json::to_string(&unknown).unwrap();
        assert!(matches!(
            decode_object_row_body::<HostRecordWire>(&unknown, 1),
            Err(Error::Internal)
        ));
    }
}

// A finite, ephemeral issuer. It does not modify Store/Session or grant a role.
pub use owned_config_workspace::{OwnedConfigSnapshot, OwnedConfigWorkspace, OwnedWorkspaceError};
mod owned_config_workspace {
    use evo_core::evaluation::config_precedence_generator::{
        ClosedCaseSelector, ConfigIntent, GeneratedConfigCase, KeyType, Layers, Template,
    };
    use std::{fmt, io};

    #[derive(Debug)]
    pub enum OwnedWorkspaceError {
        Unsupported(&'static str),
        Active,
        Poisoned,
        Capacity(&'static str),
        Identity(&'static str),
        OriginMismatch,
        AlreadyRead,
        WriteForbidden,
        WorkspaceBoundary,
        UnsupportedFormat(&'static str),
        Core(evo_core::Error),
        Io {
            operation: &'static str,
            source: io::Error,
        },
        Cleanup(Vec<OwnedWorkspaceError>),
        ExecutionAndCleanup {
            execution: Box<OwnedWorkspaceError>,
            cleanup: Box<OwnedWorkspaceError>,
        },
    }
    impl fmt::Display for OwnedWorkspaceError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "owned configuration workspace: {self:?}")
        }
    }
    impl std::error::Error for OwnedWorkspaceError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::Io { source, .. } => Some(source),
                Self::Core(e) => Some(e),
                Self::ExecutionAndCleanup { execution, .. } => Some(execution.as_ref()),
                _ => None,
            }
        }
    }
    impl From<evo_core::Error> for OwnedWorkspaceError {
        fn from(e: evo_core::Error) -> Self {
            Self::Core(e)
        }
    }
    type WsResult<T> = std::result::Result<T, OwnedWorkspaceError>;
    #[derive(Debug)]
    pub struct SnapshotFile {
        logical_id: &'static str,
        source_path: &'static str,
        format: &'static str,
        bytes: Vec<u8>,
    }
    impl SnapshotFile {
        pub fn logical_id(&self) -> &'static str {
            self.logical_id
        }
        pub fn source_path(&self) -> &'static str {
            self.source_path
        }
        pub fn format(&self) -> &'static str {
            self.format
        }
        pub fn bytes(&self) -> &[u8] {
            &self.bytes
        }
    }
    #[derive(Debug)]
    pub struct OwnedConfigSnapshot {
        selector: ClosedCaseSelector,
        template: Template,
        layers: Layers,
        key_type: KeyType,
        query: Vec<&'static str>,
        intent: ConfigIntent,
        files: Vec<SnapshotFile>,
    }
    impl OwnedConfigSnapshot {
        pub fn selector(&self) -> ClosedCaseSelector {
            self.selector
        }
        pub fn template(&self) -> Template {
            self.template
        }
        pub fn layers(&self) -> Layers {
            self.layers
        }
        pub fn key_type(&self) -> KeyType {
            self.key_type
        }
        pub fn query_segments(&self) -> &[&'static str] {
            &self.query
        }
        pub fn intent(&self) -> ConfigIntent {
            self.intent
        }
        pub fn files(&self) -> &[SnapshotFile] {
            &self.files
        }
    }
    pub struct OwnedConfigWorkspace {
        #[cfg(unix)]
        state: Option<State>,
    }
    // Keep the public opaque value from disclosing private directory names/FDs.
    impl fmt::Debug for OwnedConfigWorkspace {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("OwnedConfigWorkspace")
                .finish_non_exhaustive()
        }
    }
    #[cfg(test)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Parent,
        Create,
        Seal,
        Execute,
        Cleanup,
    }
    #[cfg(not(test))]
    #[derive(Clone, Copy)]
    enum Phase {
        Parent,
        Create,
        Seal,
        Execute,
        Cleanup,
    }
    #[cfg(test)]
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct IoEvent {
        phase: Phase,
        operation: &'static str,
        component: String,
        parent: Option<(u64, u64)>,
        fd: Option<(u64, u64)>,
        offset: Option<u64>,
        requested: usize,
        bytes: Vec<u8>,
        errno: Option<i32>,
        completed: bool,
        injected: bool,
    }
    #[cfg(test)]
    #[derive(Debug, Clone, Copy)]
    enum ReadFault {
        Actual,
        Interrupted,
        EarlyEof,
    }
    #[cfg(test)]
    std::thread_local! {
        static IO_EVENTS:std::cell::RefCell<Vec<IoEvent>> = const { std::cell::RefCell::new(Vec::new()) };
        static READ_FAULTS:std::cell::RefCell<std::collections::VecDeque<ReadFault>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    }
    #[cfg(unix)]
    use {
        rustix::fs::{self as rfs, AtFlags, Mode, OFlags},
        std::{
            fs::File,
            io::Write,
            os::unix::fs::{FileExt, MetadataExt},
            sync::atomic::{AtomicU8, AtomicU64, Ordering},
        },
    };
    #[cfg(unix)]
    static WORKSPACE_STATE: AtomicU8 = AtomicU8::new(0); // free, active, poisoned
    #[cfg(unix)]
    static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Identity {
        dev: u64,
        ino: u64,
        uid: u32,
    }
    #[cfg(unix)]
    fn identity(m: &std::fs::Metadata) -> Identity {
        Identity {
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
        }
    }
    #[cfg(unix)]
    fn io_error(operation: &'static str, source: impl Into<io::Error>) -> OwnedWorkspaceError {
        OwnedWorkspaceError::Io {
            operation,
            source: source.into(),
        }
    }
    #[cfg(unix)]
    fn directory_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }
    #[cfg(unix)]
    fn observed_open(
        _phase: Phase,
        parent: Option<&File>,
        component: &str,
        flags: OFlags,
        mode: Mode,
    ) -> WsResult<File> {
        #[cfg(test)]
        let parent_id = parent
            .and_then(|p| p.metadata().ok())
            .map(|m| (m.dev(), m.ino()));
        #[cfg(test)]
        IO_EVENTS.with(|events| {
            events.borrow_mut().push(IoEvent {
                phase: _phase,
                operation: "open",
                component: component.into(),
                parent: parent_id,
                fd: None,
                offset: None,
                requested: 0,
                bytes: Vec::new(),
                errno: None,
                completed: false,
                injected: false,
            })
        });
        let opened = match parent {
            Some(p) => rfs::openat(p, component, flags, mode),
            None => rfs::open(component, flags, mode),
        }
        .map(File::from);
        #[cfg(test)]
        {
            let (fd, errno) = match &opened {
                Ok(f) => (f.metadata().ok().map(|m| (m.dev(), m.ino())), None),
                Err(e) => (None, Some(e.raw_os_error())),
            };
            IO_EVENTS.with(|events| {
                events.borrow_mut().push(IoEvent {
                    phase: _phase,
                    operation: "open",
                    component: component.into(),
                    parent: parent_id,
                    fd,
                    offset: None,
                    requested: 0,
                    bytes: Vec::new(),
                    errno,
                    completed: true,
                    injected: false,
                })
            });
        }
        opened.map_err(|e| io_error("openat", e))
    }
    #[cfg(unix)]
    fn observed_read(
        _phase: Phase,
        file: &File,
        logical_id: &str,
        buffer: &mut [u8],
        offset: u64,
    ) -> io::Result<usize> {
        #[cfg(test)]
        let fault = READ_FAULTS.with(|faults| faults.borrow_mut().pop_front());
        #[cfg(test)]
        let result = match fault {
            Some(ReadFault::Interrupted) => Err(io::Error::from(rustix::io::Errno::INTR)),
            Some(ReadFault::EarlyEof) => Ok(0),
            Some(ReadFault::Actual) | None => file.read_at(buffer, offset),
        };
        #[cfg(not(test))]
        let result = file.read_at(buffer, offset);
        #[cfg(test)]
        IO_EVENTS.with(|events| {
            events.borrow_mut().push(IoEvent {
                phase: _phase,
                operation: "read",
                component: logical_id.into(),
                parent: None,
                fd: file.metadata().ok().map(|m| (m.dev(), m.ino())),
                offset: Some(offset),
                requested: buffer.len(),
                bytes: result
                    .as_ref()
                    .map(|n| buffer[..*n].to_vec())
                    .unwrap_or_default(),
                errno: result.as_ref().err().and_then(io::Error::raw_os_error),
                completed: true,
                injected: matches!(fault, Some(ReadFault::Interrupted | ReadFault::EarlyEof)),
            })
        });
        #[cfg(not(test))]
        let _ = logical_id;
        result
    }
    #[cfg(test)]
    #[derive(Debug, Clone)]
    struct BufferSample {
        phase: Phase,
        logical_id: String,
        expected_len: usize,
        capacity: usize,
        eof_slot: usize,
    }
    #[cfg(test)]
    #[derive(Debug, Clone, Copy)]
    struct MemorySample {
        phase: Phase,
        origin_case_heap_capacity: usize,
        state_inline_bytes: usize,
        directory_vector_capacity: usize,
        member_vector_capacity: usize,
        snapshot_vector_capacity: usize,
        snapshot_bytes_capacity: usize,
        query_vector_capacity: usize,
    }
    #[cfg(test)]
    std::thread_local! {
        static BUFFER_SAMPLES:std::cell::RefCell<Vec<BufferSample>> = const { std::cell::RefCell::new(Vec::new()) };
        static MEMORY_SAMPLES:std::cell::RefCell<Vec<MemorySample>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    #[cfg(unix)]
    fn bounded_read(
        phase: Phase,
        file: &File,
        logical_id: &str,
        expected: &[u8],
    ) -> WsResult<Vec<u8>> {
        let mut bytes = vec![0; expected.len()];
        #[cfg(test)]
        BUFFER_SAMPLES.with(|s| {
            s.borrow_mut().push(BufferSample {
                phase,
                logical_id: logical_id.into(),
                expected_len: expected.len(),
                capacity: bytes.capacity(),
                eof_slot: 1,
            })
        });
        let (mut offset, mut interrupted) = (0usize, 0usize);
        while offset < bytes.len() {
            match observed_read(
                phase,
                file,
                logical_id,
                &mut bytes[offset..],
                u64::try_from(offset).map_err(|_| OwnedWorkspaceError::Capacity("read offset"))?,
            ) {
                Ok(0) => return Err(OwnedWorkspaceError::Identity("truncated original fd")),
                Ok(n) => {
                    offset = offset
                        .checked_add(n)
                        .ok_or(OwnedWorkspaceError::Capacity("read count"))?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted && interrupted < 8 => {
                    interrupted += 1
                }
                Err(e) => return Err(io_error("read_at", e)),
            }
        }
        let mut eof = [0u8; 1];
        loop {
            match observed_read(
                phase,
                file,
                logical_id,
                &mut eof,
                u64::try_from(offset).map_err(|_| OwnedWorkspaceError::Capacity("EOF offset"))?,
            ) {
                Ok(0) => break,
                Ok(_) => return Err(OwnedWorkspaceError::Identity("original fd grew")),
                Err(e) if e.kind() == io::ErrorKind::Interrupted && interrupted < 8 => {
                    interrupted += 1
                }
                Err(e) => return Err(io_error("read_at EOF", e)),
            }
        }
        if bytes != expected {
            return Err(OwnedWorkspaceError::OriginMismatch);
        }
        Ok(bytes)
    }
    #[cfg(unix)]
    fn next_root_name(uid: u32, pid: u32, counter: &AtomicU64) -> WsResult<String> {
        let value = counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .map_err(|_| OwnedWorkspaceError::Capacity("root name counter exhausted"))?;
        Ok(format!("rsia-ctw-v1-{uid}-{pid}-{value}"))
    }
    #[cfg(unix)]
    #[derive(Debug)]
    struct Directory {
        fd: Option<File>,
        parent: Option<usize>,
        name: String,
        identity: Option<Identity>,
    }
    #[cfg(unix)]
    #[derive(Debug)]
    struct Member {
        parent: usize,
        name: String,
        identity: Option<Identity>,
        read: Option<File>,
        source: usize,
    }
    #[cfg(unix)]
    #[derive(Debug)]
    struct State {
        case: GeneratedConfigCase,
        parent: Option<File>,
        directories: Vec<Directory>,
        members: Vec<Member>,
        read: bool,
        uid: u32,
        permit: Permit,
    }
    #[cfg(test)]
    #[derive(Debug, Clone, Copy)]
    struct FdSample {
        phase: Phase,
        directories: usize,
        reads: usize,
        writes: usize,
        total: usize,
    }
    #[cfg(test)]
    std::thread_local! { static FD_SAMPLES:std::cell::RefCell<Vec<FdSample>> = const { std::cell::RefCell::new(Vec::new()) }; }
    #[cfg(unix)]
    #[derive(Clone, Copy)]
    struct FileLayout<'a> {
        logical_id: &'a str,
        components: &'a [&'a str],
        source_path: &'a str,
        bytes: usize,
    }
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    struct ResourceBudget {
        payload: usize,
        directories: usize,
        readers: usize,
        writers: usize,
        peak: usize,
    }
    #[cfg(unix)]
    fn preflight_layout(files: &[FileLayout<'_>]) -> WsResult<ResourceBudget> {
        if files.is_empty() || files.len() > 16 {
            return Err(OwnedWorkspaceError::Capacity("regular count"));
        }
        let mut bytes = 0usize;
        let mut directories = 1usize;
        let mut paths = std::collections::BTreeSet::new();
        let mut logical = std::collections::BTreeSet::new();
        for file in files {
            let components = file.components;
            if components.is_empty()
                || components.len() > 4
                || components.iter().any(|s| {
                    s.is_empty() || matches!(*s, "." | "..") || s.contains(['/', '\\', '\0'])
                })
            {
                return Err(OwnedWorkspaceError::WorkspaceBoundary);
            }
            if !logical.insert(file.logical_id) || !paths.insert(file.source_path) {
                return Err(OwnedWorkspaceError::Identity("duplicate member"));
            }
            if components.join("/") != file.source_path {
                return Err(OwnedWorkspaceError::OriginMismatch);
            }
            for prior in &paths {
                if *prior != file.source_path
                    && (prior.starts_with(&format!("{}/", file.source_path))
                        || file.source_path.starts_with(&format!("{prior}/")))
                {
                    return Err(OwnedWorkspaceError::WorkspaceBoundary);
                }
            }
            if file.bytes > 65_536 {
                return Err(OwnedWorkspaceError::Capacity("file bytes"));
            }
            bytes = bytes
                .checked_add(file.bytes)
                .ok_or(OwnedWorkspaceError::Capacity("total bytes overflow"))?;
            directories = directories
                .checked_add(components.len() - 1)
                .ok_or(OwnedWorkspaceError::Capacity("directory count overflow"))?;
        }
        let reads = files.len();
        let dirs = directories
            .checked_add(1)
            .ok_or(OwnedWorkspaceError::Capacity("parent count overflow"))?;
        let peak = dirs
            .checked_add(reads)
            .and_then(|n| n.checked_add(1))
            .ok_or(OwnedWorkspaceError::Capacity("FD count overflow"))?;
        if bytes > 262_144 || dirs > 50 || peak > 72 {
            return Err(OwnedWorkspaceError::Capacity("workspace preflight"));
        }
        Ok(ResourceBudget {
            payload: bytes,
            directories: dirs,
            readers: reads,
            writers: 1,
            peak,
        })
    }
    #[cfg(all(test, unix))]
    #[derive(Debug)]
    struct TestLayout {
        logical_id: String,
        components: Vec<String>,
        source_path: String,
        bytes: usize,
    }
    #[cfg(all(test, unix))]
    #[derive(Debug, Clone, Copy)]
    struct LayoutEvent {
        members: usize,
        injected: bool,
    }
    #[cfg(all(test, unix))]
    std::thread_local! {
        static H2_LAYOUT_OVERRIDE:std::cell::RefCell<Option<Vec<TestLayout>>> = const { std::cell::RefCell::new(None) };
        static LAYOUT_EVENTS:std::cell::RefCell<Vec<LayoutEvent>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    #[cfg(unix)]
    fn preflight(case: &GeneratedConfigCase) -> WsResult<ResourceBudget> {
        let files: Vec<_> = case
            .files()
            .iter()
            .map(|f| FileLayout {
                logical_id: f.logical_id(),
                components: f.components(),
                source_path: f.source_path(),
                bytes: f.raw_utf8().len(),
            })
            .collect();
        #[cfg(test)]
        if let Some(layout) = H2_LAYOUT_OVERRIDE.with(|input| input.borrow_mut().take()) {
            // Negative-only fixture projection at the real preflight boundary.
            // It cannot create a replacement case, State or executable workspace.
            let components: Vec<Vec<&str>> = layout
                .iter()
                .map(|f| f.components.iter().map(String::as_str).collect())
                .collect();
            let injected: Vec<_> = layout
                .iter()
                .zip(&components)
                .map(|(f, c)| FileLayout {
                    logical_id: &f.logical_id,
                    components: c,
                    source_path: &f.source_path,
                    bytes: f.bytes,
                })
                .collect();
            LAYOUT_EVENTS.with(|events| {
                events.borrow_mut().push(LayoutEvent {
                    members: layout.len(),
                    injected: true,
                })
            });
            return match preflight_layout(&injected) {
                Err(error) => Err(error),
                Ok(_) => Err(OwnedWorkspaceError::Identity(
                    "test layout override must be a rejection",
                )),
            };
        }
        #[cfg(test)]
        LAYOUT_EVENTS.with(|events| {
            events.borrow_mut().push(LayoutEvent {
                members: files.len(),
                injected: false,
            })
        });
        preflight_layout(&files)
    }
    #[cfg(unix)]
    fn check_ancestor(file: &File, uid: u32, parent: bool) -> WsResult<()> {
        let m = file
            .metadata()
            .map_err(|e| io_error("ancestor metadata", e))?;
        if !m.is_dir()
            || (m.uid() != 0 && m.uid() != uid)
            || ((m.mode() & 0o022) != 0 && !(m.uid() == 0 && (m.mode() & 0o1000) != 0))
            || (parent && !(m.uid() == 0 && (m.mode() & 0o1000) != 0))
        {
            return Err(OwnedWorkspaceError::Identity(
                "insecure fixed parent or ancestor",
            ));
        }
        Ok(())
    }
    #[cfg(unix)]
    fn fixed_parent(uid: u32) -> WsResult<File> {
        let mut current =
            observed_open(Phase::Parent, None, "/", directory_flags(), Mode::empty())?;
        #[cfg(test)]
        FD_SAMPLES.with(|s| {
            s.borrow_mut().push(FdSample {
                phase: Phase::Parent,
                directories: 1,
                reads: 0,
                writes: 0,
                total: 1,
            })
        });
        check_ancestor(&current, uid, false)?;
        let components: &[&str] = if cfg!(target_os = "macos") {
            &["private", "tmp"]
        } else if cfg!(target_os = "linux") {
            &["tmp"]
        } else {
            return Err(OwnedWorkspaceError::Unsupported("platform"));
        };
        for (index, component) in components.iter().enumerate() {
            let next = observed_open(
                Phase::Parent,
                Some(&current),
                component,
                directory_flags(),
                Mode::empty(),
            )?;
            #[cfg(test)]
            FD_SAMPLES.with(|s| {
                s.borrow_mut().push(FdSample {
                    phase: Phase::Parent,
                    directories: 2,
                    reads: 0,
                    writes: 0,
                    total: 2,
                })
            });
            check_ancestor(&next, uid, index + 1 == components.len())?;
            current = next;
        }
        Ok(current)
    }
    #[cfg(unix)]
    impl State {
        fn sample(&self, _phase: Phase, writes: usize) -> WsResult<()> {
            let dirs = usize::from(self.parent.is_some())
                + self.directories.iter().filter(|d| d.fd.is_some()).count();
            let reads = self.members.iter().filter(|m| m.read.is_some()).count();
            #[cfg(test)]
            MEMORY_SAMPLES.with(|s| {
                s.borrow_mut().push(MemorySample {
                    phase: _phase,
                    origin_case_heap_capacity: self.case.heap_capacity_bytes(),
                    state_inline_bytes: std::mem::size_of::<State>(),
                    directory_vector_capacity: self.directories.capacity()
                        * std::mem::size_of::<Directory>(),
                    member_vector_capacity: self.members.capacity() * std::mem::size_of::<Member>(),
                    snapshot_vector_capacity: 0,
                    snapshot_bytes_capacity: 0,
                    query_vector_capacity: 0,
                })
            });
            let total = dirs
                .checked_add(reads)
                .and_then(|n| n.checked_add(writes))
                .ok_or(OwnedWorkspaceError::Capacity("live FD count"))?;
            #[cfg(test)]
            FD_SAMPLES.with(|s| {
                s.borrow_mut().push(FdSample {
                    phase: _phase,
                    directories: dirs,
                    reads,
                    writes,
                    total,
                })
            });
            if dirs > 50 || reads > 16 || writes > 1 || total > 72 {
                return Err(OwnedWorkspaceError::Capacity("live FD peak"));
            }
            Ok(())
        }
        fn directory(&self, index: usize) -> WsResult<&File> {
            self.directories
                .get(index)
                .and_then(|d| d.fd.as_ref())
                .ok_or(OwnedWorkspaceError::Identity(
                    "missing original directory fd",
                ))
        }
        fn parent_fd(&self, parent: Option<usize>) -> WsResult<&File> {
            match parent {
                Some(i) => self.directory(i),
                None => self
                    .parent
                    .as_ref()
                    .ok_or(OwnedWorkspaceError::Identity("missing fixed parent fd")),
            }
        }
        fn create_directory(&mut self, parent: Option<usize>, name: String) -> WsResult<usize> {
            #[cfg(test)]
            if parent.is_none() {
                test_hook(HookPoint::RootMkdir, Some(self.parent_fd(parent)?), &name)?;
            }
            if let Err(error) = rfs::mkdirat(self.parent_fd(parent)?, name.as_str(), Mode::RWXU) {
                // A non-EEXIST error never authorizes adoption. If absence cannot
                // be proved, retain an unknown identity so cleanup poisons safely.
                if parent.is_none() && error != rustix::io::Errno::EXIST {
                    let absent = matches!(rfs::statat(self.parent_fd(parent)?,name.as_str(),AtFlags::SYMLINK_NOFOLLOW),Err(e) if e==rustix::io::Errno::NOENT);
                    if !absent {
                        self.directories.push(Directory {
                            fd: None,
                            parent,
                            name,
                            identity: None,
                        });
                    }
                }
                return Err(io_error("mkdirat", error));
            }
            let index = self.directories.len();
            self.directories.push(Directory {
                fd: None,
                parent,
                name,
                identity: None,
            });
            let d = &self.directories[index];
            let st = rfs::statat(
                self.parent_fd(parent)?,
                d.name.as_str(),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|e| io_error("created directory statat", e))?;
            self.directories[index].identity = Some(stat_identity(&st));
            let fd = observed_open(
                Phase::Create,
                Some(self.parent_fd(parent)?),
                self.directories[index].name.as_str(),
                directory_flags(),
                Mode::empty(),
            )?;
            self.directories[index].fd = Some(fd);
            self.sample(Phase::Create, 0)?;
            let m = self
                .directory(index)?
                .metadata()
                .map_err(|e| io_error("created directory metadata", e))?;
            if !m.is_dir()
                || m.uid() != self.uid
                || m.mode() & 0o7777 != 0o700
                || Some(identity(&m)) != self.directories[index].identity
            {
                return Err(OwnedWorkspaceError::Identity("created directory differs"));
            }
            Ok(index)
        }
    }
    #[cfg(unix)]
    fn check_regular(
        file: &File,
        original: Identity,
        uid: u32,
        len: usize,
        mode: u32,
    ) -> WsResult<()> {
        let m = file
            .metadata()
            .map_err(|e| io_error("original file metadata", e))?;
        if !m.is_file()
            || identity(&m) != original
            || m.uid() != uid
            || m.nlink() != 1
            || m.len()
                != u64::try_from(len).map_err(|_| OwnedWorkspaceError::Capacity("file length"))?
            || m.mode() & 0o7777 != mode
        {
            return Err(OwnedWorkspaceError::Identity(
                "original regular identity/link/mode/length differs",
            ));
        }
        Ok(())
    }
    #[cfg(unix)]
    fn write_original(file: &mut File, logical_id: &str, bytes: &[u8]) -> WsResult<()> {
        let (mut offset, mut interrupted) = (0usize, 0usize);
        while offset < bytes.len() {
            let written = file.write(&bytes[offset..]);
            #[cfg(test)]
            IO_EVENTS.with(|events| {
                events.borrow_mut().push(IoEvent {
                    phase: Phase::Create,
                    operation: "write",
                    component: logical_id.into(),
                    parent: None,
                    fd: file.metadata().ok().map(|m| (m.dev(), m.ino())),
                    offset: Some(offset as u64),
                    requested: bytes.len() - offset,
                    bytes: written
                        .as_ref()
                        .map(|n| bytes[offset..offset + *n].to_vec())
                        .unwrap_or_default(),
                    errno: written.as_ref().err().and_then(io::Error::raw_os_error),
                    completed: true,
                    injected: false,
                })
            });
            match written {
                Ok(0) => return Err(io_error("write", io::Error::from(io::ErrorKind::WriteZero))),
                Ok(n) => {
                    offset = offset
                        .checked_add(n)
                        .ok_or(OwnedWorkspaceError::Capacity("write count"))?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted && interrupted < 8 => {
                    interrupted += 1
                }
                Err(e) => return Err(io_error("write", e)),
            }
        }
        #[cfg(not(test))]
        let _ = logical_id;
        file.sync_all().map_err(|e| io_error("file fsync", e))
    }
    #[cfg(unix)]
    impl State {
        fn populate(&mut self) -> WsResult<()> {
            let pid = rustix::process::getpid().as_raw_nonzero().get() as u32;
            let mut root = None;
            for _ in 0..8 {
                let name = next_root_name(self.uid, pid, &ROOT_COUNTER)?;
                match self.create_directory(None, name) {
                    Ok(index) => {
                        root = Some(index);
                        break;
                    }
                    Err(OwnedWorkspaceError::Io { source, .. })
                        if source.raw_os_error()
                            == Some(rustix::io::Errno::EXIST.raw_os_error()) => {}
                    Err(e) => return Err(e),
                }
            }
            let root =
                root.ok_or_else(|| io_error("mkdirat collisions", rustix::io::Errno::EXIST))?;
            #[cfg(test)]
            test_hook(HookPoint::RootCreated, Some(self.directory(root)?), "")?;
            for source_index in 0..self.case.files().len() {
                let components = self.case.files()[source_index].components();
                let mut parent = root;
                for component in &components[..components.len() - 1] {
                    parent = match self
                        .directories
                        .iter()
                        .position(|d| d.parent == Some(parent) && d.name == *component)
                    {
                        Some(index) => index,
                        None => self.create_directory(Some(parent), (*component).into())?,
                    };
                }
                let name = components[components.len() - 1];
                #[cfg(test)]
                test_hook(
                    HookPoint::BeforeMemberCreate,
                    Some(self.directory(parent)?),
                    name,
                )?;
                let mut writer = observed_open(
                    Phase::Create,
                    Some(self.directory(parent)?),
                    name,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )?;
                let member = self.members.len();
                self.members.push(Member {
                    parent,
                    name: name.into(),
                    identity: None,
                    read: None,
                    source: source_index,
                });
                self.sample(Phase::Create, 1)?;
                let m = writer
                    .metadata()
                    .map_err(|e| io_error("created regular metadata", e))?;
                let original = identity(&m);
                self.members[member].identity = Some(original);
                if !m.is_file()
                    || m.uid() != self.uid
                    || m.nlink() != 1
                    || m.mode() & 0o7777 != 0o600
                {
                    return Err(OwnedWorkspaceError::Identity("created regular differs"));
                }
                self.sample(Phase::Create, 1)?;
                let source = &self.case.files()[source_index];
                write_original(
                    &mut writer,
                    source.logical_id(),
                    source.raw_utf8().as_bytes(),
                )?;
                #[cfg(test)]
                test_hook(
                    HookPoint::BeforePreopen,
                    Some(self.directory(parent)?),
                    name,
                )?;
                let reader = observed_open(
                    Phase::Seal,
                    Some(self.directory(parent)?),
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                self.members[member].read = Some(reader);
                self.sample(Phase::Seal, 1)?;
                #[cfg(test)]
                test_hook(HookPoint::AfterPreopen, Some(self.directory(parent)?), name)?;
                let reader = self.members[member]
                    .read
                    .as_ref()
                    .ok_or(OwnedWorkspaceError::Identity("preopen missing"))?;
                check_regular(&writer, original, self.uid, source.raw_utf8().len(), 0o600)?;
                check_regular(reader, original, self.uid, source.raw_utf8().len(), 0o600)?;
                let seal_bytes = bounded_read(
                    Phase::Seal,
                    reader,
                    source.logical_id(),
                    source.raw_utf8().as_bytes(),
                )?;
                check_regular(reader, original, self.uid, source.raw_utf8().len(), 0o600)?;
                drop(seal_bytes);
                rfs::fchmod(&writer, Mode::RUSR).map_err(|e| io_error("seal fchmod", e))?;
                check_regular(&writer, original, self.uid, source.raw_utf8().len(), 0o400)?;
                check_regular(reader, original, self.uid, source.raw_utf8().len(), 0o400)?;
                drop(writer);
                self.sample(Phase::Seal, 0)?;
            }
            Ok(())
        }
        fn snapshot(&mut self) -> WsResult<OwnedConfigSnapshot> {
            if self.read {
                return Err(OwnedWorkspaceError::AlreadyRead);
            }
            self.read = true;
            let mut files = Vec::with_capacity(self.members.len());
            for member in &self.members {
                let source = &self.case.files()[member.source];
                let file = member
                    .read
                    .as_ref()
                    .ok_or(OwnedWorkspaceError::Identity("original read fd missing"))?;
                let original = member
                    .identity
                    .ok_or(OwnedWorkspaceError::Identity("creation identity missing"))?;
                check_regular(file, original, self.uid, source.raw_utf8().len(), 0o400)?;
                let bytes = bounded_read(
                    Phase::Execute,
                    file,
                    source.logical_id(),
                    source.raw_utf8().as_bytes(),
                )?;
                check_regular(file, original, self.uid, source.raw_utf8().len(), 0o400)?;
                files.push(SnapshotFile {
                    logical_id: source.logical_id(),
                    source_path: source.source_path(),
                    format: source.format(),
                    bytes,
                });
                #[cfg(test)]
                MEMORY_SAMPLES.with(|s| {
                    s.borrow_mut().push(MemorySample {
                        phase: Phase::Execute,
                        origin_case_heap_capacity: self.case.heap_capacity_bytes(),
                        state_inline_bytes: std::mem::size_of::<State>(),
                        directory_vector_capacity: self.directories.capacity()
                            * std::mem::size_of::<Directory>(),
                        member_vector_capacity: self.members.capacity()
                            * std::mem::size_of::<Member>(),
                        snapshot_vector_capacity: files.capacity()
                            * std::mem::size_of::<SnapshotFile>(),
                        snapshot_bytes_capacity: files.iter().map(|f| f.bytes.capacity()).sum(),
                        query_vector_capacity: 0,
                    })
                });
            }
            self.sample(Phase::Execute, 0)?;
            let query = self.case.query_segments().to_vec();
            #[cfg(test)]
            MEMORY_SAMPLES.with(|s| {
                s.borrow_mut().push(MemorySample {
                    phase: Phase::Execute,
                    origin_case_heap_capacity: self.case.heap_capacity_bytes(),
                    state_inline_bytes: std::mem::size_of::<State>(),
                    directory_vector_capacity: self.directories.capacity()
                        * std::mem::size_of::<Directory>(),
                    member_vector_capacity: self.members.capacity() * std::mem::size_of::<Member>(),
                    snapshot_vector_capacity: files.capacity()
                        * std::mem::size_of::<SnapshotFile>(),
                    snapshot_bytes_capacity: files.iter().map(|f| f.bytes.capacity()).sum(),
                    query_vector_capacity: query.capacity() * std::mem::size_of::<&str>(),
                })
            });
            Ok(OwnedConfigSnapshot {
                selector: self.case.selector(),
                template: self.case.template(),
                layers: self.case.layer_count(),
                key_type: self.case.key_type(),
                query,
                intent: self.case.intent(),
                files,
            })
        }
    }
    #[cfg(unix)]
    #[derive(Debug)]
    struct Permit {
        resolved: bool,
    }
    #[cfg(unix)]
    impl Permit {
        fn acquire() -> WsResult<Self> {
            match WORKSPACE_STATE.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => Ok(Self { resolved: false }),
                Err(2) => Err(OwnedWorkspaceError::Poisoned),
                Err(_) => Err(OwnedWorkspaceError::Active),
            }
        }
        fn resolve(&mut self, clean: bool) {
            self.resolved = true;
            WORKSPACE_STATE.store(if clean { 0 } else { 2 }, Ordering::SeqCst);
        }
    }
    #[cfg(unix)]
    impl Drop for Permit {
        fn drop(&mut self) {
            if !self.resolved {
                WORKSPACE_STATE.store(2, Ordering::SeqCst);
            }
        }
    }
    #[cfg(unix)]
    #[allow(clippy::unnecessary_cast)] // Stat field widths differ between macOS and Linux.
    fn stat_identity(st: &rfs::Stat) -> Identity {
        Identity {
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            uid: st.st_uid,
        }
    }
    #[cfg(unix)]
    fn remove_known(
        parent: &File,
        name: &str,
        original: Option<Identity>,
        directory: bool,
    ) -> WsResult<()> {
        let original = original.ok_or(OwnedWorkspaceError::Identity(
            "cleanup lacks creation identity",
        ))?;
        let st = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| io_error("cleanup statat", e))?;
        let expected = if directory {
            rfs::FileType::Directory
        } else {
            rfs::FileType::RegularFile
        };
        if stat_identity(&st) != original
            || rfs::FileType::from_raw_mode(st.st_mode) != expected
            || (!directory && st.st_nlink != 1)
        {
            return Err(OwnedWorkspaceError::Identity(
                "cleanup member identity differs",
            ));
        }
        let removed = rfs::unlinkat(
            parent,
            name,
            if directory {
                AtFlags::REMOVEDIR
            } else {
                AtFlags::empty()
            },
        );
        #[cfg(test)]
        IO_EVENTS.with(|events| {
            events.borrow_mut().push(IoEvent {
                phase: Phase::Cleanup,
                operation: "unlink",
                component: name.into(),
                parent: parent.metadata().ok().map(|m| (m.dev(), m.ino())),
                fd: Some((original.dev, original.ino)),
                offset: None,
                requested: 0,
                bytes: Vec::new(),
                errno: removed.as_ref().err().map(|e| e.raw_os_error()),
                completed: true,
                injected: false,
            })
        });
        removed.map_err(|e| io_error("cleanup unlinkat", e))
    }
    #[cfg(unix)]
    impl State {
        fn cleanup(mut self) -> WsResult<()> {
            for member in &mut self.members {
                drop(member.read.take());
            }
            let mut failures = Vec::new();
            if let Err(e) = self.sample(Phase::Cleanup, 0) {
                failures.push(e);
            }
            for member in self.members.iter().rev() {
                let result = self
                    .directory(member.parent)
                    .and_then(|p| remove_known(p, &member.name, member.identity, false));
                if let Err(e) = result {
                    failures.push(e);
                }
            }
            for index in (0..self.directories.len()).rev() {
                let directory = &self.directories[index];
                let result = self
                    .parent_fd(directory.parent)
                    .and_then(|p| remove_known(p, &directory.name, directory.identity, true));
                if let Err(e) = result {
                    failures.push(e);
                }
                drop(self.directories[index].fd.take());
                if let Err(e) = self.sample(Phase::Cleanup, 0) {
                    failures.push(e);
                }
            }
            self.members.clear();
            self.directories.clear();
            drop(self.parent.take());
            if let Err(e) = self.sample(Phase::Cleanup, 0) {
                failures.push(e);
            }
            self.permit.resolve(failures.is_empty());
            if failures.is_empty() {
                Ok(())
            } else {
                Err(OwnedWorkspaceError::Cleanup(failures))
            }
        }
    }
    #[cfg(unix)]
    impl OwnedConfigWorkspace {
        pub fn create(case: GeneratedConfigCase) -> WsResult<Self> {
            if !cfg!(any(target_os = "macos", target_os = "linux")) {
                return Err(OwnedWorkspaceError::Unsupported("platform"));
            }
            let uid = rustix::process::getuid().as_raw();
            let effective = rustix::process::geteuid().as_raw();
            #[cfg(test)]
            let (uid, effective) = test_uids(uid, effective)?;
            if uid == 0 || uid != effective {
                return Err(OwnedWorkspaceError::Unsupported(
                    "real/effective UID must match and be nonzero",
                ));
            }
            let budget = preflight(&case)?;
            if budget.payload > 262_144
                || budget.directories > 50
                || budget.readers > 16
                || budget.writers > 1
                || budget.peak > 72
            {
                return Err(OwnedWorkspaceError::Capacity("preflight budget differs"));
            }
            let mut permit = Permit::acquire()?;
            let parent = match fixed_parent(uid) {
                Ok(p) => p,
                Err(e) => {
                    permit.resolve(true);
                    return Err(e);
                }
            };
            let mut workspace = Self {
                state: Some(State {
                    case,
                    parent: Some(parent),
                    directories: Vec::with_capacity(49),
                    members: Vec::with_capacity(16),
                    read: false,
                    uid,
                    permit,
                }),
            };
            let result = workspace
                .state
                .as_mut()
                .ok_or(OwnedWorkspaceError::Identity("creation state missing"))?
                .populate();
            if let Err(execution) = result {
                return match workspace.finish() {
                    Ok(()) => Err(execution),
                    Err(cleanup) => Err(OwnedWorkspaceError::ExecutionAndCleanup {
                        execution: Box::new(execution),
                        cleanup: Box::new(cleanup),
                    }),
                };
            }
            Ok(workspace)
        }
        pub fn read_once(&mut self) -> WsResult<OwnedConfigSnapshot> {
            self.state
                .as_mut()
                .ok_or(OwnedWorkspaceError::Identity("workspace finished"))?
                .snapshot()
        }
        pub fn finish(mut self) -> WsResult<()> {
            match self.state.take() {
                Some(state) => state.cleanup(),
                None => Err(OwnedWorkspaceError::Identity("workspace already finished")),
            }
        }
    }
    #[cfg(unix)]
    impl Drop for OwnedConfigWorkspace {
        fn drop(&mut self) {
            if let Some(state) = self.state.take() {
                let _ = state.cleanup();
            }
        }
    }
    #[cfg(not(unix))]
    impl OwnedConfigWorkspace {
        pub fn create(_case: GeneratedConfigCase) -> WsResult<Self> {
            Err(OwnedWorkspaceError::Unsupported("platform"))
        }
        pub fn read_once(&mut self) -> WsResult<OwnedConfigSnapshot> {
            Err(OwnedWorkspaceError::Unsupported("platform"))
        }
        pub fn finish(self) -> WsResult<()> {
            Err(OwnedWorkspaceError::Unsupported("platform"))
        }
    }
    #[cfg(all(test, unix))]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HookPoint {
        UidPolicy,
        RootMkdir,
        RootCreated,
        BeforeMemberCreate,
        BeforePreopen,
        AfterPreopen,
    }
    #[cfg(all(test, unix))]
    #[derive(Debug)]
    enum TestAction {
        Uids { real: u32, effective: u32 },
        MkdirError(rustix::io::Errno),
        Panic,
        Symlink(std::path::PathBuf),
        Hardlink(std::path::PathBuf),
        Fifo,
    }
    #[cfg(all(test, unix))]
    #[derive(Debug, Clone)]
    struct HookEvent {
        point: HookPoint,
        action: &'static str,
        component: String,
        errno: Option<i32>,
    }
    #[cfg(all(test, unix))]
    #[derive(Debug, Clone)]
    struct UidEvent {
        actual_real: u32,
        actual_effective: u32,
        selected_real: u32,
        selected_effective: u32,
        injected: bool,
    }
    #[cfg(all(test, unix))]
    std::thread_local! {
        static H2_HOOKS:std::cell::RefCell<std::collections::VecDeque<(HookPoint,TestAction)>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
        static HOOK_EVENTS:std::cell::RefCell<Vec<HookEvent>> = const { std::cell::RefCell::new(Vec::new()) };
        static UID_EVENTS:std::cell::RefCell<Vec<UidEvent>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    #[cfg(all(test, unix))]
    fn take_hook(point: HookPoint) -> Option<TestAction> {
        H2_HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            if hooks.front().is_some_and(|(p, _)| *p == point) {
                hooks.pop_front().map(|(_, a)| a)
            } else {
                None
            }
        })
    }
    #[cfg(all(test, unix))]
    fn test_uids(real: u32, effective: u32) -> WsResult<(u32, u32)> {
        let action = take_hook(HookPoint::UidPolicy);
        let injected = action.is_some();
        let selected = match action {
            Some(TestAction::Uids { real, effective }) => (real, effective),
            None => (real, effective),
            _ => return Err(OwnedWorkspaceError::Identity("wrong UID hook action")),
        };
        UID_EVENTS.with(|events| {
            events.borrow_mut().push(UidEvent {
                actual_real: real,
                actual_effective: effective,
                selected_real: selected.0,
                selected_effective: selected.1,
                injected,
            })
        });
        Ok(selected)
    }
    // Fixture mutations are deliberately separate from production open/read observations.
    // No hook is public, and none replaces identity, origin, read or cleanup validation.
    #[cfg(all(test, unix))]
    fn fixture_fifo(parent: &File, name: &str) -> WsResult<()> {
        #[cfg(target_os = "linux")]
        {
            rfs::mkfifoat(parent, name, Mode::RUSR | Mode::WUSR)
                .map_err(|e| io_error("fixture injected mutation", e))
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::{ffi::OsStrExt, fs::FileTypeExt};
            let raw_path =
                rfs::getpath(parent).map_err(|e| io_error("fixture parent getpath", e))?;
            let parent_path =
                std::path::Path::new(std::ffi::OsStr::from_bytes(raw_path.as_bytes()));
            let root_path = parent_path.parent().ok_or(OwnedWorkspaceError::Identity(
                "fixture parent lacks owned root",
            ))?;
            let fixed_path = root_path.parent().ok_or(OwnedWorkspaceError::Identity(
                "fixture root lacks fixed parent",
            ))?;
            let root_name = root_path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or(OwnedWorkspaceError::Identity("fixture root name"))?;
            let directory = parent_path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or(OwnedWorkspaceError::Identity("fixture member directory"))?;
            let prefix = format!(
                "rsia-ctw-v1-{}-{}-",
                rustix::process::getuid().as_raw(),
                std::process::id()
            );
            if !root_name
                .strip_prefix(&prefix)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                || !matches!(
                    (directory, name),
                    ("override", "config.json")
                        | ("project", "config.json")
                        | ("defaults", "base.json")
                        | ("config", "unsupported.toml")
                )
            {
                return Err(OwnedWorkspaceError::WorkspaceBoundary);
            }
            let parent_before = parent
                .metadata()
                .map_err(|e| io_error("fixture parent metadata", e))?;
            let root_before = std::fs::symlink_metadata(root_path)
                .map_err(|e| io_error("fixture root metadata", e))?;
            let fixed = std::fs::symlink_metadata(fixed_path)
                .map_err(|e| io_error("fixture fixed metadata", e))?;
            let expected_fixed = std::fs::symlink_metadata("/private/tmp")
                .map_err(|e| io_error("fixture expected fixed metadata", e))?;
            let parent_id = (parent_before.dev(), parent_before.ino());
            let root_id = (root_before.dev(), root_before.ino());
            let fixed_id = (fixed.dev(), fixed.ino());
            let uid = rustix::process::getuid().as_raw();
            let linked = IO_EVENTS.with(|events| {
                let events = events.borrow();
                events.iter().any(|e| {
                    e.phase == Phase::Create
                        && e.operation == "open"
                        && e.completed
                        && e.component == root_name
                        && e.fd == Some(root_id)
                        && e.parent == Some(fixed_id)
                }) && events.iter().any(|e| {
                    e.phase == Phase::Create
                        && e.operation == "open"
                        && e.completed
                        && e.component == directory
                        && e.fd == Some(parent_id)
                        && e.parent == Some(root_id)
                })
            });
            let parent_at_path = std::fs::symlink_metadata(parent_path)
                .map_err(|e| io_error("fixture path parent metadata", e))?;
            if !linked
                || !parent_before.is_dir()
                || parent_before.uid() != uid
                || !root_before.is_dir()
                || root_before.uid() != uid
                || !parent_at_path.is_dir()
                || identity(&parent_at_path) != identity(&parent_before)
                || fixed_id != (expected_fixed.dev(), expected_fixed.ino())
                || fixed.uid() != 0
                || fixed.mode() & 0o1000 == 0
            {
                return Err(OwnedWorkspaceError::Identity(
                    "fixture FIFO parent is not this owned creation",
                ));
            }
            let path = parent_path.join(name);
            let argv = vec![
                "/usr/bin/mkfifo".to_string(),
                "-m".into(),
                "600".into(),
                path.to_string_lossy().into_owned(),
            ];
            let output = std::process::Command::new("/usr/bin/mkfifo")
                .args(["-m", "600"])
                .arg(&path)
                .output();
            let output = match output {
                Ok(output) => output,
                Err(source) => {
                    println!(
                        "RSIA_AG174B_H2_FIFO_FIXTURE {}",
                        serde_json::json!({"argv":argv,"spawn_errno":source.raw_os_error(),"error":source.to_string(),"fixture_only":true})
                    );
                    return Err(io_error("fixture mkfifo spawn", source));
                }
            };
            println!(
                "RSIA_AG174B_H2_FIFO_FIXTURE {}",
                serde_json::json!({"argv":argv,"status":format!("{:?}",output.status),"code":output.status.code(),"success":output.status.success(),"stdout":output.stdout,"stderr":output.stderr,"parent_before":parent_id,"root":root_id,"fixed_parent":fixed_id,"fixture_only":true,"production_execution_permission":false})
            );
            if !output.status.success() {
                return Err(io_error(
                    "fixture mkfifo exit",
                    io::Error::other("fixed mkfifo failed; original status and streams recorded"),
                ));
            }
            let fifo = std::fs::symlink_metadata(&path)
                .map_err(|e| io_error("fixture FIFO metadata", e))?;
            let parent_after = std::fs::symlink_metadata(parent_path)
                .map_err(|e| io_error("fixture FIFO parent recheck", e))?;
            let root_after = std::fs::symlink_metadata(root_path)
                .map_err(|e| io_error("fixture FIFO root recheck", e))?;
            println!(
                "RSIA_AG174B_H2_FIFO_FIXTURE {}",
                serde_json::json!({"path":path.to_string_lossy(),"dev":fifo.dev(),"ino":fifo.ino(),"uid":fifo.uid(),"mode":fifo.mode(),"nlink":fifo.nlink(),"is_fifo":fifo.file_type().is_fifo(),"parent_after":(parent_after.dev(),parent_after.ino()),"fixture_only":true})
            );
            if !fifo.file_type().is_fifo()
                || fifo.uid() != uid
                || !parent_after.is_dir()
                || identity(&parent_after) != identity(&parent_before)
                || !root_after.is_dir()
                || identity(&root_after) != identity(&root_before)
            {
                return Err(OwnedWorkspaceError::Identity(
                    "fixture FIFO or parent changed",
                ));
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (parent, name);
            Err(OwnedWorkspaceError::Unsupported("fixture FIFO platform"))
        }
    }
    #[cfg(all(test, unix))]
    fn test_hook(point: HookPoint, parent: Option<&File>, name: &str) -> WsResult<()> {
        let Some(action) = take_hook(point) else {
            return Ok(());
        };
        let label = match &action {
            TestAction::Uids { .. } => "uids",
            TestAction::MkdirError(_) => "mkdir-error",
            TestAction::Panic => "panic",
            TestAction::Symlink(_) => "symlink",
            TestAction::Hardlink(_) => "hardlink",
            TestAction::Fifo => "fifo",
        };
        HOOK_EVENTS.with(|events| {
            events.borrow_mut().push(HookEvent {
                point,
                action: label,
                component: name.into(),
                errno: None,
            })
        });
        if matches!(action, TestAction::Panic) {
            panic!("injected owned workspace creation unwind");
        }
        if let TestAction::MkdirError(errno) = action {
            HOOK_EVENTS.with(|events| {
                events.borrow_mut().push(HookEvent {
                    point,
                    action: label,
                    component: name.into(),
                    errno: Some(errno.raw_os_error()),
                })
            });
            return Err(io_error("injected mkdirat", errno));
        }
        let parent = parent.ok_or(OwnedWorkspaceError::Identity(
            "fixture hook lacks known parent",
        ))?;
        if matches!(point, HookPoint::BeforePreopen | HookPoint::AfterPreopen) {
            rfs::renameat(parent, name, parent, "h2-displaced-original")
                .map_err(|e| io_error("fixture rename original", e))?;
        }
        let result = match action {
            TestAction::Symlink(target) => rfs::symlinkat(target.as_path(), parent, name)
                .map_err(|e| io_error("fixture injected mutation", e)),
            TestAction::Hardlink(target) => {
                rfs::linkat(rfs::CWD, target.as_path(), parent, name, AtFlags::empty())
                    .map_err(|e| io_error("fixture injected mutation", e))
            }
            TestAction::Fifo => fixture_fifo(parent, name),
            _ => return Err(OwnedWorkspaceError::Identity("wrong fixture hook action")),
        };
        HOOK_EVENTS.with(|events| {
            events.borrow_mut().push(HookEvent {
                point,
                action: label,
                component: name.into(),
                errno: result.as_ref().err().and_then(|e| match e {
                    OwnedWorkspaceError::Io { source, .. } => source.raw_os_error(),
                    _ => None,
                }),
            })
        });
        result
    }
    // BEGIN RSIA AG174B INDEPENDENT H2
    // Independent controller-owned H2 assertions, mechanically inserted before H freeze.
    #[cfg(test)]
    #[cfg(unix)]
    mod tests {
        use super::*;
        use evo_core::evaluation::config_precedence_generator::{Placement, Seed, generate_case};
        use serde_json::{Value, json};
        use std::{
            collections::BTreeSet,
            os::unix::fs::PermissionsExt,
            path::{Path, PathBuf},
            process::Command,
            sync::Arc,
        };

        const CHILD: &str = "RSIA_AG174B_H2_CASE";
        const SENTINEL: &[u8] = b"independent-h2-external-sentinel-never-product-input";
        std::thread_local! { static FIXTURE_IO: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) }; }
        fn selector() -> ClosedCaseSelector {
            ClosedCaseSelector::Task {
                template: Template::F,
                layer_count: Layers::L3,
                key_type: KeyType::B,
                placement: Placement::E,
                seed: Seed::Zero,
            }
        }
        fn generated() -> GeneratedConfigCase {
            generate_case(selector()).expect("independent closed input")
        }
        fn events() -> Vec<IoEvent> {
            IO_EVENTS.with(|e| e.borrow().clone())
        }
        fn fixture(operation: &str, detail: Value) {
            FIXTURE_IO.with(|v| {
                v.borrow_mut()
                    .push(json!({"operation":operation,"detail":detail}))
            });
        }
        fn file_id(file: &File) -> (u64, u64) {
            let m = file.metadata().expect("fixture metadata");
            (m.dev(), m.ino())
        }
        fn report(case: &str, expected: Value) {
            let io:Vec<_>=events().iter().map(|e|json!({"phase":format!("{:?}",e.phase),"operation":e.operation,"component":e.component,"parent":e.parent,"fd":e.fd,"offset":e.offset,"requested":e.requested,"bytes":e.bytes,"errno":e.errno,"completed":e.completed,"injected":e.injected})).collect();
            let fd:Vec<_>=FD_SAMPLES.with(|s|s.borrow().iter().map(|s|json!({"phase":format!("{:?}",s.phase),"directories":s.directories,"reads":s.reads,"writes":s.writes,"total":s.total})).collect());
            let buffers:Vec<_>=BUFFER_SAMPLES.with(|s|s.borrow().iter().map(|s|json!({"phase":format!("{:?}",s.phase),"logical_id":s.logical_id,"expected_len":s.expected_len,"capacity":s.capacity,"eof_slot":s.eof_slot})).collect());
            let memory:Vec<_>=MEMORY_SAMPLES.with(|s|s.borrow().iter().map(|s|json!({"phase":format!("{:?}",s.phase),"origin_case_heap_capacity":s.origin_case_heap_capacity,"state_inline_bytes":s.state_inline_bytes,"directory_vector_capacity":s.directory_vector_capacity,"member_vector_capacity":s.member_vector_capacity,"snapshot_vector_capacity":s.snapshot_vector_capacity,"snapshot_bytes_capacity":s.snapshot_bytes_capacity,"query_vector_capacity":s.query_vector_capacity})).collect());
            let hooks:Vec<_>=HOOK_EVENTS.with(|s|s.borrow().iter().map(|e|json!({"point":format!("{:?}",e.point),"action":e.action,"component":e.component,"errno":e.errno})).collect());
            let uids:Vec<_>=UID_EVENTS.with(|s|s.borrow().iter().map(|e|json!({"actual_real":e.actual_real,"actual_effective":e.actual_effective,"selected_real":e.selected_real,"selected_effective":e.selected_effective,"injected":e.injected})).collect());
            let layouts: Vec<_> = LAYOUT_EVENTS.with(|s| {
                s.borrow()
                    .iter()
                    .map(|e| json!({"members":e.members,"injected":e.injected}))
                    .collect()
            });
            let fixture_io = FIXTURE_IO.with(|s| s.borrow().clone());
            println!(
                "RSIA_AG174B_H2 {}",
                json!({"case":case,"selector":{"template":"F","layer_count":3,"key_type":"B","placement":"E","seed":0},"expected":expected,"actual_uid":rustix::process::getuid().as_raw(),"actual_euid":rustix::process::geteuid().as_raw(),"actual_pid":std::process::id(),"production_io":io,"fd_samples":fd,"buffer_samples":buffers,"memory_samples":memory,"fixture_hook_events":hooks,"uid_events":uids,"layout_events":layouts,"fixture_io":fixture_io,"claim":"exact software case; injected actions explicitly identified; no close errno or global sandbox claim"})
            );
        }
        fn isolated(name: &str, cases: &[&str], run: fn(&str)) {
            let prefix = format!("{name}::");
            if let Ok(value) = std::env::var(CHILD) {
                let case = value
                    .strip_prefix(&prefix)
                    .expect("exact child test identity");
                assert!(
                    cases.contains(&case),
                    "only explicitly selected subcase may execute"
                );
                run(case);
                println!("RSIA_AG174B_H2_CHILD_FINISHED {value}");
                return;
            }
            for case in cases {
                let identity = format!("{name}::{case}");
                let output =
                    Command::new(std::env::current_exe().expect("actual fixed-H test binary"))
                        .args(["--exact", name, "--nocapture"])
                        .env(CHILD, &identity)
                        .env_remove("RUST_MIN_STACK")
                        .env_remove("RUST_TEST_THREADS")
                        .output()
                        .expect("launch exact current test binary");
                let stdout = String::from_utf8(output.stdout).expect("child stdout UTF8");
                let stderr = String::from_utf8(output.stderr).expect("child stderr UTF8");
                println!(
                    "RSIA_AG174B_H2_CHILD_RESULT {}",
                    json!({"test":name,"case":case,"status":output.status.code(),"success":output.status.success(),"stdout":stdout,"stderr":stderr})
                );
                assert!(
                    output.status.success(),
                    "actual child failed: {identity}\n{stdout}\n{stderr}"
                );
                assert!(
                    stdout.contains(&format!("RSIA_AG174B_H2_CHILD_FINISHED {identity}")),
                    "complete assertion tail not reached"
                );
                assert!(
                    stdout.contains("test result: ok. 1 passed; 0 failed; 0 ignored;"),
                    "exact child must actually run one test"
                );
            }
        }
        fn expected_files(workspace: &OwnedConfigWorkspace) -> Vec<(String, Vec<u8>, (u64, u64))> {
            let state = workspace.state.as_ref().expect("real created state");
            state
                .members
                .iter()
                .map(|m| {
                    let source = &state.case.files()[m.source];
                    let reader = m.read.as_ref().expect("real preopened reader");
                    let id = m.identity.expect("real creation identity");
                    assert_eq!(identity(&reader.metadata().unwrap()), id);
                    (
                        source.logical_id().into(),
                        source.raw_utf8().as_bytes().to_vec(),
                        (id.dev, id.ino),
                    )
                })
                .collect()
        }
        fn assert_phase_reads(
            rows: &[IoEvent],
            phase: Phase,
            expected: &[(String, Vec<u8>, (u64, u64))],
        ) {
            for (logical, origin, id) in expected {
                let reads: Vec<_> = rows
                    .iter()
                    .filter(|e| {
                        e.phase == phase && e.operation == "read" && e.component == *logical
                    })
                    .collect();
                assert!(
                    !reads.is_empty(),
                    "missing actual {phase:?} reads for {logical}"
                );
                let mut bytes = Vec::new();
                for event in &reads {
                    assert_eq!(event.fd, Some(*id));
                    if !event.injected && !event.bytes.is_empty() {
                        assert_eq!(event.offset, Some(bytes.len() as u64));
                        bytes.extend_from_slice(&event.bytes);
                    }
                }
                assert_eq!(
                    &bytes, origin,
                    "complete actual FD bytes must equal the generator origin"
                );
                let eof = reads.last().unwrap();
                assert!(
                    !eof.injected
                        && eof.bytes.is_empty()
                        && eof.offset == Some(origin.len() as u64)
                        && eof.requested == 1,
                    "normal EOF must be a real one-byte probe at original length"
                );
                assert!(reads.iter().map(|e| e.bytes.len()).sum::<usize>() <= origin.len() + 1);
            }
        }
        fn complete_health() {
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
            assert_ne!(
                rustix::process::getuid().as_raw(),
                0,
                "H2 health requires the admitted non-root UID"
            );
            assert_eq!(rustix::process::getuid(), rustix::process::geteuid());
            let mut workspace = OwnedConfigWorkspace::create(generated())
                .expect("legitimate actual producer health");
            let expected = expected_files(&workspace);
            let before = events().len();
            assert!(matches!(
                OwnedConfigWorkspace::create(generated()),
                Err(OwnedWorkspaceError::Active)
            ));
            assert_eq!(
                events().len(),
                before,
                "active refusal must precede extra production open/read"
            );
            let snapshot = workspace
                .read_once()
                .expect("actual original-FD read health");
            assert_eq!(snapshot.files().len(), 3);
            for (file, (logical, bytes, _)) in snapshot.files().iter().zip(&expected) {
                assert_eq!(file.logical_id(), logical.as_str());
                assert_eq!(file.bytes(), bytes.as_slice());
                assert_eq!(file.format(), "json");
            }
            assert_phase_reads(&events(), Phase::Seal, &expected);
            assert_phase_reads(&events(), Phase::Execute, &expected);
            assert!(matches!(
                workspace.read_once(),
                Err(OwnedWorkspaceError::AlreadyRead)
            ));
            workspace.finish().expect("normal explicit finish");
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
            let mut next = OwnedConfigWorkspace::create(generated())
                .expect("next real workspace after finish");
            next.read_once().expect("next read");
            next.finish().expect("next finish");
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
            report(
                "healthy_complete_chain",
                json!({"real_create_seal_read_finish":true,"next_workspace":true}),
            );
        }
        fn fixture_writer(workspace: &OwnedConfigWorkspace, index: usize) -> File {
            let state = workspace.state.as_ref().unwrap();
            let member = &state.members[index];
            let reader = member.read.as_ref().unwrap();
            rfs::fchmod(reader, Mode::RUSR | Mode::WUSR).expect("fixture enables own write-open");
            let writer = File::from(
                rfs::openat(
                    state.directory(member.parent).unwrap(),
                    member.name.as_str(),
                    OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .expect("fixture opens same owned inode"),
            );
            rfs::fchmod(&writer, Mode::RUSR).expect("restore sealed mode before target mutation");
            assert_eq!(file_id(&writer), file_id(reader));
            fixture(
                "owned_writer_open_then_mode_restored",
                json!({"fd":file_id(&writer),"sealed_mode":256}),
            );
            writer
        }
        fn no_external_reads(rows: &[IoEvent], sentinel: (u64, u64)) {
            assert!(
                rows.iter().all(|e| e.fd != Some(sentinel)),
                "production must never target the actual external sentinel inode"
            );
            assert!(
                rows.iter().all(|e| e.operation != "open"),
                "execute interval must not reopen the replaced path"
            );
        }
        fn first_root_path(workspace: &OwnedConfigWorkspace) -> PathBuf {
            Path::new(if cfg!(target_os = "macos") {
                "/private/tmp"
            } else {
                "/tmp"
            })
            .join(&workspace.state.as_ref().unwrap().directories[0].name)
        }
        fn remove_fixture_file(parent: &File, name: &str) {
            let st = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
                .expect("verify own extra before unlink");
            fixture(
                "unlink_own_extra",
                json!({"name":name,"dev":st.st_dev,"ino":st.st_ino,"uid":st.st_uid}),
            );
            rfs::unlinkat(parent, name, AtFlags::empty()).expect("remove only own extra");
        }
        #[derive(Debug)]
        struct RootInfo {
            path: PathBuf,
            id: (u64, u64),
        }
        fn root_from_events(rows: &[IoEvent]) -> RootInfo {
            let event = rows
                .iter()
                .find(|e| {
                    e.phase == Phase::Create
                        && e.operation == "open"
                        && e.completed
                        && e.fd.is_some()
                        && e.component.starts_with("rsia-ctw-v1-")
                })
                .expect("real successful root open");
            RootInfo {
                path: Path::new(if cfg!(target_os = "macos") {
                    "/private/tmp"
                } else {
                    "/tmp"
                })
                .join(&event.component),
                id: event.fd.unwrap(),
            }
        }
        fn fixture_cleanup(root: &RootInfo) {
            let m = std::fs::symlink_metadata(&root.path).expect("actual residual root");
            assert!(m.is_dir());
            assert_eq!((m.dev(), m.ino()), root.id);
            assert_eq!(m.uid(), rustix::process::getuid().as_raw());
            // A closed list of names created in this case, never a directory walk.
            for name in [
                "override/config.json",
                "project/config.json",
                "defaults/base.json",
                "config/unsupported.toml",
                "override/h2-displaced-original",
                "project/h2-displaced-original",
                "defaults/h2-displaced-original",
                "h2-unknown",
            ] {
                let path = root.path.join(name);
                match std::fs::symlink_metadata(&path) {
                    Ok(m) => {
                        assert!(!m.is_dir());
                        assert_eq!(m.uid(), rustix::process::getuid().as_raw());
                        fixture(
                            "remove_own_known_residual",
                            json!({"relative":name,"dev":m.dev(),"ino":m.ino(),"mode":m.mode()}),
                        );
                        std::fs::remove_file(path)
                            .expect("fixture removes its explicit known member only");
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => panic!("residual metadata error: {e}"),
                }
            }
            for name in ["override", "project", "defaults", "config"] {
                let path = root.path.join(name);
                match std::fs::symlink_metadata(&path) {
                    Ok(m) => {
                        assert!(m.is_dir());
                        assert_eq!(m.uid(), rustix::process::getuid().as_raw());
                        fixture(
                            "remove_own_known_empty_directory",
                            json!({"relative":name,"dev":m.dev(),"ino":m.ino()}),
                        );
                        std::fs::remove_dir(path)
                            .expect("never recursively remove an unknown residual");
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => panic!("residual directory error: {e}"),
                }
            }
            let m = std::fs::symlink_metadata(&root.path).unwrap();
            assert_eq!((m.dev(), m.ino()), root.id);
            std::fs::remove_dir(&root.path).expect("fixture final empty known root");
            fixture(
                "remove_own_empty_root",
                json!({"dev":root.id.0,"ino":root.id.1}),
            );
        }
        fn cleanup_zero() {
            FD_SAMPLES.with(|s| {
                let s = s.borrow();
                let last = s.last().expect("actual cleanup sample");
                assert_eq!(last.phase, Phase::Cleanup);
                assert_eq!(
                    (last.directories, last.reads, last.writes, last.total),
                    (0, 0, 0, 0)
                );
            });
        }
        fn external() -> (tempfile::TempDir, PathBuf, (u64, u64)) {
            let dir = tempfile::tempdir().expect("own external fixture directory");
            let path = dir.path().join("sentinel");
            std::fs::write(&path, SENTINEL).expect("own sentinel write");
            let file = File::open(&path).unwrap();
            let id = file_id(&file);
            fixture(
                "create_external_sentinel",
                json!({"id":id,"bytes":SENTINEL}),
            );
            (dir, path, id)
        }
        fn sentinel_unchanged(path: &Path, id: (u64, u64)) {
            let actual = std::fs::read(path).expect("fixture alone reads its external sentinel");
            assert_eq!(actual, SENTINEL);
            fixture(
                "fixture_read_external_sentinel_after_product",
                json!({"id":id,"actual_bytes":actual}),
            );
        }
        fn assert_poisoned_without_new_os() {
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 2);
            let before = events().len();
            let counter = ROOT_COUNTER.load(Ordering::SeqCst);
            assert!(matches!(
                OwnedConfigWorkspace::create(generated()),
                Err(OwnedWorkspaceError::Poisoned)
            ));
            assert_eq!(events().len(), before);
            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), counter);
        }
        fn reject_layout_at_actual_create(layout: &[FileLayout<'_>]) -> OwnedWorkspaceError {
            let supplied: Vec<_> = layout
                .iter()
                .map(|f| TestLayout {
                    logical_id: f.logical_id.into(),
                    components: f.components.iter().map(|c| (*c).into()).collect(),
                    source_path: f.source_path.into(),
                    bytes: f.bytes,
                })
                .collect();
            let descriptor:Vec<_>=layout.iter().map(|f|json!({"logical_id":f.logical_id,"components":f.components,"source_path":f.source_path,"bytes":f.bytes})).collect();
            H2_LAYOUT_OVERRIDE.with(|v| {
                assert!(v.borrow().is_none());
                *v.borrow_mut() = Some(supplied);
            });
            let counter = ROOT_COUNTER.load(Ordering::SeqCst);
            let before = events().len();
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
            let error = OwnedConfigWorkspace::create(generated())
                .expect_err("actual producer must reject injected invalid plan before OS creation");
            H2_LAYOUT_OVERRIDE.with(|v| {
                assert!(
                    v.borrow().is_none(),
                    "real production preflight consumed one-shot layout"
                )
            });
            LAYOUT_EVENTS.with(|v| {
                let v = v.borrow();
                let last = v.last().unwrap();
                assert!(last.injected);
                assert_eq!(last.members, layout.len());
            });
            assert_eq!(events().len(), before);
            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), counter);
            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
            fixture(
                "negative_plan_at_actual_create_preflight",
                json!({"layout":descriptor,"error":format!("{error:?}"),"production_os_events_added":0,"injected":true,"actual_16_file_FS_creation_claim":false}),
            );
            error
        }

        #[test]
        fn uid_and_parent_policy_are_fixed() {
            isolated(
                "owned_config_workspace::tests::uid_and_parent_policy_are_fixed",
                &[
                    "real_uid_and_fixed_parent",
                    "zero_uid_injected",
                    "mismatched_uid_injected",
                    "insecure_owned_ancestor",
                ],
                |case| {
                    complete_health();
                    match case {
                        "real_uid_and_fixed_parent" => {
                            let start = events().len();
                            let workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                            let state = workspace.state.as_ref().unwrap();
                            let parent = state.parent.as_ref().unwrap().metadata().unwrap();
                            assert_eq!(parent.uid(), 0);
                            assert_ne!(parent.mode() & 0o1000, 0);
                            let root = state.directory(0).unwrap().metadata().unwrap();
                            assert_eq!(root.uid(), rustix::process::getuid().as_raw());
                            assert_eq!(root.mode() & 0o777, 0o700);
                            let rows = events();
                            let components: Vec<_> = rows[start..]
                                .iter()
                                .filter(|e| {
                                    e.phase == Phase::Parent
                                        && e.operation == "open"
                                        && !e.completed
                                })
                                .map(|e| e.component.as_str())
                                .collect();
                            assert_eq!(
                                components,
                                if cfg!(target_os = "macos") {
                                    vec!["/", "private", "tmp"]
                                } else {
                                    vec!["/", "tmp"]
                                }
                            );
                            UID_EVENTS.with(|e| {
                                assert!(e.borrow().iter().any(|u| !u.injected
                                    && u.actual_real == u.selected_real
                                    && u.actual_effective == u.selected_effective))
                            });
                            workspace.finish().unwrap();
                            cleanup_zero();
                        }
                        "zero_uid_injected" | "mismatched_uid_injected" => {
                            let uid = rustix::process::getuid().as_raw();
                            let selected = if case == "zero_uid_injected" {
                                (0, 0)
                            } else {
                                (uid, uid.checked_add(1).unwrap())
                            };
                            H2_HOOKS.with(|h| {
                                h.borrow_mut().push_back((
                                    HookPoint::UidPolicy,
                                    TestAction::Uids {
                                        real: selected.0,
                                        effective: selected.1,
                                    },
                                ))
                            });
                            let before = events().len();
                            let counter = ROOT_COUNTER.load(Ordering::SeqCst);
                            assert!(matches!(
                                OwnedConfigWorkspace::create(generated()),
                                Err(OwnedWorkspaceError::Unsupported(
                                    "real/effective UID must match and be nonzero"
                                ))
                            ));
                            assert_eq!(events().len(), before);
                            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), counter);
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            UID_EVENTS.with(|e| {
                                let e = e.borrow();
                                let u = e.last().unwrap();
                                assert!(u.injected);
                                assert_eq!((u.selected_real, u.selected_effective), selected);
                                assert_eq!(
                                    (u.actual_real, u.actual_effective),
                                    (uid, rustix::process::geteuid().as_raw())
                                );
                            });
                        }
                        "insecure_owned_ancestor" => {
                            let dir = tempfile::tempdir().unwrap();
                            std::fs::set_permissions(
                                dir.path(),
                                std::fs::Permissions::from_mode(0o700),
                            )
                            .unwrap();
                            let fd = File::open(dir.path()).unwrap();
                            check_ancestor(&fd, rustix::process::getuid().as_raw(), false)
                                .expect("healthy actual owned ancestor");
                            std::fs::set_permissions(
                                dir.path(),
                                std::fs::Permissions::from_mode(0o777),
                            )
                            .unwrap();
                            assert!(matches!(
                                check_ancestor(&fd, rustix::process::getuid().as_raw(), false),
                                Err(OwnedWorkspaceError::Identity(
                                    "insecure fixed parent or ancestor"
                                ))
                            ));
                            fixture(
                                "single_actual_ancestor_mode_mutation",
                                json!({"id":file_id(&fd),"before":448,"after":511}),
                            );
                            std::fs::set_permissions(
                                dir.path(),
                                std::fs::Permissions::from_mode(0o700),
                            )
                            .unwrap();
                        }
                        _ => unreachable!(),
                    }
                    report(
                        case,
                        json!({"policy_fixed":true,"injected_uid_is_not_a_real_root_run":true}),
                    );
                },
            );
        }

        #[test]
        fn symlink_hardlink_special_objects_are_refused() {
            isolated(
                "owned_config_workspace::tests::symlink_hardlink_special_objects_are_refused",
                &[
                    "symlink_before_create",
                    "symlink_before_preopen",
                    "hardlink_before_create",
                    "hardlink_after_seal",
                    "fifo_before_create",
                    "fifo_before_preopen",
                    "directory_is_not_regular",
                ],
                |case| {
                    complete_health();
                    let (_sentinel_dir, sentinel, sentinel_id) = external();
                    if case == "hardlink_after_seal" || case == "directory_is_not_regular" {
                        let mut workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                        if case == "hardlink_after_seal" {
                            let state = workspace.state.as_ref().unwrap();
                            let member = &state.members[0];
                            let parent = state.directory(member.parent).unwrap();
                            rfs::linkat(
                                parent,
                                member.name.as_str(),
                                parent,
                                "h2-alias",
                                AtFlags::empty(),
                            )
                            .unwrap();
                            fixture(
                                "hardlink_actual_original",
                                json!({"id":file_id(member.read.as_ref().unwrap()),"nlink":2}),
                            );
                            assert!(matches!(
                                workspace.read_once(),
                                Err(OwnedWorkspaceError::Identity(
                                    "original regular identity/link/mode/length differs"
                                ))
                            ));
                            let state = workspace.state.as_ref().unwrap();
                            remove_fixture_file(
                                state.directory(state.members[0].parent).unwrap(),
                                "h2-alias",
                            );
                        } else {
                            let state = workspace.state.as_ref().unwrap();
                            let directory = state.directory(0).unwrap();
                            let id = identity(&directory.metadata().unwrap());
                            assert!(matches!(
                                check_regular(directory, id, state.uid, 0, 0o700),
                                Err(OwnedWorkspaceError::Identity(
                                    "original regular identity/link/mode/length differs"
                                ))
                            ));
                            fixture(
                                "actual_product_regular_guard_on_real_owned_directory_fd",
                                json!({"id":(id.dev,id.ino),"not_a_fake_state":true}),
                            );
                        }
                        workspace.finish().unwrap();
                        cleanup_zero();
                    } else {
                        let point = if case.ends_with("preopen") {
                            HookPoint::BeforePreopen
                        } else {
                            HookPoint::BeforeMemberCreate
                        };
                        let action = if case.starts_with("symlink") {
                            TestAction::Symlink(sentinel.clone())
                        } else if case.starts_with("hardlink") {
                            TestAction::Hardlink(sentinel.clone())
                        } else {
                            TestAction::Fifo
                        };
                        H2_HOOKS.with(|h| h.borrow_mut().push_back((point, action)));
                        let start = events().len();
                        let result = OwnedConfigWorkspace::create(generated());
                        assert!(
                            matches!(
                                &result,
                                Err(OwnedWorkspaceError::ExecutionAndCleanup { .. })
                            ),
                            "real injected object must be refused and its unknown identity must remain a cleanup error: {result:?}"
                        );
                        let err = result.unwrap_err();
                        let OwnedWorkspaceError::ExecutionAndCleanup { execution, cleanup } = err
                        else {
                            unreachable!()
                        };
                        if case.ends_with("create") {
                            assert!(
                                matches!(*execution,OwnedWorkspaceError::Io{source,..} if source.raw_os_error()==Some(rustix::io::Errno::EXIST.raw_os_error()))
                            );
                        } else if case.starts_with("symlink") {
                            assert!(
                                matches!(*execution,OwnedWorkspaceError::Io{operation:"openat",source} if source.raw_os_error()==Some(rustix::io::Errno::LOOP.raw_os_error()))
                            );
                        } else {
                            assert!(matches!(
                                *execution,
                                OwnedWorkspaceError::Identity(
                                    "original regular identity/link/mode/length differs"
                                )
                            ));
                        }
                        assert!(matches!(*cleanup, OwnedWorkspaceError::Cleanup(_)));
                        let rows = events();
                        assert!(
                            rows[start..].iter().all(|e| e.fd != Some(sentinel_id)),
                            "no product open/read may target the external sentinel"
                        );
                        assert!(
                            rows[start..]
                                .iter()
                                .all(|e| !(e.operation == "read" && e.phase == Phase::Seal)),
                            "refusal must precede content reads on an injected object"
                        );
                        cleanup_zero();
                        assert_poisoned_without_new_os();
                        let root = root_from_events(&rows[start..]);
                        fixture_cleanup(&root);
                        assert_poisoned_without_new_os();
                    }
                    sentinel_unchanged(&sentinel, sentinel_id);
                    report(
                        case,
                        json!({"actual_os_object_refused":true,"external_sentinel_never_product_input":true}),
                    );
                },
            );
        }

        #[test]
        fn preopened_identity_survives_or_rejects_path_replacement() {
            isolated(
                "owned_config_workspace::tests::preopened_identity_survives_or_rejects_path_replacement",
                &[
                    "rename_only",
                    "rename_then_external_symlink",
                    "hook_after_preopen_external_symlink",
                ],
                |case| {
                    complete_health();
                    let (_sentinel_dir, sentinel, sentinel_id) = external();
                    if case == "hook_after_preopen_external_symlink" {
                        H2_HOOKS.with(|h| {
                            h.borrow_mut().push_back((
                                HookPoint::AfterPreopen,
                                TestAction::Symlink(sentinel.clone()),
                            ))
                        });
                    }
                    let mut workspace = OwnedConfigWorkspace::create(generated())
                        .expect("original preopened FD remains bound to the created inode");
                    let expected = expected_files(&workspace);
                    if case != "hook_after_preopen_external_symlink" {
                        let state = workspace.state.as_ref().unwrap();
                        let member = &state.members[0];
                        let parent = state.directory(member.parent).unwrap();
                        rfs::renameat(
                            parent,
                            member.name.as_str(),
                            parent,
                            "h2-displaced-original",
                        )
                        .unwrap();
                        if case == "rename_then_external_symlink" {
                            rfs::symlinkat(sentinel.as_path(), parent, member.name.as_str())
                                .unwrap();
                        }
                        fixture(
                            "rename_known_original_and_optional_external_symlink",
                            json!({"original":expected[0].2,"external":sentinel_id,"relative":member.name}),
                        );
                    }
                    let start = events().len();
                    let snapshot = workspace
                        .read_once()
                        .expect("path changes must not change the original FD contents");
                    let rows = events();
                    no_external_reads(&rows[start..], sentinel_id);
                    assert_phase_reads(&rows[start..], Phase::Execute, &expected);
                    for (f, (_, bytes, _)) in snapshot.files().iter().zip(&expected) {
                        assert_eq!(f.bytes(), bytes.as_slice());
                    }
                    let state = workspace.state.as_ref().unwrap();
                    let member = &state.members[0];
                    let parent = state.directory(member.parent).unwrap();
                    if case != "rename_only" {
                        remove_fixture_file(parent, &member.name);
                    }
                    rfs::renameat(
                        parent,
                        "h2-displaced-original",
                        parent,
                        member.name.as_str(),
                    )
                    .unwrap();
                    fixture(
                        "restore_own_original_before_finish",
                        json!({"id":expected[0].2}),
                    );
                    workspace.finish().unwrap();
                    cleanup_zero();
                    assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                    sentinel_unchanged(&sentinel, sentinel_id);
                    report(
                        case,
                        json!({"actual_old_inode_read":true,"external_production_opens_and_reads":0,"fixture_restoration_before_finish":true}),
                    );
                },
            );
        }
        #[test]
        fn truncation_growth_and_origin_mismatch_are_detected() {
            isolated(
                "owned_config_workspace::tests::truncation_growth_and_origin_mismatch_are_detected",
                &[
                    "truncated",
                    "grown",
                    "one_byte_changed",
                    "early_eof_injected",
                    "interrupted_8_payload",
                    "interrupted_9_payload",
                    "interrupted_8_eof",
                    "interrupted_9_eof",
                    "interrupted_shared_limit",
                    "actual_eof_growth_probe",
                ],
                |case| {
                    complete_health();
                    let mut workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                    let expected = expected_files(&workspace);
                    let start = events().len();
                    match case {
                        "truncated" | "grown" | "one_byte_changed" => {
                            let writer = fixture_writer(&workspace, 0);
                            let len = expected[0].1.len();
                            if case == "truncated" {
                                writer.set_len((len - 1) as u64).unwrap();
                            } else if case == "grown" {
                                writer.write_all_at(b"!", len as u64).unwrap();
                            } else {
                                writer.write_all_at(&[expected[0].1[0] ^ 1], 0).unwrap();
                            }
                            writer.sync_all().unwrap();
                            fixture(
                                "single_owned_inode_content_mutation",
                                json!({"kind":case,"id":file_id(&writer),"original":expected[0].1,"actual_len":writer.metadata().unwrap().len(),"sealed_mode":writer.metadata().unwrap().mode()&0o777}),
                            );
                            let result = workspace.read_once();
                            if case == "one_byte_changed" {
                                assert!(matches!(result, Err(OwnedWorkspaceError::OriginMismatch)));
                                assert!(events()[start..].iter().any(|e| e.phase
                                    == Phase::Execute
                                    && e.operation == "read"
                                    && !e.injected
                                    && e.fd == Some(expected[0].2)
                                    && !e.bytes.is_empty()));
                            } else {
                                assert!(matches!(
                                    result,
                                    Err(OwnedWorkspaceError::Identity(
                                        "original regular identity/link/mode/length differs"
                                    ))
                                ));
                                assert!(
                                    events()[start..].iter().all(|e| e.phase != Phase::Execute)
                                );
                            }
                            drop(writer);
                        }
                        "actual_eof_growth_probe" => {
                            let state = workspace.state.as_ref().unwrap();
                            let reader = state.members[0].read.as_ref().unwrap();
                            assert!(matches!(
                                bounded_read(
                                    Phase::Execute,
                                    reader,
                                    &expected[0].0,
                                    &expected[0].1[..expected[0].1.len() - 1]
                                ),
                                Err(OwnedWorkspaceError::Identity("original fd grew"))
                            ));
                            let rows = events();
                            let reads: Vec<_> = rows[start..]
                                .iter()
                                .filter(|e| e.operation == "read")
                                .collect();
                            assert_eq!(reads.len(), 2);
                            assert_eq!(reads[0].bytes, expected[0].1[..expected[0].1.len() - 1]);
                            assert_eq!(reads[1].requested, 1);
                            assert_eq!(reads[1].offset, Some((expected[0].1.len() - 1) as u64));
                            assert_eq!(reads[1].bytes.len(), 1);
                            assert!(
                                reads
                                    .iter()
                                    .all(|e| !e.injected && e.fd == Some(expected[0].2))
                            );
                            fixture(
                                "direct_real_bounded_read_boundary",
                                json!({"expected_length":expected[0].1.len()-1,"actual_inode_length":expected[0].1.len(),"claim":"EOF growth guard on actual original FD; separate from the real file-growth snapshot case"}),
                            );
                        }
                        _ => {
                            let (faults, ok, errno_stage) = match case {
                                "early_eof_injected" => (vec![ReadFault::EarlyEof], false, "early"),
                                "interrupted_8_payload" => {
                                    (vec![ReadFault::Interrupted; 8], true, "payload")
                                }
                                "interrupted_9_payload" => {
                                    (vec![ReadFault::Interrupted; 9], false, "payload")
                                }
                                "interrupted_8_eof" => {
                                    let mut v = vec![ReadFault::Actual];
                                    v.extend([ReadFault::Interrupted; 8]);
                                    (v, true, "EOF")
                                }
                                "interrupted_9_eof" => {
                                    let mut v = vec![ReadFault::Actual];
                                    v.extend([ReadFault::Interrupted; 9]);
                                    (v, false, "EOF")
                                }
                                "interrupted_shared_limit" => {
                                    let mut v = vec![ReadFault::Interrupted; 4];
                                    v.push(ReadFault::Actual);
                                    v.extend([ReadFault::Interrupted; 5]);
                                    (v, false, "EOF")
                                }
                                _ => unreachable!(),
                            };
                            let requested: Vec<_> =
                                faults.iter().map(|f| format!("{f:?}")).collect();
                            READ_FAULTS.with(|f| f.borrow_mut().extend(faults));
                            let result = workspace.read_once();
                            if ok {
                                let snapshot =
                                    result.expect("exactly eight interruptions remain recoverable");
                                assert_eq!(snapshot.files()[0].bytes(), expected[0].1.as_slice());
                                assert_phase_reads(&events()[start..], Phase::Execute, &expected);
                            } else if errno_stage == "early" {
                                assert!(matches!(
                                    result,
                                    Err(OwnedWorkspaceError::Identity("truncated original fd"))
                                ));
                            } else {
                                assert!(
                                    matches!(result,Err(OwnedWorkspaceError::Io {operation,source}) if operation==(if errno_stage=="EOF" {"read_at EOF"} else {"read_at"}) && source.kind()==io::ErrorKind::Interrupted && source.raw_os_error()==Some(rustix::io::Errno::INTR.raw_os_error()))
                                );
                            }
                            let rows = events();
                            let injected = rows[start..]
                                .iter()
                                .filter(|e| e.operation == "read" && e.injected)
                                .count();
                            assert_eq!(
                                injected,
                                if errno_stage == "early" {
                                    1
                                } else if ok {
                                    8
                                } else {
                                    9
                                }
                            );
                            READ_FAULTS.with(|f| assert!(f.borrow().is_empty()));
                            fixture(
                                "declared_read_fault_queue",
                                json!({"faults":requested,"injected_events":injected,"actual_reads_remain_distinguished":true}),
                            );
                        }
                    }
                    assert!(events()[start..].iter().all(|e| !(e.phase == Phase::Execute
                        && matches!(e.operation, "open" | "write" | "unlink"))));
                    workspace.finish().unwrap();
                    cleanup_zero();
                    assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                    report(
                        case,
                        json!({"single_target_mutation":true,"precise_refusal_or_eight_interrupt_recovery":true,"explicit_finish":true}),
                    );
                },
            );
        }

        #[test]
        fn resource_preflight_and_each_stage_fd_peak_are_bounded() {
            isolated(
                "owned_config_workspace::tests::resource_preflight_and_each_stage_fd_peak_are_bounded",
                &[
                    "real_fd_buffer_memory_ledger",
                    "layout_regular_count",
                    "layout_file_bytes",
                    "layout_total_bytes",
                    "layout_depth",
                    "layout_bad_components",
                    "layout_duplicate",
                    "layout_origin_mismatch",
                    "layout_prefix_conflict",
                    "layout_16_boundary",
                    "name_counter_exhaustion",
                    "concurrent_names",
                    "eight_eexist_unchanged",
                    "mkdir_non_eexist",
                ],
                |case| {
                    complete_health();
                    match case {
                        "real_fd_buffer_memory_ledger" => {
                            let start_fd = FD_SAMPLES.with(|s| s.borrow().len());
                            let start_mem = MEMORY_SAMPLES.with(|s| s.borrow().len());
                            let start_buf = BUFFER_SAMPLES.with(|s| s.borrow().len());
                            let mut workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                            let expected = expected_files(&workspace);
                            let snapshot = workspace.read_once().unwrap();
                            let payload: usize = expected.iter().map(|(_, b, _)| b.len()).sum();
                            assert_eq!(
                                snapshot
                                    .files()
                                    .iter()
                                    .map(|f| f.bytes().len())
                                    .sum::<usize>(),
                                payload
                            );
                            FD_SAMPLES.with(|s| {
                                let s = s.borrow();
                                let rows = &s[start_fd..];
                                assert!(rows.iter().any(|s| s.phase == Phase::Parent));
                                assert!(rows.iter().any(|s| s.phase == Phase::Create));
                                assert!(rows.iter().any(|s| s.phase == Phase::Seal));
                                assert!(rows.iter().any(|s| s.phase == Phase::Execute));
                                for f in rows {
                                    assert_eq!(f.total, f.directories + f.reads + f.writes);
                                    assert!(
                                        f.directories <= 50
                                            && f.reads <= 16
                                            && f.writes <= 1
                                            && f.total <= 72
                                    );
                                }
                                assert_eq!(rows.iter().map(|s| s.total).max(), Some(9));
                            });
                            BUFFER_SAMPLES.with(|s| {
                                let s = s.borrow();
                                for b in &s[start_buf..] {
                                    assert!(b.capacity >= b.expected_len);
                                    assert_eq!(b.eof_slot, 1);
                                    assert!(
                                        expected.iter().any(|(id, bytes, _)| id == &b.logical_id
                                            && bytes.len() == b.expected_len)
                                    );
                                }
                            });
                            MEMORY_SAMPLES.with(|s| {
                                let s = s.borrow();
                                let rows = &s[start_mem..];
                                assert!(
                                    rows.iter().all(|m| m.origin_case_heap_capacity >= payload)
                                );
                                let final_q = rows
                                    .iter()
                                    .rev()
                                    .find(|m| m.phase == Phase::Execute)
                                    .unwrap();
                                assert!(final_q.snapshot_bytes_capacity >= payload);
                                assert!(
                                    final_q.snapshot_vector_capacity
                                        >= 3 * std::mem::size_of::<SnapshotFile>()
                                );
                                assert!(
                                    final_q.query_vector_capacity >= std::mem::size_of::<&str>()
                                );
                                assert_eq!(
                                    final_q.state_inline_bytes,
                                    std::mem::size_of::<State>()
                                );
                            });
                            drop(snapshot);
                            workspace.finish().unwrap();
                            cleanup_zero();
                            let l2 = ClosedCaseSelector::Task {
                                template: Template::F,
                                layer_count: Layers::L2,
                                key_type: KeyType::B,
                                placement: Placement::E,
                                seed: Seed::Zero,
                            };
                            let start = FD_SAMPLES.with(|s| s.borrow().len());
                            let mut workspace =
                                OwnedConfigWorkspace::create(generate_case(l2).unwrap()).unwrap();
                            workspace.read_once().unwrap();
                            workspace.finish().unwrap();
                            FD_SAMPLES.with(|s| {
                                assert_eq!(
                                    s.borrow()[start..].iter().map(|s| s.total).max(),
                                    Some(7)
                                )
                            });
                            cleanup_zero();
                            fixture(
                                "resource_claim_boundary",
                                json!({"real_L3_peak":9,"real_L2_peak":7,"general_layout_peak_67_is_not_a_16_file_FS_measurement":true,"allocator_RSS_and_harness_excluded_from_owned_counts":true}),
                            );
                        }
                        "name_counter_exhaustion" => {
                            let uid = rustix::process::getuid().as_raw();
                            let pid = std::process::id();
                            let counter = AtomicU64::new(u64::MAX - 1);
                            let before = events().len();
                            assert_eq!(
                                next_root_name(uid, pid, &counter).unwrap(),
                                format!("rsia-ctw-v1-{uid}-{pid}-{}", u64::MAX - 1)
                            );
                            assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
                            assert!(matches!(
                                next_root_name(uid, pid, &counter),
                                Err(OwnedWorkspaceError::Capacity("root name counter exhausted"))
                            ));
                            assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
                            assert_eq!(events().len(), before);
                            fixture(
                                "actual_production_name_allocator_local_atomic",
                                json!({"start":u64::MAX-1,"after":u64::MAX,"wrap":false,"claim":"allocator boundary, not global counter reset or a new root"}),
                            );
                        }
                        "concurrent_names" => {
                            let uid = rustix::process::getuid().as_raw();
                            let pid = std::process::id();
                            let counter = Arc::new(AtomicU64::new(100));
                            let threads: Vec<_> = (0..4)
                                .map(|_| {
                                    let counter = Arc::clone(&counter);
                                    std::thread::spawn(move || {
                                        (0..16)
                                            .map(|_| next_root_name(uid, pid, &counter).unwrap())
                                            .collect::<Vec<_>>()
                                    })
                                })
                                .collect();
                            let names: Vec<_> = threads
                                .into_iter()
                                .flat_map(|h| h.join().unwrap())
                                .collect();
                            let unique: BTreeSet<_> = names.iter().cloned().collect();
                            assert_eq!(names.len(), 64);
                            assert_eq!(unique.len(), 64);
                            assert_eq!(counter.load(Ordering::SeqCst), 164);
                            assert_eq!(
                                unique,
                                (100..164)
                                    .map(|n| format!("rsia-ctw-v1-{uid}-{pid}-{n}"))
                                    .collect()
                            );
                            fixture(
                                "concurrent_actual_name_allocator",
                                json!({"threads":4,"calls_each":16,"start":100,"after":164,"names":names,"actual_root_creation_claim":false}),
                            );
                        }
                        "eight_eexist_unchanged" => {
                            let uid = rustix::process::getuid().as_raw();
                            let pid = std::process::id();
                            let start = ROOT_COUNTER.load(Ordering::SeqCst);
                            let parent = File::open(if cfg!(target_os = "macos") {
                                "/private/tmp"
                            } else {
                                "/tmp"
                            })
                            .unwrap();
                            let mut collisions = Vec::new();
                            for n in start..start + 8 {
                                let name = format!("rsia-ctw-v1-{uid}-{pid}-{n}");
                                rfs::mkdirat(&parent, name.as_str(), Mode::RWXU).expect(
                                    "fixture exclusively preoccupies its exact future name",
                                );
                                let directory = File::from(
                                    rfs::openat(
                                        &parent,
                                        name.as_str(),
                                        directory_flags(),
                                        Mode::empty(),
                                    )
                                    .unwrap(),
                                );
                                let marker =
                                    format!("independent-collision-{n}-untouched").into_bytes();
                                let mut writer = File::from(
                                    rfs::openat(
                                        &directory,
                                        "marker",
                                        OFlags::WRONLY
                                            | OFlags::CREATE
                                            | OFlags::EXCL
                                            | OFlags::NOFOLLOW
                                            | OFlags::CLOEXEC,
                                        Mode::RUSR | Mode::WUSR,
                                    )
                                    .unwrap(),
                                );
                                writer.write_all(&marker).unwrap();
                                writer.sync_all().unwrap();
                                drop(writer);
                                fixture(
                                    "preoccupy_own_collision",
                                    json!({"name":name,"id":file_id(&directory),"marker":marker}),
                                );
                                collisions.push((name, directory, marker));
                            }
                            let before = events().len();
                            let result = OwnedConfigWorkspace::create(generated());
                            assert!(
                                matches!(result,Err(OwnedWorkspaceError::Io {operation:"mkdirat collisions",source}) if source.raw_os_error()==Some(rustix::io::Errno::EXIST.raw_os_error()))
                            );
                            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), start + 8);
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            assert!(events()[before..].iter().all(|e| e.phase == Phase::Parent));
                            for (name, directory, marker) in collisions {
                                let st =
                                    rfs::statat(&parent, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                                        .unwrap();
                                let stat_id = stat_identity(&st);
                                assert_eq!((stat_id.dev, stat_id.ino), file_id(&directory));
                                let reader = File::from(
                                    rfs::openat(
                                        &directory,
                                        "marker",
                                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                                        Mode::empty(),
                                    )
                                    .unwrap(),
                                );
                                let mut read = vec![0; marker.len()];
                                assert_eq!(reader.read_at(&mut read, 0).unwrap(), marker.len());
                                assert_eq!(read, marker);
                                fixture(
                                    "collision_after_product_refusal",
                                    json!({"name":name,"id":file_id(&directory),"marker":read}),
                                );
                                drop(reader);
                                rfs::unlinkat(&directory, "marker", AtFlags::empty()).unwrap();
                                drop(directory);
                                rfs::unlinkat(&parent, name.as_str(), AtFlags::REMOVEDIR).unwrap();
                            }
                            cleanup_zero();
                        }
                        "mkdir_non_eexist" => {
                            H2_HOOKS.with(|h| {
                                h.borrow_mut().push_back((
                                    HookPoint::RootMkdir,
                                    TestAction::MkdirError(rustix::io::Errno::ACCESS),
                                ))
                            });
                            let counter = ROOT_COUNTER.load(Ordering::SeqCst);
                            let start = events().len();
                            assert!(
                                matches!(OwnedConfigWorkspace::create(generated()),Err(OwnedWorkspaceError::Io {operation:"injected mkdirat",source}) if source.raw_os_error()==Some(rustix::io::Errno::ACCESS.raw_os_error()))
                            );
                            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), counter + 1);
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            assert!(events()[start..].iter().all(|e| e.phase == Phase::Parent));
                            cleanup_zero();
                            fixture(
                                "fault_injected_non_eexist",
                                json!({"errno":rustix::io::Errno::ACCESS.raw_os_error(),"counter_consumed":1,"real_os_permission_denial_claim":false}),
                            );
                        }
                        _ => {
                            let names: Vec<_> = (0..17).map(|i| format!("f{i}")).collect();
                            let paths: Vec<_> =
                                names.iter().map(|n| format!("a/b/c/{n}")).collect();
                            let components: Vec<_> = names
                                .iter()
                                .map(|n| vec!["a", "b", "c", n.as_str()])
                                .collect();
                            let base: Vec<_> = (0..16)
                                .map(|i| FileLayout {
                                    logical_id: &names[i],
                                    components: &components[i],
                                    source_path: &paths[i],
                                    bytes: 0,
                                })
                                .collect();
                            let budget = preflight_layout(&base)
                                .expect("16 actual planner entries at depth four");
                            assert_eq!(
                                (
                                    budget.directories,
                                    budget.readers,
                                    budget.writers,
                                    budget.peak
                                ),
                                (50, 16, 1, 67)
                            );
                            let before = events().len();
                            let counter = ROOT_COUNTER.load(Ordering::SeqCst);
                            let outcome = match case {
                                "layout_16_boundary" => {
                                    let mut v = base.clone();
                                    for f in &mut v[..4] {
                                        f.bytes = 65_536;
                                    }
                                    let b = preflight_layout(&v).unwrap();
                                    assert_eq!(b.payload, 262_144);
                                    Ok(())
                                }
                                "layout_regular_count" => {
                                    assert!(matches!(
                                        reject_layout_at_actual_create(&[]),
                                        OwnedWorkspaceError::Capacity("regular count")
                                    ));
                                    let mut v = base.clone();
                                    v.push(FileLayout {
                                        logical_id: &names[16],
                                        components: &components[16],
                                        source_path: &paths[16],
                                        bytes: 0,
                                    });
                                    Err(reject_layout_at_actual_create(&v))
                                }
                                "layout_file_bytes" => {
                                    let mut v = base.clone();
                                    v[0].bytes = 65_537;
                                    Err(reject_layout_at_actual_create(&v))
                                }
                                "layout_total_bytes" => {
                                    let mut v = base.clone();
                                    for f in &mut v[..5] {
                                        f.bytes = 65_536;
                                    }
                                    Err(reject_layout_at_actual_create(&v))
                                }
                                "layout_depth" => {
                                    Err(reject_layout_at_actual_create(&[FileLayout {
                                        logical_id: "depth",
                                        components: &["a", "b", "c", "d", "f"],
                                        source_path: "a/b/c/d/f",
                                        bytes: 1,
                                    }]))
                                }
                                "layout_bad_components" => {
                                    for bad in [".", "..", "", "a/b", "a\\b", "a\0b"] {
                                        assert!(matches!(
                                            reject_layout_at_actual_create(&[FileLayout {
                                                logical_id: "bad",
                                                components: &[bad],
                                                source_path: bad,
                                                bytes: 1
                                            }]),
                                            OwnedWorkspaceError::WorkspaceBoundary
                                        ));
                                    }
                                    Err(OwnedWorkspaceError::WorkspaceBoundary)
                                }
                                "layout_duplicate" => {
                                    let same_path = FileLayout {
                                        logical_id: "other-logical-id",
                                        ..base[0]
                                    };
                                    assert!(matches!(
                                        reject_layout_at_actual_create(&[base[0], same_path]),
                                        OwnedWorkspaceError::Identity("duplicate member")
                                    ));
                                    Err(reject_layout_at_actual_create(&[base[0], base[0]]))
                                }
                                "layout_origin_mismatch" => {
                                    Err(reject_layout_at_actual_create(&[FileLayout {
                                        logical_id: "origin",
                                        components: &["a", "f"],
                                        source_path: "different/f",
                                        bytes: 1,
                                    }]))
                                }
                                "layout_prefix_conflict" => Err(reject_layout_at_actual_create(&[
                                    FileLayout {
                                        logical_id: "file",
                                        components: &["a"],
                                        source_path: "a",
                                        bytes: 1,
                                    },
                                    FileLayout {
                                        logical_id: "child",
                                        components: &["a", "f"],
                                        source_path: "a/f",
                                        bytes: 1,
                                    },
                                ])),
                                _ => unreachable!(),
                            };
                            match case {
                                "layout_16_boundary" => assert!(outcome.is_ok()),
                                "layout_regular_count" => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::Capacity("regular count"))
                                )),
                                "layout_file_bytes" => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::Capacity("file bytes"))
                                )),
                                "layout_total_bytes" => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::Capacity("workspace preflight"))
                                )),
                                "layout_duplicate" => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::Identity("duplicate member"))
                                )),
                                "layout_origin_mismatch" => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::OriginMismatch)
                                )),
                                _ => assert!(matches!(
                                    outcome,
                                    Err(OwnedWorkspaceError::WorkspaceBoundary)
                                )),
                            }
                            assert_eq!(events().len(), before);
                            assert_eq!(ROOT_COUNTER.load(Ordering::SeqCst), counter);
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            fixture(
                                "production_preflight_layout_boundary",
                                json!({"layout_case":case,"derived_16_boundary":{"directories":budget.directories,"readers":budget.readers,"writers":budget.writers,"peak":budget.peak},"actual_16_file_FS_creation_claim":false,"original_create_calls_this_preflight_before_any_OS_creation":true}),
                            );
                        }
                    }
                    report(
                        case,
                        json!({"real_FS_health_precedes_private_boundary_cases":true,"owned_fd_bounds":72,"real_stage_samples_separate_from_layout_arithmetic":true}),
                    );
                },
            );
        }

        struct UnwindWitness {
            case: String,
            sender: std::sync::mpsc::Sender<(u8, Vec<IoEvent>, bool)>,
        }
        impl Drop for UnwindWitness {
            fn drop(&mut self) {
                let state = WORKSPACE_STATE.load(Ordering::SeqCst);
                let zero = FD_SAMPLES.with(|s| {
                    s.borrow().last().is_some_and(|s| {
                        s.phase == Phase::Cleanup
                            && s.directories == 0
                            && s.reads == 0
                            && s.writes == 0
                            && s.total == 0
                    })
                });
                report(
                    &self.case,
                    json!({"unwind_witness_after_product_drop":true,"workspace_state":state,"actual_owned_fd_final_sample_zero":zero,"close_errno_claim":false}),
                );
                let _ = self.sender.send((state, events(), zero));
            }
        }
        fn unknown_member(workspace: &OwnedConfigWorkspace) {
            let root = workspace.state.as_ref().unwrap().directory(0).unwrap();
            let mut file = File::from(
                rfs::openat(
                    root,
                    "h2-unknown",
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .unwrap(),
            );
            file.write_all(b"fixture-owned-unknown-member").unwrap();
            file.sync_all().unwrap();
            fixture(
                "create_unknown_member",
                json!({"name":"h2-unknown","id":file_id(&file),"bytes":b"fixture-owned-unknown-member"}),
            );
        }
        #[test]
        fn partial_error_drop_unwind_and_residual_poison_are_fenced() {
            isolated(
                "owned_config_workspace::tests::partial_error_drop_unwind_and_residual_poison_are_fenced",
                &[
                    "partial_seal_read_failure",
                    "partial_after_root_injected_errno",
                    "drop_healthy",
                    "unwind_after_create",
                    "unwind_during_create",
                    "finish_unknown_member_poison",
                    "finish_identity_mismatch_poison",
                    "drop_unknown_member_poison",
                    "unwind_unknown_member_poison",
                ],
                |case| {
                    complete_health();
                    match case {
                        "partial_seal_read_failure" => {
                            READ_FAULTS.with(|f| f.borrow_mut().push_back(ReadFault::EarlyEof));
                            let start = events().len();
                            assert!(matches!(
                                OwnedConfigWorkspace::create(generated()),
                                Err(OwnedWorkspaceError::Identity("truncated original fd"))
                            ));
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            cleanup_zero();
                            let root = root_from_events(&events()[start..]);
                            assert!(!root.path.exists());
                            assert!(events()[start..].iter().any(|e| e.phase == Phase::Create
                                && e.operation == "write"
                                && !e.bytes.is_empty()));
                            assert!(events()[start..].iter().any(|e| e.phase == Phase::Seal
                                && e.operation == "read"
                                && e.injected));
                        }
                        "partial_after_root_injected_errno" => {
                            H2_HOOKS.with(|h| {
                                h.borrow_mut().push_back((
                                    HookPoint::RootCreated,
                                    TestAction::MkdirError(rustix::io::Errno::ACCESS),
                                ))
                            });
                            let start = events().len();
                            assert!(
                                matches!(OwnedConfigWorkspace::create(generated()),Err(OwnedWorkspaceError::Io{operation:"injected mkdirat",source}) if source.raw_os_error()==Some(rustix::io::Errno::ACCESS.raw_os_error()))
                            );
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            cleanup_zero();
                            let root = root_from_events(&events()[start..]);
                            assert!(!root.path.exists());
                            fixture(
                                "post_root_injected_errno",
                                json!({"actual_root_existed":true,"fault_injected":true,"real_permission_denial_claim":false}),
                            );
                        }
                        "drop_healthy" => {
                            let workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                            let root = RootInfo {
                                path: first_root_path(&workspace),
                                id: file_id(
                                    workspace.state.as_ref().unwrap().directory(0).unwrap(),
                                ),
                            };
                            drop(workspace);
                            assert!(!root.path.exists());
                            assert_eq!(WORKSPACE_STATE.load(Ordering::SeqCst), 0);
                            cleanup_zero();
                        }
                        "unwind_after_create"
                        | "unwind_during_create"
                        | "unwind_unknown_member_poison" => {
                            let label = case.to_string();
                            let mode = case.to_string();
                            let (sender, receiver) = std::sync::mpsc::channel();
                            let worker: std::thread::JoinHandle<()> = std::thread::spawn(
                                move || {
                                    let _witness = UnwindWitness {
                                        case: label,
                                        sender,
                                    };
                                    if mode == "unwind_during_create" {
                                        H2_HOOKS.with(|h| {
                                            h.borrow_mut().push_back((
                                                HookPoint::RootCreated,
                                                TestAction::Panic,
                                            ))
                                        });
                                    }
                                    let workspace = OwnedConfigWorkspace::create(generated())
                                        .expect("real creation before intentional test unwind");
                                    if mode == "unwind_unknown_member_poison" {
                                        unknown_member(&workspace);
                                    }
                                    let _owned = workspace;
                                    panic!(
                                        "H2 intentional default-stack test unwind after real creation"
                                    );
                                },
                            );
                            let payload=worker.join().expect_err("the test thread must actually unwind; no product catch is installed");
                            let message = payload
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                                .expect("intentional string panic payload");
                            assert_eq!(
                                message,
                                if case == "unwind_during_create" {
                                    "injected owned workspace creation unwind"
                                } else {
                                    "H2 intentional default-stack test unwind after real creation"
                                }
                            );
                            let (state, rows, zero) =
                                receiver.recv().expect("witness follows real product Drop");
                            assert!(
                                zero,
                                "actual worker cleanup must finish with zero owned FD sample"
                            );
                            let root = root_from_events(&rows);
                            if case == "unwind_unknown_member_poison" {
                                assert_eq!(state, 2);
                                assert!(root.path.join("h2-unknown").is_file());
                                assert_poisoned_without_new_os();
                                fixture_cleanup(&root);
                                assert_poisoned_without_new_os();
                            } else {
                                assert_eq!(state, 0);
                                assert!(!root.path.exists());
                                let mut next = OwnedConfigWorkspace::create(generated()).unwrap();
                                next.read_once().unwrap();
                                next.finish().unwrap();
                            }
                        }
                        _ => {
                            let workspace = OwnedConfigWorkspace::create(generated()).unwrap();
                            let root = RootInfo {
                                path: first_root_path(&workspace),
                                id: file_id(
                                    workspace.state.as_ref().unwrap().directory(0).unwrap(),
                                ),
                            };
                            let protected = if case == "finish_identity_mismatch_poison" {
                                let state = workspace.state.as_ref().unwrap();
                                let member = &state.members[0];
                                let parent = state.directory(member.parent).unwrap();
                                let parent_id = file_id(parent);
                                rfs::renameat(
                                    parent,
                                    member.name.as_str(),
                                    parent,
                                    "h2-displaced-original",
                                )
                                .unwrap();
                                let mut replacement = File::from(
                                    rfs::openat(
                                        parent,
                                        member.name.as_str(),
                                        OFlags::WRONLY
                                            | OFlags::CREATE
                                            | OFlags::EXCL
                                            | OFlags::NOFOLLOW
                                            | OFlags::CLOEXEC,
                                        Mode::RUSR | Mode::WUSR,
                                    )
                                    .unwrap(),
                                );
                                replacement
                                    .write_all(state.case.files()[0].raw_utf8().as_bytes())
                                    .unwrap();
                                replacement.sync_all().unwrap();
                                rfs::fchmod(&replacement, Mode::RUSR).unwrap();
                                let id = file_id(&replacement);
                                fixture(
                                    "same_name_different_real_identity",
                                    json!({"replacement":id,"original":member.identity.unwrap().ino}),
                                );
                                Some((parent_id, member.name.clone(), id))
                            } else {
                                unknown_member(&workspace);
                                None
                            };
                            let start = events().len();
                            if case == "drop_unknown_member_poison" {
                                drop(workspace);
                            } else {
                                let OwnedWorkspaceError::Cleanup(failures) =
                                    workspace.finish().expect_err(
                                        "unknown/different identity requires cleanup failure",
                                    )
                                else {
                                    panic!("exact cleanup error required");
                                };
                                if case == "finish_identity_mismatch_poison" {
                                    assert!(failures.iter().any(|e| matches!(
                                        e,
                                        OwnedWorkspaceError::Identity(
                                            "cleanup member identity differs"
                                        )
                                    )));
                                } else {
                                    assert!(failures.iter().any(|e|matches!(e,OwnedWorkspaceError::Io{operation:"cleanup unlinkat",source} if source.raw_os_error()==Some(rustix::io::Errno::NOTEMPTY.raw_os_error()))));
                                }
                            }
                            cleanup_zero();
                            assert!(root.path.is_dir());
                            assert_poisoned_without_new_os();
                            if let Some((parent, name, id)) = protected {
                                let rows = events();
                                assert!(rows[start..].iter().all(|e| !(e.phase == Phase::Cleanup
                                    && e.operation == "unlink"
                                    && e.parent == Some(parent)
                                    && e.component == name
                                    && e.errno.is_none())));
                                let m = std::fs::symlink_metadata(
                                    root.path.join("override/config.json"),
                                )
                                .unwrap();
                                assert_eq!((m.dev(), m.ino()), id);
                            } else {
                                assert_eq!(
                                    std::fs::read(root.path.join("h2-unknown")).unwrap(),
                                    b"fixture-owned-unknown-member"
                                );
                                assert!(
                                    events()[start..]
                                        .iter()
                                        .all(|e| e.component != "h2-unknown")
                                );
                            }
                            fixture_cleanup(&root);
                            assert_poisoned_without_new_os();
                        }
                    }
                    if WORKSPACE_STATE.load(Ordering::SeqCst) == 0 {
                        let mut next = OwnedConfigWorkspace::create(generated())
                            .expect("real post-error/drop next workspace");
                        next.read_once().unwrap();
                        next.finish().unwrap();
                        cleanup_zero();
                    }
                    report(
                        case,
                        json!({"actual_cleanup_or_unwind":true,"residual_poison_is_not_reset":true,"known_member_only_cleanup":true,"safe_RAII_not_close_errno_receipt":true}),
                    );
                },
            );
        }
    }
    // END RSIA AG174B INDEPENDENT H2
}
