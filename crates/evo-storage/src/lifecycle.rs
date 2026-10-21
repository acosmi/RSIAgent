//! Resumable cleanup derived from the authoritative tombstone, watermark and typed edges.

use super::{Store, internal};
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::sqlite::SqliteTransactionManager;
use sqlx::{ConnectOptions, Connection, Row, TransactionManager};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Directory authority and original-source readers are intentionally non-Serde.
// A filesystem path or a missing lock file is never a stability capability.
use crate::owned_occurrence::{PreparePreimages, collect_floor_rusqlite};
use crate::write_clock::OperationClock;
use evo_core::clock::WallClock;
use evo_core::prepare_contract::HostPrepareConfig;
use rusqlite::{Connection as ReadConnection, OpenFlags, TransactionBehavior, config::DbConfig};
use std::fs::{File, OpenOptions, TryLockError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

const WRITER_LOCK_NAME: &str = ".rsia.lock";
type DirectoryOwners = Mutex<BTreeMap<PathBuf, Weak<DirectoryWriterGuard>>>;
static DIRECTORY_OWNERS: OnceLock<DirectoryOwners> = OnceLock::new();

#[derive(Clone, Copy)]
enum AcquireMode {
    SharedStore,
    NewMount,
}

fn directory_owners() -> &'static DirectoryOwners {
    DIRECTORY_OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn data_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// A real process-owned OS writer lock. Its weak registry never extends its
/// lifetime: Store/operation/close owners retain their own Arc until native
/// shutdown. A close followed by reopen must not be prevented by a dead entry.
#[derive(Debug)]
pub struct DirectoryWriterGuard {
    directory: PathBuf,
    _directory_file: Arc<File>,
    _legacy_file: Arc<File>,
    activity: Arc<RwLock<()>>,
}

/// One writer operation's lifetime, including rollback and native close.
/// Nested work in that operation clones this capability instead of acquiring
/// the lock again. This conveys lifetime only, never role or data provenance.
#[derive(Debug, Clone)]
pub struct WriterActivity {
    _inner: Arc<WriterActivityInner>,
}

impl WriterActivity {
    pub(super) fn belongs_to(&self, owner: &Arc<DirectoryWriterGuard>) -> bool {
        Arc::ptr_eq(&self._inner._directory, owner)
    }
}

#[derive(Debug)]
struct WriterActivityInner {
    _directory: Arc<DirectoryWriterGuard>,
    _activity: OwnedRwLockReadGuard<()>,
}

#[derive(Debug)]
pub struct StableReadScope {
    _directory: Arc<DirectoryWriterGuard>,
    _activity: OwnedRwLockWriteGuard<()>,
}

impl DirectoryWriterGuard {
    /// Compatibility signature; production uses acquire_checked so Clock
    /// remains typed instead of being converted into an I/O diagnostic.
    pub fn acquire(data: &Path) -> std::io::Result<Arc<Self>> {
        Self::acquire_checked(data, &[]).map_err(|error| {
            let kind = match &error {
                Error::Conflict(_) => std::io::ErrorKind::WouldBlock,
                Error::Invalid(_) => std::io::ErrorKind::InvalidInput,
                _ => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, error)
        })
    }

    pub fn acquire_checked(data: &Path, candidates: &[HostPrepareConfig]) -> Result<Arc<Self>> {
        Self::acquire_checked_with_clock(data, candidates, &WallClock::system())
    }

    /// The trusted Store factory supplies the exact reader before opening any
    /// native handle. Neither a wire value nor an existing Store can replace it.
    pub(super) fn acquire_checked_with_clock(
        data: &Path,
        candidates: &[HostPrepareConfig],
        clock: &WallClock,
    ) -> Result<Arc<Self>> {
        Self::acquire_mode(data, candidates, AcquireMode::SharedStore, clock)
    }

    /// A distinct production mount cannot reuse another mount's authority.
    /// Stores inside that mount share the actual guard through open_locked.
    pub fn acquire_new_mount_checked(
        data: &Path,
        candidates: &[HostPrepareConfig],
    ) -> Result<Arc<Self>> {
        Self::acquire_mode(
            data,
            candidates,
            AcquireMode::NewMount,
            &WallClock::system(),
        )
    }

    fn acquire_mode(
        data: &Path,
        candidates: &[HostPrepareConfig],
        mode: AcquireMode,
        source: &WallClock,
    ) -> Result<Arc<Self>> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Err(Error::Invalid(
                "unsupported data-directory locking platform".into(),
            ));
        }
        // No mkdir or compatibility-lock create precedes this original-source
        // qualification and fresh system sample.
        readonly_wall_seconds_with_clock(data, candidates, &[], source)?;
        let parent = data_parent(data);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(internal)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(parent).map_err(internal)?;
        let directory = parent.canonicalize().map_err(internal)?;
        let mut owners = directory_owners().lock().map_err(internal)?;
        if let Some(existing) = owners.get(&directory).and_then(Weak::upgrade) {
            if matches!(mode, AcquireMode::NewMount) {
                return Err(Error::Conflict(
                    "data directory already has a mounted writer owner".into(),
                ));
            }
            if !existing.matches_data(data) {
                return Err(Error::ClockRegressed);
            }
            drop(owners);
            // An existing owner may have active writers. Use the normal
            // activity-aware reader, never manufacture a stable heap scope.
            readonly_wall_seconds_with_clock(data, candidates, &[], source)?;
            return Ok(existing);
        }
        let directory_file = Arc::new(File::open(&directory).map_err(internal)?);
        try_exclusive(&directory_file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Conflict("data directory is already locked by another RSIA process".into())
            } else {
                internal(error)
            }
        })?;
        // Retain an existing compatibility lock before treating the original
        // source as stable. Opening it never creates a directory member.
        let existing_legacy = existing_legacy_lock(&directory, true)?.map(Arc::new);
        // This private capability can only be minted after both real OS lock
        // checks. It is neither registered nor returned to callers.
        let source_scope = LockedOriginalSource {
            directory: &directory,
            file: &directory_file,
            legacy: existing_legacy.as_ref(),
        };
        readonly_wall_seconds_scoped(data, candidates, &[], Some(&source_scope), source)?;
        // Full floor + fresh system wall time were just rechecked under that
        // same lock, before the first compatibility-lock member is created.
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let legacy_file = match existing_legacy {
            Some(file) => file,
            None => {
                let file = options
                    .open(directory.join(WRITER_LOCK_NAME))
                    .map_err(internal)?;
                try_exclusive(&file).map_err(lock_error)?;
                Arc::new(file)
            }
        };
        if !same_directory(&directory_file, &directory).map_err(internal)?
            || !same_regular_file(&legacy_file, &directory.join(WRITER_LOCK_NAME))
                .map_err(internal)?
        {
            return Err(Error::ClockRegressed);
        }
        let guard = Arc::new(Self {
            directory: directory.clone(),
            _directory_file: directory_file,
            _legacy_file: legacy_file,
            activity: Arc::new(RwLock::new(())),
        });
        owners.insert(directory, Arc::downgrade(&guard));
        Ok(guard)
    }

    pub fn matches_data(&self, data: &Path) -> bool {
        data_parent(data).canonicalize().is_ok_and(|directory| {
            directory == self.directory
                && same_directory(&self._directory_file, &directory).unwrap_or(false)
                && same_regular_file(&self._legacy_file, &directory.join(WRITER_LOCK_NAME))
                    .unwrap_or(false)
        })
    }

    pub async fn write_scope(self: &Arc<Self>) -> WriterActivity {
        let activity = Arc::clone(&self.activity).read_owned().await;
        WriterActivity {
            _inner: Arc::new(WriterActivityInner {
                _directory: Arc::clone(self),
                _activity: activity,
            }),
        }
    }

    pub async fn stable_read_scope(self: &Arc<Self>) -> StableReadScope {
        let activity = Arc::clone(&self.activity).write_owned().await;
        StableReadScope {
            _directory: Arc::clone(self),
            _activity: activity,
        }
    }

    pub fn try_stable_read_scope(self: &Arc<Self>) -> Option<StableReadScope> {
        Arc::clone(&self.activity)
            .try_write_owned()
            .ok()
            .map(|activity| StableReadScope {
                _directory: Arc::clone(self),
                _activity: activity,
            })
    }
}

fn try_exclusive(file: &File) -> std::io::Result<()> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "data directory is held by a live writer",
        )),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn lock_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Error::Conflict("data directory is already locked by another RSIA process".into())
    } else {
        internal(error)
    }
}

fn existing_legacy_lock(directory: &Path, exclusive: bool) -> Result<Option<File>> {
    let path = directory.join(WRITER_LOCK_NAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(Error::ClockRegressed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::ClockRegressed),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options.open(&path).map_err(|_| Error::ClockRegressed)?;
    if exclusive {
        try_exclusive(&file).map_err(lock_error)?;
    } else {
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Conflict(
                    "data directory is held by a live writer".into(),
                ));
            }
            Err(TryLockError::Error(_)) => return Err(Error::ClockRegressed),
        }
    }
    if !same_regular_file(&file, &path).map_err(|_| Error::ClockRegressed)? {
        return Err(Error::ClockRegressed);
    }
    Ok(Some(file))
}

#[derive(Debug)]
enum StableSource {
    Process {
        scope: StableReadScope,
    },
    External {
        directory: PathBuf,
        file: Arc<File>,
        legacy: Option<Arc<File>>,
    },
}

impl StableSource {
    fn recheck(&self, data: &Path) -> Result<()> {
        match self {
            Self::Process { scope } if scope._directory.matches_data(data) => Ok(()),
            Self::Process { .. } => Err(Error::ClockRegressed),
            Self::External {
                directory,
                file,
                legacy,
            } => LockedOriginalSource {
                directory,
                file,
                legacy: legacy.as_ref(),
            }
            .recheck(data),
        }
    }
}

fn source_stability(data: &Path) -> Result<Option<StableSource>> {
    let directory = data_parent(data)
        .canonicalize()
        .map_err(|_| Error::ClockRegressed)?;
    let existing = directory_owners()
        .lock()
        .map_err(|_| Error::ClockRegressed)?
        .get(&directory)
        .and_then(Weak::upgrade);
    if let Some(owner) = existing {
        if !owner.matches_data(data) {
            return Err(Error::ClockRegressed);
        }
        return Ok(owner
            .try_stable_read_scope()
            .map(|scope| StableSource::Process { scope }));
    }
    let directory_file = Arc::new(File::open(&directory).map_err(|_| Error::ClockRegressed)?);
    match directory_file.try_lock_shared() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(_)) => return Err(Error::ClockRegressed),
    }
    // Positive directory-fd locking is required even when .rsia.lock is absent.
    let legacy = match existing_legacy_lock(&directory, false) {
        Ok(legacy) => legacy.map(Arc::new),
        Err(Error::Conflict(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let stable = StableSource::External {
        directory,
        file: directory_file,
        legacy,
    };
    stable.recheck(data)?;
    Ok(Some(stable))
}

struct LockedOriginalSource<'a> {
    directory: &'a Path,
    file: &'a Arc<File>,
    legacy: Option<&'a Arc<File>>,
}

fn same_directory(file: &File, path: &Path) -> std::io::Result<bool> {
    same_member(file, path, true)
}

fn same_regular_file(file: &File, path: &Path) -> std::io::Result<bool> {
    same_member(file, path, false)
}

fn same_member(file: &File, path: &Path, directory: bool) -> std::io::Result<bool> {
    let retained = file.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    let kinds_match = if directory {
        current.is_dir() && retained.is_dir()
    } else {
        current.is_file() && retained.is_file()
    };
    if current.file_type().is_symlink() || !kinds_match {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(retained.dev() == current.dev() && retained.ino() == current.ino())
    }
    #[cfg(not(unix))]
    Ok(false)
}

impl LockedOriginalSource<'_> {
    fn retain_after_failed_close(&self) {
        // Exceptional native close failure has no terminal credit. Keep the
        // actual locked file descriptions alive, not a path or a copied bool.
        std::mem::forget(Arc::clone(self.file));
        if let Some(legacy) = self.legacy {
            std::mem::forget(Arc::clone(legacy));
        }
    }

    fn recheck(&self, data: &Path) -> Result<()> {
        let directory = data_parent(data)
            .canonicalize()
            .map_err(|_| Error::ClockRegressed)?;
        if directory != self.directory
            || !same_directory(self.file, &directory).map_err(|_| Error::ClockRegressed)?
        {
            return Err(Error::ClockRegressed);
        }
        if let Some(legacy) = self.legacy
            && !same_regular_file(legacy, &directory.join(WRITER_LOCK_NAME))
                .map_err(|_| Error::ClockRegressed)?
        {
            return Err(Error::ClockRegressed);
        }
        Ok(())
    }
}

fn sidecar_path(data: &Path, suffix: &str) -> PathBuf {
    let mut path = data.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn regular_source_member(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::ClockRegressed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::ClockRegressed),
    }
}

#[cfg(unix)]
fn original_read_shm_uri(path: &Path) -> Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute() || bytes.contains(&0) {
        return Err(Error::ClockRegressed);
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut uri = String::from("file://");
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    uri.push_str("?readonly_shm=1");
    Ok(uri)
}

#[cfg(not(unix))]
fn original_read_shm_uri(_path: &Path) -> Result<String> {
    Err(Error::Invalid("unsupported read-only platform".into()))
}

struct OriginalReadCloseAck(());

struct OriginalReadStabilityUntilAck<'a, 'b> {
    scope: Option<&'a LockedOriginalSource<'b>>,
    stability: Option<StableSource>,
    // Lifetime only: this does not make the directory static, select heap SHM,
    // take the writer activity barrier, or grant content/role authority.
    mount_lifetime: Option<Arc<DirectoryWriterGuard>>,
}

impl OriginalReadStabilityUntilAck<'_, '_> {
    fn acknowledged(&mut self, _ack: OriginalReadCloseAck) {
        self.scope = None;
        drop(self.stability.take());
        drop(self.mount_lifetime.take());
    }
}

impl Drop for OriginalReadStabilityUntilAck<'_, '_> {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.take() {
            scope.retain_after_failed_close();
        }
        std::mem::forget(self.stability.take());
        std::mem::forget(self.mount_lifetime.take());
    }
}

struct OriginalReadOwner<'a, 'b> {
    connection: Option<ReadConnection>,
    source: OriginalReadStabilityUntilAck<'a, 'b>,
    close_ack: Option<OriginalReadCloseAck>,
    close_error: Option<rusqlite::Error>,
}

impl OriginalReadOwner<'_, '_> {
    fn close_native(&mut self) -> Result<()> {
        let connection = self.connection.take().ok_or(Error::ClockRegressed)?;
        match connection.close() {
            Ok(()) => {
                self.close_ack = Some(OriginalReadCloseAck(()));
                Ok(())
            }
            Err((connection, error)) => {
                std::mem::forget(connection);
                self.close_error = Some(error);
                Err(Error::ClockRegressed)
            }
        }
    }
}

impl Drop for OriginalReadOwner<'_, '_> {
    fn drop(&mut self) {
        if self.connection.is_some() && self.close_native().is_err() {
            return;
        }
        if let Some(ack) = self.close_ack.take() {
            self.source.acknowledged(ack);
        }
    }
}

// Constructor remains private to this module; it is neither a wire bool nor a
// re-usable native/Current permit. Only the fixed after-close source route gets it.
pub(crate) struct OriginalReadCompletedR38(());

pub(super) async fn read_formal_source_r38(
    data: PathBuf,
    mount: Arc<DirectoryWriterGuard>,
    ctx: Context,
    selector: crate::FormalReadSelectorR38,
) -> Result<Option<crate::ClosedFormalSourceR38>> {
    selector.validate_request(&ctx)?;
    if !mount.matches_data(&data) {
        return Err(Error::Forbidden);
    }
    // A cancelled waiter does not drop the running blocking owner/native handle.
    // The real mount lease is moved inside OriginalReadOwner before any read SQL
    // and retained on close failure/unwind according to the R34 ACK semantics.
    tokio::task::spawn_blocking(move || {
        let (prepared, completed) = if live_original_source_r50(&data, None)? {
            let (raw, completed) = read_original_rows_r50(&data, None, Some(mount))?;
            let prepared = crate::owned_occurrence::e05_issue::prepare_formal_source_from_raw_r50(
                raw, &ctx, &selector,
            )?;
            (prepared, completed)
        } else {
            let prepared =
                original_read_scoped_mounted_r38(&data, None, Some(mount), |connection| {
                    crate::owned_occurrence::e05_issue::capture_formal_source_r38(
                        connection, &ctx, &selector,
                    )
                })?;
            (prepared, OriginalReadCompletedR38(()))
        };
        Ok(prepared.map(|source| {
            crate::owned_occurrence::e05_issue::finish_formal_source_r38(source, completed)
        }))
    })
    .await
    .map_err(internal)?
}

pub(super) async fn reject_known_host_complete_path_r49(
    data: PathBuf,
    ctx: Context,
    ticket_id: String,
) -> Result<()> {
    ctx.require(&[Role::Evaluator])?;
    // Missing/new files follow the unchanged ordinary mount/open path. This
    // preflight never creates a DB and never returns a transferable capability.
    match std::fs::symlink_metadata(&data) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(internal(error)),
        Ok(metadata) if !metadata.file_type().is_file() => return Err(Error::ClockRegressed),
        Ok(_) => {}
    }
    tokio::task::spawn_blocking(move || {
        if live_original_source_r50(&data, None)? {
            let (raw, _completed) = read_original_rows_r50(&data, None, None)?;
            crate::owned_occurrence::e05_issue::reject_known_host_complete_raw_r50(
                &raw, &ctx, &ticket_id,
            )
        } else {
            original_read_scoped(&data, None, |connection| {
                crate::owned_occurrence::e05_issue::reject_known_host_complete_connection_r49(
                    connection, &ctx, &ticket_id,
                )
            })
        }
    })
    .await
    .map_err(internal)?
}

pub(super) async fn read_formal_snapshot_r39(
    data: PathBuf,
    mount: Arc<DirectoryWriterGuard>,
    ctx: Context,
    selector: crate::FormalReadSelectorR38,
) -> Result<crate::ClosedFormalReadR39> {
    selector.validate_request(&ctx)?;
    if !mount.matches_data(&data) {
        return Err(Error::Forbidden);
    }
    tokio::task::spawn_blocking(move || {
        let (prepared, completed) = if live_original_source_r50(&data, None)? {
            let (raw, completed) = read_original_rows_r50(&data, None, Some(mount))?;
            let prepared =
                crate::owned_occurrence::e05_issue::prepare_formal_snapshot_from_raw_r50(
                    raw, &ctx, &selector,
                )?;
            (prepared, completed)
        } else {
            let prepared =
                original_read_scoped_mounted_r38(&data, None, Some(mount), |connection| {
                    crate::owned_occurrence::e05_issue::capture_formal_snapshot_r39(
                        connection, &ctx, &selector,
                    )
                })?;
            (prepared, OriginalReadCompletedR38(()))
        };
        Ok(crate::owned_occurrence::e05_issue::finish_formal_snapshot_r39(prepared, completed))
    })
    .await
    .map_err(internal)?
}

/// Run a read on the original directory and original native connection.
/// Immutable is limited to a held stable main-only source; nonempty WAL is
/// never ignored. No database serialization/copy, checkpoint, chmod or cleanup.
/// Native event/strict-atime qualification remains a separate acceptance gate.
fn original_read<T>(data: &Path, read: impl FnOnce(&ReadConnection) -> Result<T>) -> Result<T> {
    original_read_scoped(data, None, read)
}

fn original_read_scoped<T>(
    data: &Path,
    scope: Option<&LockedOriginalSource<'_>>,
    read: impl FnOnce(&ReadConnection) -> Result<T>,
) -> Result<T> {
    original_read_scoped_mounted_r38(data, scope, None, read)
}

fn original_read_scoped_mounted_r38<T>(
    data: &Path,
    scope: Option<&LockedOriginalSource<'_>>,
    mount_lifetime: Option<Arc<DirectoryWriterGuard>>,
    read: impl FnOnce(&ReadConnection) -> Result<T>,
) -> Result<T> {
    original_native_read_r50(
        data,
        scope,
        mount_lifetime,
        OriginalReadRouteR50::Scoped,
        read,
    )
}

enum OriginalReadRouteR50 {
    Scoped,
    FreshExec,
}

pub(super) fn fresh_live_read_worker_r50<T>(
    data: &Path,
    read: impl FnOnce(&ReadConnection) -> Result<T>,
) -> Result<T> {
    original_native_read_r50(data, None, None, OriginalReadRouteR50::FreshExec, read)
}

// Only called after a real borrowed scope or StableSource has been acquired.
// These descriptors stay live through rollback, native close and source recheck.
fn original_heap_source_r50(data: &Path) -> Result<(File, Option<File>, Option<String>)> {
    let main = restore_regular_file_r50(data).map_err(|_| Error::ClockRegressed)?;
    let shm = regular_source_member(&sidecar_path(data, "-shm"))?;
    if regular_source_member(&sidecar_path(data, "-journal"))? {
        // No hot/cold journal distinction is inferred from its name or length.
        return Err(Error::ClockRegressed);
    }
    let wal_path = sidecar_path(data, "-wal");
    let wal = if regular_source_member(&wal_path)? {
        Some(restore_regular_file_r50(&wal_path).map_err(|_| Error::ClockRegressed)?)
    } else {
        None
    };
    let main_only = match &wal {
        None => true,
        Some(file) => file.metadata().map_err(|_| Error::ClockRegressed)?.len() == 0,
    };
    let uri = if main_only {
        if shm {
            return Err(Error::ClockRegressed);
        }
        let encoded = original_read_shm_uri(data)?;
        let base = encoded
            .strip_suffix("?readonly_shm=1")
            .ok_or(Error::ClockRegressed)?;
        Some(format!("{base}?immutable=1"))
    } else {
        None
    };
    Ok((main, wal, uri))
}

fn original_native_read_r50<T>(
    data: &Path,
    scope: Option<&LockedOriginalSource<'_>>,
    mount_lifetime: Option<Arc<DirectoryWriterGuard>>,
    route: OriginalReadRouteR50,
    read: impl FnOnce(&ReadConnection) -> Result<T>,
) -> Result<T> {
    if let Some(mount) = &mount_lifetime
        && !mount.matches_data(data)
    {
        return Err(Error::Forbidden);
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Err(Error::Invalid("unsupported read-only platform".into()));
    }
    if !regular_source_member(data)? {
        return Err(Error::ClockRegressed);
    }
    let canonical = data.canonicalize().map_err(|_| Error::ClockRegressed)?;
    let shared_memory = regular_source_member(&sidecar_path(&canonical, "-shm"))?;
    let stability = match scope {
        Some(scope) => {
            scope.recheck(&canonical)?;
            None
        }
        // A legitimate live SHM already supplies SQLite's readonly snapshot
        // protocol. Do not acquire the process writer barrier for this route.
        None if shared_memory => None,
        None => source_stability(&canonical)?,
    };
    if let Some(stability) = &stability {
        stability.recheck(&canonical)?;
    }
    let heap_index = scope.is_some() || stability.is_some();
    // A live SHM reader retains SQLite's native snapshot/lock protocol. The
    // heap-index path requires the actual stability scope retained until close.
    if (!heap_index && (!shared_memory || !matches!(route, OriginalReadRouteR50::FreshExec)))
        || (heap_index && matches!(route, OriginalReadRouteR50::FreshExec))
    {
        return Err(Error::ClockRegressed);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let heap_source = if heap_index {
        Some(original_heap_source_r50(&canonical)?)
    } else {
        None
    };
    let connection = if let Some((_, _, Some(uri))) = heap_source.as_ref() {
        ReadConnection::open_with_flags_and_vfs(Path::new(uri), flags, "unix-none")
    } else if heap_index {
        ReadConnection::open_with_flags_and_vfs(&canonical, flags, "unix-none")
    } else {
        ReadConnection::open_with_flags_and_vfs(
            Path::new(&original_read_shm_uri(&canonical)?),
            flags,
            "unix",
        )
    }
    .map_err(|_| Error::ClockRegressed)?;
    let mut close_owner = OriginalReadOwner {
        connection: Some(connection),
        source: OriginalReadStabilityUntilAck {
            scope,
            stability,
            mount_lifetime,
        },
        close_ack: None,
        close_error: None,
    };
    let result = (|| {
        let connection = close_owner
            .connection
            .as_mut()
            .ok_or(Error::ClockRegressed)?;
        let config = DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE;
        let _initial_value = connection
            .db_config(config)
            .map_err(|_| Error::ClockRegressed)?;
        if !connection
            .set_db_config(config, true)
            .map_err(|_| Error::ClockRegressed)?
            || !connection
                .db_config(config)
                .map_err(|_| Error::ClockRegressed)?
        {
            return Err(Error::ClockRegressed);
        }
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| Error::ClockRegressed)?;
        connection
            .execute_batch("PRAGMA query_only=ON; PRAGMA temp_store=MEMORY;")
            .map_err(|_| Error::ClockRegressed)?;
        if heap_index {
            connection
                .execute_batch("PRAGMA locking_mode=EXCLUSIVE;")
                .map_err(|_| Error::ClockRegressed)?;
        }
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| Error::ClockRegressed)?;
        // Observe the actual backend. No invented numeric minimum or Python
        // SQLite version can qualify this handle.
        let result = (|| {
            let identity: (String, String) = tx
                .query_row("SELECT sqlite_version(),sqlite_source_id()", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|_| Error::ClockRegressed)?;
            if identity.0.is_empty() || identity.1.is_empty() {
                return Err(Error::ClockRegressed);
            }
            read(&tx)
        })();
        let rollback = tx.rollback().map_err(|_| Error::ClockRegressed);
        match (result, rollback) {
            (Err(error), Err(rollback)) => {
                tracing::error!(?rollback, "read-only rollback failed after primary refusal");
                Err(error)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    })();
    let close = close_owner.close_native().map_err(|error| {
        if let Some(native_error) = close_owner.close_error.as_ref() {
            tracing::error!(
                error=?native_error,
                "original read-only native close failed; retaining native source owner"
            );
        }
        error
    });
    let source_recheck = scope
        .map(|scope| scope.recheck(&canonical))
        .transpose()
        .and_then(|_| {
            close_owner
                .source
                .stability
                .as_ref()
                .map(|stable| stable.recheck(&canonical))
                .transpose()
        })
        .and_then(|_| {
            if let Some((main, wal, uri)) = &heap_source {
                if !same_regular_file(main, &canonical).map_err(|_| Error::ClockRegressed)? {
                    return Err(Error::ClockRegressed);
                }
                let wal_path = sidecar_path(&canonical, "-wal");
                if let Some(file) = wal
                    && !same_regular_file(file, &wal_path).map_err(|_| Error::ClockRegressed)?
                {
                    return Err(Error::ClockRegressed);
                }
                if uri.is_some() {
                    if regular_source_member(&sidecar_path(&canonical, "-shm"))?
                        || regular_source_member(&sidecar_path(&canonical, "-journal"))?
                    {
                        return Err(Error::ClockRegressed);
                    }
                    match wal {
                        None if regular_source_member(&wal_path)? => {
                            return Err(Error::ClockRegressed);
                        }
                        Some(file)
                            if file.metadata().map_err(|_| Error::ClockRegressed)?.len() != 0 =>
                        {
                            return Err(Error::ClockRegressed);
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        });
    drop(close_owner);
    if source_recheck.is_err() && result.is_ok() {
        return Err(Error::ClockRegressed);
    }
    match (result, close) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn live_original_source_r50(data: &Path, scope: Option<&LockedOriginalSource<'_>>) -> Result<bool> {
    if let Some(scope) = scope {
        scope.recheck(data)?;
        return Ok(false);
    }
    if !regular_source_member(data)? {
        return Err(Error::ClockRegressed);
    }
    let canonical = data.canonicalize().map_err(|_| Error::ClockRegressed)?;
    regular_source_member(&sidecar_path(&canonical, "-shm"))
}

fn read_original_rows_r50(
    data: &Path,
    scope: Option<&LockedOriginalSource<'_>>,
    mount: Option<Arc<DirectoryWriterGuard>>,
) -> Result<(
    crate::owned_occurrence::RawDatabase,
    OriginalReadCompletedR38,
)> {
    if mount
        .as_ref()
        .is_some_and(|guard| !guard.matches_data(data))
    {
        return Err(Error::Forbidden);
    }
    if !live_original_source_r50(data, scope)? {
        return Err(Error::ClockRegressed);
    }
    let closed = crate::readonly_worker::run_live(
        data,
        crate::readonly_worker::Operation::RawSnapshot,
        mount,
    )?;
    let raw = crate::owned_occurrence::raw_from_worker_r50(closed.into_value())?;
    // Only the live transport's real child close, wait and both EOFs yield this.
    Ok((raw, OriginalReadCompletedR38(())))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlFactsWireR50 {
    watermarks: Vec<(String, i64, String)>,
    tombstones: Vec<(String, String, String)>,
    protected_objects: Vec<(String, String, String, String)>,
    root_budget_tables: Vec<(String, Vec<String>)>,
}

pub(super) fn control_facts_worker_value_r50(
    connection: &ReadConnection,
) -> Result<serde_json::Value> {
    let facts = read_facts_native(connection)?;
    let wire = ControlFactsWireR50 {
        watermarks: facts
            .watermarks
            .into_iter()
            .map(|(ns, value)| (ns, value.seq, value.digest))
            .collect(),
        tombstones: facts
            .tombstones
            .into_iter()
            .map(|((ns, id), value)| Ok((ns, id, serde_json::to_string(&value).map_err(internal)?)))
            .collect::<Result<Vec<_>>>()?,
        protected_objects: facts
            .protected_objects
            .into_iter()
            .map(|((ns, kind, id), value)| {
                Ok((
                    ns,
                    kind,
                    id,
                    serde_json::to_string(&value).map_err(internal)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?,
        root_budget_tables: facts.root_budget_tables.into_iter().collect(),
    };
    serde_json::to_value(wire).map_err(internal)
}

fn control_facts_from_worker_r50(value: serde_json::Value) -> Result<ControlPlaneFacts> {
    let wire: ControlFactsWireR50 =
        serde_json::from_value(value).map_err(|_| Error::ClockRegressed)?;
    let mut facts = ControlPlaneFacts {
        watermarks: BTreeMap::new(),
        tombstones: BTreeMap::new(),
        protected_objects: BTreeMap::new(),
        root_budget_tables: BTreeMap::new(),
    };
    for (ns, seq, digest) in wire.watermarks {
        if facts
            .watermarks
            .insert(ns, WatermarkFact { seq, digest })
            .is_some()
        {
            return Err(Error::ClockRegressed);
        }
    }
    for (ns, id, value) in wire.tombstones {
        let value = parse_strict_json(&value)?;
        if facts.tombstones.insert((ns, id), value).is_some() {
            return Err(Error::ClockRegressed);
        }
    }
    for (ns, kind, id, value) in wire.protected_objects {
        let value = parse_strict_json(&value)?;
        if facts
            .protected_objects
            .insert((ns, kind, id), value)
            .is_some()
        {
            return Err(Error::ClockRegressed);
        }
    }
    for (name, rows) in wire.root_budget_tables {
        if facts.root_budget_tables.insert(name, rows).is_some() {
            return Err(Error::ClockRegressed);
        }
    }
    Ok(facts)
}

/// Fresh same-snapshot wall time for a trusted FS caller. Extra values are data
/// only: their owner must qualify the actual record and cannot authorize a clock.
/// One actual, already-admitted sample. The private fields are not a clock
/// injection API. Consuming it binds an occurrence and its temporary-name
/// nanoseconds to the same native snapshot and same system observation.
#[derive(Debug)]
pub struct ReadonlyWallSample {
    seconds: i64,
    nanos: u128,
}

impl ReadonlyWallSample {
    pub fn into_parts(self) -> (i64, u128) {
        (self.seconds, self.nanos)
    }
}

fn admitted_sample(
    floor: &crate::owned_occurrence::OccurrenceFloor,
    clock: &mut OperationClock,
) -> Result<ReadonlyWallSample> {
    let stamp = clock.nanos(floor)?;
    let nanos = clock.take_nanos(stamp)?;
    let seconds = i64::try_from(nanos / 1_000_000_000).map_err(|_| clock.fail())?;
    Ok(ReadonlyWallSample { seconds, nanos })
}

pub fn readonly_wall_seconds(
    data: &Path,
    candidates: &[HostPrepareConfig],
    extra_occurrences: &[i64],
) -> Result<i64> {
    readonly_wall_seconds_with_clock(data, candidates, extra_occurrences, &WallClock::system())
}

fn readonly_wall_seconds_with_clock(
    data: &Path,
    candidates: &[HostPrepareConfig],
    extra_occurrences: &[i64],
    source: &WallClock,
) -> Result<i64> {
    readonly_wall_sample_scoped(data, candidates, extra_occurrences, None, source)
        .map(|sample| sample.seconds)
}

pub fn readonly_wall_sample(
    data: &Path,
    candidates: &[HostPrepareConfig],
    extra_occurrences: &[i64],
) -> Result<ReadonlyWallSample> {
    readonly_wall_sample_scoped(
        data,
        candidates,
        extra_occurrences,
        None,
        &WallClock::system(),
    )
}

fn readonly_wall_seconds_scoped(
    data: &Path,
    candidates: &[HostPrepareConfig],
    extra_occurrences: &[i64],
    scope: Option<&LockedOriginalSource<'_>>,
    source: &WallClock,
) -> Result<i64> {
    readonly_wall_sample_scoped(data, candidates, extra_occurrences, scope, source)
        .map(|sample| sample.seconds)
}

fn readonly_wall_sample_scoped(
    data: &Path,
    candidates: &[HostPrepareConfig],
    extra_occurrences: &[i64],
    scope: Option<&LockedOriginalSource<'_>>,
    source: &WallClock,
) -> Result<ReadonlyWallSample> {
    let mut clock = OperationClock::with_source(source.clone());
    if let Some(scope) = scope {
        scope.recheck(data)?;
    }
    if !regular_source_member(data)? {
        if regular_source_member(&sidecar_path(data, "-wal"))?
            || regular_source_member(&sidecar_path(data, "-shm"))?
        {
            return Err(clock.fail());
        }
        let mut floor = crate::owned_occurrence::OccurrenceFloor::default();
        for value in extra_occurrences {
            floor.observe_seconds(*value).map_err(|_| clock.fail())?;
        }
        return admitted_sample(&floor, &mut clock);
    }
    let inputs = PreparePreimages {
        candidates: candidates.to_vec(),
        ..Default::default()
    };
    let floor = if live_original_source_r50(data, scope)? {
        let (raw, _completed) = read_original_rows_r50(data, scope, None)?;
        let mut floor = crate::owned_occurrence::collect_floor_from_raw_r50(&raw, &inputs)
            .map_err(|_| clock.fail())?;
        for value in extra_occurrences {
            floor.observe_seconds(*value).map_err(|_| clock.fail())?;
        }
        floor
    } else {
        original_read_scoped(data, scope, |connection| {
            let mut floor =
                collect_floor_rusqlite(connection, &inputs).map_err(|_| clock.fail())?;
            for value in extra_occurrences {
                floor.observe_seconds(*value).map_err(|_| clock.fail())?;
            }
            Ok(floor)
        })?
    };
    // Rollback and native close above are terminal before the production wall
    // sample used by the caller's next filesystem event is obtained.
    admitted_sample(&floor, &mut clock)
}

pub async fn preflight_occurrence_clock(
    data: &Path,
    candidates: &[HostPrepareConfig],
) -> Result<()> {
    preflight_occurrence_clock_with_clock(data, candidates, WallClock::system()).await
}

pub(super) async fn preflight_occurrence_clock_with_clock(
    data: &Path,
    candidates: &[HostPrepareConfig],
    clock: WallClock,
) -> Result<()> {
    let data = data.to_path_buf();
    let candidates = candidates.to_vec();
    let source = clock.clone();
    // The worker owns the exact source and source scopes/connection through
    // actual rollback and close, even if the awaiting entry is cancelled.
    // The outer operation has no terminal native credit merely from cancellation.
    tokio::task::spawn_blocking(move || {
        readonly_wall_seconds_with_clock(&data, &candidates, &[], &source).map(|_| ())
    })
    .await
    .map_err(internal)?
}

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

fn valid_native_trace_authority(node: &TypedObjectRef, value: &serde_json::Value) -> bool {
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
            let status = load_status_by_source(
                session.transaction()?,
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
        .execute(&mut **session.write_transaction().await?)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
             VALUES(?,?,'__budget_scan',?)",
        )
        .bind(ctx.namespace())
        .bind(&job_id)
        .bind(&source.id)
        .execute(&mut **session.write_transaction().await?)
        .await
        .map_err(internal)?;
        sqlx::query("INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id) VALUES(?,?,'__world_scan',?)")
            .bind(ctx.namespace()).bind(&job_id).bind(&source.id).execute(&mut **session.write_transaction().await?).await.map_err(internal)?;
        sqlx::query(
            "INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id)
             VALUES(?,?,?,?)",
        )
        .bind(ctx.namespace())
        .bind(&job_id)
        .bind(&source.kind)
        .bind(&source.id)
        .execute(&mut **session.write_transaction().await?)
        .await
        .map_err(internal)?;
        insert_cleanup_event(
            session.transaction()?,
            ctx.namespace(),
            &job_id,
            Some(&source),
            "logical_block_committed",
            now,
            json!({"watermark_seq":watermark_seq,"watermark_digest":watermark_digest}),
        )
        .await?;
        let status = load_status(session.transaction()?, ctx.namespace(), &job_id)
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
        let activity = store.runtime.writer()?.write_scope().await;
        let _blob_guard = store.blob_lock.write().await;
        let (mut tx, mut clock, _activity) =
            store.begin_write_transaction_in_activity(activity).await?;
        let status = load_status(&mut tx, ctx.namespace(), job_id)
            .await?
            .ok_or(Error::NotFound)?;
        if status.state == CleanupState::Complete {
            store.commit_write_transaction(tx, &mut clock).await?;
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
        store.commit_write_transaction(tx, &mut clock).await?;
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
/// reference that does not decode, or no reference and a request whose schema or
/// body cannot be read) is isolated (`mark_unknown_scope`, which fails the job) and
/// left as it is. Redacts at most `limit` calls; returns whether it
/// redacted or isolated any.
async fn redact_late_budget_calls(
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
/// So is a call whose reference is stored but does not decode
/// (`unreadable_budget_call_ref:<schema>`): its closure is neither trusted nor
/// guessed at, and the request is not read in its place (plan §11.5: isolate what
/// lacks the facts it is judged by; E16.2: a dependency source that cannot be
/// resolved is isolated). The legacy gates refuse the same call with `Internal` and write
/// nothing (`registered_source_ids`); the cleanup isolates it instead, records it
/// (`mark_unknown_scope`: a `Failed` job an Admin can retry) and goes on with the
/// rest of its namespace, because an error here would roll the whole step back and
/// leave the job running with no diagnostic, unreachable by a retry (AG-096). The
/// two answers differ on purpose.
async fn budget_call_source_closure(
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
        crate::budget::RegisteredCallSources::Undecodable(_) => Ok(BudgetRequestSources::Unknown(
            format!("unreadable_budget_call_ref:{schema}"),
        )),
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
        "rsia.bound_model_request.v1" => read().as_ref().and_then(bound_request_closure_ids),
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
fn bound_request_closure_ids(bound: &serde_json::Value) -> Option<Vec<String>> {
    if bound.get("schema_version")?.as_str()? != "rsia.bound_model_request.v1" {
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

pub(crate) async fn backup_consistent(store: &Store, destination: &Path) -> Result<()> {
    let store = store.clone();
    let destination = destination.to_path_buf();
    let (send, receive) = tokio::sync::oneshot::channel();
    // The request's cancellation cannot drop the real stability scope while a
    // SQLite/FS worker is still executing. This owner is independent of the
    // caller's runtime; its result is sent only after actual operation return.
    let _worker = std::thread::Builder::new()
        .name("rsia-backup-drain".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = send.send(Err(internal(error)));
                    return;
                }
            };
            let owner = match store.runtime.writer() {
                Ok(owner) => owner,
                Err(error) => {
                    let _ = send.send(Err(error));
                    return;
                }
            };
            let stable_source = runtime.block_on(owner.stable_read_scope());
            let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(backup_consistent_owned(&store, &destination))
            }));
            let result = match completed {
                Ok(BackupCompletion::CommandsAcknowledged(result)) => result,
                Ok(BackupCompletion::Unconfirmed(error)) => {
                    tracing::error!("backup command drain failed; retaining source exclusion");
                    std::mem::forget(stable_source);
                    std::mem::forget(owner);
                    let _ = send.send(Err(error));
                    return;
                }
                Err(_) => {
                    // Neither pool size nor pool.close() is a public native
                    // shutdown receipt after a panic or unknown worker state.
                    tracing::error!("backup worker panicked; retaining source exclusion");
                    std::mem::forget(stable_source);
                    std::mem::forget(owner);
                    let _ = send.send(Err(Error::Internal));
                    return;
                }
            };
            drop(stable_source);
            let _ = send.send(result);
        })
        .map_err(internal)?;
    receive.await.map_err(internal)?
}

enum BackupCompletion {
    CommandsAcknowledged(Result<()>),
    Unconfirmed(Error),
}

async fn backup_consistent_owned(store: &Store, destination: &Path) -> BackupCompletion {
    if !cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple"
    )) {
        return BackupCompletion::CommandsAcknowledged(Err(Error::Invalid(
            "atomic no-overwrite backup publication unsupported platform".into(),
        )));
    }
    if destination.exists() {
        return BackupCompletion::CommandsAcknowledged(Err(Error::Conflict(
            "backup destination already exists".into(),
        )));
    }
    // The thread owner retains its real source scope through this whole
    // original VACUUM algorithm. No nested acquire is used for clock guards.
    let _blob_guard = store.blob_lock.write().await;
    let mut connection = match store.pool.acquire().await {
        Ok(connection) => connection,
        Err(error) => return BackupCompletion::Unconfirmed(internal(error)),
    };
    let mut clock = store.operation_clock();
    let result: Result<()> = async {
        crate::write_clock::seconds_on_connection(
            &mut connection,
            &store.prepare_inputs,
            &mut clock,
        )
        .await?;
        let parent = destination.parent().unwrap_or(Path::new("."));
        create_private_backup_directory(parent, true).await?;
        let name = destination
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| Error::Invalid("backup destination needs a UTF-8 name".into()))?;
        let temp = parent.join(format!(".{name}.tmp-{}", uuid::Uuid::new_v4()));
        crate::write_clock::seconds_on_connection(
            &mut connection,
            &store.prepare_inputs,
            &mut clock,
        )
        .await?;
        create_private_backup_directory(&temp, false).await?;
        let result = backup_into_temp(store, &temp, &mut connection, &mut clock).await;
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
        if let Err(error) = crate::write_clock::seconds_on_connection(
            &mut connection,
            &store.prepare_inputs,
            &mut clock,
        )
        .await
        {
            let _ = tokio::fs::remove_dir_all(&temp).await;
            return Err(error);
        }
        let source = temp.clone();
        let target = destination.to_path_buf();
        let published =
            tokio::task::spawn_blocking(move || publish_backup_directory(&source, &target))
                .await
                .map_err(internal)?;
        if let Err(error) = published {
            let _ = tokio::fs::remove_dir_all(&temp).await;
            return Err(error);
        }
        sync_directory(parent.to_path_buf()).await?;
        Ok(())
    }
    .await;
    // Await the original worker's queued rollback and ping explicitly before
    // its public pool return. No Drop, pool-size or flush is an acknowledgement.
    let settled: Result<()> = async {
        if connection.is_in_transaction() {
            SqliteTransactionManager::rollback(&mut *connection)
                .await
                .map_err(internal)?;
        }
        connection.ping().await.map_err(internal)?;
        if connection.is_in_transaction() {
            return Err(Error::Internal);
        }
        Ok(())
    }
    .await;
    if let Err(error) = settled {
        tracing::error!(
            ?error,
            "backup original connection command state unconfirmed"
        );
        return BackupCompletion::Unconfirmed(result.err().unwrap_or(error));
    }
    // The private native owner retains the actual connection through pending
    // rollback and its worker acknowledgement. A failed release cannot be
    // credited as terminal completion or release the source exclusion scope.
    if let Err(error) = connection.release().await {
        tracing::error!(%error, "backup native owner release unconfirmed");
        return BackupCompletion::Unconfirmed(result.err().unwrap_or_else(|| internal(error)));
    }
    BackupCompletion::CommandsAcknowledged(result)
}

async fn create_private_backup_directory(path: &Path, recursive: bool) -> Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await.map_err(internal)
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

async fn backup_into_temp(
    store: &Store,
    temp: &Path,
    connection: &mut sqlx::SqliteConnection,
    clock: &mut OperationClock,
) -> Result<()> {
    let database_path = temp.join("rsia.sqlite3");
    crate::write_clock::seconds_on_connection(connection, &store.prepare_inputs, clock).await?;
    // SQLite permits an existing empty VACUUM INTO output. Creating it with
    // the final private mode avoids a transient permissive database file.
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&database_path).await.map_err(internal)?;
    file.sync_all().await.map_err(internal)?;
    drop(file);
    crate::write_clock::seconds_on_connection(connection, &store.prepare_inputs, clock).await?;
    sqlx::query("VACUUM INTO ?")
        .bind(database_path.to_string_lossy().as_ref())
        .execute(&mut *connection)
        .await
        .map_err(internal)?;
    sync_file(&database_path).await?;
    let database = file_entry(&database_path, "rsia.sqlite3").await?;
    let migrations = load_backup_migrations(connection).await?;
    let watermarks = load_backup_watermarks(connection).await?;
    let mut blobs = copy_backup_blobs(store, temp).await?;
    blobs.sort_by(|left, right| left.path.cmp(&right.path));

    crate::write_clock::seconds_on_connection(connection, &store.prepare_inputs, clock).await?;
    let manifest_path = temp.join("backup-manifest.json");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&manifest_path).await.map_err(internal)?;
    // The open awaited: obtain the manifest occurrence only now, using the
    // original held connection and the same operation poison latch.
    let created_at =
        crate::write_clock::seconds_on_connection(connection, &store.prepare_inputs, clock).await?;
    let manifest = BackupManifest {
        schema_version: BACKUP_SCHEMA.into(),
        complete: true,
        database,
        blobs,
        migrations,
        watermarks,
        created_at,
    };
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(internal)?;
    use tokio::io::AsyncWriteExt;
    file.write_all(&bytes).await.map_err(internal)?;
    file.sync_all().await.map_err(internal)?;
    drop(file);
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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

#[allow(clippy::too_many_arguments)]
async fn expand_world_scan(
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
    store: &Store,
    ctx: &Context,
    job_id: &str,
    node: &TypedObjectRef,
    body: &str,
    value: &serde_json::Value,
    export_directory_rechecked: bool,
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
        && !export_directory_rechecked
    {
        remove_controlled_local_export(store, ctx, node, job_id, tx, now).await?;
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

/// The caller's typed node is a member of this job's persisted frontier. A
/// directory failure from another job in the same namespace still belongs to
/// that node. EXISTS avoids loading the history; its scan cost is not constant.
async fn has_failed_local_export(
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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

async fn remove_controlled_local_export(
    store: &Store,
    ctx: &Context,
    node: &TypedObjectRef,
    job_id: &str,
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
    now: i64,
) -> Result<()> {
    // The export attempt id names the directory under the namespace export
    // root; an id such as ".." would resolve outside it, so refuse to touch
    // the filesystem for any id the export entry points would not accept.
    if crate::validate_export_id(&node.id).is_err() {
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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

/// Every payload field of an E16 record schema that can carry a registered
/// blob digest. This is the single source of truth for both content
/// redaction (which blobs a revoked record may release) and shared-blob
/// liveness (which blobs a surviving record may keep).
fn e16_blob_digest_fields(schema: &str) -> &'static [&'static str] {
    match schema {
        "rsia.e16.import_source.v1" => &["raw_blob_digest"],
        "rsia.e16.staged_asset.v1" => &["content_blob_digest"],
        "rsia.e16.seed_install.v1" => &[
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
        "rsia.e16.seed_install.v1" => ("status", &["prepared", "installed"]),
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
    ctx: &Context,
    job_id: &str,
    digest: &str,
) -> Result<bool> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT o.body FROM objects o
         WHERE o.namespace=? AND o.kind='artifact'
         AND json_extract(o.body,'$.schema_version') IN (
           'rsia.e16.import_source.v1','rsia.e16.staged_asset.v1',
           'rsia.e16.seed_install.v1','rsia.e16.export_attempt.v1',
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
        validate_e16_envelope(ctx, &node, &value)?;
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    let value = body
        .as_deref()
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .map_err(internal)?;
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
                && validate_e16_envelope(ctx, node, value).is_err())
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
        remove_controlled_local_export(store, ctx, node, job_id, tx, now).await?;
    }
    let (Some(body), Some(value)) = (body, value) else {
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
            | "evaluation_ticket_issue_receipt_v2"
            | "execution_receipt_v2"
            | "independent_grader_receipt_v2"
            | "formal_evaluation_v2"
            | "development_control_v1"
            | "development_execution_receipt_v1"
            | "development_grader_receipt_v1"
            | "development_run_receipt_v1",
        ) => true,
        ("release", "rsia.persistent_release.v2", _)
        | ("artifact", "rsia.issued_formal_authority.v3", _)
        | ("artifact", "rsia.independent_grade_capture.v2", _)
        | ("artifact", "rsia.release_action.v2", _)
        | ("artifact", evo_core::formal_version::SCHEMA, _)
        | ("artifact", evo_core::formal_version::NATIVE_TICKET_ENVELOPE, "evaluation_ticket_v2") => {
            true
        }
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
            | "replay_economic_report_v1",
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
        ("job", "rsia.management_job.v1", _) => true,
        _ => false,
    };
    let redact = match (node.kind.as_str(), schema, record_kind) {
        (_, "rsia.redacted.v1", _) => return Ok(()),
        // Only complete native Host authorities qualify, bound to this closure node.
        ("run", "rsia.optimization.source.v1", _) => valid_native_trace_authority(node, &value),
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
        ("artifact", "rsia.run_application.v1", _)
        | ("receipt", "rsia.host_application.v1", _)
        | ("artifact", "rsia.host_execution_receipt.v1", _) => true,
        ("artifact", "rsia.resolved_bundle.v2", _) => true,
        // Monitoring content derived from source runs (environment and observation
        // identities, per-task pairings, the claim, the consolidation proposal with
        // its candidate bundle and the binding that reserves its candidate id) is
        // redacted; the preserved scope, run and drift facts above keep the ids
        // and counts needed for reconciliation.
        (
            "artifact",
            "rsia.monitoring.environment.v1"
            | "rsia.monitoring.observation.v1"
            | "rsia.monitoring.development_cycle.v1"
            | "rsia.monitoring.development_cycle.v2"
            | "rsia.monitoring.development_report_binding.v1"
            | "rsia.monitoring.consolidation_claim.v1"
            | "rsia.monitoring.consolidation_proposal.v1"
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
        ("artifact", "rsia.practice_attempt_set.v1", _) => true,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    if schema == "rsia.optimization.stage_fact.v1"
        || schema == "rsia.exploration_artifact_envelope.v1"
    {
        metadata["historical_facts"] = historical_stage_facts(&value["payload"]);
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
    ] {
        if let Some(item) = object.get(key) {
            output.insert(key.into(), item.clone());
        }
    }
    serde_json::Value::Object(output)
}

async fn ensure_logical_block(
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
    tx: &mut impl std::ops::DerefMut<Target = sqlx::SqliteConnection>,
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
pub const PROTECTED_SCHEMA_VERSIONS: [&str; 9] = [
    "rsia.typed_artifact_envelope.v1",
    "rsia.exploration_artifact_envelope.v1",
    "rsia.optimization.stage_fact.v1",
    "rsia.budget_call_ref.v1",
    "rsia.budget_call_e16_ref.v1",
    "rsia.e16.export_attempt.v1",
    "rsia.e16.delivery_audit.v1",
    "rsia.e16.export_attempt.v2",
    "rsia.e16.delivery_audit.v2",
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
pub const PROTECTED_ACTION_SCHEMA_VERSIONS: [&str; 11] = [
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

/// Read recovery diagnostics on the original native source, in one read-only
/// transaction. This deliberately does not require a healthy wall clock: a
/// malformed occurrence must remain inspectable. Source permission, latest-WAL,
/// strict-atime and every-file-event controls still require native acceptance.
pub async fn read_control_plane_facts(path: &Path) -> Result<ControlPlaneFacts> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if live_original_source_r50(&path, None)? {
            let closed = crate::readonly_worker::run_live(
                &path,
                crate::readonly_worker::Operation::ControlFacts,
                None,
            )?;
            control_facts_from_worker_r50(closed.into_value())
        } else {
            original_read(&path, read_facts_native)
        }
    })
    .await
    .map_err(internal)?
}

fn read_facts_native(connection: &ReadConnection) -> Result<ControlPlaneFacts> {
    fn unreadable(context: &'static str) -> impl Fn(rusqlite::Error) -> Error {
        move |error| Error::Invalid(format!("{context}: {error}"))
    }
    let integrity = connection
        .prepare("PRAGMA integrity_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(unreadable("integrity check could not run"))?;
    if integrity.len() != 1 || integrity[0] != "ok" {
        let first = integrity.first().map(String::as_str).unwrap_or("no result");
        return Err(Error::Conflict(format!(
            "database integrity check failed: {}",
            first.chars().take(200).collect::<String>()
        )));
    }
    let migrations = connection
        .prepare(
            "SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(unreadable("migration set unreadable"))?;
    let compiled = sqlx::migrate!("./migrations");
    if migrations.is_empty()
        || migrations.iter().any(|row| row.3 == 0)
        || migrations.len() != compiled.iter().count()
        || migrations
            .iter()
            .zip(compiled.iter())
            .any(|(saved, current)| {
                saved.0 != current.version || saved.2 != current.checksum.as_ref()
            })
    {
        return Err(Error::Conflict(
            "migration set is not the compiled-in set".into(),
        ));
    }
    let mut watermarks = BTreeMap::new();
    let rows = connection
        .prepare("SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(unreadable("revoke watermarks unreadable"))?;
    for (namespace, seq, digest) in rows {
        watermarks.insert(namespace, WatermarkFact { seq, digest });
    }

    let mut tombstones = BTreeMap::new();
    let mut protected_objects = BTreeMap::new();
    let rows = connection
        .prepare("SELECT namespace,kind,id,body FROM objects ORDER BY namespace,kind,id")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(unreadable("objects unreadable"))?;
    for (namespace, kind, id, body) in rows {
        let in_scope = watermarks.contains_key(&namespace);
        if !in_scope && kind != "tombstone" {
            continue;
        }
        let value = parse_strict_json(&body).map_err(|error| {
            Error::Invalid(format!(
                "object {namespace}/{kind}/{id} has an unreadable body: {error}"
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
        if kind == "tombstone" {
            tombstones.insert((namespace.clone(), id.clone()), value.clone());
        }
        if in_scope
            && (PROTECTED_OBJECT_KINDS.contains(&kind.as_str())
                || schema.as_deref().is_some_and(is_protected_schema))
        {
            protected_objects.insert((namespace, kind, id), value);
        }
    }

    let mut tables = connection
        .prepare(ROOT_BUDGET_TABLE_QUERY)
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(unreadable("root budget tables unreadable"))?;
    tables.sort();
    let mut root_budget_tables = BTreeMap::new();
    for table in tables {
        let columns = connection
            .prepare("SELECT name FROM pragma_table_info(?) ORDER BY cid")
            .and_then(|mut statement| {
                statement
                    .query_map([&table], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
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
        let mut rows = connection
            .prepare(&format!(
                "SELECT {quoted} FROM {}",
                quote_identifier(&table),
            ))
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(unreadable("root budget rows unreadable"))?;
        rows.sort();
        root_budget_tables.insert(table, rows);
    }
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

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod directory_capability_tests {
    use super::*;

    #[test]
    fn the_fixture_clock_refuses_before_the_first_directory_member() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("unopened").join("db.sqlite3");
        let source = WallClock::fixture_seconds(Arc::new(|| -1));
        assert!(matches!(
            DirectoryWriterGuard::acquire_checked_with_clock(&data, &[], &source),
            Err(Error::ClockRegressed)
        ));
        assert!(!data_parent(&data).exists());
    }

    #[test]
    fn the_lock_recheck_uses_the_original_fixture_reader() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("db.sqlite3");
        let first = AtomicBool::new(true);
        let source = WallClock::fixture_seconds(Arc::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                100
            } else {
                -1
            }
        }));
        assert!(matches!(
            DirectoryWriterGuard::acquire_checked_with_clock(&data, &[], &source),
            Err(Error::ClockRegressed)
        ));
        assert!(!data.exists());
        assert!(!root.path().join(WRITER_LOCK_NAME).exists());
    }

    #[tokio::test]
    async fn the_preflight_worker_keeps_the_same_fixture_source() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("db.sqlite3");
        let value = Arc::new(AtomicI64::new(-1));
        let reader = Arc::clone(&value);
        let source = WallClock::fixture_seconds(Arc::new(move || reader.load(Ordering::SeqCst)));
        assert!(matches!(
            preflight_occurrence_clock_with_clock(&data, &[], source.clone()).await,
            Err(Error::ClockRegressed)
        ));
        value.store(100, Ordering::SeqCst);
        preflight_occurrence_clock_with_clock(&data, &[], source)
            .await
            .unwrap();
        assert!(!data.exists());
        assert!(!root.path().join(WRITER_LOCK_NAME).exists());
    }

    #[test]
    fn a_second_mount_cannot_reuse_an_existing_owner_but_owned_stores_can() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("first.sqlite3");
        let other = root.path().join("other.sqlite3");
        let first = DirectoryWriterGuard::acquire_new_mount_checked(&data, &[]).unwrap();
        assert!(matches!(
            DirectoryWriterGuard::acquire_new_mount_checked(&other, &[]),
            Err(Error::Conflict(_))
        ));
        let shared = DirectoryWriterGuard::acquire_checked(&other, &[]).unwrap();
        assert!(Arc::ptr_eq(&first, &shared));
        drop(shared);
        drop(first);
        assert!(DirectoryWriterGuard::acquire_new_mount_checked(&data, &[]).is_ok());
        assert!(!data.exists());
        assert!(!other.exists());
    }

    #[test]
    fn a_replaced_directory_is_not_the_retained_fd_authority() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("original");
        let data = directory.join("db.sqlite3");
        let guard = DirectoryWriterGuard::acquire_new_mount_checked(&data, &[]).unwrap();
        std::fs::rename(&directory, root.path().join("retained")).unwrap();
        std::fs::create_dir(&directory).unwrap();
        assert!(!guard.matches_data(&data));
    }

    #[test]
    fn a_replaced_compatibility_file_is_not_the_retained_fd_authority() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("db.sqlite3");
        let guard = DirectoryWriterGuard::acquire_new_mount_checked(&data, &[]).unwrap();
        let lock = root.path().join(WRITER_LOCK_NAME);
        std::fs::rename(&lock, root.path().join("old.lock")).unwrap();
        std::fs::write(&lock, []).unwrap();
        assert!(!guard.matches_data(&data));
    }

    use std::io::{Read as _, Write as _};

    type R34Pipe = std::thread::JoinHandle<std::io::Result<Vec<u8>>>;
    type R34Output = (std::process::ExitStatus, Vec<u8>, Vec<u8>);
    const R34_READY: &[u8] = b"RSIA_R34_READY\n";

    fn r34_drain(
        mut pipe: impl std::io::Read + Send + 'static,
        mut ready: Option<std::sync::mpsc::Sender<()>>,
    ) -> R34Pipe {
        std::thread::spawn(move || {
            let mut all = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = pipe.read(&mut chunk)?;
                if count == 0 {
                    break;
                }
                all.extend_from_slice(&chunk[..count]);
                if all.windows(R34_READY.len()).any(|part| part == R34_READY)
                    && let Some(sender) = ready.take()
                {
                    let _ = sender.send(());
                }
            }
            Ok(all)
        })
    }

    fn r34_join(pipe: Option<R34Pipe>) -> std::io::Result<Vec<u8>> {
        match pipe {
            Some(pipe) => pipe
                .join()
                .map_err(|_| std::io::Error::other("pipe thread panicked"))?,
            None => Ok(Vec::new()),
        }
    }

    struct R34Child {
        child: Option<std::process::Child>,
        stdout: Option<R34Pipe>,
        stderr: Option<R34Pipe>,
        ready: std::sync::mpsc::Receiver<()>,
    }

    impl R34Child {
        fn start(test: &str, case: &str, directory: &Path) -> Self {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test])
                .env("RSIA_R34_CASE", case)
                .env("RSIA_R34_DIR", directory)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let (sender, ready) = std::sync::mpsc::channel();
            let mut owner = Self {
                child: Some(child),
                stdout: None,
                stderr: None,
                ready,
            };
            let stdout = owner.child.as_mut().unwrap().stdout.take().unwrap();
            owner.stdout = Some(r34_drain(stdout, Some(sender)));
            let stderr = owner.child.as_mut().unwrap().stderr.take().unwrap();
            owner.stderr = Some(r34_drain(stderr, None));
            owner
        }

        fn reap(&mut self, kill: bool) -> std::io::Result<R34Output> {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let child = self.child.as_mut().unwrap();
            if kill {
                drop(child.stdin.take());
            }
            let mut observed = child.try_wait()?;
            if kill
                && observed.is_none()
                && let Err(error) = child.kill()
            {
                observed = child.try_wait()?;
                if observed.is_none() {
                    return Err(error);
                }
            }
            let status = loop {
                if let Some(status) = observed {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::other(
                        "child exit unconfirmed after reap deadline",
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
                observed = child.try_wait()?;
            };
            drop(self.child.take());
            let stdout = r34_join(self.stdout.take());
            let stderr = r34_join(self.stderr.take());
            Ok((status, stdout?, stderr?))
        }

        fn done(&mut self) -> std::io::Result<R34Output> {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                if self.child.as_mut().unwrap().try_wait()?.is_some() {
                    return self.reap(false);
                }
                if std::time::Instant::now() >= deadline {
                    let output = self.reap(true)?;
                    return Err(std::io::Error::other(format!("child deadline: {output:?}")));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    impl Drop for R34Child {
        fn drop(&mut self) {
            if self.child.is_some() && self.reap(true).is_err() {
                // No unconfirmed child may be followed by TempDir removal.
                // Abort is a failed test-binary terminal, never a passed case.
                std::process::abort();
            }
        }
    }

    fn r34_seed(directory: &Path) {
        std::fs::create_dir(directory).unwrap();
        let connection = ReadConnection::open(directory.join("db.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE r34_marker(value INTEGER); INSERT INTO r34_marker VALUES(7);",
            )
            .unwrap();
        connection.close().map_err(|(_, error)| error).unwrap();
    }

    fn r34_holder(case: &str, directory: &Path) {
        let data = directory.join("db.sqlite3");
        let read = |connection: &ReadConnection| -> Result<i64> {
            let value = connection
                .query_row("SELECT value FROM r34_marker", [], |row| row.get(0))
                .map_err(|_| Error::ClockRegressed)?;
            if case.contains("busy") {
                let statement = connection.prepare("SELECT value FROM r34_marker").unwrap();
                std::mem::forget(statement);
            }
            if case.contains("panic") {
                panic!("r34 callback panic");
            }
            if case.contains("primary") {
                return Err(Error::Invalid("r34 primary".into()));
            }
            Ok(value)
        };
        let outcome = if case.starts_with("borrowed") {
            let directory = directory.canonicalize().unwrap();
            let file = Arc::new(File::open(&directory).unwrap());
            file.try_lock_shared().unwrap();
            let outcome = {
                let scope = LockedOriginalSource {
                    directory: &directory,
                    file: &file,
                    legacy: None,
                };
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    original_read_scoped(&data, Some(&scope), read)
                }))
            };
            drop(file);
            outcome
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| original_read(&data, read)))
        };
        if case.contains("panic") {
            assert!(outcome.is_err());
        } else if case.contains("primary") {
            assert!(matches!(outcome, Ok(Err(Error::Invalid(value))) if value == "r34 primary"));
        } else if case.contains("busy") {
            assert!(matches!(outcome, Ok(Err(Error::ClockRegressed))));
        } else {
            assert!(matches!(outcome, Ok(Ok(7))));
        }
    }

    fn r34_close_test(local_name: &str, cases: &[&str]) {
        let module = module_path!().split_once("::").unwrap().1;
        let test = format!("{module}::{local_name}");
        if let Some(case) = std::env::var_os("RSIA_R34_CASE") {
            let case = case.to_str().unwrap();
            assert!(cases.contains(&case));
            let args: Vec<_> = std::env::args().collect();
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--exact" && pair[1] == test)
            );
            let directory = PathBuf::from(std::env::var_os("RSIA_R34_DIR").unwrap());
            r34_holder(case, &directory);
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(R34_READY).unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            let mut release = [0];
            std::io::stdin().read_exact(&mut release).unwrap();
            assert_eq!(release, [b'x']);
            return;
        }
        assert!(std::env::var_os("RSIA_R34_DIR").is_none());
        let root = tempfile::tempdir().unwrap();
        for case in cases {
            let directory = root.path().join(case);
            r34_seed(&directory);
            let mut child = R34Child::start(&test, case, &directory);
            child.ready.recv_timeout(Duration::from_secs(30)).unwrap();
            let alive_before = child.child.as_mut().unwrap().try_wait().unwrap().is_none();
            let observed = {
                let probe = File::open(&directory).unwrap();
                match probe.try_lock() {
                    Ok(()) => false,
                    Err(TryLockError::WouldBlock) => true,
                    Err(error) => panic!("directory lock probe failed: {error:?}"),
                }
            };
            let alive_after = child.child.as_mut().unwrap().try_wait().unwrap().is_none();
            child
                .child
                .as_mut()
                .unwrap()
                .stdin
                .take()
                .unwrap()
                .write_all(b"x")
                .unwrap();
            let output = child.done().unwrap();
            assert!(output.0.success(), "child {case}: {output:?}");
            assert!(
                alive_before && alive_after,
                "holder exited before the lock probe"
            );
            assert_eq!(observed, case.contains("busy"), "case {case}: {output:?}");
            {
                let after_exit = File::open(&directory).unwrap();
                after_exit.try_lock().unwrap();
            }
        }
        root.close().unwrap();
    }

    #[test]
    fn r34_owned_close_lifetime() {
        r34_close_test(
            "r34_owned_close_lifetime",
            &["owned-normal", "owned-panic", "owned-busy-panic"],
        );
    }

    #[test]
    fn r34_borrowed_close_lifetime() {
        r34_close_test(
            "r34_borrowed_close_lifetime",
            &[
                "borrowed-normal",
                "borrowed-panic",
                "borrowed-busy",
                "borrowed-primary-busy",
                "borrowed-busy-panic",
            ],
        );
    }

    #[test]
    fn readonly_shm_uri_preserves_unix_path_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/tmp/rsia ?%#\xc3\xa9\xff"));
        assert_eq!(
            original_read_shm_uri(path).unwrap(),
            "file:///tmp/rsia%20%3F%25%23%C3%A9%FF?readonly_shm=1"
        );
        assert!(matches!(
            original_read_shm_uri(Path::new("relative")),
            Err(Error::ClockRegressed)
        ));
        assert!(matches!(
            original_read_shm_uri(Path::new(OsStr::from_bytes(b"/tmp/bad\0name"))),
            Err(Error::ClockRegressed)
        ));
    }

    const WAL_BASE: i64 = 2_000_000_000;
    const WAL_FLOOR: i64 = WAL_BASE + 64;

    fn wal_clock(seconds: i64) -> WallClock {
        WallClock::fixture_seconds(Arc::new(move || seconds))
    }

    async fn wal_audit(store: &Store, name: &str) {
        let context = Context::new("wal-clock", "wal-admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session.audit(&context, name, "wal-fixture").await.unwrap();
        session.commit().await.unwrap();
    }

    fn wal_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn wal_seed(directory: &Path) -> Vec<u8> {
        std::fs::create_dir(directory).unwrap();
        let data = directory.join("db.sqlite3");
        wal_runtime().block_on(async {
            let store = Store::open_with_fixture_clock(&data, Vec::new(), wal_clock(WAL_BASE))
                .await
                .unwrap();
            wal_audit(&store, "wal.seed").await;
            {
                let mut native = store.pool.acquire().await.unwrap();
                sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&mut *native)
                    .await
                    .unwrap();
            }
            store.close().await;
        });
        assert_eq!(
            original_read(&data, |connection| {
                Ok(collect_floor_rusqlite(connection, &PreparePreimages::default())?.max_nanos())
            })
            .unwrap(),
            Some(i128::from(WAL_BASE) * 1_000_000_000)
        );
        std::fs::read(data).unwrap()
    }

    fn wal_live_child(directory: &Path) {
        use std::sync::atomic::{AtomicI64, Ordering};
        let seconds = Arc::new(AtomicI64::new(WAL_FLOOR));
        let actual = Arc::clone(&seconds);
        let clock = WallClock::fixture_seconds(Arc::new(move || actual.load(Ordering::SeqCst)));
        let runtime = wal_runtime();
        let store = runtime
            .block_on(Store::open_with_fixture_clock(
                &directory.join("db.sqlite3"),
                Vec::new(),
                clock,
            ))
            .unwrap();
        runtime.block_on(async {
            let mut native = store.pool.acquire().await.unwrap();
            sqlx::query("PRAGMA wal_autocheckpoint=0")
                .execute(&mut *native)
                .await
                .unwrap();
        });
        runtime.block_on(wal_audit(&store, "wal.first"));
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(R34_READY).unwrap();
        stdout.flush().unwrap();
        drop(stdout);
        let mut command = [0];
        std::io::stdin().read_exact(&mut command).unwrap();
        assert_eq!(command, [b'x']);
        seconds.store(WAL_FLOOR + 1, Ordering::SeqCst);
        runtime.block_on(wal_audit(&store, "wal.parallel"));
        // Keep a committed WAL instead of letting graceful close checkpoint it.
        std::process::exit(0);
    }

    fn wal_sample(data: &Path, value: i64) -> Result<i64> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let samples = Arc::new(AtomicUsize::new(0));
        let actual = Arc::clone(&samples);
        let clock = WallClock::fixture_seconds(Arc::new(move || {
            actual.fetch_add(1, Ordering::SeqCst);
            value
        }));
        let result = readonly_wall_seconds_with_clock(data, &[], &[], &clock);
        assert_eq!(samples.load(Ordering::SeqCst), 1);
        result
    }

    #[test]
    fn live_and_retained_wal_keep_the_qualified_floor_and_snapshot() {
        let name = "live_and_retained_wal_keep_the_qualified_floor_and_snapshot";
        let module = module_path!().split_once("::").unwrap().1;
        let test = format!("{module}::{name}");
        if let Some(case) = std::env::var_os("RSIA_R34_CASE") {
            assert_eq!(case, "wal-live");
            let args: Vec<_> = std::env::args().collect();
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--exact" && pair[1] == test)
            );
            wal_live_child(&PathBuf::from(std::env::var_os("RSIA_R34_DIR").unwrap()));
            unreachable!();
        }
        assert!(std::env::var_os("RSIA_R34_DIR").is_none());
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("live");
        let original = wal_seed(&directory);
        let data = directory.join("db.sqlite3");
        let copy = root.path().join("only-main");
        std::fs::create_dir(&copy).unwrap();
        std::fs::write(copy.join("db.sqlite3"), &original).unwrap();
        assert_eq!(
            wal_sample(&copy.join("db.sqlite3"), WAL_BASE).unwrap(),
            WAL_BASE
        );
        let mut child = R34Child::start(&test, "wal-live", &directory);
        child.ready.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(std::fs::read(&data).unwrap(), original);
        let wal = sidecar_path(&data, "-wal");
        let shm = sidecar_path(&data, "-shm");
        assert!(!std::fs::read(&wal).unwrap().is_empty());
        assert!(shm.is_file());
        // Force the same process to own a RW unixShmNode too. URI-only handling
        // cannot qualify this reader by opening a new read-only node.
        let rw = ReadConnection::open(&data).unwrap();
        assert!(
            rw.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .unwrap()
        );
        assert_eq!(
            rw.query_row("SELECT count(*) FROM audit", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let directory_bytes = || {
            std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let kind = entry.file_type().unwrap();
                    assert!(kind.is_file() && !kind.is_symlink());
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect::<std::collections::BTreeMap<std::ffi::OsString, Vec<u8>>>()
        };
        let live_directory = directory_bytes();
        let before = [
            std::fs::read(&data).unwrap(),
            std::fs::read(&wal).unwrap(),
            std::fs::read(&shm).unwrap(),
        ];
        assert!(matches!(
            wal_sample(&data, WAL_FLOOR - 1),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(wal_sample(&data, WAL_FLOOR).unwrap(), WAL_FLOOR);
        assert_eq!(
            [
                std::fs::read(&data).unwrap(),
                std::fs::read(&wal).unwrap(),
                std::fs::read(&shm).unwrap()
            ],
            before
        );
        assert_eq!(directory_bytes(), live_directory);
        let captured = crate::readonly_worker::run_live_with_ready(
            &data,
            crate::readonly_worker::Operation::RawSnapshot,
            None,
            || {
                child
                    .child
                    .as_mut()
                    .unwrap()
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"x")
                    .unwrap();
                let output = child.done().unwrap();
                assert!(output.0.success(), "child: {output:?}");
                Ok(())
            },
        )
        .unwrap();
        let raw = crate::owned_occurrence::raw_from_worker_r50(captured.into_value()).unwrap();
        let first =
            crate::owned_occurrence::collect_floor_from_raw_r50(&raw, &PreparePreimages::default())
                .unwrap()
                .max_nanos();
        // The worker verified the second native read after the actual writer
        // commit against its first native snapshot digest before closing.
        assert_eq!(first, Some(i128::from(WAL_FLOOR) * 1_000_000_000));
        rw.close().map_err(|(_, error)| error).unwrap();
        assert!(matches!(
            wal_sample(&data, WAL_FLOOR),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(wal_sample(&data, WAL_FLOOR + 1).unwrap(), WAL_FLOOR + 1);
        // After all real handles have closed, remove only the setup SHM. The
        // production stable-source reader must qualify committed WAL without
        // creating a new SHM or rewriting any original bytes.
        std::fs::remove_file(&shm).unwrap();
        let stable_directory = directory_bytes();
        let stable = [std::fs::read(&data).unwrap(), std::fs::read(&wal).unwrap()];
        assert!(matches!(
            wal_sample(&data, WAL_FLOOR),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(wal_sample(&data, WAL_FLOOR + 1).unwrap(), WAL_FLOOR + 1);
        assert!(!shm.exists());
        assert_eq!(
            [std::fs::read(&data).unwrap(), std::fs::read(&wal).unwrap()],
            stable
        );
        assert_eq!(directory_bytes(), stable_directory);
        root.close().unwrap();
    }

    #[test]
    fn live_diagnostics_preserve_complete_dynamic_accounting_facts() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("facts");
        wal_seed(&directory);
        let data = directory.join("db.sqlite3");
        let writer = ReadConnection::open(&data).unwrap();
        assert!(
            writer
                .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .unwrap()
        );
        writer.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE root_budget_forensic(i INTEGER,t TEXT,b BLOB);
             INSERT INTO root_budget_forensic VALUES(1,'1',x'00FF');"
        ).unwrap();
        let expected = writer
            .query_row(
                "SELECT quote(i)||','||quote(t)||','||quote(b) FROM root_budget_forensic",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let members = [
            data.clone(),
            sidecar_path(&data, "-wal"),
            sidecar_path(&data, "-shm"),
        ];
        let before = members
            .iter()
            .map(std::fs::read)
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        // Diagnostics do not require a caller's clock to qualify as a writer.
        assert!(wal_sample(&data, WAL_BASE - 1).is_err());
        let facts = wal_runtime()
            .block_on(read_control_plane_facts(&data))
            .unwrap();
        assert_eq!(
            facts.root_budget_tables["root_budget_forensic"],
            vec![expected]
        );
        assert_eq!(
            members
                .iter()
                .map(std::fs::read)
                .collect::<std::io::Result<Vec<_>>>()
                .unwrap(),
            before
        );
        writer.close().map_err(|(_, error)| error).unwrap();
        root.close().unwrap();
    }

    #[test]
    fn live_reader_unwind_and_cancel_wait_for_owned_child_drain() {
        let name = "live_reader_unwind_and_cancel_wait_for_owned_child_drain";
        let module = module_path!().split_once("::").unwrap().1;
        let test = format!("{module}::{name}");
        if let Some(case) = std::env::var_os("RSIA_R34_CASE") {
            let case = case.to_str().unwrap();
            assert!(["live-lock-held", "live-lock-free"].contains(&case));
            assert!(
                std::env::args()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|pair| pair[0] == "--exact" && pair[1] == test)
            );
            let directory = PathBuf::from(std::env::var_os("RSIA_R34_DIR").unwrap());
            let probe = File::open(directory).unwrap();
            match probe.try_lock() {
                Ok(()) => assert_eq!(case, "live-lock-free"),
                Err(TryLockError::WouldBlock) => assert_eq!(case, "live-lock-held"),
                Err(error) => panic!("directory lock probe failed: {error:?}"),
            }
            return;
        }
        assert!(std::env::var_os("RSIA_R34_DIR").is_none());
        fn probe(test: &str, directory: &Path, held: bool) {
            let case = if held {
                "live-lock-held"
            } else {
                "live-lock-free"
            };
            let mut child = R34Child::start(test, case, directory);
            let output = child.done().unwrap();
            assert!(output.0.success(), "lock probe: {output:?}");
        }
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("cancel");
        wal_seed(&directory);
        let data = directory.join("db.sqlite3");
        let writer = ReadConnection::open(&data).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE live_probe(value); INSERT INTO live_probe VALUES(1);").unwrap();
        assert!(sidecar_path(&data, "-shm").is_file());
        let mount = || {
            let mount =
                DirectoryWriterGuard::acquire_checked_with_clock(&data, &[], &wal_clock(WAL_BASE))
                    .unwrap();
            assert_eq!(Arc::strong_count(&mount), 1);
            mount
        };
        let retained = mount();
        let probed = std::cell::Cell::new(false);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::readonly_worker::run_live_with_ready(
                &data,
                crate::readonly_worker::Operation::RawSnapshot,
                Some(retained),
                || {
                    probe(&test, &directory, true);
                    probed.set(true);
                    panic!("parent abandoned an actual open reader")
                },
            )
        }));
        assert!(outcome.is_err());
        assert!(probed.get());
        probe(&test, &directory, false);
        // A fresh production worker succeeds after the abandoned one was reaped.
        assert!(
            crate::readonly_worker::run_live(
                &data,
                crate::readonly_worker::Operation::RawSnapshot,
                Some(mount()),
            )
            .is_ok()
        );
        let runtime = wal_runtime();
        let (ready, ready_rx) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = std::sync::mpsc::channel();
        let (done, done_rx) = std::sync::mpsc::channel();
        let path = data.clone();
        let retained = mount();
        let task = runtime.spawn(async move {
            tokio::task::spawn_blocking(move || {
                let result = crate::readonly_worker::run_live_with_ready(
                    &path,
                    crate::readonly_worker::Operation::RawSnapshot,
                    Some(retained),
                    || {
                        ready.send(()).map_err(|_| Error::Internal)?;
                        resume_rx.recv().map_err(internal)?;
                        Ok(())
                    },
                );
                let healthy = result.is_ok();
                let _ = done.send(healthy);
                result
            })
            .await
            .map_err(internal)?
        });
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), ready_rx)
                .await
                .unwrap()
                .unwrap();
        });
        task.abort();
        runtime.block_on(async {
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        });
        probe(&test, &directory, true);
        resume.send(()).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(30)).unwrap());
        probe(&test, &directory, false);
        drop(runtime);
        writer.close().map_err(|(_, error)| error).unwrap();
        root.close().unwrap();
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreReadRequestR50 {
    pub(super) path: Vec<u8>,
    pub(super) prepare_configs: Vec<HostPrepareConfig>,
    pub(super) extra_occurrences: Vec<i64>,
    pub(super) related_path: Option<Vec<u8>>,
}

fn restore_path_r50(bytes: Vec<u8>) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        if bytes.contains(&0) {
            return Err(Error::ClockRegressed);
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        if !path.is_absolute() {
            return Err(Error::ClockRegressed);
        }
        Ok(path)
    }
    #[cfg(not(unix))]
    {
        let _ = bytes;
        Err(Error::Invalid("unsupported restore platform".into()))
    }
}

fn restore_original_rows_r50(path: &Path) -> Result<crate::owned_occurrence::RestoreNativeRowsR50> {
    let file = restore_regular_file_r50(path)?;
    let value = if live_original_source_r50(path, None)? {
        crate::readonly_worker::run_live(
            path,
            crate::readonly_worker::Operation::RestoreSnapshot,
            None,
        )?
        .into_value()
    } else {
        original_read(path, crate::owned_occurrence::restore_worker_value_r50)?
    };
    if !same_regular_file(&file, path).map_err(internal)? {
        return Err(Error::ClockRegressed);
    }
    crate::owned_occurrence::restore_value_from_worker_r50(value)
}

fn restore_qualify_rows_r50(
    rows: &crate::owned_occurrence::RestoreNativeRowsR50,
    inputs: &PreparePreimages,
    extra_occurrences: &[i64],
    clock: &mut OperationClock,
) -> Result<()> {
    let mut floor = crate::owned_occurrence::collect_floor_from_raw_r50(&rows.raw, inputs)
        .map_err(|_| clock.fail())?;
    for value in extra_occurrences {
        floor.observe_seconds(*value).map_err(|_| clock.fail())?;
    }
    let stamp = clock.nanos(&floor)?;
    let _ = clock.take_nanos(stamp)?;
    Ok(())
}

pub(super) fn restore_read_r50(request: RestoreReadRequestR50) -> Result<serde_json::Value> {
    let path = restore_path_r50(request.path)?;
    restore_check_ancestors_r50(&path, false)?;
    for config in &request.prepare_configs {
        config.validate()?;
    }
    let inputs = PreparePreimages {
        candidates: request.prepare_configs,
        ..Default::default()
    };
    let rows = restore_original_rows_r50(&path)?;
    let related = request
        .related_path
        .map(restore_path_r50)
        .transpose()?
        .map(|path| {
            restore_check_ancestors_r50(&path, false)?;
            restore_original_rows_r50(&path)
        })
        .transpose()?;
    let mut floor = if let Some(related) = &related {
        crate::owned_occurrence::restore_union_floor_r50(&rows.raw, &related.raw, &inputs, &[])?
    } else {
        crate::owned_occurrence::collect_floor_from_raw_r50(&rows.raw, &inputs)?
    };
    for value in request.extra_occurrences {
        floor.observe_seconds(value)?;
    }
    let sample = admitted_sample(
        &floor,
        &mut OperationClock::with_source(WallClock::system()),
    )?;
    let (seconds, nanos) = sample.into_parts();
    let mut facts = rows.facts;
    facts.as_object_mut().ok_or(Error::ClockRegressed)?.insert(
        "wall".into(),
        serde_json::json!({"seconds":seconds,"nanos":nanos}),
    );
    Ok(facts)
}

fn restore_check_ancestors_r50(path: &Path, missing_last: bool) -> Result<()> {
    use std::path::Component;
    if !path.is_absolute() {
        return Err(Error::Invalid("restore path must be absolute".into()));
    }
    let mut current = PathBuf::new();
    let components: Vec<_> = path.components().collect();
    for (index, part) in components.iter().enumerate() {
        if matches!(part, Component::ParentDir | Component::CurDir) {
            return Err(Error::Invalid("non-canonical restore path".into()));
        }
        current.push(part.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::Invalid("symlink in restore path".into()));
            }
            Ok(_) => {}
            Err(error)
                if missing_last
                    && index + 1 == components.len()
                    && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(internal(error)),
        }
    }
    Ok(())
}

fn restore_regular_file_r50(path: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        restore_check_ancestors_r50(path, false)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
            .map_err(internal)?;
        let metadata = file.metadata().map_err(internal)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || !same_regular_file(&file, path).map_err(internal)?
        {
            return Err(Error::Invalid("linked or nonregular restore member".into()));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::Invalid("unsupported restore platform".into()))
    }
}

fn restore_relative_r50(root: &Path, relative: &str) -> Result<PathBuf> {
    use std::path::Component;
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
            != relative
    {
        return Err(Error::Invalid("unsafe restore member".into()));
    }
    let result = root.join(path);
    restore_check_ancestors_r50(&result, false)?;
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreEventR50 {
    seq: u64,
    previous_digest: String,
    digest: String,
    source_kind: String,
    source_id: String,
    tombstone_digest: String,
    reason: String,
    created_at: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreDeltaNamespaceR50 {
    namespace: String,
    base_seq: u64,
    base_digest: String,
    events: Vec<RestoreEventR50>,
    latest_seq: u64,
    latest_digest: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreDeltaR50 {
    schema_version: String,
    base_manifest_sha256: String,
    namespaces: Vec<RestoreDeltaNamespaceR50>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreCopyRequestR50 {
    backup: Vec<u8>,
    dest: Vec<u8>,
    anchor: Vec<u8>,
    manifest_sha256: String,
    delta: RestoreDeltaR50,
    prepare_configs: Vec<HostPrepareConfig>,
}

fn restore_rows_r50<'a>(
    facts: &'a serde_json::Value,
    key: &str,
) -> Result<&'a Vec<serde_json::Value>> {
    facts
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or(Error::ClockRegressed)
}
fn restore_tombstones_r50(
    facts: &serde_json::Value,
) -> Result<BTreeMap<(String, String), (RevokeTombstone, String)>> {
    let mut result = BTreeMap::new();
    for value in restore_rows_r50(facts, "objects")? {
        let row: (String, String, String, String) =
            serde_json::from_value(value.clone()).map_err(internal)?;
        if row.1 != "tombstone" {
            continue;
        }
        let body = parse_strict_json(&row.3)?;
        let tombstone: RevokeTombstone = serde_json::from_value(body).map_err(internal)?;
        if tombstone.id != row.2
            || tombstone.schema_version != REVOKE_TOMBSTONE_SCHEMA
            || !matches!(tombstone.source_kind.as_str(), "run" | "artifact")
            || result.insert((row.0, row.2), (tombstone, row.3)).is_some()
        {
            return Err(Error::ClockRegressed);
        }
    }
    Ok(result)
}

fn restore_events_r50(
    manifest: &BackupManifest,
    manifest_sha: &str,
    delta: &RestoreDeltaR50,
    backup: &serde_json::Value,
    anchor: &serde_json::Value,
) -> Result<Vec<(String, RestoreEventR50)>> {
    if delta.schema_version != "rsia.revoke_delta.v1"
        || delta.base_manifest_sha256 != manifest_sha
        || delta.namespaces.len() != manifest.watermarks.len()
    {
        return Err(Error::Invalid(
            "delta schema or backup binding differs".into(),
        ));
    }
    // The manifest describes these actual original rows, not caller claims.
    let migrations = restore_rows_r50(backup, "migrations")?;
    if migrations.len() != manifest.migrations.len()
        || manifest
            .migrations
            .iter()
            .zip(migrations)
            .any(|(expected, actual)| {
                actual["version"].as_i64() != Some(expected.version)
                    || actual["description"].as_str() != Some(expected.description.as_str())
                    || actual["checksum_hex"].as_str() != Some(expected.checksum_hex.as_str())
                    || actual["success"].as_i64() != Some(if expected.success { 1 } else { 0 })
            })
    {
        return Err(Error::Invalid(
            "backup migrations differ from manifest".into(),
        ));
    }
    let mut observed: Vec<(String, i64, String)> = restore_rows_r50(backup, "watermarks")?
        .iter()
        .map(|row| serde_json::from_value(row.clone()).map_err(internal))
        .collect::<Result<_>>()?;
    let mut declared: Vec<_> = manifest
        .watermarks
        .iter()
        .map(|row| (row.namespace.clone(), row.seq, row.digest.clone()))
        .collect();
    observed.sort();
    declared.sort();
    if observed != declared {
        return Err(Error::Invalid(
            "backup watermarks differ from manifest".into(),
        ));
    }
    let base: BTreeMap<_, _> = manifest
        .watermarks
        .iter()
        .map(|row| (row.namespace.clone(), (row.seq, row.digest.clone())))
        .collect();
    if base.len() != manifest.watermarks.len() || base.is_empty() {
        return Err(Error::ClockRegressed);
    }
    let latest: BTreeMap<String, (i64, String)> = restore_rows_r50(anchor, "watermarks")?
        .iter()
        .map(|value| {
            let (ns, seq, digest): (String, i64, String) =
                serde_json::from_value(value.clone()).map_err(internal)?;
            Ok((ns, (seq, digest)))
        })
        .collect::<Result<_>>()?;
    let old = restore_tombstones_r50(backup)?;
    let trusted = restore_tombstones_r50(anchor)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut output = Vec::new();
    for entry in &delta.namespaces {
        if !seen.insert(&entry.namespace) {
            return Err(Error::ClockRegressed);
        }
        let (seq, digest) = base.get(&entry.namespace).ok_or(Error::ClockRegressed)?;
        let (latest_seq, latest_digest) =
            latest.get(&entry.namespace).ok_or(Error::ClockRegressed)?;
        if u64::try_from(*seq).ok() != Some(entry.base_seq)
            || *digest != entry.base_digest
            || u64::try_from(*latest_seq).ok() != Some(entry.latest_seq)
            || *latest_digest != entry.latest_digest
        {
            return Err(Error::Invalid(
                "delta does not reach trusted latest watermark".into(),
            ));
        }
        let mut expected = Vec::new();
        for ((ns, id), (body, raw)) in &trusted {
            if ns != &entry.namespace {
                continue;
            }
            if body.watermark_seq <= entry.base_seq {
                let prior = old.get(&(ns.clone(), id.clone())).ok_or_else(|| {
                    Error::Invalid("backup omits an already consumed revocation".into())
                })?;
                if parse_strict_json(&prior.1)? != parse_strict_json(raw)? {
                    return Err(Error::Invalid(
                        "backup omits an already consumed revocation".into(),
                    ));
                }
            } else {
                expected.push((body.clone(), evo_core::hash(raw.as_bytes())));
            }
        }
        expected.sort_by_key(|value| value.0.watermark_seq);
        let mut previous = entry.base_digest.clone();
        let mut sequence = entry.base_seq;
        let mut events = Vec::new();
        for (body, raw_digest) in expected {
            sequence = sequence.checked_add(1).ok_or(Error::ClockRegressed)?;
            if body.watermark_seq != sequence {
                return Err(Error::ClockRegressed);
            }
            let event = RestoreEventR50 {
                seq: body.watermark_seq,
                previous_digest: previous,
                digest: body.watermark_digest.clone(),
                source_kind: body.source_kind,
                source_id: body.id,
                tombstone_digest: raw_digest,
                reason: body.reason,
                created_at: body.created_at,
            };
            previous = body.watermark_digest;
            events.push(event);
        }
        if sequence != entry.latest_seq || previous != entry.latest_digest || events != entry.events
        {
            return Err(Error::Invalid(
                "delta is incomplete or differs from trusted revocation events".into(),
            ));
        }
        output.extend(
            events
                .into_iter()
                .map(|event| (entry.namespace.clone(), event)),
        );
    }
    Ok(output)
}

fn restore_sql_literal_r50(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn restore_replay_sql_r50(events: &[(String, RestoreEventR50)]) -> Result<String> {
    // Fixed statements only. Caller data is never interpreted as SQL.
    let mut sql = String::new();
    for (namespace, event) in events {
        identifier(namespace)?;
        identifier(&event.source_id)?;
        let body = serde_json::to_string(&json!({
            "id":event.source_id,"schema_version":REVOKE_TOMBSTONE_SCHEMA,
            "source_kind":event.source_kind,"reason":event.reason,
            "watermark_seq":event.seq,"watermark_digest":event.digest,"created_at":event.created_at
        }))
        .map_err(internal)?;
        let identity = serde_json::to_vec(&(
            namespace,
            &event.source_kind,
            &event.source_id,
            event.seq,
            &event.digest,
        ))
        .map_err(internal)?;
        let job = format!("revoke-{}", &evo_core::hash(&identity)[..32]);
        let ns = restore_sql_literal_r50(namespace);
        let id = restore_sql_literal_r50(&event.source_id);
        let kind = restore_sql_literal_r50(&event.source_kind);
        let watermark = restore_sql_literal_r50(&event.digest);
        let job = restore_sql_literal_r50(&job);
        let body = restore_sql_literal_r50(&body);
        let details = restore_sql_literal_r50(
            &serde_json::to_string(&json!({
                "watermark_seq":event.seq,"watermark_digest":event.digest
            }))
            .map_err(internal)?,
        );
        sql.push_str(&format!(
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES({ns},'tombstone',{id},'restore',{body}) \
             ON CONFLICT(namespace,kind,id) DO UPDATE SET body=excluded.body,revision=objects.revision+1;\n\
             UPDATE revoke_watermark SET seq={},digest={watermark} WHERE namespace={ns};\n\
             INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,state,created_at,updated_at) \
             VALUES({ns},{job},{kind},{id},{},{watermark},'pending',{},{});\n",
             event.seq,event.seq,event.created_at,event.created_at
        ));
        for node_kind in ["__budget_scan", "__world_scan", event.source_kind.as_str()] {
            sql.push_str(&format!(
                "INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id) \
                 VALUES({ns},{job},{},{id});\n",
                restore_sql_literal_r50(node_kind)
            ));
        }
        sql.push_str(&format!(
            "INSERT INTO revoke_cleanup_events(namespace,job_id,node_kind,node_id,event_kind,event_at,details) \
             VALUES({ns},{job},{kind},{id},'logical_block_restored',{},{});\n",
             event.created_at, details
        ));
    }
    Ok(sql)
}

fn restore_manifest_r50(backup: &Path, expected_sha: &str) -> Result<BackupManifest> {
    use std::io::Read;
    let mut file = restore_regular_file_r50(&backup.join("backup-manifest.json"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(internal)?;
    if evo_core::hash(&bytes) != expected_sha {
        return Err(Error::Invalid("backup manifest changed".into()));
    }
    let value =
        crate::owned_occurrence::parse_owned_json(std::str::from_utf8(&bytes).map_err(internal)?)?;
    let manifest: BackupManifest = serde_json::from_value(value).map_err(internal)?;
    let compiled = sqlx::migrate!("./migrations");
    let versions: Vec<i64> = compiled.iter().map(|migration| migration.version).collect();
    let actual: Vec<i64> = manifest
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect();
    if manifest.schema_version != BACKUP_SCHEMA
        || !manifest.complete
        || manifest.database.path != "rsia.sqlite3"
        || manifest.created_at < 0
        || actual.is_empty()
        || actual.len() > versions.len()
        || actual != versions[..actual.len()]
        || actual.last().copied().unwrap_or(0) < 5
        || manifest
            .migrations
            .iter()
            .zip(compiled.iter())
            .any(|(old, code)| !old.success || old.checksum_hex != hex(code.checksum.as_ref()))
    {
        return Err(Error::Invalid("unsupported backup migration prefix".into()));
    }
    Ok(manifest)
}
fn restore_packet_r50(backup: &Path, manifest: &BackupManifest) -> Result<Vec<BackupFileEntry>> {
    use std::collections::BTreeSet;
    use std::io::Read;
    let mut allowed_files = BTreeSet::from(["backup-manifest.json".to_owned()]);
    let mut allowed_dirs = BTreeSet::new();
    let mut output = Vec::new();
    for (index, entry) in std::iter::once(&manifest.database)
        .chain(&manifest.blobs)
        .enumerate()
    {
        if !allowed_files.insert(entry.path.clone())
            || (index > 0
                && (entry.path.split('/').count() != 3
                    || !entry.path.starts_with("blobs/")
                    || Path::new(&entry.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        != Some(entry.sha256.as_str())))
        {
            return Err(Error::Invalid("duplicate or invalid backup member".into()));
        }
        for parent in Path::new(&entry.path).ancestors().skip(1) {
            if !parent.as_os_str().is_empty() {
                allowed_dirs.insert(parent.to_path_buf());
            }
        }
        let path = restore_relative_r50(backup, &entry.path)?;
        let mut file = restore_regular_file_r50(&path)?;
        if file.metadata().map_err(internal)?.len() != entry.bytes {
            return Err(Error::Invalid("member bytes or digest mismatch".into()));
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(internal)?;
        if u64::try_from(data.len()).map_err(internal)? != entry.bytes
            || evo_core::hash(&data) != entry.sha256
        {
            return Err(Error::Invalid("member bytes or digest mismatch".into()));
        }
        output.push(entry.clone());
    }
    let mut stack = vec![backup.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).map_err(internal)? {
            let entry = entry.map_err(internal)?;
            let path = entry.path();
            let relative = path.strip_prefix(backup).map_err(internal)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(internal)?;
            if metadata.file_type().is_symlink() {
                return Err(Error::Invalid("linked backup member".into()));
            }
            if metadata.is_dir() {
                if !allowed_dirs.contains(relative) {
                    return Err(Error::Invalid("linked or unlisted backup directory".into()));
                }
                stack.push(path);
            } else if !metadata.is_file()
                || !allowed_files.contains(relative.to_str().ok_or(Error::ClockRegressed)?)
            {
                return Err(Error::Invalid("unlisted backup file".into()));
            }
        }
    }
    Ok(output)
}

fn restore_private_owner_r50(
    temp: &Path,
    mut before: impl FnMut() -> Result<()>,
) -> Result<Arc<DirectoryWriterGuard>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
        // mkdir is atomic and refuses EEXIST: this owner creates the namespace.
        before()?;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(temp)
            .map_err(internal)?;
        let directory = Arc::new(File::open(temp).map_err(internal)?);
        let metadata = directory.metadata().map_err(internal)?;
        if metadata.mode() & 0o777 != 0o700
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || !same_directory(&directory, temp).map_err(internal)?
        {
            return Err(Error::Invalid("insecure private restore directory".into()));
        }
        try_exclusive(&directory).map_err(internal)?;
        before()?;
        let legacy = Arc::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(temp.join(WRITER_LOCK_NAME))
                .map_err(internal)?,
        );
        try_exclusive(&legacy).map_err(internal)?;
        Ok(Arc::new(DirectoryWriterGuard {
            directory: temp.to_path_buf(),
            _directory_file: directory,
            _legacy_file: legacy,
            activity: Arc::new(RwLock::new(())),
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = temp;
        Err(Error::Invalid("unsupported restore platform".into()))
    }
}
fn restore_fresh_fs_r50(
    backup: &crate::owned_occurrence::RestoreNativeRowsR50,
    anchor: &crate::owned_occurrence::RestoreNativeRowsR50,
    inputs: &PreparePreimages,
    manifest_time: i64,
    clock: &mut OperationClock,
) -> Result<()> {
    let floor = crate::owned_occurrence::restore_union_floor_r50(
        &backup.raw,
        &anchor.raw,
        inputs,
        &[manifest_time],
    )
    .map_err(|_| clock.fail())?;
    let stamp = clock.nanos(&floor)?;
    let _ = clock.take_nanos(stamp)?;
    Ok(())
}
async fn restore_copy_transaction_r50(
    connection: &mut sqlx::SqliteConnection,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
    events: &[(String, RestoreEventR50)],
) -> Result<crate::owned_occurrence::RestoreNativeRowsR50> {
    if !events.is_empty() {
        let sql = restore_replay_sql_r50(events)?;
        let mut tx = connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let result: Result<()> = async {
            crate::write_clock::seconds_in_transaction(&mut tx, inputs, clock).await?;
            // The original fixed replay statements form one logical batch.
            // No async gap exposes an unqualified intermediate tombstone prefix.
            sqlx::raw_sql(&sql)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            if let Err(rollback) = tx.rollback().await {
                clock.record_rollback_error(rollback);
            }
            return Err(error);
        }
        crate::write_clock::finish_transaction(tx, inputs, clock, true)
            .await
            .map_err(crate::write_clock::TransactionFailure::into_error)?;
    }
    let mut tx = connection.begin().await.map_err(internal)?;
    let value = crate::owned_occurrence::restore_sqlx_value_r50(&mut tx).await;
    let rollback = tx.rollback().await.map_err(internal);
    let value = match (value, rollback) {
        (Err(error), _) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Ok(value), Ok(())) => value,
    };
    let rows = crate::owned_occurrence::restore_value_from_worker_r50(value)?;
    restore_qualify_rows_r50(&rows, inputs, &[], clock)?;
    Ok(rows)
}

pub(super) async fn restore_copy_r50(request: RestoreCopyRequestR50) -> Result<serde_json::Value> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    };
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Err(Error::Invalid("unsupported restore platform".into()));
    }
    #[cfg(unix)]
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let backup = restore_path_r50(request.backup)?;
    let destination = restore_path_r50(request.dest)?;
    let anchor_path = restore_path_r50(request.anchor)?;
    restore_check_ancestors_r50(&backup, false)?;
    restore_check_ancestors_r50(&anchor_path, false)?;
    restore_check_ancestors_r50(&destination, true)?;
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(Error::Conflict("dest exists; refusing overwrite".into()));
    }
    if anchor_path.starts_with(&backup)
        || anchor_path
            .parent()
            .is_some_and(|parent| parent.join("backup-manifest.json").exists())
    {
        return Err(Error::Invalid(
            "revocation anchor must be independent of backup".into(),
        ));
    }
    let manifest = restore_manifest_r50(&backup, &request.manifest_sha256)?;
    let packet = restore_packet_r50(&backup, &manifest)?;
    for config in &request.prepare_configs {
        config.validate()?;
    }
    let inputs = PreparePreimages {
        candidates: request.prepare_configs,
        ..Default::default()
    };
    let backup_rows = restore_original_rows_r50(&backup.join("rsia.sqlite3"))?;
    let anchor_rows = restore_original_rows_r50(&anchor_path)?;
    let events = restore_events_r50(
        &manifest,
        &request.manifest_sha256,
        &request.delta,
        &backup_rows.facts,
        &anchor_rows.facts,
    )?;
    let mut clock = OperationClock::with_source(WallClock::system());
    restore_fresh_fs_r50(
        &backup_rows,
        &anchor_rows,
        &inputs,
        manifest.created_at,
        &mut clock,
    )?;
    let parent = destination.parent().ok_or(Error::ClockRegressed)?;
    let name = destination
        .file_name()
        .ok_or(Error::ClockRegressed)?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.restore-{}", uuid::Uuid::new_v4().simple()));
    let guard = restore_private_owner_r50(&temp, || {
        restore_fresh_fs_r50(
            &backup_rows,
            &anchor_rows,
            &inputs,
            manifest.created_at,
            &mut clock,
        )
    })?;
    let result: Result<serde_json::Value> = async {
        for entry in &packet {
            // Retain one verified member at a time, never all backup blobs.
            let source = restore_relative_r50(&backup, &entry.path)?;
            let mut original = restore_regular_file_r50(&source)?;
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut original, &mut data).map_err(internal)?;
            if u64::try_from(data.len()).map_err(internal)? != entry.bytes
                || evo_core::hash(&data) != entry.sha256
            {
                return Err(Error::Invalid("backup member changed before copy".into()));
            }
            let target = temp.join(&entry.path);
            restore_fresh_fs_r50(
                &backup_rows,
                &anchor_rows,
                &inputs,
                manifest.created_at,
                &mut clock,
            )?;
            #[cfg(unix)]
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(target.parent().ok_or(Error::ClockRegressed)?)
                .map_err(internal)?;
            #[cfg(not(unix))]
            return Err(Error::Invalid("unsupported restore platform".into()));
            restore_fresh_fs_r50(
                &backup_rows,
                &anchor_rows,
                &inputs,
                manifest.created_at,
                &mut clock,
            )?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
            let mut file = options.open(&target).map_err(internal)?;
            restore_fresh_fs_r50(
                &backup_rows,
                &anchor_rows,
                &inputs,
                manifest.created_at,
                &mut clock,
            )?;
            file.write_all(&data).map_err(internal)?;
            file.sync_all().map_err(internal)?;
            if file.metadata().map_err(internal)?.len() != entry.bytes {
                return Err(Error::Invalid("copied member digest differs".into()));
            }
        }
        let data = temp.join("rsia.sqlite3");
        let source_versions: Vec<i64> = manifest.migrations.iter().map(|row| row.version).collect();
        let compiled = sqlx::migrate!("./migrations");
        let applied: Vec<serde_json::Value> = compiled
            .iter()
            .filter(|migration| !source_versions.contains(&migration.version))
            .map(|migration| {
                json!({"version":migration.version,
                "checksum_sha384":hex(migration.checksum.as_ref())})
            })
            .collect();
        let pool = super::NativePool::new(Arc::clone(&guard));
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&data)
            .create_if_missing(false)
            .foreign_keys(true)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5))
            .disable_statement_logging();
        // Original-source admission precedes the driver's neutral initialization.
        restore_fresh_fs_r50(
            &backup_rows,
            &anchor_rows,
            &inputs,
            manifest.created_at,
            &mut clock,
        )?;
        let initialized = pool.initialize(options, clock).await;
        let mut clock = match initialized {
            Ok(clock) => clock,
            Err(error) => {
                let close = pool.close().await;
                if close.is_err() {
                    std::mem::forget(Arc::clone(&guard));
                }
                return Err(error);
            }
        };
        let acquired = pool.acquire().await;
        let mut connection = match acquired {
            Ok(connection) => connection,
            Err(error) => {
                if pool.close().await.is_err() {
                    std::mem::forget(Arc::clone(&guard));
                }
                return Err(internal(error));
            }
        };
        let changed: Result<crate::owned_occurrence::RestoreNativeRowsR50> = async {
            crate::write_clock::seconds_on_connection(&mut connection, &inputs, &mut clock).await?;
            if let Err(failure) =
                crate::migration_clock::run_migrations(&mut connection, &inputs, &mut clock).await
            {
                tracing::error!(migration_error=?failure.migration_error,
                    rollback_error=?failure.rollback_error,
                    committed_versions=?failure.committed_versions,
                    commit_outcome_unknown=failure.commit_outcome_unknown,
                    "private restore migrations stopped");
                return Err(failure.error);
            }
            restore_copy_transaction_r50(&mut connection, &inputs, &mut clock, &events).await
        }
        .await;
        let release = connection.release().await.map_err(internal);
        let close = pool.close().await.map_err(internal);
        if close.is_err() {
            std::mem::forget(Arc::clone(&guard));
        }
        let rows = match (changed, release, close) {
            (Err(error), _, _) => return Err(error),
            (Ok(_), Err(error), _) => return Err(error),
            (Ok(_), Ok(()), Err(error)) => return Err(error),
            (Ok(rows), Ok(()), Ok(())) => rows,
        };
        // The private database is now actually closed under our live directory FD.
        // No path, caller boolean or serialized stamp creates this writer owner.
        if !guard.matches_data(&data) {
            return Err(Error::ClockRegressed);
        }
        restore_qualify_rows_r50(&rows, &inputs, &[], &mut clock)?;
        std::fs::remove_file(temp.join(WRITER_LOCK_NAME)).map_err(internal)?;
        let mut db = restore_regular_file_r50(&data)?;
        let metadata = db.metadata().map_err(internal)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut db, &mut bytes).map_err(internal)?;
        guard._directory_file.sync_all().map_err(internal)?;
        #[cfg(unix)]
        return Ok(json!({
            "private_path":temp.as_os_str().as_bytes(),
            "directory":{"dev":guard._directory_file.metadata().map_err(internal)?.dev(),
                "ino":guard._directory_file.metadata().map_err(internal)?.ino(),
                "uid":guard._directory_file.metadata().map_err(internal)?.uid(),"mode":0o700},
            "database":{"dev":metadata.dev(),"ino":metadata.ino(),"bytes":metadata.len(),
                "sha256":evo_core::hash(&bytes)},
            "applied":applied,"source_versions":source_versions
        }));
        #[cfg(not(unix))]
        Err(Error::Invalid("unsupported restore platform".into()))
    }
    .await;
    // Failure preserves a private copy for diagnosis; never remove a DB whose
    // close may be unknown. It is never published or granted admission here.
    result
}
