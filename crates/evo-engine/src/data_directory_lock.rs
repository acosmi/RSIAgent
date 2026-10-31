//! Shared writer admission for the data directory (plan §7.3.1).
//! Both gate evaluations are read-only; acquiring the lock happens between them.
//! The returned guard must outlive every database operation and connection close.
use crate::startup_gate::{
    DATA_LOCK_FILE, GateDecision, StartupGate, StartupGateError, data_directory,
};
use evo_storage::{LocalPermissionError, LocalPermissionTarget, check_local_permissions};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Error, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

#[derive(Debug)]
pub struct DataDirectoryLock {
    _file: File,
}

impl DataDirectoryLock {
    pub fn acquire(data: &Path) -> std::io::Result<Self> {
        check_local_permissions(LocalPermissionTarget::Database(data)).map_err(|error| match error {
            LocalPermissionError::Io(error) => error,
            error @ LocalPermissionError::Insecure => Error::new(ErrorKind::PermissionDenied, error),
            error @ LocalPermissionError::Unsupported => Error::new(ErrorKind::Unsupported, error),
        })?;
        Self::acquire_checked(data)
    }

    // mount_gate has already performed its read-only permission preflight.
    // A separate public acquire still performs its own boundary check above.
    fn acquire_checked(data: &Path) -> std::io::Result<Self> {
        let directory = data_directory(data);
        if !directory.try_exists()? {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(&directory)?;
        }
        let directory = directory.canonicalize()?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(directory.join(DATA_LOCK_FILE))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::new(
                ErrorKind::WouldBlock,
                format!(
                    "data directory is already locked by another RSIA process: {}",
                    directory.display()
                ),
            )),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

pub async fn mount_gate(
    data: &Path,
    trusted_revocations_db: Option<&Path>,
) -> std::result::Result<(DataDirectoryLock, GateDecision), StartupGateError> {
    let gate = StartupGate::new(data, trusted_revocations_db);
    gate.evaluate().await?;
    let lock = DataDirectoryLock::acquire_checked(data)
        .map_err(|error| StartupGateError::Rejected(error.to_string()))?;
    let decision = gate.evaluate().await?;
    Ok((lock, decision))
}
