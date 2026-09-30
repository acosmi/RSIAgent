//! MVP capacity, persistent recovery verification, and deployment safety gates.
//! Exceeding capacity limits refuses new derive; it never silent-truncates.
use crate::executor::IsolationPolicy;
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;

pub const MAX_RUNS: u64 = 1_000;
pub const MAX_EVENTS: u64 = 10_000;
pub const MAX_SKILLS: u64 = 1_000;
pub const MAX_PREPARE: u64 = 5;

// v4.1 capacity bounds
pub const MAX_EXPLORATION_NODES: u64 = 500;
pub const MAX_REPLAY_WORLDS: u64 = 100;
pub const MAX_ACTIVE_LEASES: u64 = 10;
pub const MAX_STAGED_PACKAGES: u64 = 20;
pub const MAX_CONCURRENT_DISPATCH: u64 = 1;

/// The 16-byte magic header every SQLite 3 database file starts with.
const SQLITE_HEADER: [u8; 16] = *b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub runs: u64,
    pub events: u64,
    pub skills: u64,
    pub inflight_prepare: u64,
}

pub fn admit(u: Usage) -> Result<()> {
    if u.runs >= MAX_RUNS
        || u.events >= MAX_EVENTS
        || u.skills >= MAX_SKILLS
        || u.inflight_prepare >= MAX_PREPARE
    {
        return Err(Error::Conflict(
            "MVP capacity exceeded; stop new derive and scale or clean up".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct V41CapacityUsage {
    pub runs: u64,
    pub events: u64,
    pub skills: u64,
    pub inflight_prepare: u64,
    pub exploration_nodes: u64,
    pub replay_worlds: u64,
    pub active_leases: u64,
    pub staged_packages: u64,
    pub concurrent_dispatches: u64,
}

pub fn admit_v41(u: &V41CapacityUsage) -> Result<()> {
    if u.runs >= MAX_RUNS {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: runs {} >= limit {}",
            u.runs, MAX_RUNS
        )));
    }
    if u.events >= MAX_EVENTS {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: events {} >= limit {}",
            u.events, MAX_EVENTS
        )));
    }
    if u.skills >= MAX_SKILLS {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: skills {} >= limit {}",
            u.skills, MAX_SKILLS
        )));
    }
    if u.inflight_prepare >= MAX_PREPARE {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: in-flight prepare {} >= limit {}",
            u.inflight_prepare, MAX_PREPARE
        )));
    }
    if u.exploration_nodes >= MAX_EXPLORATION_NODES {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: exploration nodes {} >= limit {}",
            u.exploration_nodes, MAX_EXPLORATION_NODES
        )));
    }
    if u.replay_worlds >= MAX_REPLAY_WORLDS {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: replay worlds {} >= limit {}",
            u.replay_worlds, MAX_REPLAY_WORLDS
        )));
    }
    if u.active_leases >= MAX_ACTIVE_LEASES {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: active leases {} >= limit {}",
            u.active_leases, MAX_ACTIVE_LEASES
        )));
    }
    if u.staged_packages >= MAX_STAGED_PACKAGES {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: staged packages {} >= limit {}",
            u.staged_packages, MAX_STAGED_PACKAGES
        )));
    }
    if u.concurrent_dispatches > MAX_CONCURRENT_DISPATCH {
        return Err(Error::Conflict(format!(
            "MVP capacity exceeded: concurrent dispatches {} > limit {}",
            u.concurrent_dispatches, MAX_CONCURRENT_DISPATCH
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryStateManifest {
    pub backup_timestamp: i64,
    pub trusted_revocation_watermark: Option<u64>,
    pub consumed_queries: u64,
    pub total_exposure_count: u64,
    pub dispatched_expenses_incurred: i64,
    pub uncertain_dispatches_count: usize,
    pub has_early_stopping_ticket: bool,
    pub revoked_sources: Vec<String>,
    pub installed_packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAudit {
    pub recovery_allowed: bool,
    pub restored_watermark: u64,
    pub can_promote_early_stopping: bool,
    pub refunded_uncertain_costs: bool,
    pub notes: Vec<String>,
}

/// Verify that a backup manifest may be restored over the live system.
///
/// `previous_*` are the live system's already-consumed facts; the ledger rule is
/// that consumed query, exposure, dispatch and expense facts never rewind.
/// `previously_revoked_sources` are ids the live system has already revoked; a
/// backup that forgets any of them would resurrect it, so it enters quarantine.
pub fn verify_recovery_state(
    manifest: &RecoveryStateManifest,
    previous_watermark: u64,
    previous_consumed_queries: u64,
    previous_exposure_count: u64,
    previous_spent: i64,
    previously_revoked_sources: &[String],
) -> Result<RecoveryAudit> {
    // 1. Watermark check (V017, V018): Missing watermark enters quarantine
    let wm = manifest.trusted_revocation_watermark.ok_or_else(|| {
        Error::Invalid("recovery_quarantine: missing trusted revocation watermark in backup".into())
    })?;

    if wm < previous_watermark {
        return Err(Error::Conflict(format!(
            "recovery_quarantine: backup watermark {} is behind trusted live watermark {}",
            wm, previous_watermark
        )));
    }

    // 2. Content closure (V075): a backup can never drop an already revoked source
    for id in previously_revoked_sources {
        if !manifest.revoked_sources.contains(id) {
            return Err(Error::Conflict(format!(
                "recovery_quarantine: backup drops revoked source '{id}'"
            )));
        }
    }

    // 3. Query consumption, exposure and financial accounting cannot be rewound
    if manifest.consumed_queries < previous_consumed_queries {
        return Err(Error::Conflict(format!(
            "accounting_violation: recovery cannot rewind consumed queries from {} to {}",
            previous_consumed_queries, manifest.consumed_queries
        )));
    }
    if manifest.total_exposure_count < previous_exposure_count {
        return Err(Error::Conflict(format!(
            "accounting_violation: recovery cannot rewind exposure count from {} to {}",
            previous_exposure_count, manifest.total_exposure_count
        )));
    }
    if manifest.dispatched_expenses_incurred < previous_spent {
        return Err(Error::Conflict(format!(
            "accounting_violation: recovery cannot rewind spent funds from {} to {}",
            previous_spent, manifest.dispatched_expenses_incurred
        )));
    }

    // 4. Early stopping tickets remain unpromotable after recovery
    let can_promote_early_stopping = !manifest.has_early_stopping_ticket;

    // 5. Uncertain calls stay uncertain and are not refunded
    let refunded_uncertain_costs = false;

    Ok(RecoveryAudit {
        recovery_allowed: true,
        restored_watermark: wm,
        can_promote_early_stopping,
        refunded_uncertain_costs,
        notes: vec![
            format!("Restored trusted revocation watermark at {}", wm),
            format!(
                "Maintained {} consumed queries, {} exposures and {} spent facts",
                manifest.consumed_queries,
                manifest.total_exposure_count,
                manifest.dispatched_expenses_incurred
            ),
            format!(
                "Preserved all {} previously revoked sources ({} revoked in backup)",
                previously_revoked_sources.len(),
                manifest.revoked_sources.len()
            ),
        ],
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSecurityConfig {
    pub sandbox_enabled: bool,
    pub allow_code_execution: bool,
    pub allow_external_network: bool,
    pub trusted_revocations_db_path: Option<String>,
}

/// Deployment safety gate.
///
/// Code execution is admitted only when the deployment config asks for a sandbox
/// AND the E04 executor's isolation facts report a verified sandbox runtime with
/// code execution enabled. No configuration boolean or environment variable can
/// stand in for those facts; `IsolationPolicy::reference_host` (the production
/// constructor) reports `sandbox_available = false` and code execution disabled,
/// so code execution is never admitted on the reference host.
///
/// The trusted revocations anchor must be an existing regular file (never a
/// symlink or directory) whose first 16 bytes are the SQLite format 3 header.
pub fn validate_deployment_security(
    cfg: &DeploymentSecurityConfig,
    isolation: &IsolationPolicy,
) -> Result<()> {
    if cfg.allow_code_execution {
        if !cfg.sandbox_enabled {
            return Err(Error::Forbidden);
        }
        if !isolation.sandbox_available || isolation.code_execution != "enabled" {
            return Err(Error::Invalid(
                "deployment_rejected: code execution requires a verified sandbox runtime; E04 isolation reports sandbox_unavailable / code execution disabled".into(),
            ));
        }
    }

    verify_trusted_revocations_anchor(cfg.trusted_revocations_db_path.as_deref())
}

fn verify_trusted_revocations_anchor(db_path: Option<&str>) -> Result<()> {
    let db_path = db_path.ok_or_else(|| {
        Error::Invalid("deployment_rejected: trusted revocations database is required".into())
    })?;

    if db_path.trim().is_empty() {
        return Err(Error::Invalid(
            "deployment_rejected: trusted revocations db path is empty".into(),
        ));
    }

    let p = std::path::Path::new(db_path);
    // symlink_metadata does not follow links, so a symlink is visible as such.
    let meta = std::fs::symlink_metadata(p).map_err(|e| {
        Error::Invalid(format!(
            "deployment_rejected: trusted revocations db is not accessible: {db_path}: {e}"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(Error::Invalid(format!(
            "deployment_rejected: trusted revocations db must not be a symlink: {db_path}"
        )));
    }
    if !meta.is_file() {
        return Err(Error::Invalid(format!(
            "deployment_rejected: trusted revocations db is not a regular file: {db_path}"
        )));
    }

    // Read exactly the 16-byte header; never the whole file.
    let mut file = std::fs::File::open(p).map_err(|e| {
        Error::Invalid(format!(
            "deployment_rejected: cannot open trusted revocations db: {db_path}: {e}"
        ))
    })?;
    let mut header = [0u8; 16];
    file.read_exact(&mut header).map_err(|e| {
        Error::Invalid(format!(
            "deployment_rejected: trusted revocations db is shorter than the 16-byte SQLite header: {e}"
        ))
    })?;
    if header != SQLITE_HEADER {
        return Err(Error::Invalid(
            "deployment_rejected: trusted revocations db is not a valid SQLite format 3 file"
                .into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_cap_stops_not_truncates() {
        assert!(
            admit(Usage {
                runs: 0,
                events: 0,
                skills: 0,
                inflight_prepare: 0
            })
            .is_ok()
        );
        assert!(
            admit(Usage {
                runs: MAX_RUNS,
                events: 0,
                skills: 0,
                inflight_prepare: 0
            })
            .is_err()
        );
    }

    #[test]
    fn code_execution_is_never_admitted_on_reference_host() {
        // sandbox_enabled is only a request; the E04 isolation facts decide.
        let cfg = DeploymentSecurityConfig {
            sandbox_enabled: true,
            allow_code_execution: true,
            allow_external_network: false,
            trusted_revocations_db_path: Some("/nonexistent/revocations.db".into()),
        };
        let iso = IsolationPolicy::reference_host("/tmp/ws");
        match validate_deployment_security(&cfg, &iso) {
            Err(Error::Invalid(msg)) => assert!(
                msg.starts_with(
                    "deployment_rejected: code execution requires a verified sandbox runtime"
                ),
                "unexpected message: {msg}"
            ),
            other => panic!("expected deployment_rejected, got {other:?}"),
        }
    }

    #[test]
    fn recovery_never_rewinds_exposure_or_drops_revoked_sources() {
        let manifest = RecoveryStateManifest {
            backup_timestamp: 1,
            trusted_revocation_watermark: Some(10),
            consumed_queries: 100,
            total_exposure_count: 40,
            dispatched_expenses_incurred: 200,
            uncertain_dispatches_count: 0,
            has_early_stopping_ticket: false,
            revoked_sources: vec!["a".into()],
            installed_packages: vec![],
        };
        match verify_recovery_state(&manifest, 10, 100, 50, 200, &[]) {
            Err(Error::Conflict(msg)) => assert_eq!(
                msg,
                "accounting_violation: recovery cannot rewind exposure count from 50 to 40"
            ),
            other => panic!("expected exposure rewind conflict, got {other:?}"),
        }
        let live_revoked = vec!["a".to_string(), "b".to_string()];
        match verify_recovery_state(&manifest, 10, 100, 40, 200, &live_revoked) {
            Err(Error::Conflict(msg)) => {
                assert_eq!(msg, "recovery_quarantine: backup drops revoked source 'b'")
            }
            other => panic!("expected dropped-revocation quarantine, got {other:?}"),
        }
        assert!(verify_recovery_state(&manifest, 10, 100, 40, 200, &live_revoked[..1]).is_ok());
    }
}
