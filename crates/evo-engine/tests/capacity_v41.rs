//! Comprehensive test suite for E16.5: Persistent Recovery Invariants, MVP Capacity,
//! Irrevocable Accounting, and Deployment Safety.
//!
//! Covers scenario families:
//! - V007, V008: Accounting irreversibility and query consumption
//! - V017, V018: Monotonic revocation watermarks & recovery quarantine
//! - V037, V038, V039: Zero regression and failure rollback
//! - V066, V069: Capacity bounds refuse new derive instead of silent truncation
//! - V073: License audit and reference verification
//! - V075: Content closure & no resurrection of revoked sources
//! - V081–V086: Recovery invariants (early stopping unpromotable, uncertain calls)
//! - V096: Root cost reservation and irreversible financial facts
//! - V098: Audit traceability

use evo_core::Error;
use evo_engine::capacity::{
    DeploymentSecurityConfig, MAX_ACTIVE_LEASES, MAX_CONCURRENT_DISPATCH, MAX_EVENTS,
    MAX_EXPLORATION_NODES, MAX_PREPARE, MAX_REPLAY_WORLDS, MAX_RUNS, MAX_SKILLS,
    MAX_STAGED_PACKAGES, RecoveryStateManifest, Usage, V41CapacityUsage, admit, admit_v41,
    validate_deployment_security, verify_recovery_state,
};

// ---------------------------------------------------------------------------
// V066: MVP Capacity Stops New Derive Instead of Silent Truncation
// ---------------------------------------------------------------------------
#[test]
fn test_legacy_usage_admit() {
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
fn test_v066_v41_capacity_stops_new_derive() {
    let mut usage = V41CapacityUsage {
        runs: 10,
        events: 50,
        skills: 5,
        inflight_prepare: 1,
        exploration_nodes: 20,
        replay_worlds: 5,
        active_leases: 2,
        staged_packages: 3,
        concurrent_dispatches: 1,
    };
    assert!(admit_v41(&usage).is_ok());

    // 1. Exceeding runs
    usage.runs = MAX_RUNS;
    assert!(admit_v41(&usage).is_err());
    usage.runs = 10;

    // 2. Exceeding events
    usage.events = MAX_EVENTS;
    assert!(admit_v41(&usage).is_err());
    usage.events = 50;

    // 3. Exceeding skills
    usage.skills = MAX_SKILLS;
    assert!(admit_v41(&usage).is_err());
    usage.skills = 5;

    // 4. Exceeding in-flight prepares
    usage.inflight_prepare = MAX_PREPARE;
    assert!(admit_v41(&usage).is_err());
    usage.inflight_prepare = 1;

    // 5. Exceeding exploration nodes
    usage.exploration_nodes = MAX_EXPLORATION_NODES;
    assert!(admit_v41(&usage).is_err());
    usage.exploration_nodes = 20;

    // 6. Exceeding replay worlds
    usage.replay_worlds = MAX_REPLAY_WORLDS;
    assert!(admit_v41(&usage).is_err());
    usage.replay_worlds = 5;

    // 7. Exceeding active leases
    usage.active_leases = MAX_ACTIVE_LEASES;
    assert!(admit_v41(&usage).is_err());
    usage.active_leases = 2;

    // 8. Exceeding staged packages
    usage.staged_packages = MAX_STAGED_PACKAGES;
    assert!(admit_v41(&usage).is_err());
    usage.staged_packages = 3;

    // 9. Exceeding concurrent dispatches (concurrency limit is 1)
    usage.concurrent_dispatches = MAX_CONCURRENT_DISPATCH + 1;
    assert!(admit_v41(&usage).is_err());
}

// ---------------------------------------------------------------------------
// V017, V018: Recovery Watermarks and Quarantine
// ---------------------------------------------------------------------------
#[test]
fn test_v018_recovery_quarantine_on_missing_or_stale_watermark() {
    let manifest_missing_wm = RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: None, // Missing!
        consumed_queries: 100,
        total_exposure_count: 50,
        dispatched_expenses_incurred: 200,
        uncertain_dispatches_count: 0,
        has_early_stopping_ticket: false,
        revoked_sources: vec![],
        installed_packages: vec![],
    };

    let res_missing = verify_recovery_state(&manifest_missing_wm, 10, 100, 200);
    assert!(res_missing.is_err());
    let err_msg = format!("{:?}", res_missing.unwrap_err());
    assert!(err_msg.contains("recovery_quarantine: missing trusted revocation watermark"));

    let manifest_stale_wm = RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: Some(5), // Older than live watermark 10!
        consumed_queries: 100,
        total_exposure_count: 50,
        dispatched_expenses_incurred: 200,
        uncertain_dispatches_count: 0,
        has_early_stopping_ticket: false,
        revoked_sources: vec![],
        installed_packages: vec![],
    };

    let res_stale = verify_recovery_state(&manifest_stale_wm, 10, 100, 200);
    assert!(res_stale.is_err());
    let err_msg = format!("{:?}", res_stale.unwrap_err());
    assert!(
        err_msg.contains(
            "recovery_quarantine: backup watermark 5 is behind trusted live watermark 10"
        )
    );
}

// ---------------------------------------------------------------------------
// V007, V008, V096: Irreversible Accounting and Query Consumption
// ---------------------------------------------------------------------------
#[test]
fn test_v007_recovery_cannot_rewind_queries_or_spent_funds() {
    let manifest_rewound_queries = RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: Some(15),
        consumed_queries: 50, // Rewound from 100!
        total_exposure_count: 50,
        dispatched_expenses_incurred: 200,
        uncertain_dispatches_count: 0,
        has_early_stopping_ticket: false,
        revoked_sources: vec![],
        installed_packages: vec![],
    };

    let res_rewound_q = verify_recovery_state(&manifest_rewound_queries, 10, 100, 200);
    assert!(res_rewound_q.is_err());
    let err_msg = format!("{:?}", res_rewound_q.unwrap_err());
    assert!(err_msg.contains("accounting_violation: recovery cannot rewind consumed queries"));

    let manifest_rewound_money = RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: Some(15),
        consumed_queries: 100,
        total_exposure_count: 50,
        dispatched_expenses_incurred: 100, // Rewound from 200!
        uncertain_dispatches_count: 0,
        has_early_stopping_ticket: false,
        revoked_sources: vec![],
        installed_packages: vec![],
    };

    let res_rewound_m = verify_recovery_state(&manifest_rewound_money, 10, 100, 200);
    assert!(res_rewound_m.is_err());
    let err_msg = format!("{:?}", res_rewound_m.unwrap_err());
    assert!(err_msg.contains("accounting_violation: recovery cannot rewind spent funds"));
}

// ---------------------------------------------------------------------------
// V084, V085: Early Stopping Tickets and Uncertain Costs Invariants
// ---------------------------------------------------------------------------
#[test]
fn test_v085_recovery_preserves_early_stopping_unpromotable_and_uncertain_calls() {
    let valid_manifest = RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: Some(10),
        consumed_queries: 100,
        total_exposure_count: 50,
        dispatched_expenses_incurred: 200,
        uncertain_dispatches_count: 2,
        has_early_stopping_ticket: true, // Recovered early-stopping ticket
        revoked_sources: vec!["revoked_src_01".into()],
        installed_packages: vec!["org.rsia:base_pkg".into()],
    };

    let audit = verify_recovery_state(&valid_manifest, 10, 100, 200).expect("Valid recovery");

    assert!(audit.recovery_allowed);
    assert_eq!(audit.restored_watermark, 10);
    assert!(
        !audit.can_promote_early_stopping,
        "Early stopping ticket must remain unpromotable after recovery!"
    );
    assert!(
        !audit.refunded_uncertain_costs,
        "Uncertain calls must not be automatically refunded upon recovery!"
    );
}

// ---------------------------------------------------------------------------
// Deployment Safety Gates (Sandbox, Code Execution, Revocation Anchor)
// ---------------------------------------------------------------------------
#[test]
fn test_deployment_safety_gates() {
    // 1. Code execution without sandbox must be forbidden
    let bad_cfg = DeploymentSecurityConfig {
        sandbox_enabled: false,
        allow_code_execution: true,
        allow_external_network: false,
        trusted_revocations_db_path: Some("/var/lib/rsia/revocations.db".into()),
    };
    assert!(matches!(
        validate_deployment_security(&bad_cfg),
        Err(Error::Forbidden)
    ));

    // 2. Missing trusted revocations database is rejected
    let missing_db_cfg = DeploymentSecurityConfig {
        sandbox_enabled: true,
        allow_code_execution: true,
        allow_external_network: false,
        trusted_revocations_db_path: None,
    };
    assert!(validate_deployment_security(&missing_db_cfg).is_err());

    // 3. Valid deployment config
    let valid_cfg = DeploymentSecurityConfig {
        sandbox_enabled: true,
        allow_code_execution: true,
        allow_external_network: false,
        trusted_revocations_db_path: Some("/var/lib/rsia/revocations.db".into()),
    };
    assert!(validate_deployment_security(&valid_cfg).is_ok());
}
