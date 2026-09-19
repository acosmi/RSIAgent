#[path = "../../../examples/reference-host/src/curriculum_target.rs"]
mod curriculum_target;

use evo_core::curriculum::{
    CurriculumControlProfileV1, LearnerStateV2, ProbeTerminal, ProposalAttemptOutcome,
    RegisteredProperty, StructuredTestProposalV1,
};
use evo_core::evaluation::DataUse;
use evo_core::{Context, Role, fingerprint, hash};
use evo_engine::curriculum::{
    CurriculumSourceArtifactV1, CurriculumSourceKindV1, CurriculumTaskCandidateV1,
    DevelopmentCycleReceiptV1, PersistentCurriculumCoordinator, ProposalAttemptReceiptV1,
};
use evo_engine::curriculum_profiles::{
    ClampOutput, OfflineProfileCheck, RegisteredPureFunctionProfileV1, ValidityReportV1,
    check_target_outputs, registered_profile_source_bodies, runtime_validity_report,
};
use evo_storage::Store;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn profile() -> CurriculumControlProfileV1 {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    registered.validate().unwrap();
    assert_ne!(registered.target_digest, registered.oracle_digest);
    CurriculumControlProfileV1::offline_default(
        "pure_function_test_proposal.v1",
        d("task-space"),
        registered.oracle_digest,
        registered.runner_digest,
    )
    .unwrap()
}

fn source(
    id: &str,
    source_kind: CurriculumSourceKindV1,
    subject_digest: String,
    body: serde_json::Value,
) -> CurriculumSourceArtifactV1 {
    CurriculumSourceArtifactV1 {
        schema_version: CurriculumSourceArtifactV1::SCHEMA.into(),
        id: id.into(),
        source_kind,
        data_use: DataUse::Development,
        subject_digest,
        body_digest: fingerprint(&body).unwrap(),
        body,
        dependency_ids: vec![],
    }
}

fn state() -> LearnerStateV2 {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: "learner-state".into(),
        profile_id: "pure_function_test_proposal.v1".into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("environment"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

async fn setup() -> (tempfile::TempDir, Store, PersistentCurriculumCoordinator) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("curriculum.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = PersistentCurriculumCoordinator::new(
        store.clone(),
        Context::new("n", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    assert!(
        worker
            .register_profile_and_state(profile(), state())
            .await
            .is_err()
    );
    let coordinator =
        PersistentCurriculumCoordinator::new(store.clone(), admin.clone(), "admin").unwrap();
    let mut prefilled = state();
    prefilled.cooldown_remaining_cycles = 1;
    assert!(
        coordinator
            .register_profile_and_state(profile(), prefilled)
            .await
            .is_err()
    );
    coordinator
        .register_source(source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            serde_json::json!({"buckets":["clamp-boundary"]}),
        ))
        .await
        .unwrap();
    for (id, body) in registered_profile_source_bodies() {
        let kind = if id.contains("target") || id.contains("source") {
            CurriculumSourceKindV1::TargetSpec
        } else if id.contains("oracle") {
            CurriculumSourceKindV1::OracleSpec
        } else {
            CurriculumSourceKindV1::RunnerSpec
        };
        coordinator
            .register_source(source(&id, kind, fingerprint(&body).unwrap(), body))
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "curriculum-initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    coordinator
        .register_profile_and_state(profile(), state())
        .await
        .unwrap();
    (directory, store, coordinator)
}

#[tokio::test]
async fn persistent_trigger_attempt_limit_cooldown_and_revoke_are_fail_closed() {
    let (directory, store, coordinator) = setup().await;
    let job = coordinator
        .schedule_probe("pure_function_test_proposal.v1", "learner-state", 1_000_000)
        .await
        .unwrap();
    assert_eq!(job.curriculum_share_limit_micros, 200_000);
    assert_eq!(job.effective_monetary_limit_micros, 0);
    assert_eq!(job.provider_dispatch_count, 0);
    assert_eq!(job.terminal, Some(ProbeTerminal::BudgetExhausted));
    let cooldown = coordinator
        .schedule_probe("pure_function_test_proposal.v1", "learner-state", 1_000_000)
        .await
        .unwrap();
    assert_ne!(cooldown.id, job.id);
    assert_eq!(cooldown.terminal, Some(ProbeTerminal::Cooldown));
    store.close().await;

    let reopened = Store::open(&directory.path().join("curriculum.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let resumed =
        PersistentCurriculumCoordinator::new(reopened.clone(), admin.clone(), "admin").unwrap();
    assert_eq!(
        resumed
            .schedule_probe("pure_function_test_proposal.v1", "learner-state", 1_000_000)
            .await
            .unwrap()
            .id,
        cooldown.id
    );
    assert!(
        resumed
            .record_probe_attempt(ProposalAttemptReceiptV1 {
                schema_version: "rsia.curriculum_proposal_attempt.v1".into(),
                id: "attempt-disabled".into(),
                job_id: job.id.clone(),
                outcome: ProposalAttemptOutcome::Malformed,
                source_artifact_ids: vec![
                    RegisteredPureFunctionProfileV1::clamp_i64().runner_source_id,
                ],
            })
            .await
            .is_err()
    );
    let mut session = reopened.session().await.unwrap();
    session
        .bump_watermark(&admin, "curriculum-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        resumed
            .schedule_probe("pure_function_test_proposal.v1", "learner-state", 1_000_000)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn independent_oracle_is_offline_and_sandbox_unavailable_never_selects_a_task() {
    let (_directory, _store, coordinator) = setup().await;
    let job = coordinator
        .schedule_probe("pure_function_test_proposal.v1", "learner-state", 1_000_000)
        .await
        .unwrap();
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: "proposal-boundary".into(),
        probe_job_id: job.id,
        target_id: curriculum_target::TARGET_ID.into(),
        property: RegisteredProperty::BelowMapsToMin,
        value: -2,
        min: -1,
        max: 1,
        parent_family: "clamp-family".into(),
        data_use: DataUse::Development,
        source_artifact_ids: vec!["task-space-source".into()],
        reason: "exercise registered lower boundary".into(),
    };
    coordinator
        .store_proposal("learner-state", proposal.clone())
        .await
        .unwrap();
    let target = curriculum_target::clamp_i64(curriculum_target::ClampInput {
        value: proposal.value,
        min: proposal.min,
        max: proposal.max,
    })
    .unwrap();
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let offline = check_target_outputs(
        &registered,
        &proposal,
        ClampOutput { clamped: target },
        ClampOutput { clamped: target },
    )
    .unwrap();
    assert!(matches!(offline, OfflineProfileCheck::Passed { .. }));
    let report = runtime_validity_report(&registered, &proposal, &offline).unwrap();
    assert!(matches!(
        report,
        ValidityReportV1::SandboxUnavailable { .. }
    ));
    let report_id = coordinator
        .record_validity_report("learner-state", report)
        .await
        .unwrap();
    assert!(
        coordinator
            .select_next_task(
                "learner-state",
                &[CurriculumTaskCandidateV1 {
                    id: "candidate-task".into(),
                    parent_family: proposal.parent_family.clone(),
                    coverage_bucket_id: None,
                    stable_choice_seq: 1,
                    proposal_id: proposal.id.clone(),
                    validity_report_id: report_id,
                }],
            )
            .await
            .is_err()
    );
    assert!(
        coordinator
            .record_applied_learning_asset(
                "learner-state",
                "missing-run",
                "missing-application",
                "missing-execution",
            )
            .await
            .is_err()
    );

    let disagreement = check_target_outputs(
        &registered,
        &proposal,
        ClampOutput { clamped: 99 },
        ClampOutput { clamped: 99 },
    )
    .unwrap();
    assert!(matches!(
        disagreement,
        OfflineProfileCheck::Quarantined { .. }
    ));
    let mut false_precondition = proposal.clone();
    false_precondition.property = RegisteredProperty::BelowMapsToMin;
    false_precondition.value = 0;
    let false_target = ClampOutput { clamped: 0 };
    assert!(matches!(
        check_target_outputs(&registered, &false_precondition, false_target, false_target,)
            .unwrap(),
        OfflineProfileCheck::Quarantined { .. }
    ));
    let mut forged =
        serde_json::to_value(runtime_validity_report(&registered, &proposal, &offline).unwrap())
            .unwrap();
    forged["sandbox_verified"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ValidityReportV1>(forged).is_err());
}

#[tokio::test]
async fn untyped_sources_and_boolean_like_results_never_update_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("untyped.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let coordinator =
        PersistentCurriculumCoordinator::new(store.clone(), admin.clone(), "admin").unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "task-space-source",
            "admin",
            &serde_json::json!({"authorized":true,"learned":true}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "untyped-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        coordinator
            .register_profile_and_state(profile(), state())
            .await
            .is_err()
    );

    let (_directory, _store, coordinator) = setup().await;
    assert!(
        coordinator
            .record_cycle(DevelopmentCycleReceiptV1 {
                schema_version: "rsia.development_cycle_receipt.v1".into(),
                id: "missing-cycle".into(),
                state_id: "learner-state".into(),
                development_request_fact_id: "missing-request-fact".into(),
                development_observed_fact_id: "missing-observed-fact".into(),
            })
            .await
            .is_err()
    );
}
