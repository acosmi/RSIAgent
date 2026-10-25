//! AG-170 (plan §8.1.1/§8.4.1, V109): the runner-verified eligibility chain,
//! the v3 learner-state write path and the v2 three-arm selection record.
//!
//! The registered-runner fixtures are copied from
//! `tests/curriculum_cleanup_v42.rs` and `tests/development_receipts.rs`
//! (test crates cannot import each other); the originals are untouched.

#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::curriculum::{
    CurriculumControlProfileV1, FailureRecordStatus, FailureRecordType, FailureRecordV1,
    LearnerStateV2, LearnerStateV3, RegisteredProperty, RetestPriorityMarkV1,
    StructuredTestProposalV1,
};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Role, fingerprint, hash};
use evo_engine::curriculum::{
    ConsumedAssetInfluenceV1, CurriculumSourceArtifactV1, CurriculumSourceKindV1,
    CurriculumTaskCandidateV1, DevelopmentCycleReceiptV1, PersistentCurriculumCoordinator,
    SelectionPriorityCategory, TaskSelectionDecisionV2,
};
use evo_engine::curriculum_profiles::{
    CLAMP_TARGET_ID, RegisteredPureFunctionProfileV1, check_target_outputs,
    registered_profile_source_bodies, runner_verified_report, runtime_validity_report,
};
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
    registered_runner_digest,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::optimization::{
    DevRunner, DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";
const PROFILE_ID: &str = "pure_function_test_proposal.v1";
const STATE_ID: &str = "learner-state";
const ENVELOPE: &str = "rsia.curriculum_artifact_envelope.v1";
const STATE_KIND: &str = "learner_state_v2";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn storage_id(record_kind: &str, id: &str) -> String {
    format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
}

// ---------------------------------------------------------------------------
// Fixtures (copied from tests/curriculum_cleanup_v42.rs)
// ---------------------------------------------------------------------------

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    }
}

fn authority(id: &str, family: &str) -> StoredTraceAuthority {
    let body = format!("trusted-run-{id}");
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: family.into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: family.into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.clone(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

fn tasks() -> Vec<DevelopmentTaskSpecV1> {
    vec![
        DevelopmentTaskSpecV1::registered(
            "task-a",
            "family-a",
            RegisteredTargetInputV1::ClampI64 {
                value: -2,
                min: -1,
                max: 1,
            },
        )
        .unwrap(),
    ]
}

fn control() -> DevelopmentControlV1 {
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = grader_spec();
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: CONTROL_ID.into(),
        namespace: NAMESPACE.into(),
        billing_scope: SCOPE.into(),
        root_budget_id: "dev-root".into(),
        executor_actor: "dev-executor".into(),
        grader_actor: "dev-grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "dev-manifest".into(),
        manifest_digest: String::new(),
        tasks: tasks(),
        environment_digest: d("environment"),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        source_ids: vec!["run-a".into(), "run-b".into()],
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

fn build_request(
    control: &DevelopmentControlV1,
    request_id: &str,
    episode_id: &str,
) -> DevelopmentRunRequest {
    DevelopmentRunRequest {
        request_id: request_id.into(),
        namespace: NAMESPACE.into(),
        purpose: Purpose::Development,
        episode_id: episode_id.into(),
        step: 1,
        attempt: 1,
        manifest: control.manifest().unwrap(),
        parent_bundle_digest: d("parent-bundle"),
        candidate_bundle_digest: trusted_compiler_inputs::software_candidate(
            NAMESPACE,
            &control.environment_digest,
            "candidate-bundle",
        )
        .digest,
        environment_digest: control.environment_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        rules_digest: control.rules_digest.clone(),
        tools_digest: control.tools_digest.clone(),
        revoke_watermark: 1,
        idempotency_key: format!("{request_id}-idem"),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    _host: Context,
    executor: Context,
    grader: Context,
    control: DevelopmentControlV1,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("curriculum-v45.sqlite3");
        let store = Store::open(&path).await.unwrap();
        let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
        let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
        let executor = Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap();
        let grader = Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap();
        for (id, family) in [("run-a", "family-a"), ("run-b", "family-b")] {
            store_trace_authority(&store, &host, &authority(id, family))
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, "initial-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "dev-root".into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "fixture-only".into(),
                    authorization_receipt_digest: d("admin-budget-receipt"),
                    per_call_cap_micros: 10,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let control = control();
        let candidate = trusted_compiler_inputs::software_candidate(
            NAMESPACE,
            &control.environment_digest,
            "candidate-bundle",
        );
        trusted_compiler_inputs::persist_bundle(
            &store,
            NAMESPACE,
            &candidate,
            &control.environment_digest,
            1,
        )
        .await
        .unwrap();
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        Self {
            _dir: dir,
            store,
            admin,
            _host: host,
            executor,
            grader,
            control,
        }
    }

    fn runner(&self) -> RegisteredDevelopmentRunner {
        RegisteredDevelopmentRunner::with_clock(
            self.store.clone(),
            self.executor.clone(),
            self.grader.clone(),
            CONTROL_ID,
            600,
            Arc::new(|| 100),
        )
        .unwrap()
    }

    async fn commit_facts(
        &self,
        request: &DevelopmentRunRequest,
        report: &DevelopmentRunReport,
    ) -> (String, String) {
        let journal =
            StoreOptimizationJournal::new(self.store.clone(), self.admin.clone(), "admin").unwrap();
        let extras = vec![
            StageDependency {
                kind: "run".into(),
                id: "run-a".into(),
            },
            StageDependency {
                kind: "run".into(),
                id: "run-b".into(),
            },
            StageDependency {
                kind: "revoke_watermark".into(),
                id: "1".into(),
            },
            StageDependency {
                kind: "artifact".into(),
                id: "optgrant-fixture".into(),
            },
            StageDependency {
                kind: "artifact".into(),
                id: "optstage-step-fixture".into(),
            },
        ];
        let request_fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: NAMESPACE.into(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentRequestPrepared,
            request_id: request.request_id.clone(),
            input_digest: request.manifest.digest.clone(),
            output_digest: None,
            dependencies: extras
                .iter()
                .cloned()
                .chain(std::iter::once(StageDependency {
                    kind: "environment".into(),
                    id: request.environment_digest.clone(),
                }))
                .collect(),
            payload: serde_json::to_value(request).unwrap(),
        }
        .seal()
        .unwrap();
        let mut dependencies: Vec<StageDependency> = report
            .results
            .iter()
            .flat_map(|result| {
                [
                    StageDependency {
                        kind: "execution".into(),
                        id: result.parent_execution_id.clone(),
                    },
                    StageDependency {
                        kind: "execution".into(),
                        id: result.candidate_execution_id.clone(),
                    },
                    StageDependency {
                        kind: "grader".into(),
                        id: result.grader_receipt_digest.clone(),
                    },
                ]
            })
            .collect();
        dependencies.extend(evo_engine::development::typed_receipt_closure(report).unwrap());
        dependencies.extend(extras);
        let observed_fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: NAMESPACE.into(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id: request.request_id.clone(),
            input_digest: request.manifest.digest.clone(),
            output_digest: None,
            dependencies,
            payload: serde_json::to_value(report).unwrap(),
        }
        .seal()
        .unwrap();
        let inputs = trusted_compiler_inputs::bundle_use(
            &self.store,
            NAMESPACE,
            &request.candidate_bundle_digest,
        )
        .await
        .unwrap();
        journal
            .commit_bound(request_fact.clone(), &inputs)
            .await
            .unwrap();
        journal
            .commit_bound(observed_fact.clone(), &inputs)
            .await
            .unwrap();
        (request_fact.artifact_id, observed_fact.artifact_id)
    }
}

fn curriculum_source(
    id: &str,
    source_kind: CurriculumSourceKindV1,
    subject_digest: String,
    body: Value,
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

fn curriculum_source_ids() -> Vec<String> {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    vec![
        registered.oracle_source_id,
        registered.runner_source_id,
        registered.target_source_id,
        "task-space-source".into(),
    ]
}

async fn curriculum_with_profile(
    fixture: &Fixture,
    seed_commitment: Option<String>,
) -> PersistentCurriculumCoordinator {
    let coordinator =
        PersistentCurriculumCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
            .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets":["family-a","family-b","family-retest"]}),
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
            .register_source(curriculum_source(
                &id,
                kind,
                fingerprint(&body).unwrap(),
                body,
            ))
            .await
            .unwrap();
    }
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let mut profile = CurriculumControlProfileV1::offline_default(
        PROFILE_ID,
        d("task-space"),
        registered.oracle_digest.clone(),
        registered.runner_digest.clone(),
    )
    .unwrap();
    profile.fixed_random_seed_commitment = seed_commitment;
    let state = LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: STATE_ID.into(),
        profile_id: PROFILE_ID.into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: fixture.control.environment_digest.clone(),
        grader_digest: fixture.control.grader_digest.clone(),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: curriculum_source_ids(),
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    };
    coordinator
        .register_profile_and_state(profile, state)
        .await
        .unwrap();
    coordinator
}

/// Writes a v3 learner state at the production storage address in the exact
/// envelope shape the production code writes (the same hand-rolled record
/// pattern `curriculum_cleanup_v42.rs` uses for kinds the offline profile
/// cannot produce through the public API).
async fn overwrite_state(store: &Store, admin: &Context, state: &LearnerStateV3) {
    #[derive(Serialize)]
    struct ArtifactEnvelope<'a, T> {
        schema_version: &'a str,
        id: String,
        record_kind: &'a str,
        payload: &'a T,
    }
    let mut session = store.session().await.unwrap();
    session
        .put(
            admin,
            "artifact",
            &storage_id(STATE_KIND, &state.id),
            "admin",
            &ArtifactEnvelope {
                schema_version: ENVELOPE,
                id: storage_id(STATE_KIND, &state.id),
                record_kind: STATE_KIND,
                payload: state,
            },
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn base_state(fixture: &Fixture) -> LearnerStateV3 {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    LearnerStateV3::from_v2(LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: STATE_ID.into(),
        profile_id: PROFILE_ID.into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: fixture.control.environment_digest.clone(),
        grader_digest: fixture.control.grader_digest.clone(),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: curriculum_source_ids(),
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    })
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn record_cycle_writes_the_v3_wire_and_clean_passes_create_no_records() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum_with_profile(&fixture, None).await;
    let request = build_request(&fixture.control, "dev-request-1", "episode-1");
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let state = coordinator
        .record_cycle(DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: "cycle-1".into(),
            state_id: STATE_ID.into(),
            development_request_fact_id: request_fact_id,
            development_observed_fact_id: observed_fact_id,
        })
        .await
        .unwrap();
    // The registered pure-function pair is a verified pass, so no failure
    // record appears and the legacy cluster list stays empty.
    assert_eq!(state.schema_version, "rsia.learner_state.v3");
    assert!(state.failure_records.is_empty());
    assert!(state.failure_clusters.is_empty());
    assert_eq!(state.completed_cycles.len(), 1);
    // The stored payload really is the v3 wire at the fixed v2 address.
    let mut session = fixture.store.session().await.unwrap();
    let body: Value = session
        .get(
            &fixture.admin,
            "artifact",
            &storage_id(STATE_KIND, STATE_ID),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        body["payload"]["schema_version"],
        json!("rsia.learner_state.v3")
    );
}

#[tokio::test]
async fn runner_verified_report_selects_through_the_public_api_with_three_arms() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum_with_profile(&fixture, Some(d("fixed-random-seed"))).await;
    // One real development cycle so a verified fact pair exists to bind.
    let request = build_request(&fixture.control, "dev-request-1", "episode-1");
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    // The offline report is sandbox-unavailable and stays ineligible.
    let job = coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: "proposal-boundary".into(),
        probe_job_id: job.id.clone(),
        target_id: CLAMP_TARGET_ID.into(),
        property: RegisteredProperty::BelowMapsToMin,
        value: -2,
        min: -1,
        max: 1,
        parent_family: "family-b".into(),
        data_use: DataUse::Development,
        source_artifact_ids: vec!["task-space-source".into()],
        reason: "exercise registered lower boundary".into(),
    };
    coordinator
        .store_proposal(STATE_ID, proposal.clone())
        .await
        .unwrap();
    let offline = check_target_outputs(
        &registered,
        &proposal,
        evo_engine::curriculum_profiles::ClampOutput { clamped: -1 },
        evo_engine::curriculum_profiles::ClampOutput { clamped: -1 },
    )
    .unwrap();
    let offline_report = runtime_validity_report(&registered, &proposal, &offline).unwrap();
    let offline_id = coordinator
        .record_validity_report(STATE_ID, offline_report)
        .await
        .unwrap();
    // The runner-verified report binds the real fact pair of the cycle.
    let verified_report =
        runner_verified_report(&registered, &proposal, &request_fact_id, &observed_fact_id)
            .unwrap();
    let verified_id = coordinator
        .record_validity_report(STATE_ID, verified_report)
        .await
        .unwrap();
    let decision = coordinator
        .select_next_task(
            STATE_ID,
            &[
                CurriculumTaskCandidateV1 {
                    id: "task-offline".into(),
                    parent_family: "family-a".into(),
                    coverage_bucket_id: Some("family-a".into()),
                    stable_choice_seq: 1,
                    proposal_id: proposal.id.clone(),
                    validity_report_id: offline_id,
                },
                CurriculumTaskCandidateV1 {
                    id: "task-verified".into(),
                    parent_family: "family-b".into(),
                    coverage_bucket_id: Some("family-b".into()),
                    stable_choice_seq: 2,
                    proposal_id: proposal.id.clone(),
                    validity_report_id: verified_id,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(decision.schema_version, "rsia.curriculum_selection.v2");
    assert_eq!(decision.chosen_task_id, "task-verified");
    assert_eq!(decision.priority_category, SelectionPriorityCategory::Other);
    // Both counterfactual arms are recorded; the frozen arm is the lowest
    // stable sequence among the eligible set (the verified candidate only).
    assert_eq!(decision.frozen_state_choice_task_id, "task-verified");
    assert_eq!(decision.fixed_random_choice_task_id, "task-verified");
    assert_eq!(
        decision.fixed_random_seed_commitment.as_deref(),
        Some(d("fixed-random-seed").as_str())
    );
    // The basis is state-derived, never a fixed literal.
    assert!(decision.selection_basis.contains("priority=Other"));
    assert!(!decision.selection_basis.is_empty());
}

#[tokio::test]
async fn retest_mark_grants_one_shot_priority_over_lower_sequences() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum_with_profile(&fixture, Some(d("fixed-random-seed"))).await;
    let request = build_request(&fixture.control, "dev-request-1", "episode-1");
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let job = coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();
    // One proposal per family: the production check requires a candidate's
    // family to equal its proposal's family.
    let mut verified_ids = Vec::new();
    for (proposal_id, family) in [
        ("proposal-plain", "family-a"),
        ("proposal-retest", "family-retest"),
    ] {
        let proposal = StructuredTestProposalV1 {
            schema_version: "rsia.structured_test_proposal.v1".into(),
            id: proposal_id.into(),
            probe_job_id: job.id.clone(),
            target_id: CLAMP_TARGET_ID.into(),
            property: RegisteredProperty::BelowMapsToMin,
            value: -2,
            min: -1,
            max: 1,
            parent_family: family.into(),
            data_use: DataUse::Development,
            source_artifact_ids: vec!["task-space-source".into()],
            reason: "exercise registered lower boundary".into(),
        };
        coordinator
            .store_proposal(STATE_ID, proposal.clone())
            .await
            .unwrap();
        let verified_report =
            runner_verified_report(&registered, &proposal, &request_fact_id, &observed_fact_id)
                .unwrap();
        verified_ids.push(
            coordinator
                .record_validity_report(STATE_ID, verified_report)
                .await
                .unwrap(),
        );
    }
    let [verified_id_plain, verified_id_retest]: [&str; 2] = [&verified_ids[0], &verified_ids[1]];
    // A hand-written v3 state (the exact envelope shape production writes)
    // grants the retest family a one-shot mark: the family holds no failure
    // record, exactly the §8.4.1 case where the mark itself carries the
    // priority.
    let mut state = base_state(&fixture);
    state.retest_priority_marks = vec![RetestPriorityMarkV1 {
        family: "family-retest".into(),
        release_id: "release-1".into(),
        consumed: false,
    }];
    state.validate().unwrap();
    overwrite_state(&fixture.store, &fixture.admin, &state).await;
    let candidates = [
        CurriculumTaskCandidateV1 {
            id: "task-plain".into(),
            parent_family: "family-a".into(),
            coverage_bucket_id: Some("family-a".into()),
            stable_choice_seq: 1,
            proposal_id: "proposal-plain".into(),
            validity_report_id: verified_id_plain.to_string(),
        },
        CurriculumTaskCandidateV1 {
            id: "task-retest".into(),
            parent_family: "family-retest".into(),
            coverage_bucket_id: Some("family-retest".into()),
            stable_choice_seq: 7,
            proposal_id: "proposal-retest".into(),
            validity_report_id: verified_id_retest.to_string(),
        },
    ];
    let first = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    // The adaptive arm outranks the lower sequence; the frozen arm, blind to
    // the mark, still picks the lowest sequence: the visible difference
    // V109.b requires, attributable to the asset via the release id.
    assert_eq!(first.chosen_task_id, "task-retest");
    assert_eq!(
        first.priority_category,
        SelectionPriorityCategory::RetestDue
    );
    assert_eq!(
        first.consumed_asset,
        Some(ConsumedAssetInfluenceV1 {
            release_id: "release-1".into(),
            influence: "retest_after_apply".into(),
        })
    );
    assert_eq!(first.frozen_state_choice_task_id, "task-plain");
    assert_ne!(first.chosen_task_id, first.frozen_state_choice_task_id);
    let first_basis = first.selection_basis.clone();
    // The mark is one-shot: the next selection over the same candidate set no
    // longer outranks the lower sequence, and the basis reflects the change.
    let second = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(second.chosen_task_id, "task-plain");
    assert_eq!(second.priority_category, SelectionPriorityCategory::Other);
    assert_ne!(first_basis, second.selection_basis);
}

#[tokio::test]
async fn unsupported_scope_when_an_active_failure_cluster_has_no_registered_bucket() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum_with_profile(&fixture, None).await;
    // A hand-written v3 state carrying one active absolute-failure record
    // whose cluster is not a registered coverage bucket.
    let mut state = base_state(&fixture);
    state.failure_records = vec![FailureRecordV1 {
        record_id: "failure-family-orphan".into(),
        cluster_id: "family-orphan".into(),
        record_type: FailureRecordType::AbsoluteFailure,
        status: FailureRecordStatus::Active,
        cycle_id: "cycle-1".into(),
        execution_receipt_digest: d("exec"),
        grader_receipt_digest: d("grader"),
        skill_snapshot_digest: state.skill_snapshot_digest.clone(),
        environment_digest: state.environment_digest.clone(),
        grader_version_digest: state.grader_digest.clone(),
        transition_note: None,
        retest_release_id: None,
    }];
    state.validate().unwrap();
    overwrite_state(&fixture.store, &fixture.admin, &state).await;
    let error = coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unsupported_scope"),
        "expected unsupported_scope, got {error}"
    );
    // With the cluster registered as a bucket the failure gap schedules (and
    // the zero-budget offline profile ends the probe in BudgetExhausted).
    let mut state = base_state(&fixture);
    state.failure_records = vec![FailureRecordV1 {
        record_id: "failure-family-a".into(),
        cluster_id: "family-a".into(),
        record_type: FailureRecordType::AbsoluteFailure,
        status: FailureRecordStatus::Active,
        cycle_id: "cycle-1".into(),
        execution_receipt_digest: d("exec"),
        grader_receipt_digest: d("grader"),
        skill_snapshot_digest: state.skill_snapshot_digest.clone(),
        environment_digest: state.environment_digest.clone(),
        grader_version_digest: state.grader_digest.clone(),
        transition_note: None,
        retest_release_id: None,
    }];
    state
        .coverage_buckets
        .push(evo_core::curriculum::CoverageBucketState {
            bucket_id: "family-a".into(),
            task_space_source_id: "task-space-source".into(),
            observed_independent_clusters: 0,
            required_independent_clusters: 1,
        });
    state.validate().unwrap();
    overwrite_state(&fixture.store, &fixture.admin, &state).await;
    let job = coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();
    assert_eq!(
        job.terminal,
        Some(evo_core::curriculum::ProbeTerminal::BudgetExhausted)
    );
}

#[tokio::test]
async fn fixed_random_arm_is_reproducible_across_equal_candidate_sets() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum_with_profile(&fixture, Some(d("fixed-random-seed"))).await;
    let request = build_request(&fixture.control, "dev-request-1", "episode-1");
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let job = coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: "proposal-boundary".into(),
        probe_job_id: job.id.clone(),
        target_id: CLAMP_TARGET_ID.into(),
        property: RegisteredProperty::BelowMapsToMin,
        value: -2,
        min: -1,
        max: 1,
        parent_family: "family-a".into(),
        data_use: DataUse::Development,
        source_artifact_ids: vec!["task-space-source".into()],
        reason: "exercise registered lower boundary".into(),
    };
    coordinator
        .store_proposal(STATE_ID, proposal.clone())
        .await
        .unwrap();
    let verified_report =
        runner_verified_report(&registered, &proposal, &request_fact_id, &observed_fact_id)
            .unwrap();
    let verified_id = coordinator
        .record_validity_report(STATE_ID, verified_report)
        .await
        .unwrap();
    let mut candidates: Vec<CurriculumTaskCandidateV1> = (0..4)
        .map(|index| CurriculumTaskCandidateV1 {
            id: format!("task-{index}"),
            parent_family: "family-a".into(),
            coverage_bucket_id: None,
            stable_choice_seq: index,
            proposal_id: proposal.id.clone(),
            validity_report_id: verified_id.clone(),
        })
        .collect();
    candidates.reverse();
    let decision: TaskSelectionDecisionV2 = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    // Reproducible: the same committed seed over the same set picks the same
    // task again, independent of the input order.
    let again = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(
        again.fixed_random_choice_task_id,
        decision.fixed_random_choice_task_id
    );
    assert_eq!(
        again.frozen_state_choice_task_id,
        decision.frozen_state_choice_task_id
    );
    // The frozen arm is order-independent too: the lowest stable sequence.
    assert_eq!(decision.frozen_state_choice_task_id, "task-0");
}
