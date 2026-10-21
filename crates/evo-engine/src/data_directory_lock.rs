//! Shared writer admission for the data directory (plan §7.3.1).
//! The storage-owned capability retains the actual OS locks through native close.
use crate::startup_gate::{GateDecision, StartupGate, StartupGateError};
use evo_core::prepare_contract::HostPrepareConfig;
use evo_storage::lifecycle::DirectoryWriterGuard;
use std::{path::Path, sync::Arc};

#[derive(Debug)]
pub struct DataDirectoryLock {
    guard: Arc<DirectoryWriterGuard>,
}

impl DataDirectoryLock {
    pub fn acquire(data: &Path) -> std::io::Result<Self> {
        Self::acquire_checked(data, &[]).map_err(|error| {
            let kind = match &error {
                evo_core::Error::Conflict(_) => std::io::ErrorKind::WouldBlock,
                evo_core::Error::Invalid(_) => std::io::ErrorKind::InvalidInput,
                _ => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, error)
        })
    }

    pub fn acquire_checked(
        data: &Path,
        candidates: &[HostPrepareConfig],
    ) -> evo_core::Result<Self> {
        DirectoryWriterGuard::acquire_new_mount_checked(data, candidates)
            .map(|guard| Self { guard })
    }

    /// Transfer real lock ownership to Store/operation/close owners, not a
    /// caller-supplied stability or authorization flag.
    pub fn guard(&self) -> Arc<DirectoryWriterGuard> {
        Arc::clone(&self.guard)
    }
}

pub async fn mount_gate(
    data: &Path,
    trusted_revocations_db: Option<&Path>,
) -> std::result::Result<(DataDirectoryLock, GateDecision), StartupGateError> {
    mount_gate_with_prepare_configs(data, trusted_revocations_db, &[]).await
}

pub async fn mount_gate_with_prepare_configs(
    data: &Path,
    trusted_revocations_db: Option<&Path>,
    candidates: &[HostPrepareConfig],
) -> std::result::Result<(DataDirectoryLock, GateDecision), StartupGateError> {
    let gate = StartupGate::new(data, trusted_revocations_db).with_prepare_configs(candidates)?;
    // Before mkdir/compatibility-lock creation or any database initialization.
    gate.evaluate().await?;
    let lock = DataDirectoryLock::acquire_checked(data, candidates)
        .map_err(|error| StartupGateError::Rejected(error.to_string()))?;
    // Requalify the actual original source under the real acquired OS locks.
    let decision = gate.evaluate().await?;
    Ok((lock, decision))
}
