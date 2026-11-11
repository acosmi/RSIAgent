//! Comprehensive test suite for E16.3: Built-in Seeds, B/L/U Comparison,
//! Local Modification Protection, and Safe Reset Gates.
//!
//! Covers scenario families:
//! - V014: Seed-Baseline-Immutability (corrupt/missing baseline rejected)
//! - V018: Revocation-Watermark-Anchor (watermark advancement blocks reset)
//! - V038: Interruption-Clean-Rollback (session abort preserves Active)
//! - V063: BLU-Classification-Complete (all §11.1 comparison branches)
//! - V064: Unmodified-Upstream-Staged-Only (no auto-activation)
//! - V065: Three-Way-Diff-Conflicts (conflict annotation, no auto-merge to active)
//! - V078: Reset-Safety-Gates (revocation, regression, corrupt baseline rejected)
//! - V091: All-Effective-Content-Gated
//! - V092: No-Auto-Elevation
//! - V098: Audit-Traceability

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJobState, ManagementResultV2};
use evo_engine::packages::{E16Envelope, E16SourceRef, PersistentPackageStore, StagedAssetState};
use evo_engine::seeds::{
    BUILTIN_I0_BYTES, DiffCategory, E16_CURRENT_SEED_INSTALL_SCHEMA as E16_SEED_INSTALL_SCHEMA,
    E16_SEED_INSTALL_SCHEMA as E16_V1_SEED_INSTALL_SCHEMA, InstallCurrentSeedRequest,
    PersistentSeedStore, SeedClass, SeedIdentity, SeedInstallPayloadV1, SeedInstallRecord,
    SeedStatus, SeedTriple, SeedUpstream, StagingSession, auto_activate, classify,
    compute_three_way_diff, safe_reset_to_baseline,
};
use evo_storage::Store;

fn sample_digest(content: &str) -> String {
    hash(content.as_bytes())
}

// ---------------------------------------------------------------------------
// V063: BLU-Classification-Complete (all §11.1 comparison branches)
// ---------------------------------------------------------------------------
#[test]
fn test_v063_blu_all_branches() {
    let b = sample_digest("baseline content");
    let l_edited = sample_digest("local edited content");
    let u_newer = sample_digest("upstream newer content");

    // 1. U = B, L = B -> Unmodified
    let t_unmod = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(b.clone()),
        upstream: Some(b.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_unmod), SeedClass::Unmodified);

    // 2. U = B, L != B -> LocallyEdited
    let t_local_edited = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(l_edited.clone()),
        upstream: Some(b.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_local_edited), SeedClass::LocallyEdited);

    // 3. U != B, L = B -> UpstreamNewer
    let t_upstream_newer = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(b.clone()),
        upstream: Some(u_newer.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_upstream_newer), SeedClass::UpstreamNewer);

    // 4. U != B, L != B, U != L -> BothChanged
    let t_both_changed = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(l_edited.clone()),
        upstream: Some(u_newer.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_both_changed), SeedClass::BothChanged);

    // 5. L = U, but L != B -> IdenticalToUpstream
    let t_ident_upstream = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(u_newer.clone()),
        upstream: Some(u_newer.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_ident_upstream), SeedClass::IdenticalToUpstream);

    // 6. Same name but different publisher -> SameNameDifferentPublisher
    let t_diff_pub = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(b.clone()),
        upstream: Some(u_newer.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "foreign.corp".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_01".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_diff_pub), SeedClass::SameNameDifferentPublisher);

    // 7. Same name but different asset_id -> SameNameDifferentPublisher
    let t_diff_id = SeedTriple {
        bundled: Some(b.clone()),
        local: Some(b.clone()),
        upstream: Some(u_newer.clone()),
        local_marked: true,
        publisher_bundled: "org.rsia".into(),
        publisher_local: "org.rsia".into(),
        asset_id_bundled: Some("skill_01".into()),
        asset_id_local: Some("skill_02".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    };
    assert_eq!(classify(&t_diff_id), SeedClass::SameNameDifferentPublisher);

    // 8. Missing marker -> MissingMarker
    let mut t_missing_marker = t_unmod.clone();
    t_missing_marker.local_marked = false;
    assert_eq!(classify(&t_missing_marker), SeedClass::MissingMarker);

    // 9. Missing or corrupt baseline -> Unknown
    let mut t_corrupt = t_unmod.clone();
    t_corrupt.bundled = None;
    assert_eq!(classify(&t_corrupt), SeedClass::Unknown);

    let mut t_corrupt_str = t_unmod;
    t_corrupt_str.bundled = Some("not_a_valid_64_hex_hash_for_sure_1234567890".into());
    assert_eq!(classify(&t_corrupt_str), SeedClass::Unknown);
}

// ---------------------------------------------------------------------------
// V064 & V091: Unmodified-Upstream-Staged-Only & Never Auto-Activate
// ---------------------------------------------------------------------------
#[test]
fn test_v064_auto_activate_is_always_forbidden() {
    let classes = [
        SeedClass::Unmodified,
        SeedClass::LocallyEdited,
        SeedClass::UpstreamNewer,
        SeedClass::BothChanged,
        SeedClass::IdenticalToUpstream,
        SeedClass::SameNameDifferentPublisher,
        SeedClass::MissingMarker,
        SeedClass::CorruptBaseline,
        SeedClass::Unknown,
    ];

    for class in classes {
        let res = auto_activate(class);
        assert!(
            res.is_err(),
            "auto_activate must fail for class {:?}",
            class
        );
        match res {
            Err(Error::Conflict(msg)) => {
                assert!(
                    msg.contains("cannot auto-activate")
                        || msg.contains("staging, evaluation, and approval")
                );
            }
            other => panic!("Expected Error::Conflict, got {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// V065: Three-Way-Diff-Conflicts & Conflict Markers
// ---------------------------------------------------------------------------
#[test]
fn test_v065_three_way_diff_clean_and_conflicts() {
    let baseline = "fn execute() {\n    step1();\n    step2();\n}\n";
    let local_clean = baseline;
    let upstream_clean = "fn execute() {\n    step1();\n    step2();\n    step3();\n}\n";

    // 1. Upstream modified, local unmodified: clean diff
    let diff_upstream_only = compute_three_way_diff(
        "org.rsia",
        "my_skill",
        baseline,
        local_clean,
        upstream_clean,
    );
    assert_eq!(
        diff_upstream_only.category,
        DiffCategory::UpstreamOnlyModified
    );
    assert!(!diff_upstream_only.has_conflicts);
    assert!(diff_upstream_only.requires_new_evaluation);
    assert!(!diff_upstream_only.auto_activated);

    // 2. Local modified, upstream unmodified: local only
    let local_mod = "fn execute() {\n    step1();\n    step_custom();\n}\n";
    let diff_local_only =
        compute_three_way_diff("org.rsia", "my_skill", baseline, local_mod, baseline);
    assert_eq!(diff_local_only.category, DiffCategory::LocalOnlyModified);
    assert!(!diff_local_only.has_conflicts);
    assert!(!diff_local_only.requires_new_evaluation);

    // 3. Both modified with identical new content
    let diff_both_ident = compute_three_way_diff(
        "org.rsia",
        "my_skill",
        baseline,
        upstream_clean,
        upstream_clean,
    );
    assert_eq!(
        diff_both_ident.category,
        DiffCategory::BothModifiedIdentical
    );
    assert!(!diff_both_ident.has_conflicts);
    assert!(diff_both_ident.requires_new_evaluation);

    // 4. Both modified differently: CONFLICT
    let diff_conflict =
        compute_three_way_diff("org.rsia", "my_skill", baseline, local_mod, upstream_clean);
    assert_eq!(diff_conflict.category, DiffCategory::Conflict);
    assert!(diff_conflict.has_conflicts);
    assert!(diff_conflict.requires_new_evaluation);
    assert!(!diff_conflict.auto_activated);

    let preview = diff_conflict.merged_preview.unwrap();
    assert!(preview.contains("<<<<<<< LOCAL (user modified)"));
    assert!(preview.contains("======="));
    assert!(preview.contains(">>>>>>> UPSTREAM (new upstream)"));
}

// ---------------------------------------------------------------------------
// V078, V014, V018: Reset-Safety-Gates, Immutability & Revocation Watermarks
// ---------------------------------------------------------------------------
#[test]
fn test_v078_safe_reset_to_baseline_gates() {
    let baseline_str = "original baseline instruction\n";
    let b_digest = sample_digest(baseline_str);
    let l_digest = sample_digest("modified user copy\n");

    let record = SeedInstallRecord {
        publisher: "org.rsia".into(),
        asset_id: "reasoning_seed".into(),
        kind: "skill".into(),
        baseline_digest: b_digest.clone(),
        local_digest: l_digest,
        upstream_digest: None,
        installed_at: 1000,
        status: SeedStatus::Installed,
        quarantine_reason: None,
        revocation_watermark: 5,
    };

    // Case 1: Missing baseline content
    let res_missing = safe_reset_to_baseline(&record, None, |_dig| false, 5, |_dig| false);
    assert!(
        matches!(res_missing, Err(Error::Invalid(msg)) if msg.contains("missing_baseline_content"))
    );

    // Case 2: Corrupt baseline content (digest mismatch)
    let res_corrupt = safe_reset_to_baseline(
        &record,
        Some("corrupted baseline content that differs"),
        |_dig| false,
        5,
        |_dig| false,
    );
    assert!(matches!(res_corrupt, Err(Error::Invalid(msg)) if msg.contains("corrupt_baseline")));

    // Case 3: Revocation watermark advanced since install (V018)
    let res_watermark = safe_reset_to_baseline(
        &record,
        Some(baseline_str),
        |_dig| false,
        6, // watermark 6 > 5
        |_dig| false,
    );
    assert!(matches!(res_watermark, Err(Error::Conflict(msg)) if msg.contains("baseline_revoked")));

    // Case 4: Baseline source has been revoked
    let res_revoked = safe_reset_to_baseline(
        &record,
        Some(baseline_str),
        |dig| dig == b_digest, // baseline source is revoked
        5,
        |_dig| false,
    );
    assert!(matches!(res_revoked, Err(Error::Conflict(msg)) if msg.contains("baseline_revoked")));

    // Case 5: Resetting to baseline causes critical regression
    let res_regression = safe_reset_to_baseline(
        &record,
        Some(baseline_str),
        |_dig| false,
        5,
        |_dig| true, // regression detected
    );
    assert!(
        matches!(res_regression, Err(Error::Conflict(msg)) if msg.contains("regression_blocked"))
    );

    // Case 6: Safe reset succeeds into STAGED reset (NEVER Active)
    let staged_reset =
        safe_reset_to_baseline(&record, Some(baseline_str), |_dig| false, 5, |_dig| false)
            .expect("Safe reset should succeed");

    assert_eq!(staged_reset.target_digest, b_digest);
    assert!(staged_reset.requires_evaluation);
    assert!(!staged_reset.is_active, "Reset must NEVER auto-activate!");
}

// ---------------------------------------------------------------------------
// V038 & V098: Interruption-Clean-Rollback & Staging Session Audit
// ---------------------------------------------------------------------------
#[test]
fn test_v038_staging_session_commit_and_abort() {
    let session = StagingSession::new("org.rsia", "skill_staging", "digest_1234")
        .expect("Valid session creation");

    assert!(!session.committed);
    assert!(!session.rolled_back);
    assert!(!session.active_overwritten);

    // Abort session (e.g. simulated disk failure, cancellation, or error)
    let aborted = session.abort();
    assert!(aborted.rolled_back);
    assert!(!aborted.committed);
    assert!(!aborted.active_overwritten);

    // Attempting to commit a rolled-back session must fail
    assert!(aborted.commit().is_err());

    // Fresh session commit
    let session2 = StagingSession::new("org.rsia", "skill_staging", "digest_5678")
        .expect("Valid session creation");
    let committed = session2.commit().expect("Commit should succeed");
    assert!(committed.committed);
    assert!(!committed.rolled_back);
    // Even when committed into staging, Active is NOT overwritten!
    assert!(!committed.active_overwritten);
}

// =========================================================================
// F05 Adversarial Regression (from controller review)
// =========================================================================
#[test]
fn test_f05_review_seed_reset_requires_consistent_current_watermark() {
    let record = SeedInstallRecord {
        publisher: "p".into(),
        asset_id: "a".into(),
        kind: "skill".into(),
        baseline_digest: hash(b"baseline"),
        local_digest: hash(b"local"),
        upstream_digest: None,
        installed_at: 1,
        status: SeedStatus::Installed,
        quarantine_reason: None,
        revocation_watermark: 10,
    };
    assert!(
        safe_reset_to_baseline(&record, Some("baseline"), |_| false, 1, |_| false).is_err(),
        "seed reset accepted a current watermark older than installation"
    );
}

async fn seed_store() -> (tempfile::TempDir, Store, Context, Vec<E16SourceRef>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("seeds.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let body = serde_json::json!({"schema_version":"fixture.seed_source.v1","id":"seed-source"});
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", "seed-source", admin.actor(), &body)
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "seed-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    (
        directory,
        store,
        admin,
        vec![E16SourceRef {
            kind: "artifact".into(),
            id: "seed-source".into(),
            digest: fingerprint(&body).unwrap(),
        }],
    )
}

fn install_request(key: &str, sources: Vec<E16SourceRef>) -> InstallCurrentSeedRequest {
    InstallCurrentSeedRequest {
        request_key: key.into(),
        baseline_identity: Some(SeedIdentity {
            publisher: "org.rsia".into(),
            asset_id: "reasoning-seed".into(),
            kind: "skill".into(),
        }),
        local_identity: Some(SeedIdentity {
            publisher: "org.rsia".into(),
            asset_id: "reasoning-seed".into(),
            kind: "skill".into(),
        }),
        local_marked: true,
        baseline_bytes: Some(b"baseline seed".to_vec()),
        local_bytes: Some(b"local seed".to_vec()),
        upstream: Some(SeedUpstream {
            path: "seeds/upstream.txt".into(),
            content_digest: hash(b"upstream seed"),
            bytes: b"upstream seed".to_vec(),
        }),
        compiler_version: "compiler-v1".into(),
        confirmed_bytes: None,
        environment_digest: hash(b"environment"),
        source_refs: sources,
    }
}

#[tokio::test]
async fn persistent_seed_install_restart_identity_and_reset_staging() {
    let (directory, store, admin, sources) = seed_store().await;
    let installed = PersistentSeedStore::install_current(
        &admin,
        &store,
        install_request("install-key", sources.clone()),
    )
    .await
    .map(|comparison| comparison.install)
    .unwrap();
    assert_eq!(installed.schema_version, E16_SEED_INSTALL_SCHEMA);
    assert_eq!(installed.payload.status, SeedStatus::Installed);
    assert_eq!(
        PersistentSeedStore::install_current(
            &admin,
            &store,
            install_request("install-key", sources.clone()),
        )
        .await
        .map(|comparison| comparison.install)
        .unwrap()
        .id,
        installed.id
    );
    let mut changed = install_request("different-key", sources);
    changed.baseline_bytes = Some(b"different baseline".to_vec());
    assert!(
        PersistentSeedStore::install_current(&admin, &store, changed)
            .await
            .map(|comparison| comparison.install)
            .is_err()
    );
    let staged = PersistentSeedStore::stage_reset_to_baseline(
        &admin,
        &store,
        &installed.id,
        "reset-key",
        &hash(b"environment"),
        "compiler-v1",
    )
    .await
    .unwrap();
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    assert!(staged.payload.candidate_ref.is_none());
    store.close().await;

    let reopened = Store::open(&directory.path().join("seeds.sqlite3"))
        .await
        .unwrap();
    let worker = Context::new("n", "admin", Role::Worker).unwrap();
    PersistentSeedStore::read_install(&worker, &reopened, &installed.id)
        .await
        .unwrap();
    PersistentPackageStore::read_staged(&worker, &reopened, &staged.id)
        .await
        .unwrap();
    let unrelated_worker = Context::new("n", "worker", Role::Worker).unwrap();
    assert!(
        PersistentSeedStore::read_install(&unrelated_worker, &reopened, &installed.id)
            .await
            .is_err()
    );
    assert_eq!(
        PersistentPackageStore::abort_staged(&admin, &reopened, &staged.id)
            .await
            .unwrap()
            .payload
            .state,
        StagedAssetState::Aborted
    );
}

#[tokio::test]
async fn seed_reset_reads_current_watermark_sources_environment_and_permissions() {
    let (_directory, store, admin, sources) = seed_store().await;
    let installed = PersistentSeedStore::install_current(
        &admin,
        &store,
        install_request("install-key", sources.clone()),
    )
    .await
    .map(|comparison| comparison.install)
    .unwrap();
    assert!(
        PersistentSeedStore::stage_reset_to_baseline(
            &admin,
            &store,
            &installed.id,
            "bad-environment",
            &hash(b"other-environment"),
            "compiler-v1",
        )
        .await
        .is_err()
    );
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    assert!(
        PersistentSeedStore::read_install(&agent, &store, &installed.id)
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "seed-source",
            admin.actor(),
            &serde_json::json!({"id":"seed-source"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "seed-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        PersistentSeedStore::install_current(
            &admin,
            &store,
            install_request("install-key", sources),
        )
        .await
        .map(|comparison| comparison.install)
        .is_err()
    );
    assert!(
        PersistentSeedStore::stage_reset_to_baseline(
            &admin,
            &store,
            &installed.id,
            "after-revoke",
            &hash(b"environment"),
            "compiler-v1",
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn prepared_seed_install_resumes_after_restart_without_unregistered_blobs() {
    let (directory, store, admin, sources) = seed_store().await;
    let request = install_request("prepared-restart", sources);
    let installed = PersistentSeedStore::install_current(&admin, &store, request.clone())
        .await
        .map(|comparison| comparison.install)
        .unwrap();
    let mut interrupted = installed.clone();
    interrupted.payload.status = SeedStatus::Prepared;
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &interrupted.id,
            admin.actor(),
            &interrupted,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let reopened = Store::open(&directory.path().join("seeds.sqlite3"))
        .await
        .unwrap();
    let resumed = PersistentSeedStore::install_current(&admin, &reopened, request)
        .await
        .map(|comparison| comparison.install)
        .unwrap();
    assert_eq!(resumed.payload.status, SeedStatus::Installed);
    assert_eq!(
        resumed.payload.baseline_digest,
        installed.payload.baseline_digest
    );
    assert_eq!(resumed.payload.local_digest, installed.payload.local_digest);
    assert_eq!(
        resumed.payload.upstream_digest,
        installed.payload.upstream_digest
    );
    // Current restart reads bind all actual registered bytes, not only hashes
    // copied from the envelope. Preserve the original successful recovery above.
    let upstream_path = directory
        .path()
        .join("blobs")
        .join(hash(b"n"))
        .join(resumed.payload.upstream_blob_digest.as_ref().unwrap());
    std::fs::write(&upstream_path, b"changed upstream bytes").unwrap();
    assert!(
        PersistentSeedStore::read_install(&admin, &reopened, &resumed.id)
            .await
            .is_err()
    );
    std::fs::write(&upstream_path, b"upstream seed").unwrap();
    PersistentSeedStore::read_install(&admin, &reopened, &resumed.id)
        .await
        .unwrap();
}

// =========================================================================
// F18 Adversarial Regression (from controller review): the pure reset gate
// must mirror the persistent path's status check.
// =========================================================================
#[test]
fn f18_pure_seed_reset_rejects_install_records_that_are_not_installed() {
    let installed = SeedInstallRecord {
        publisher: "org.rsia".into(),
        asset_id: "reasoning_seed".into(),
        kind: "skill".into(),
        baseline_digest: hash(b"baseline seed"),
        local_digest: hash(b"local seed edited"),
        upstream_digest: None,
        installed_at: 1000,
        status: SeedStatus::Installed,
        quarantine_reason: None,
        revocation_watermark: 10,
    };
    let accepted =
        safe_reset_to_baseline(&installed, Some("baseline seed"), |_| false, 10, |_| false)
            .expect("an installed record at the current watermark resets");
    assert!(!accepted.is_active);
    assert!(accepted.requires_evaluation);
    for status in [
        SeedStatus::Prepared,
        SeedStatus::Staged,
        SeedStatus::Quarantined,
        SeedStatus::Unknown,
    ] {
        let mut record = installed.clone();
        record.status = status;
        record.quarantine_reason =
            (status == SeedStatus::Quarantined).then(|| "revoked_dependency".to_string());
        let result =
            safe_reset_to_baseline(&record, Some("baseline seed"), |_| false, 10, |_| false);
        assert!(
            matches!(
                &result,
                Err(Error::Conflict(message))
                    if message == "reset_rejected: install record is not in the installed state"
            ),
            "safe_reset_to_baseline accepted a {status:?} install record: {result:?}"
        );
    }
}

// =========================================================================
// AG-166c install-only: the shared kernel reuses the E16.5 staging cap; the
// management first copy creates no staging entry.
// =========================================================================
#[tokio::test]
async fn management_seed_install_shares_the_global_capacity_gates() {
    let (_directory, store, admin, sources) = seed_store().await;
    // One real update staging plus nineteen explicit safe-B stagings fill the
    // instance-wide 20-package limit.
    let mut first = install_request("baseline-install", sources.clone());
    first.local_bytes = Some(b"baseline seed".to_vec());
    first.upstream = Some(SeedUpstream {
        path: "upstream/seed-v2.txt".into(),
        content_digest: hash(b"upstream v2 seed"),
        bytes: b"upstream v2 seed".to_vec(),
    });
    let installed = PersistentSeedStore::install_current(&admin, &store, first)
        .await
        .unwrap();
    assert!(installed.staged.is_some());
    for index in 0..19 {
        PersistentSeedStore::stage_reset_to_baseline(
            &admin,
            &store,
            &installed.install.id,
            &format!("fill-{index}"),
            &hash(b"environment"),
            "compiler-v1",
        )
        .await
        .unwrap();
    }
    let usage: evo_engine::capacity::V41CapacityUsage = {
        let mut session = store.session().await.unwrap();
        let usage = session
            .capacity_usage_v41(evo_engine::capacity::unix_now_secs().unwrap())
            .await
            .unwrap()
            .into();
        session.commit().await.unwrap();
        usage
    };
    assert_eq!(usage.staged_packages, 20);

    // The management builtin first copy creates no staging entry, so it is not
    // blocked by the full staging counter and does not change it.
    let i0_digest = store
        .put_blob_bounded(&admin, BUILTIN_I0_BYTES, 1024 * 1024)
        .await
        .unwrap();
    let input = serde_json::json!({
        "schema_version": "rsia.management.seed_install_input.v1",
        "catalog_asset_id": "improver-i0",
        "source_refs": sources,
        "environment_digest": hash(b"environment"),
        "compiler_version": "builtin-i0-v2",
    });
    let input_digest = store
        .put_blob_bounded(&admin, &serde_json::to_vec(&input).unwrap(), 64 * 1024)
        .await
        .unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "seed.install",
            serde_json::json!({
                "schema_version": "rsia.management.seed_install.v1",
                "request_key": "capacity-first-copy",
                "install_input_blob_digest": input_digest,
                "baseline_blob_digest": i0_digest,
                "local_blob_digest": i0_digest,
                "upstream_blob_digest": i0_digest,
            }),
        )
        .await
        .unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let status = dispatcher
                .status_view(&admin, queued.identity().id)
                .await
                .unwrap();
            if matches!(
                status.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("seed.install reaches a terminal state");
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert!(matches!(
        status.result.as_ref(),
        Some(ManagementResultV2::Seed(_))
    ));
    let usage: evo_engine::capacity::V41CapacityUsage = {
        let mut session = store.session().await.unwrap();
        let usage = session
            .capacity_usage_v41(evo_engine::capacity::unix_now_secs().unwrap())
            .await
            .unwrap()
            .into();
        session.commit().await.unwrap();
        usage
    };
    assert_eq!(usage.staged_packages, 20, "first copy consumes no staging");

    // The same kernel still refuses a 21st staging entry for an update.
    let mut overflow = install_request("overflow", sources);
    overflow.baseline_identity = Some(SeedIdentity {
        publisher: "org.rsia".into(),
        asset_id: "other-seed".into(),
        kind: "skill".into(),
    });
    overflow.local_identity = overflow.baseline_identity.clone();
    overflow.local_bytes = Some(b"other baseline".to_vec());
    overflow.baseline_bytes = Some(b"other baseline".to_vec());
    overflow.upstream = Some(SeedUpstream {
        path: "upstream/other.txt".into(),
        content_digest: hash(b"other upstream"),
        bytes: b"other upstream".to_vec(),
    });
    assert!(matches!(
        PersistentSeedStore::install_current(&admin, &store, overflow).await,
        Err(Error::Capacity(detail))
            if detail.item == evo_core::capacity::CapacityItem::StagingPackage
                && detail.limit == 20
    ));
    assert!(installed.staged.is_some());
    store.close().await;
}

// =========================================================================
// AG-166c: same-key replay keeps the original immutable image and its bytes.
// =========================================================================
#[tokio::test]
async fn management_seed_replay_preserves_original_baseline_and_local_bytes() {
    let (directory, store, admin, sources) = seed_store().await;
    let request = install_request("replay-key", sources.clone());
    let first = PersistentSeedStore::install_current(&admin, &store, request.clone())
        .await
        .unwrap();
    assert_eq!(first.class, SeedClass::BothChanged);
    let install_id = first.install.id.clone();
    let mut session = store.session().await.unwrap();
    let original_body = session
        .object_row(&admin, "artifact", &install_id)
        .await
        .unwrap()
        .unwrap()
        .body;
    session.commit().await.unwrap();
    let baseline_path = directory
        .path()
        .join("blobs")
        .join(hash(b"n"))
        .join(&first.install.payload.baseline_blob_digest);
    let local_path = directory
        .path()
        .join("blobs")
        .join(hash(b"n"))
        .join(&first.install.payload.local_blob_digest);
    let baseline_bytes = std::fs::read(&baseline_path).unwrap();
    let local_bytes = std::fs::read(&local_path).unwrap();
    assert_eq!(baseline_bytes, b"baseline seed");
    assert_eq!(local_bytes, b"local seed");

    let replay = PersistentSeedStore::install_current(&admin, &store, request)
        .await
        .unwrap();
    assert_eq!(replay.install.id, install_id);
    assert_eq!(
        fingerprint(&replay.install).unwrap(),
        fingerprint(&first.install).unwrap()
    );
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", &install_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        original_body
    );
    session.commit().await.unwrap();
    assert_eq!(std::fs::read(&baseline_path).unwrap(), baseline_bytes);
    assert_eq!(std::fs::read(&local_path).unwrap(), local_bytes);

    // A new key with the same identity and baseline cannot rewrite the original
    // baseline/local digests; it only returns the immutable install image.
    let mut changed = install_request("new-local-key", sources);
    changed.environment_digest = first.install.payload.environment_digest.clone();
    let changed = PersistentSeedStore::install_current(&admin, &store, changed)
        .await
        .unwrap();
    assert_eq!(changed.install.id, install_id);
    assert_eq!(
        changed.install.payload.baseline_digest,
        first.install.payload.baseline_digest
    );
    assert_eq!(
        changed.install.payload.local_digest,
        first.install.payload.local_digest
    );
    assert_eq!(std::fs::read(&baseline_path).unwrap(), baseline_bytes);
    assert_eq!(std::fs::read(&local_path).unwrap(), local_bytes);
    store.close().await;
}

// =========================================================================
// AG-166c: a legacy read confers no Current authority and no management reset.
// =========================================================================
#[tokio::test]
async fn management_legacy_install_read_does_not_authorize_new_reset() {
    let (_directory, store, admin, sources) = seed_store().await;
    let baseline = b"legacy baseline";
    let local = b"legacy local";
    let baseline_blob = store
        .put_blob_bounded(&admin, baseline, 1024 * 1024)
        .await
        .unwrap();
    let local_blob = store
        .put_blob_bounded(&admin, local, 1024 * 1024)
        .await
        .unwrap();
    let envelope = E16Envelope {
        schema_version: E16_V1_SEED_INSTALL_SCHEMA.into(),
        id: "e16-seed-install-legacy".into(),
        namespace: admin.namespace().into(),
        owner_actor: admin.actor().into(),
        request_key: "legacy-install".into(),
        input_digest: hash(b"legacy-input"),
        created_at: 1,
        updated_at: 1,
        source_refs: sources.clone(),
        revoke_watermark: 1,
        payload: SeedInstallPayloadV1 {
            publisher: "org.rsia".into(),
            asset_id: "legacy-seed".into(),
            kind: "skill".into(),
            baseline_blob_digest: baseline_blob,
            baseline_digest: hash(baseline),
            local_blob_digest: local_blob,
            local_digest: hash(local),
            upstream_blob_digest: None,
            upstream_digest: None,
            environment_digest: hash(b"environment"),
            status: SeedStatus::Installed,
            quarantine_reason: None,
        },
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", &envelope.id, admin.actor(), &envelope)
        .await
        .unwrap();
    for source in &sources {
        session
            .put_edge(&admin, "artifact", &envelope.id, &source.kind, &source.id)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let legacy = PersistentSeedStore::read_legacy_install(&admin, &store, &envelope.id)
        .await
        .unwrap();
    assert_eq!(legacy.schema_version, E16_V1_SEED_INSTALL_SCHEMA);
    assert!(matches!(
        PersistentSeedStore::read_install(&admin, &store, &envelope.id).await,
        Err(Error::Conflict(message)) if message == "seed_install_legacy"
    ));
    // The management reset stays unwired: a legacy read cannot mint a reset
    // job, and the stored record is untouched.
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let error = dispatcher
        .submit(
            &admin,
            "seed.reset_stage",
            serde_json::json!({
                "schema_version": "rsia.management.seed_reset_stage.v1",
                "request_key": "legacy-reset",
                "install_id": envelope.id,
            }),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Invalid(message) if message == "management_operation_not_wired"),
        "{error:?}"
    );
    let mut session = store.session().await.unwrap();
    let body: serde_json::Value = session
        .need(&admin, "artifact", &envelope.id)
        .await
        .unwrap();
    let jobs: Vec<serde_json::Value> = session.list(&admin, "job").await.unwrap();
    let staged: Vec<serde_json::Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(body, serde_json::to_value(&envelope).unwrap());
    assert!(jobs.is_empty());
    let seed_artifacts = staged
        .iter()
        .filter(|body| {
            body["schema_version"]
                .as_str()
                .is_some_and(|schema| schema.starts_with("rsia.e16.seed"))
        })
        .count();
    assert_eq!(seed_artifacts, 1, "only the legacy install artifact");
    store.close().await;
}
