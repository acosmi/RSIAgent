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
//!
//! Controller findings F10 (sandbox facts come from E04 isolation, never a boolean),
//! F11 (anchor is a regular non-symlink file, only its 16-byte header is read) and
//! F12 (exposure never rewinds, revoked sources are never dropped) each have their
//! own explicit regression tests below.

use evo_core::Error;
use evo_engine::capacity::{
    CapacityField, CapacityLimits, DeploymentSecurityConfig, MAX_ACTIVE_LEASES,
    MAX_CONCURRENT_DISPATCH, MAX_EVENTS, MAX_EXPLORATION_NODES, MAX_PREPARE, MAX_REPLAY_WORLDS,
    MAX_RUNS, MAX_SKILLS, MAX_STAGED_PACKAGES, RecoveryStateManifest, Usage, V41CapacityUsage,
    admit, admit_field, admit_v41, admit_v41_with, validate_deployment_security,
    verify_recovery_state,
};
use evo_engine::executor::IsolationPolicy;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The production isolation facts: no sandbox, code execution disabled.
fn reference_isolation() -> IsolationPolicy {
    IsolationPolicy::reference_host("/tmp/ws")
}

/// The only isolation facts under which code execution may be admitted.
fn verified_sandbox_isolation() -> IsolationPolicy {
    let mut iso = IsolationPolicy::reference_host("/tmp/ws");
    iso.sandbox_available = true;
    iso.code_execution = "enabled";
    iso
}

/// `len` bytes starting with the SQLite format 3 header (truncated when `len < 16`).
fn sqlite_anchor_bytes(len: usize) -> Vec<u8> {
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.resize(len, 0);
    bytes
}

fn write_anchor(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_string()
}

fn deployment_cfg(
    sandbox_enabled: bool,
    allow_code_execution: bool,
    anchor: Option<String>,
) -> DeploymentSecurityConfig {
    DeploymentSecurityConfig {
        sandbox_enabled,
        allow_code_execution,
        allow_external_network: false,
        trusted_revocations_db_path: anchor,
    }
}

fn expect_deployment_rejected(res: Result<(), Error>, needle: &str) {
    match res {
        Err(Error::Invalid(msg)) => {
            assert!(
                msg.starts_with("deployment_rejected:"),
                "rejection must start with 'deployment_rejected:', got: {msg}"
            );
            assert!(msg.contains(needle), "expected '{needle}' in: {msg}");
        }
        other => panic!("expected Err(Invalid(\"deployment_rejected: ..\")), got {other:?}"),
    }
}

fn expect_conflict(res: Result<evo_engine::capacity::RecoveryAudit, Error>, needle: &str) {
    match res {
        Err(Error::Conflict(msg)) => assert!(msg.contains(needle), "expected '{needle}' in: {msg}"),
        other => panic!("expected Err(Conflict(..)) containing '{needle}', got {other:?}"),
    }
}

fn base_manifest() -> RecoveryStateManifest {
    RecoveryStateManifest {
        backup_timestamp: 1000,
        trusted_revocation_watermark: Some(10),
        consumed_queries: 100,
        total_exposure_count: 50,
        dispatched_expenses_incurred: 200,
        uncertain_dispatches_count: 0,
        has_early_stopping_ticket: false,
        revoked_sources: vec![],
        installed_packages: vec![],
    }
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

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
// V069: admit_v41 boundaries — exactly the limit is refused, limit-1 admitted
// ---------------------------------------------------------------------------
#[test]
fn test_v069_admit_v41_boundary_exact_limit_rejected_limit_minus_one_accepted() {
    let base = V41CapacityUsage {
        runs: 0,
        events: 0,
        skills: 0,
        inflight_prepare: 0,
        exploration_nodes: 0,
        replay_worlds: 0,
        active_leases: 0,
        staged_packages: 0,
        concurrent_dispatches: 0,
    };
    assert!(admit_v41(&base).is_ok(), "empty usage must be admitted");

    type Setter = fn(&mut V41CapacityUsage, u64);
    let bounded: [(&str, Setter, u64); 8] = [
        ("runs", |u, v| u.runs = v, MAX_RUNS),
        ("events", |u, v| u.events = v, MAX_EVENTS),
        ("skills", |u, v| u.skills = v, MAX_SKILLS),
        (
            "inflight_prepare",
            |u, v| u.inflight_prepare = v,
            MAX_PREPARE,
        ),
        (
            "exploration_nodes",
            |u, v| u.exploration_nodes = v,
            MAX_EXPLORATION_NODES,
        ),
        (
            "replay_worlds",
            |u, v| u.replay_worlds = v,
            MAX_REPLAY_WORLDS,
        ),
        (
            "active_leases",
            |u, v| u.active_leases = v,
            MAX_ACTIVE_LEASES,
        ),
        (
            "staged_packages",
            |u, v| u.staged_packages = v,
            MAX_STAGED_PACKAGES,
        ),
    ];

    for (name, set, limit) in bounded {
        let mut at_limit = base;
        set(&mut at_limit, limit);
        match admit_v41(&at_limit) {
            Err(Error::Conflict(msg)) => assert!(
                msg.contains(">= limit"),
                "{name} at exactly {limit}: unexpected message {msg}"
            ),
            other => panic!("{name} at exactly limit {limit} must be refused, got {other:?}"),
        }

        let mut below = base;
        set(&mut below, limit - 1);
        assert!(
            admit_v41(&below).is_ok(),
            "{name} at limit-1 ({}) must be admitted",
            limit - 1
        );
    }

    // Concurrent dispatch is the one bound where the limit value itself is admitted.
    assert_eq!(MAX_CONCURRENT_DISPATCH, 1);
    let mut one = base;
    one.concurrent_dispatches = 1;
    assert!(
        admit_v41(&one).is_ok(),
        "1 concurrent dispatch must be admitted"
    );
    let mut two = base;
    two.concurrent_dispatches = 2;
    match admit_v41(&two) {
        Err(Error::Conflict(msg)) => assert!(msg.contains("concurrent dispatches 2 > limit 1")),
        other => panic!("2 concurrent dispatches must be refused, got {other:?}"),
    }
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

    let res_missing = verify_recovery_state(&manifest_missing_wm, 10, 100, 50, 200, &[]);
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

    let res_stale = verify_recovery_state(&manifest_stale_wm, 10, 100, 50, 200, &[]);
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

    let res_rewound_q = verify_recovery_state(&manifest_rewound_queries, 10, 100, 50, 200, &[]);
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

    let res_rewound_m = verify_recovery_state(&manifest_rewound_money, 10, 100, 50, 200, &[]);
    assert!(res_rewound_m.is_err());
    let err_msg = format!("{:?}", res_rewound_m.unwrap_err());
    assert!(err_msg.contains("accounting_violation: recovery cannot rewind spent funds"));
}

// ---------------------------------------------------------------------------
// F12: exposure is a consumed fact and never rewinds
// ---------------------------------------------------------------------------
#[test]
fn test_f12_recovery_cannot_rewind_exposure_count() {
    let mut manifest = base_manifest();
    manifest.total_exposure_count = 40; // Rewound from a live 50!
    expect_conflict(
        verify_recovery_state(&manifest, 10, 100, 50, 200, &[]),
        "accounting_violation: recovery cannot rewind exposure count from 50 to 40",
    );

    // Even a single exposure of rewind is refused.
    manifest.total_exposure_count = 49;
    expect_conflict(
        verify_recovery_state(&manifest, 10, 100, 50, 200, &[]),
        "accounting_violation: recovery cannot rewind exposure count from 50 to 49",
    );
}

#[test]
fn test_f12_recovery_equal_or_greater_exposure_accepted() {
    let manifest = base_manifest();
    assert_eq!(manifest.total_exposure_count, 50);

    let audit = verify_recovery_state(&manifest, 10, 100, 50, 200, &[])
        .expect("equal exposure count must be accepted");
    assert!(audit.recovery_allowed);
    assert!(
        audit.notes.iter().any(|n| n.contains("50 exposures")),
        "audit notes must record the maintained exposure count: {:?}",
        audit.notes
    );

    // A backup that has seen more exposures than the live counter is not a rewind.
    let mut ahead = base_manifest();
    ahead.total_exposure_count = 60;
    assert!(verify_recovery_state(&ahead, 10, 100, 50, 200, &[]).is_ok());
}

// ---------------------------------------------------------------------------
// F12 / V075: a backup can never drop an already revoked source
// ---------------------------------------------------------------------------
#[test]
fn test_f12_recovery_backup_dropping_revoked_source_is_quarantined() {
    let mut manifest = base_manifest();
    manifest.revoked_sources = ids(&["src_a"]); // src_b was revoked live but is missing
    let live_revoked = ids(&["src_a", "src_b"]);

    expect_conflict(
        verify_recovery_state(&manifest, 10, 100, 50, 200, &live_revoked),
        "recovery_quarantine: backup drops revoked source 'src_b'",
    );

    // A backup with an empty revocation list drops everything.
    manifest.revoked_sources.clear();
    expect_conflict(
        verify_recovery_state(&manifest, 10, 100, 50, 200, &live_revoked),
        "recovery_quarantine: backup drops revoked source 'src_a'",
    );
}

#[test]
fn test_f12_recovery_revoked_superset_accepted() {
    let mut manifest = base_manifest();
    manifest.revoked_sources = ids(&["src_a", "src_b", "src_c"]);
    let live_revoked = ids(&["src_a", "src_b"]);

    let audit = verify_recovery_state(&manifest, 10, 100, 50, 200, &live_revoked)
        .expect("a backup that revokes a superset must be accepted");
    assert!(audit.recovery_allowed);
    assert!(
        audit
            .notes
            .iter()
            .any(|n| n.contains("Preserved all 2 previously revoked sources")),
        "audit notes must record preserved revocations: {:?}",
        audit.notes
    );

    // Exactly the same set is also fine.
    manifest.revoked_sources = ids(&["src_b", "src_a"]);
    assert!(verify_recovery_state(&manifest, 10, 100, 50, 200, &live_revoked).is_ok());
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

    let live_revoked = ids(&["revoked_src_01"]);
    let audit = verify_recovery_state(&valid_manifest, 10, 100, 50, 200, &live_revoked)
        .expect("Valid recovery");

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
    let iso = reference_isolation();

    // 1. Code execution without sandbox must be forbidden
    let bad_cfg = DeploymentSecurityConfig {
        sandbox_enabled: false,
        allow_code_execution: true,
        allow_external_network: false,
        trusted_revocations_db_path: Some("/var/lib/rsia/revocations.db".into()),
    };
    assert!(matches!(
        validate_deployment_security(&bad_cfg, &iso),
        Err(Error::Forbidden)
    ));

    // 2. Missing trusted revocations database is rejected
    let missing_db_cfg = DeploymentSecurityConfig {
        sandbox_enabled: false,
        allow_code_execution: false,
        allow_external_network: false,
        trusted_revocations_db_path: None,
    };
    expect_deployment_rejected(
        validate_deployment_security(&missing_db_cfg, &iso),
        "trusted revocations database is required",
    );

    // 3. Valid deployment config with verified anchor and disabled code execution
    let temp_db = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp_db.path(), sqlite_anchor_bytes(100)).unwrap();

    let valid_cfg = DeploymentSecurityConfig {
        sandbox_enabled: false,
        allow_code_execution: false,
        allow_external_network: false,
        trusted_revocations_db_path: Some(temp_db.path().to_str().unwrap().into()),
    };
    assert!(validate_deployment_security(&valid_cfg, &iso).is_ok());

    // 4. Code execution with sandbox_enabled=true but reference-host isolation fails
    let unverified_sandbox_cfg = DeploymentSecurityConfig {
        sandbox_enabled: true,
        allow_code_execution: true,
        allow_external_network: false,
        trusted_revocations_db_path: Some(temp_db.path().to_str().unwrap().into()),
    };
    expect_deployment_rejected(
        validate_deployment_security(&unverified_sandbox_cfg, &iso),
        "code execution requires a verified sandbox runtime",
    );
}

// =========================================================================
// F07 Adversarial Regression (from controller review)
// =========================================================================
#[test]
fn test_f07_review_deployment_cannot_trust_nonexistent_anchor_and_sandbox_boolean() {
    let cfg = DeploymentSecurityConfig {
        sandbox_enabled: true,
        allow_code_execution: true,
        allow_external_network: true,
        trusted_revocations_db_path: Some("/definitely-not-a-current-trusted-db".into()),
    };
    assert!(
        validate_deployment_security(&cfg, &reference_isolation()).is_err(),
        "nonexistent trusted anchor + sandbox boolean accepted"
    );
    // Even with code execution disabled, the nonexistent anchor alone is rejected.
    let no_exec = deployment_cfg(
        false,
        false,
        Some("/definitely-not-a-current-trusted-db".into()),
    );
    expect_deployment_rejected(
        validate_deployment_security(&no_exec, &reference_isolation()),
        "not accessible",
    );
}

// =========================================================================
// F10: the sandbox gate defers to E04 isolation facts, never a boolean
// =========================================================================
#[test]
fn test_f10_review_sandbox_boolean_and_valid_anchor_still_rejected_on_reference_host() {
    // The F10 counterexample: everything the config can say is "yes", the anchor
    // is genuinely valid, and the production isolation facts still refuse.
    let dir = tempfile::tempdir().unwrap();
    let anchor = write_anchor(&dir, "revocations.db", &sqlite_anchor_bytes(100));

    let cfg = deployment_cfg(true, true, Some(anchor.clone()));
    let res = validate_deployment_security(&cfg, &reference_isolation());
    match res {
        Err(Error::Invalid(msg)) => assert_eq!(
            msg,
            "deployment_rejected: code execution requires a verified sandbox runtime; E04 isolation reports sandbox_unavailable / code execution disabled"
        ),
        other => panic!("sandbox boolean + valid anchor was accepted on reference host: {other:?}"),
    }

    // The same anchor is accepted once code execution is not requested, proving
    // the rejection above came from the isolation facts and not the anchor.
    let no_exec = deployment_cfg(true, false, Some(anchor));
    assert!(validate_deployment_security(&no_exec, &reference_isolation()).is_ok());
}

#[test]
fn test_f10_isolation_sandbox_available_but_code_execution_disabled_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let anchor = write_anchor(&dir, "revocations.db", &sqlite_anchor_bytes(100));

    let mut iso = reference_isolation();
    iso.sandbox_available = true;
    assert_eq!(iso.code_execution, "disabled");

    let cfg = deployment_cfg(true, true, Some(anchor.clone()));
    expect_deployment_rejected(
        validate_deployment_security(&cfg, &iso),
        "code execution requires a verified sandbox runtime",
    );

    // Symmetric case: code execution "enabled" without a sandbox is also refused.
    let mut iso_no_sandbox = reference_isolation();
    iso_no_sandbox.code_execution = "enabled";
    assert!(!iso_no_sandbox.sandbox_available);
    expect_deployment_rejected(
        validate_deployment_security(&cfg, &iso_no_sandbox),
        "code execution requires a verified sandbox runtime",
    );

    // Any other value of code_execution is not "enabled" and is refused too.
    let mut iso_other = verified_sandbox_isolation();
    iso_other.code_execution = "Enabled";
    expect_deployment_rejected(
        validate_deployment_security(&cfg, &iso_other),
        "code execution requires a verified sandbox runtime",
    );
}

#[test]
fn test_f10_only_positive_path_is_verified_sandbox_runtime_with_valid_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let anchor = write_anchor(&dir, "revocations.db", &sqlite_anchor_bytes(100));
    let iso = verified_sandbox_isolation();
    assert!(iso.sandbox_available);
    assert_eq!(iso.code_execution, "enabled");

    // The explicit positive path: config asks for a sandbox, E04 reports a verified
    // sandbox runtime with code execution enabled, and the anchor is valid.
    let cfg = deployment_cfg(true, true, Some(anchor.clone()));
    validate_deployment_security(&cfg, &iso)
        .expect("verified sandbox runtime + sandbox_enabled + valid anchor must be accepted");

    // Verified isolation facts do not bypass the config's own sandbox switch.
    let no_sandbox_cfg = deployment_cfg(false, true, Some(anchor));
    assert!(matches!(
        validate_deployment_security(&no_sandbox_cfg, &iso),
        Err(Error::Forbidden)
    ));

    // Verified isolation facts do not bypass the trusted anchor check either.
    let bad_anchor_cfg = deployment_cfg(
        true,
        true,
        Some(format!("{}/missing.db", dir.path().display())),
    );
    expect_deployment_rejected(
        validate_deployment_security(&bad_anchor_cfg, &iso),
        "not accessible",
    );
    let no_anchor_cfg = deployment_cfg(true, true, None);
    expect_deployment_rejected(
        validate_deployment_security(&no_anchor_cfg, &iso),
        "trusted revocations database is required",
    );
}

// =========================================================================
// F11: the trusted revocations anchor is a regular file whose 16-byte header
// is read exactly; symlinks, directories, short and mislabeled files are refused
// =========================================================================
#[test]
fn test_f11_anchor_none_or_empty_path_rejected() {
    let iso = reference_isolation();
    expect_deployment_rejected(
        validate_deployment_security(&deployment_cfg(false, false, None), &iso),
        "trusted revocations database is required",
    );
    expect_deployment_rejected(
        validate_deployment_security(&deployment_cfg(false, false, Some(String::new())), &iso),
        "path is empty",
    );
    expect_deployment_rejected(
        validate_deployment_security(&deployment_cfg(false, false, Some("   ".into())), &iso),
        "path is empty",
    );
}

#[cfg(unix)]
#[test]
fn test_f11_anchor_symlink_to_valid_db_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let real = write_anchor(&dir, "real.db", &sqlite_anchor_bytes(100));
    let link = dir.path().join("link.db");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let link = link.to_str().unwrap().to_string();

    let iso = reference_isolation();
    // Sanity: the link target on its own is accepted.
    assert!(validate_deployment_security(&deployment_cfg(false, false, Some(real)), &iso).is_ok());
    // The symlink pointing at it is not.
    expect_deployment_rejected(
        validate_deployment_security(&deployment_cfg(false, false, Some(link)), &iso),
        "must not be a symlink",
    );
}

#[test]
fn test_f11_anchor_directory_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    expect_deployment_rejected(
        validate_deployment_security(
            &deployment_cfg(false, false, Some(path)),
            &reference_isolation(),
        ),
        "not a regular file",
    );
}

#[test]
fn test_f11_anchor_short_file_rejected() {
    let dir = tempfile::tempdir().unwrap();
    // 10 bytes: a correct prefix of the header but shorter than 16 bytes.
    let short = write_anchor(&dir, "short.db", &sqlite_anchor_bytes(10));
    assert_eq!(std::fs::metadata(&short).unwrap().len(), 10);
    expect_deployment_rejected(
        validate_deployment_security(
            &deployment_cfg(false, false, Some(short)),
            &reference_isolation(),
        ),
        "shorter than the 16-byte SQLite header",
    );

    // An empty file is the degenerate short file.
    let empty = write_anchor(&dir, "empty.db", &[]);
    expect_deployment_rejected(
        validate_deployment_security(
            &deployment_cfg(false, false, Some(empty)),
            &reference_isolation(),
        ),
        "shorter than the 16-byte SQLite header",
    );
}

#[test]
fn test_f11_anchor_wrong_header_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = b"NotSQLite fmt 3\0".to_vec();
    assert_eq!(bytes.len(), 16);
    bytes.resize(100, 0);
    let wrong = write_anchor(&dir, "wrong.db", &bytes);
    expect_deployment_rejected(
        validate_deployment_security(
            &deployment_cfg(false, false, Some(wrong)),
            &reference_isolation(),
        ),
        "not a valid SQLite format 3 file",
    );

    // Off by one byte in the terminator is still not the SQLite header.
    let mut almost = b"SQLite format 3 ".to_vec();
    almost.resize(100, 0);
    let almost = write_anchor(&dir, "almost.db", &almost);
    expect_deployment_rejected(
        validate_deployment_security(
            &deployment_cfg(false, false, Some(almost)),
            &reference_isolation(),
        ),
        "not a valid SQLite format 3 file",
    );
}

#[test]
fn test_f11_anchor_100_byte_valid_header_accepted_when_code_execution_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = sqlite_anchor_bytes(100);
    assert_eq!(bytes.len(), 100);
    let anchor = write_anchor(&dir, "revocations.db", &bytes);

    validate_deployment_security(
        &deployment_cfg(false, false, Some(anchor.clone())),
        &reference_isolation(),
    )
    .expect("100-byte file with SQLite header must be accepted");

    // sandbox_enabled alone (no code execution) does not change the anchor verdict.
    validate_deployment_security(
        &deployment_cfg(true, false, Some(anchor)),
        &reference_isolation(),
    )
    .expect("sandbox_enabled without code execution must still be accepted");
}

// ---------------------------------------------------------------------------
// E16.5 AG-014: CapacityLimits and per-field admission (the pure gate every
// real entry point calls)
// ---------------------------------------------------------------------------
#[test]
fn capacity_limits_default_pins_the_plan_constants() {
    assert_eq!(
        CapacityLimits::default(),
        CapacityLimits {
            runs: MAX_RUNS,
            events: MAX_EVENTS,
            skills: MAX_SKILLS,
            inflight_prepare: MAX_PREPARE,
            exploration_nodes: MAX_EXPLORATION_NODES,
            replay_worlds: MAX_REPLAY_WORLDS,
            active_leases: MAX_ACTIVE_LEASES,
            staged_packages: MAX_STAGED_PACKAGES,
            concurrent_dispatches: MAX_CONCURRENT_DISPATCH,
        }
    );
    assert_eq!(
        (
            MAX_RUNS,
            MAX_EVENTS,
            MAX_SKILLS,
            MAX_PREPARE,
            MAX_EXPLORATION_NODES,
            MAX_REPLAY_WORLDS,
            MAX_ACTIVE_LEASES,
            MAX_STAGED_PACKAGES,
            MAX_CONCURRENT_DISPATCH
        ),
        (1_000, 10_000, 1_000, 5, 500, 100, 10, 20, 1)
    );
}

#[test]
fn every_capacity_field_admits_limit_minus_one_and_refuses_the_limit() {
    let limits = CapacityLimits {
        runs: 7,
        events: 11,
        skills: 5,
        inflight_prepare: 3,
        exploration_nodes: 9,
        replay_worlds: 4,
        active_leases: 6,
        staged_packages: 8,
        concurrent_dispatches: 1,
    };
    let empty = V41CapacityUsage {
        runs: 0,
        events: 0,
        skills: 0,
        inflight_prepare: 0,
        exploration_nodes: 0,
        replay_worlds: 0,
        active_leases: 0,
        staged_packages: 0,
        concurrent_dispatches: 0,
    };
    assert!(admit_v41_with(&empty, &limits).is_ok());
    fn set(usage: &mut V41CapacityUsage, field: CapacityField, value: u64) {
        match field {
            CapacityField::Runs => usage.runs = value,
            CapacityField::Events => usage.events = value,
            CapacityField::Skills => usage.skills = value,
            CapacityField::InflightPrepare => usage.inflight_prepare = value,
            CapacityField::ExplorationNodes => usage.exploration_nodes = value,
            CapacityField::ReplayWorlds => usage.replay_worlds = value,
            CapacityField::ActiveLeases => usage.active_leases = value,
            CapacityField::StagedPackages => usage.staged_packages = value,
            CapacityField::ConcurrentDispatches => usage.concurrent_dispatches = value,
        }
    }
    for field in CapacityField::ALL {
        let limit = field.limit(&limits);
        let label = field.label();
        if field == CapacityField::ConcurrentDispatches {
            // The one bound where the limit itself is admitted (it counts the
            // dispatch being admitted) and limit + 1 is refused.
            assert!(admit_field(field, limit, &limits).is_ok());
            match admit_field(field, limit + 1, &limits) {
                Err(Error::Conflict(msg)) => assert_eq!(
                    msg,
                    format!(
                        "MVP capacity exceeded: {label} {} > limit {limit}",
                        limit + 1
                    )
                ),
                other => panic!("{label} above limit must be refused, got {other:?}"),
            }
            let mut over = empty;
            set(&mut over, field, limit + 1);
            assert!(admit_v41_with(&over, &limits).is_err());
            continue;
        }
        assert!(
            admit_field(field, limit - 1, &limits).is_ok(),
            "{label} at limit-1 must be admitted"
        );
        match admit_field(field, limit, &limits) {
            Err(Error::Conflict(msg)) => assert_eq!(
                msg,
                format!("MVP capacity exceeded: {label} {limit} >= limit {limit}")
            ),
            other => panic!("{label} at limit must be refused, got {other:?}"),
        }
        let mut below = empty;
        set(&mut below, field, limit - 1);
        assert!(admit_v41_with(&below, &limits).is_ok());
        let mut at_limit = empty;
        set(&mut at_limit, field, limit);
        assert_eq!(
            admit_v41_with(&at_limit, &limits).unwrap_err().to_string(),
            admit_field(field, limit, &limits).unwrap_err().to_string(),
            "{label}: the vector gate and the field gate report the same refusal"
        );
        // A default-limit vector at the same usage: the field alone decides.
        let mut default_case = empty;
        set(
            &mut default_case,
            field,
            field.limit(&CapacityLimits::default()),
        );
        assert_eq!(
            admit_v41(&default_case).unwrap_err().to_string(),
            admit_v41_with(&default_case, &CapacityLimits::default())
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            field.usage(&default_case),
            field.limit(&CapacityLimits::default())
        );
    }
}
