//! MVP capacity, persistent recovery verification, and deployment safety gates.
//! Exceeding capacity limits refuses new derive; it never silent-truncates.
use evo_core::{Error, Result, identifier};
use serde::{Deserialize, Serialize};

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

pub fn verify_recovery_state(
    manifest: &RecoveryStateManifest,
    previous_watermark: u64,
    previous_consumed_queries: u64,
    previous_spent: i64,
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

    // 2. Query consumption and financial accounting cannot be rewound
    if manifest.consumed_queries < previous_consumed_queries {
        return Err(Error::Conflict(format!(
            "accounting_violation: recovery cannot rewind consumed queries from {} to {}",
            previous_consumed_queries, manifest.consumed_queries
        )));
    }
    if manifest.dispatched_expenses_incurred < previous_spent {
        return Err(Error::Conflict(format!(
            "accounting_violation: recovery cannot rewind spent funds from {} to {}",
            previous_spent, manifest.dispatched_expenses_incurred
        )));
    }

    // 3. Early stopping tickets remain unpromotable after recovery
    let can_promote_early_stopping = !manifest.has_early_stopping_ticket;

    // 4. Uncertain calls stay uncertain and are not refunded
    let refunded_uncertain_costs = false;

    Ok(RecoveryAudit {
        recovery_allowed: true,
        restored_watermark: wm,
        can_promote_early_stopping,
        refunded_uncertain_costs,
        notes: vec![
            format!("Restored trusted revocation watermark at {}", wm),
            format!(
                "Maintained {} consumed queries and {} spent facts",
                manifest.consumed_queries, manifest.dispatched_expenses_incurred
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

pub fn validate_deployment_security(cfg: &DeploymentSecurityConfig) -> Result<()> {
    if cfg.allow_code_execution && !cfg.sandbox_enabled {
        return Err(Error::Forbidden);
    }
    if cfg.trusted_revocations_db_path.is_none() {
        return Err(Error::Invalid(
            "deployment_rejected: trusted revocations database is required".into(),
        ));
    }
    if let Some(ref path) = cfg.trusted_revocations_db_path {
        if path.trim().is_empty() {
            return Err(Error::Invalid("empty trusted revocations db path".into()));
        }
        identifier("trusted_revocations_db")?;
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
}
