//! Shared writer admission for the data directory (plan §7.3.1).
//! Both gate evaluations are read-only; acquiring the lock happens between them.
//! The returned guard must outlive every database operation and connection close.
use crate::startup_gate::{
    DATA_LOCK_FILE, GateDecision, StartupGate, StartupGateError, data_directory,
};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Error, ErrorKind};
use std::path::Path;

#[derive(Debug)]
pub struct DataDirectoryLock {
    _file: File,
}

impl DataDirectoryLock {
    pub fn acquire(data: &Path) -> std::io::Result<Self> {
        let directory = data_directory(data);
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(directory.join(DATA_LOCK_FILE))?;
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
    let lock = DataDirectoryLock::acquire(data)
        .map_err(|error| StartupGateError::Rejected(error.to_string()))?;
    let decision = gate.evaluate().await?;
    Ok((lock, decision))
}
