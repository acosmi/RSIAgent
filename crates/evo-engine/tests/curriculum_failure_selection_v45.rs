//! AG-170 (plan §8.1.1/§8.4.1, V109): the runner-verified eligibility chain,
//! the release-family binding, the v3 learner-state write path and the v2
//! three-arm selection record.
//!
//! The registered-runner fixtures are copied from
//! `tests/curriculum_cleanup_v42.rs`, `tests/development_receipts.rs` and
//! `tests/release_store.rs` (test crates cannot import each other); the
//! originals are untouched.
//!
//! Boundaries of what an integration test can assemble (W0 ruling
//! 2026-10-25T13:45:14Z):
//! - The complete trusted application path (approve → activate → managed run
//!   → Host receipt → `record_applied_learning_asset`) cannot be assembled: the
//!   formal operator channel that approves and activates a release is only
//!   available inside the `formal_local_host` process. The tests below pin the
//!   pieces instead: the family resolver
//!   (`release_candidate_development_families`) on the run records of a real
//!   development cycle and of a real staged candidate, the production core
//!   reducer (`LearnerStateV3::apply_learner_asset`) for the state change, and
//!   the real-path refusal of an unapproved candidate and of a run without a
//!   trusted asset receipt. A state carrying an applied asset is therefore
//!   persisted with `overwrite_state`, and the release record it names (read
//!   back by selection) is written directly.
//! - Every registered-runner development cycle passes (the registered target
//!   and the independent oracle agree), so `record_cycle` never produces a
//!   failure record. States with failure records are built with the
//!   production reducer `apply_cycle_to_failure_records` and persisted with
//!   `overwrite_state`.

#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CapabilityLevel, CompileParts, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile,
    ResolvedBundleV3 as ResolvedBundle, SkillPatch, SkillSnapshot, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::curriculum::{
    AppliedLearnerAssetRef, CoverageBucketState, CurriculumControlProfileV1, FailureCycleOutcome,
    FailureRecordStatus, LEGACY_V2_UNTYPED_NOTE, LearnerStateV2, LearnerStateV3, PlateauSignalV1,
    ProbeJobV1, ProbeTerminal, RegisteredProperty, StructuredTestProposalV1,
    apply_cycle_to_failure_records, legacy_failure_record_id,
};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::curriculum::{
    AssetInfluenceKindV1, ConsumedAssetInfluenceV1, CurriculumSourceArtifactV1,
    CurriculumSourceKindV1, CurriculumTaskCandidateV1, DevelopmentCycleReceiptV1,
    PersistentCurriculumCoordinator, SelectionPriorityCategory, TaskSelectionDecisionV2,
    curriculum_state_storage_id, release_candidate_development_families, verified_probe_job_view,
};
use evo_engine::curriculum_profiles::{
    CLAMP_TARGET_ID, ClampOutput, OfflineProfileCheck, RegisteredPureFunctionProfileV1,
    ValidityReportV1, check_target_outputs, registered_offline_check,
    registered_profile_source_bodies, runner_verified_report, runtime_validity_report,
};
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
    registered_runner_digest,
};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::optimization::{
    DevRunner, DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal,
};
use evo_engine::release_store::{
    CurrentReleaseCandidateRecord as ReleaseCandidateRecord,
    CurrentStageBundleRequest as StageBundleRequest, PersistentRelease, PersistentReleaseState,
    PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence, TrustedHostExecutionReceipt,
    TypedSourceRef,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";
const PROFILE_ID: &str = "pure_function_test_proposal.v1";
const STATE_ID: &str = "learner-state";
const ENVELOPE: &str = "rsia.curriculum_artifact_envelope.v1";
const STATE_KIND: &str = "learner_state_v2";
const VALIDITY_KIND: &str = "curriculum_validity_report_v1";
/// The development families of the registered control, one task each.
const FAMILIES: [&str; 5] = ["family-a", "family-b", "family-c", "family-d", "family-e"];
const SEED: &str = "fixed-random-seed";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn storage_id(record_kind: &str, id: &str) -> String {
    format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
}

// ---------------------------------------------------------------------------
// Fixtures (copied from tests/curriculum_cleanup_v42.rs, tests/release_store.rs)
// ---------------------------------------------------------------------------

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    }
}

fn run_body(id: &str) -> String {
    format!("trusted-run-{id}")
}

fn authority(id: &str, family: &str) -> StoredTraceAuthority {
    let body = run_body(id);
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
    FAMILIES
        .iter()
        .map(|family| {
            DevelopmentTaskSpecV1::registered(
                format!("task-{}", family.trim_start_matches("family-")),
                *family,
                RegisteredTargetInputV1::ClampI64 {
                    value: -2,
                    min: -1,
                    max: 1,
                },
            )
            .unwrap()
        })
        .collect()
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
    host: Context,
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
            host,
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

fn registered_v2_state(fixture: &Fixture, buckets: &[&str]) -> LearnerStateV2 {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    LearnerStateV2 {
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
        coverage_buckets: buckets
            .iter()
            .map(|bucket| CoverageBucketState {
                bucket_id: (*bucket).into(),
                task_space_source_id: "task-space-source".into(),
                observed_independent_clusters: 0,
                required_independent_clusters: 1,
            })
            .collect(),
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

/// Registers the typed sources, the offline profile (with the given fixed
/// random seed commitment) and the initial learner state with one coverage
/// bucket per listed family.
async fn curriculum(
    fixture: &Fixture,
    seed_commitment: Option<String>,
    buckets: &[&str],
) -> PersistentCurriculumCoordinator {
    let coordinator =
        PersistentCurriculumCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
            .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets": FAMILIES}),
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
    coordinator
        .register_profile_and_state(profile, registered_v2_state(fixture, buckets))
        .await
        .unwrap();
    coordinator
}

/// One real registered development cycle reduced through `record_cycle`;
/// returns its (request, observed) stage-fact ids.
async fn real_cycle(
    fixture: &Fixture,
    coordinator: &PersistentCurriculumCoordinator,
    n: u32,
) -> (String, String) {
    let request = build_request(
        &fixture.control,
        &format!("dev-request-{n}"),
        &format!("episode-{n}"),
    );
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    coordinator
        .record_cycle(DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: format!("cycle-{n}"),
            state_id: STATE_ID.into(),
            development_request_fact_id: request_fact_id.clone(),
            development_observed_fact_id: observed_fact_id.clone(),
        })
        .await
        .unwrap();
    (request_fact_id, observed_fact_id)
}

async fn probe_job(coordinator: &PersistentCurriculumCoordinator) -> ProbeJobV1 {
    coordinator
        .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap()
}

/// A stored registered-boundary proposal (`BelowMapsToMin` on [-1, 1]).
/// `value = -2` passes the offline check; `value = 0` is quarantined (the
/// registered precondition fails).
async fn stored_proposal(
    coordinator: &PersistentCurriculumCoordinator,
    job: &ProbeJobV1,
    id: &str,
    family: &str,
    value: i64,
) -> StructuredTestProposalV1 {
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: id.into(),
        probe_job_id: job.id.clone(),
        target_id: CLAMP_TARGET_ID.into(),
        property: RegisteredProperty::BelowMapsToMin,
        value,
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
    proposal
}

/// The offline conclusion of a passing proposal, from the observed target
/// output (clamp(-2, -1, 1) = -1).
fn passed_offline(proposal: &StructuredTestProposalV1) -> OfflineProfileCheck {
    let offline = check_target_outputs(
        &RegisteredPureFunctionProfileV1::clamp_i64(),
        proposal,
        ClampOutput { clamped: -1 },
        ClampOutput { clamped: -1 },
    )
    .unwrap();
    assert!(matches!(offline, OfflineProfileCheck::Passed { .. }));
    offline
}

/// Records a runner-verified report for a passing proposal bound to `facts`.
async fn verified_report(
    coordinator: &PersistentCurriculumCoordinator,
    proposal: &StructuredTestProposalV1,
    facts: &(String, String),
) -> String {
    let report = runner_verified_report(
        &RegisteredPureFunctionProfileV1::clamp_i64(),
        proposal,
        &passed_offline(proposal),
        &facts.0,
        &facts.1,
    )
    .unwrap();
    coordinator
        .record_validity_report(STATE_ID, report)
        .await
        .unwrap()
}

/// One eligible candidate per family: a stored passing proposal and its
/// runner-verified report.
async fn eligible_candidates(
    coordinator: &PersistentCurriculumCoordinator,
    facts: &(String, String),
    specs: &[(&str, &str, u32, Option<&str>)],
) -> Vec<CurriculumTaskCandidateV1> {
    let job = probe_job(coordinator).await;
    let mut candidates = Vec::new();
    for (task_id, family, seq, bucket) in specs {
        let proposal = stored_proposal(
            coordinator,
            &job,
            &format!("proposal-{task_id}"),
            family,
            -2,
        )
        .await;
        let report_id = verified_report(coordinator, &proposal, facts).await;
        candidates.push(CurriculumTaskCandidateV1 {
            id: (*task_id).into(),
            parent_family: (*family).into(),
            coverage_bucket_id: bucket.map(str::to_string),
            stable_choice_seq: *seq,
            proposal_id: proposal.id,
            validity_report_id: report_id,
        });
    }
    candidates
}

async fn raw_state(fixture: &Fixture) -> Value {
    let mut session = fixture.store.session().await.unwrap();
    let body: Value = session
        .get(
            &fixture.admin,
            "artifact",
            &curriculum_state_storage_id(STATE_ID).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    body
}

async fn load_state(fixture: &Fixture) -> LearnerStateV3 {
    serde_json::from_value(raw_state(fixture).await["payload"].clone()).unwrap()
}

/// Writes a learner state at the production storage address in the exact
/// envelope shape the production code writes (the same hand-rolled record
/// pattern `curriculum_cleanup_v42.rs` uses for records the public API cannot
/// produce; see the module note for why).
async fn overwrite_state<T: Serialize>(store: &Store, admin: &Context, payload: &T) {
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
            &storage_id(STATE_KIND, STATE_ID),
            "admin",
            &ArtifactEnvelope {
                schema_version: ENVELOPE,
                id: storage_id(STATE_KIND, STATE_ID),
                record_kind: STATE_KIND,
                payload,
            },
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// Plants failure records produced by the production reducer for one cycle
/// the registered runner cannot produce (see the module note).
async fn plant_failures(fixture: &Fixture, cycle_id: &str, failing: &[&str]) {
    let mut state = load_state(fixture).await;
    let outcomes: Vec<FailureCycleOutcome> = failing
        .iter()
        .map(|family| FailureCycleOutcome {
            cluster_id: (*family).into(),
            task_id: format!("task-{}", family.trim_start_matches("family-")),
            passed: false,
            paired_gain_micros: -1_000_000,
            candidate_execution_id: format!("devexec-planted-{cycle_id}-{family}"),
            grader_receipt_digest: d(&format!("planted-grader-{cycle_id}-{family}")),
        })
        .collect();
    let (environment, grader, skill) = (
        state.environment_digest.clone(),
        state.grader_digest.clone(),
        state.skill_snapshot_digest.clone(),
    );
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        cycle_id,
        &environment,
        &grader,
        &skill,
        &outcomes,
    )
    .unwrap();
    state.failure_clusters = state.active_absolute_failure_clusters();
    state.validate().unwrap();
    overwrite_state(&fixture.store, &fixture.admin, &state).await;
}

fn release_system() -> SystemSnapshot {
    SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: "p1".into(),
        host_id: "reference-host".into(),
        host_version: "1.0.0".into(),
        model_id: "model-v1".into(),
        tools: vec!["tool-a".into()],
        mandatory_context_digest: d("mandatory"),
    }
}

fn release_bundle(content: &str) -> ResolvedBundle {
    let profile = Profile {
        id: "p1".into(),
        evolution_enabled: true,
        parent_digest: d("parent"),
        baseline_digest: d("baseline"),
    };
    let parent = SkillSnapshot {
        content: content.into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    trusted_compiler_inputs::bundle_content_current(
        CompileParts {
            profile: &profile,
            parent: &parent,
            baseline: &SkillSnapshot::empty(),
            parent_strategy: &Strategy::default(),
            baseline_strategy: &Strategy::default(),
            skill_patch: &SkillPatch::default(),
            improver_patch: &ImproverPatch::default(),
            caps: &HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: &BTreeSet::new(),
        },
        NAMESPACE,
        &release_system().digest().unwrap(),
    )
    .unwrap()
}

fn run_source(id: &str) -> TypedSourceRef {
    TypedSourceRef {
        kind: "run".into(),
        id: id.into(),
        content_digest: hash(run_body(id).as_bytes()),
    }
}

/// Stages a real release candidate (Worker, real bundle, real run sources).
async fn stage_candidate(
    fixture: &Fixture,
    candidate_id: &str,
    runs: &[&str],
) -> ReleaseCandidateRecord {
    let bundle = release_bundle("retest skill");
    trusted_compiler_inputs::install_bundle_current_sources(&fixture.store, &bundle)
        .await
        .unwrap();
    let proposer = Context::new(NAMESPACE, "proposer", Role::Worker).unwrap();
    ReleaseStore::stage_current_bundle(
        &proposer,
        &fixture.store,
        StageBundleRequest {
            candidate_id: candidate_id.into(),
            bundle,
            environment_digest: release_system().digest().unwrap(),
            proposer_actor: "proposer".into(),
            sources: runs.iter().map(|id| run_source(id)).collect(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap()
}

/// Writes the release record a retest mark names. A release can only be
/// approved and activated through the formal operator channel (see the module
/// note), so the record is written directly; selection reads back only its
/// identity and state. The formal fields carry values that must never appear
/// in a selection record.
async fn put_release(
    fixture: &Fixture,
    release_id: &str,
    candidate: &ReleaseCandidateRecord,
    state: PersistentReleaseState,
) {
    let release = PersistentRelease {
        id: release_id.into(),
        schema_version: "rsia.persistent_release.v1".into(),
        candidate_id: candidate.id.clone(),
        bundle_digest: candidate.bundle_digest.clone(),
        environment_digest: candidate.environment_digest.clone(),
        profile_id: candidate.profile_id.clone(),
        parent_digest: candidate.parent_digest.clone(),
        baseline_bundle_digest: d("baseline-bundle"),
        report_id: "formal-report-must-not-leak".into(),
        report_digest: d("formal-report-digest-must-not-leak"),
        proposer_actor: "proposer".into(),
        evaluator_actor: "formal-evaluator-must-not-leak".into(),
        approved_by: "approver".into(),
        expected_parent_release_id: None,
        expected_pointer_epoch: 0,
        state,
    };
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, "release", release_id, "admin", &release)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn resolve_families(
    fixture: &Fixture,
    context: &Context,
    sources: &[TypedSourceRef],
) -> Result<Vec<String>, Error> {
    let mut session = fixture.store.session().await.unwrap();
    let families = release_candidate_development_families(context, &mut session, sources).await;
    session.commit().await.unwrap();
    families
}

/// Applies one approved asset to the stored state with the production core
/// reducer, bound to the families the production resolver derives from the
/// staged candidate's sources (the trusted application itself cannot be
/// assembled; see the module note).
async fn plant_applied_asset(
    fixture: &Fixture,
    release_id: &str,
    candidate: &ReleaseCandidateRecord,
) {
    let families = resolve_families(fixture, &fixture.admin, &candidate.sources)
        .await
        .unwrap();
    let mut state = load_state(fixture).await;
    assert!(
        state
            .apply_learner_asset(
                AppliedLearnerAssetRef {
                    release_id: release_id.into(),
                    bundle_digest: candidate.bundle_digest.clone(),
                    run_application_id: "run-snapshot-host-run-planted".into(),
                    host_execution_receipt_id: "host-execution-planted".into(),
                },
                &families,
            )
            .unwrap()
    );
    overwrite_state(&fixture.store, &fixture.admin, &state).await;
}

fn assert_conflict_naming(result: Result<impl std::fmt::Debug, Error>, needles: &[&str]) {
    match result {
        Err(Error::Conflict(message)) => {
            for needle in needles {
                assert!(message.contains(needle), "{needle} missing in: {message}");
            }
        }
        other => panic!("expected a Conflict naming {needles:?}, got {other:?}"),
    }
}

fn find_record<'a>(state: &'a Value, family: &str) -> &'a Value {
    state["payload"]["failure_records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["cluster_id"] == family)
        .unwrap_or_else(|| panic!("no failure record of {family}"))
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn record_cycle_writes_the_v3_wire_and_clean_passes_create_no_records() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    real_cycle(&fixture, &coordinator, 1).await;
    let state = load_state(&fixture).await;
    // The registered pure-function pairs are verified passes, so no failure
    // record appears and the v3 cluster list stays empty.
    assert_eq!(state.schema_version, "rsia.learner_state.v3");
    assert!(state.failure_records.is_empty());
    assert!(state.failure_clusters.is_empty());
    assert_eq!(state.completed_cycles.len(), 1);
    assert_eq!(state.completed_cycles[0].cluster_ids.len(), FAMILIES.len());
    // The stored payload really is the v3 wire at the fixed v2 address.
    let body = raw_state(&fixture).await;
    assert_eq!(body["record_kind"], json!(STATE_KIND));
    assert_eq!(
        body["payload"]["schema_version"],
        json!("rsia.learner_state.v3")
    );
    assert_eq!(body["payload"]["failure_records"], json!([]));
    assert_eq!(body["payload"]["retest_priority_marks"], json!([]));
}

#[tokio::test]
async fn runner_verified_eligibility_binds_a_passed_offline_conclusion_and_the_proposal_family() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let job = probe_job(&coordinator).await;
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();

    // (a) The family of the proposal is covered by the verified observation
    // and its offline conclusion passed: the report is recorded, bound to the
    // passed conclusion and to both stage facts.
    let covered = stored_proposal(&coordinator, &job, "proposal-covered", "family-a", -2).await;
    let offline = passed_offline(&covered);
    let report =
        runner_verified_report(&registered, &covered, &offline, &facts.0, &facts.1).unwrap();
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["status"], json!("runner_verified"));
    assert_eq!(
        wire["offline_output_digest"],
        json!(fingerprint(&offline).unwrap())
    );
    // The server recomputes the same conclusion from the registered target.
    assert_eq!(
        fingerprint(&registered_offline_check(&registered, &covered).unwrap()).unwrap(),
        fingerprint(&offline).unwrap()
    );
    let report_id = coordinator
        .record_validity_report(STATE_ID, report)
        .await
        .unwrap();
    let mut session = fixture.store.session().await.unwrap();
    for fact in [&facts.0, &facts.1] {
        let dependents = session
            .dependents(&fixture.admin, "artifact", fact)
            .await
            .unwrap();
        assert!(
            dependents.iter().any(
                |(kind, id)| kind == "artifact" && id == &storage_id(VALIDITY_KIND, &report_id)
            ),
            "the runner-verified report must depend on {fact}"
        );
    }
    session.commit().await.unwrap();

    // (b) Another family's proposal cannot borrow the observation.
    let other = stored_proposal(&coordinator, &job, "proposal-other-family", "family-z", -2).await;
    let other_report = runner_verified_report(
        &registered,
        &other,
        &passed_offline(&other),
        &facts.0,
        &facts.1,
    )
    .unwrap();
    assert_conflict_naming(
        coordinator
            .record_validity_report(STATE_ID, other_report)
            .await,
        &["runner-verified", "family"],
    );

    // (c) Quarantine wins. The constructor refuses a quarantined conclusion,
    // and a wire forged without it is refused after the quarantine was
    // recorded, and also without any recorded quarantine report (the
    // registered offline check is recomputed).
    let quarantined =
        stored_proposal(&coordinator, &job, "proposal-quarantined", "family-a", 0).await;
    let quarantine = check_target_outputs(
        &registered,
        &quarantined,
        ClampOutput { clamped: 0 },
        ClampOutput { clamped: 0 },
    )
    .unwrap();
    assert!(matches!(
        quarantine,
        OfflineProfileCheck::Quarantined { .. }
    ));
    assert_conflict_naming(
        runner_verified_report(&registered, &quarantined, &quarantine, &facts.0, &facts.1),
        &["quarantin"],
    );
    let forged = |proposal: &StructuredTestProposalV1| -> ValidityReportV1 {
        serde_json::from_value(json!({
            "status": "runner_verified",
            "profile_id": registered.profile_id,
            "proposal_id": proposal.id,
            "proposal_digest": fingerprint(proposal).unwrap(),
            "target_digest": registered.target_digest,
            "oracle_digest": registered.oracle_digest,
            "runner_digest": registered.runner_digest,
            "development_request_fact_id": facts.0,
            "development_observed_fact_id": facts.1,
            "offline_output_digest": fingerprint(&offline).unwrap(),
        }))
        .unwrap()
    };
    coordinator
        .record_validity_report(
            STATE_ID,
            runtime_validity_report(&registered, &quarantined, &quarantine).unwrap(),
        )
        .await
        .unwrap();
    assert_conflict_naming(
        coordinator
            .record_validity_report(STATE_ID, forged(&quarantined))
            .await,
        &["runner-verified", "quarantin"],
    );
    let unrecorded = stored_proposal(
        &coordinator,
        &job,
        "proposal-quarantined-unrecorded",
        "family-a",
        0,
    )
    .await;
    assert_conflict_naming(
        coordinator
            .record_validity_report(STATE_ID, forged(&unrecorded))
            .await,
        &["runner-verified", "quarantin"],
    );

    // (d) A wire without `offline_output_digest` still decodes and fails
    // closed; a wrong digest fails closed too.
    let mut missing = wire.clone();
    assert!(
        missing
            .as_object_mut()
            .unwrap()
            .remove("offline_output_digest")
            .is_some()
    );
    let missing: ValidityReportV1 = serde_json::from_value(missing).unwrap();
    assert_conflict_naming(
        coordinator.record_validity_report(STATE_ID, missing).await,
        &["offline_output_digest"],
    );
    let mut wrong = wire.clone();
    wrong["offline_output_digest"] = json!(d("not-the-offline-conclusion"));
    let wrong: ValidityReportV1 = serde_json::from_value(wrong).unwrap();
    assert_conflict_naming(
        coordinator.record_validity_report(STATE_ID, wrong).await,
        &["offline_output_digest"],
    );

    // The verified report is selected through the public API.
    let decision = coordinator
        .select_next_task(
            STATE_ID,
            &[CurriculumTaskCandidateV1 {
                id: "task-covered".into(),
                parent_family: "family-a".into(),
                coverage_bucket_id: None,
                stable_choice_seq: 1,
                proposal_id: covered.id.clone(),
                validity_report_id: report_id,
            }],
        )
        .await
        .unwrap();
    let decision = serde_json::to_value(decision).unwrap();
    assert_eq!(
        decision["schema_version"],
        json!("rsia.curriculum_selection.v2")
    );
    assert_eq!(decision["chosen_task_id"], json!("task-covered"));
}

#[tokio::test]
async fn selection_rechecks_the_report_state_the_proposal_family_and_quarantine() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates =
        eligible_candidates(&coordinator, &facts, &[("task-a", "family-a", 1, None)]).await;
    let candidate = candidates[0].clone();
    // A candidate of another family than its proposal is refused.
    assert_conflict_naming(
        coordinator
            .select_next_task(
                STATE_ID,
                &[CurriculumTaskCandidateV1 {
                    parent_family: "family-b".into(),
                    ..candidate.clone()
                }],
            )
            .await,
        &["family"],
    );
    // A report stored under an id this state never recorded (the shape a
    // report recorded for another learner state has) is refused.
    let mut session = fixture.store.session().await.unwrap();
    let original: Value = session
        .get(
            &fixture.admin,
            "artifact",
            &storage_id(VALIDITY_KIND, &candidate.validity_report_id),
        )
        .await
        .unwrap()
        .unwrap();
    let forged_id = "validity-recorded-for-another-state";
    session
        .put(
            &fixture.admin,
            "artifact",
            &storage_id(VALIDITY_KIND, forged_id),
            "admin",
            &json!({
                "schema_version": ENVELOPE,
                "id": storage_id(VALIDITY_KIND, forged_id),
                "record_kind": VALIDITY_KIND,
                "payload": original["payload"],
            }),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_conflict_naming(
        coordinator
            .select_next_task(
                STATE_ID,
                &[CurriculumTaskCandidateV1 {
                    validity_report_id: forged_id.into(),
                    ..candidate.clone()
                }],
            )
            .await,
        &["not recorded for this learner state"],
    );
    // Duplicate candidate ids are invalid.
    assert!(matches!(
        coordinator
            .select_next_task(STATE_ID, &[candidate.clone(), candidate.clone()])
            .await,
        Err(Error::Invalid(_))
    ));
    // A quarantine recorded after the runner-verified report wins at
    // selection time as well.
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let mut session = fixture.store.session().await.unwrap();
    let proposal_body: Value = session
        .get(
            &fixture.admin,
            "artifact",
            &storage_id("structured_test_proposal_v1", &candidate.proposal_id),
        )
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    let proposal: StructuredTestProposalV1 =
        serde_json::from_value(proposal_body["payload"].clone()).unwrap();
    let disagreement = check_target_outputs(
        &registered,
        &proposal,
        ClampOutput { clamped: 99 },
        ClampOutput { clamped: 99 },
    )
    .unwrap();
    coordinator
        .record_validity_report(
            STATE_ID,
            runtime_validity_report(&registered, &proposal, &disagreement).unwrap(),
        )
        .await
        .unwrap();
    assert_conflict_naming(
        coordinator
            .select_next_task(STATE_ID, std::slice::from_ref(&candidate))
            .await,
        &["no runner-verified development task"],
    );
}

#[tokio::test]
async fn release_family_binding_reads_the_development_runs_of_a_real_cycle_and_candidate() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    // run-a and run-b are the source runs of the real development cycle: its
    // stage facts depend on them.
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let admin = fixture.admin.clone();
    assert_eq!(
        resolve_families(&fixture, &admin, &[run_source("run-b")])
            .await
            .unwrap(),
        vec!["family-b".to_string()]
    );
    // The union over several runs, deduplicated.
    assert_eq!(
        resolve_families(
            &fixture,
            &admin,
            &[
                run_source("run-b"),
                run_source("run-a"),
                run_source("run-b")
            ]
        )
        .await
        .unwrap(),
        vec!["family-a".to_string(), "family-b".to_string()]
    );
    // A real staged release candidate binds through its own canonical sources.
    let candidate = stage_candidate(&fixture, "candidate-ab", &["run-a", "run-b"]).await;
    assert_eq!(
        resolve_families(&fixture, &admin, &candidate.sources)
            .await
            .unwrap(),
        vec!["family-a".to_string(), "family-b".to_string()]
    );
    // What cannot bind fails closed, naming the release and the family: a
    // stage fact of the cycle (the old intersection key), an unknown run, a
    // run that is not development evidence, a record of an unknown shape, a
    // changed run, and an empty source list.
    let mut session = fixture.store.session().await.unwrap();
    let mut inspection = authority("run-inspection", "family-x");
    inspection.record.purpose = Purpose::Inspection;
    inspection.trace.purpose = Purpose::Inspection;
    session
        .put(&admin, "run", "run-inspection", "admin", &inspection)
        .await
        .unwrap();
    session
        .put(
            &admin,
            "run",
            "run-legacy",
            "admin",
            &json!({"schema_version": "rsia.legacy_run.v0", "parent_family": "family-x"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut changed = run_source("run-b");
    changed.content_digest = d("changed-run-content");
    for sources in [
        vec![TypedSourceRef {
            kind: "artifact".into(),
            id: facts.1.clone(),
            content_digest: d("stage-fact"),
        }],
        vec![run_source("run-missing")],
        vec![TypedSourceRef {
            content_digest: hash(run_body("run-inspection").as_bytes()),
            ..run_source("run-inspection")
        }],
        vec![run_source("run-legacy")],
        vec![run_source("run-a"), changed],
        vec![],
    ] {
        assert_conflict_naming(
            resolve_families(&fixture, &admin, &sources).await,
            &["release", "family"],
        );
    }
    // Only an Admin reads the binding.
    let worker = Context::new(NAMESPACE, "worker", Role::Worker).unwrap();
    assert!(matches!(
        resolve_families(&fixture, &worker, &[run_source("run-b")]).await,
        Err(Error::Forbidden)
    ));
    // A revoked run is refused like every other revoked source.
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "run-b",
            "admin",
            &json!({"id": "run-b"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        resolve_families(&fixture, &admin, &[run_source("run-b")]).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn an_applied_asset_grants_each_bound_family_one_attributable_retest_priority() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, Some(d(SEED)), &["family-b"]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates = eligible_candidates(
        &coordinator,
        &facts,
        &[
            ("task-a", "family-a", 1, None),
            ("task-b", "family-b", 7, None),
        ],
    )
    .await;
    // family-b holds one active failure; family-a holds none.
    plant_failures(&fixture, "planted-cycle-1", &["family-b"]).await;
    // The asset is bound to both families through the staged candidate's
    // development run sources.
    let candidate = stage_candidate(&fixture, "candidate-ab", &["run-a", "run-b"]).await;
    put_release(
        &fixture,
        "release-ab",
        &candidate,
        PersistentReleaseState::Active,
    )
    .await;
    plant_applied_asset(&fixture, "release-ab", &candidate).await;
    let state = raw_state(&fixture).await;
    assert_eq!(
        state["payload"]["schema_version"],
        json!("rsia.learner_state.v3")
    );
    let marks = state["payload"]["retest_priority_marks"]
        .as_array()
        .unwrap();
    for family in ["family-a", "family-b"] {
        assert!(
            marks.contains(
                &json!({"family": family, "release_id": "release-ab", "consumed": false})
            ),
            "{family} must hold an unconsumed retest mark: {marks:?}"
        );
    }
    assert_eq!(
        find_record(&state, "family-b")["status"],
        json!("retest_due")
    );
    let failure_record_id = find_record(&state, "family-b")["record_id"].clone();

    // 1st: both families rank retest_due; the lower sequence (the family
    // without failure records, treated at retest_due level once) wins.
    let first = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(first.chosen_task_id, "task-a");
    assert_eq!(
        first.priority_category,
        SelectionPriorityCategory::RetestDue
    );
    assert_eq!(
        first.consumed_asset,
        Some(ConsumedAssetInfluenceV1 {
            release_id: "release-ab".into(),
            influence: AssetInfluenceKindV1::RetestAfterApply,
        })
    );
    // 2nd: family-b still holds its own mark and outranks the lower sequence;
    // the frozen arm, blind to the asset, picks the lowest sequence: the
    // visible, attributable difference V109.b requires.
    let second = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(second.chosen_task_id, "task-b");
    assert_eq!(
        second.priority_category,
        SelectionPriorityCategory::RetestDue
    );
    assert_eq!(second.frozen_state_choice_task_id, "task-a");
    assert_ne!(second.chosen_task_id, second.frozen_state_choice_task_id);
    let wire = serde_json::to_value(second).unwrap();
    assert_eq!(wire["priority_category"], json!("retest_due"));
    assert_eq!(
        wire["consumed_asset"],
        json!({"release_id": "release-ab", "influence": "retest_after_apply"})
    );
    assert_eq!(
        wire["consumed_failure_record_ids"],
        json!([failure_record_id])
    );
    // Both marks are consumed; the retest-due record is a current gap again.
    let state = raw_state(&fixture).await;
    let marks = state["payload"]["retest_priority_marks"]
        .as_array()
        .unwrap();
    assert!(
        marks.iter().all(|mark| mark["consumed"] == json!(true)),
        "{marks:?}"
    );
    let reopened = find_record(&state, "family-b");
    assert_eq!(reopened["status"], json!("active"));
    assert_eq!(
        reopened["transition_note"],
        json!("retest_selected:release-ab")
    );
    // 3rd: the one-shot priority is spent; family-b is an ordinary active
    // failure gap of a registered bucket now.
    let third = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(third.chosen_task_id, "task-b");
    assert_eq!(
        third.priority_category,
        SelectionPriorityCategory::ActiveFailureGap
    );
    assert_eq!(third.consumed_asset, None);
    let wire = serde_json::to_value(third).unwrap();
    assert_ne!(wire["priority_category"], json!("retest_due"));
    assert_eq!(wire["consumed_asset"], Value::Null);
}

#[tokio::test]
async fn three_arms_differ_over_five_eligible_candidates() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, Some(d(SEED)), &["family-d"]).await;
    // The real cycle covers the registered bucket family-d.
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates = eligible_candidates(
        &coordinator,
        &facts,
        &[
            ("task-a", "family-a", 1, None),
            ("task-b", "family-b", 2, None),
            ("task-c", "family-c", 3, None),
            ("task-d", "family-d", 4, Some("family-d")),
            ("task-e", "family-e", 5, None),
        ],
    )
    .await;
    let candidate = stage_candidate(&fixture, "candidate-b", &["run-b"]).await;
    put_release(
        &fixture,
        "release-b",
        &candidate,
        PersistentReleaseState::Active,
    )
    .await;
    plant_applied_asset(&fixture, "release-b", &candidate).await;
    let decision = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    // Adaptive: the asset's family (retest_due). Frozen: the as-registered
    // state still sees the uncovered bucket of task-d (coverage_gap). Fixed
    // random: task-c, the smallest sha256("<commitment>:<candidate id>") of
    // the eligible set (golden computed independently of this crate).
    assert_eq!(decision.chosen_task_id, "task-b");
    assert_eq!(
        decision.priority_category,
        SelectionPriorityCategory::RetestDue
    );
    assert_eq!(decision.frozen_state_choice_task_id, "task-d");
    assert_eq!(
        decision.fixed_random_choice_task_id.as_deref(),
        Some("task-c")
    );
    let arms: BTreeSet<&str> = [
        decision.chosen_task_id.as_str(),
        decision.frozen_state_choice_task_id.as_str(),
        decision.fixed_random_choice_task_id.as_deref().unwrap(),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        arms.len(),
        3,
        "the three arms must be able to differ: {arms:?}"
    );
    assert_eq!(
        decision
            .consumed_asset
            .as_ref()
            .map(|asset| asset.release_id.as_str()),
        Some("release-b")
    );
}

#[tokio::test]
async fn fixed_random_arm_is_reproducible_across_input_orders_and_absent_without_a_commitment() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, Some(d(SEED)), &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let shared =
        eligible_candidates(&coordinator, &facts, &[("task-0", "family-a", 0, None)]).await;
    let candidates: Vec<CurriculumTaskCandidateV1> = (0..4)
        .map(|index| CurriculumTaskCandidateV1 {
            id: format!("task-{index}"),
            stable_choice_seq: index,
            ..shared[0].clone()
        })
        .collect();
    let in_order = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    let permuted: Vec<CurriculumTaskCandidateV1> = [2, 0, 3, 1]
        .into_iter()
        .map(|index| candidates[index].clone())
        .collect();
    let shuffled = coordinator
        .select_next_task(STATE_ID, &permuted)
        .await
        .unwrap();
    // Same committed seed, same eligible set, another input order: the same
    // fixed random pick (golden: smallest sha256("<commitment>:<id>")), the
    // same frozen pick and one candidate-set digest.
    assert_eq!(
        in_order.fixed_random_choice_task_id.as_deref(),
        Some("task-3")
    );
    assert_eq!(
        shuffled.fixed_random_choice_task_id,
        in_order.fixed_random_choice_task_id
    );
    assert_eq!(in_order.frozen_state_choice_task_id, "task-0");
    assert_eq!(
        shuffled.frozen_state_choice_task_id,
        in_order.frozen_state_choice_task_id
    );
    assert_eq!(shuffled.candidate_set_digest, in_order.candidate_set_digest);
    assert_eq!(shuffled.id, in_order.id);
    // I6: a profile without a seed commitment has no fixed random arm.
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates =
        eligible_candidates(&coordinator, &facts, &[("task-a", "family-a", 1, None)]).await;
    let decision: TaskSelectionDecisionV2 = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(decision.fixed_random_choice_task_id, None);
    let wire = serde_json::to_value(decision).unwrap();
    assert_eq!(wire["fixed_random_choice_task_id"], Value::Null);
    assert_eq!(wire["fixed_random_seed_commitment"], Value::Null);
}

fn surface() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "reference-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: d("surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("runner".into()),
            reason: "reference fixture mapping".into(),
        }],
    }
}

#[tokio::test]
async fn an_unapproved_candidate_or_a_missing_trusted_receipt_has_no_influence() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, Some(d(SEED)), &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates = eligible_candidates(
        &coordinator,
        &facts,
        &[
            ("task-a", "family-a", 1, None),
            ("task-b", "family-b", 7, None),
        ],
    )
    .await;
    // A staged candidate that was never approved: the profile has no active
    // release, so the Host's run of that profile is a zero-skill baseline run
    // and its applied request carries no trusted applied receipt.
    stage_candidate(&fixture, "candidate-unapproved", &["run-b"]).await;
    trusted_compiler_inputs::register_fixture_host_surface(
        &fixture.admin,
        &fixture.store,
        "surface-1",
        surface(),
        vec!["model".into()],
    )
    .await
    .unwrap();
    let snapshot = ReleaseStore::prepare_run(
        &fixture.host,
        &fixture.store,
        PrepareRunRequest {
            run_id: "host-run-1".into(),
            profile_id: "p1".into(),
            system_snapshot: release_system(),
            host_surface_id: "surface-1".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d("task-input"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::Attached,
        },
    )
    .await
    .unwrap();
    assert!(snapshot.release_id.is_none());
    ReleaseStore::store_host_execution_receipt(
        &fixture.host,
        &fixture.store,
        TrustedHostExecutionReceipt {
            id: "host-execution-1".into(),
            schema_version: "rsia.host_execution_receipt.v1".into(),
            run_id: "host-run-1".into(),
            request_digest: snapshot.request_digest.clone(),
            environment_digest: snapshot.environment_digest.clone(),
            host_surface_digest: snapshot.host_surface_digest.clone(),
            output_digest: d("host-output"),
            used_ids: vec![],
        },
    )
    .await
    .unwrap();
    let application = ReleaseStore::record_applied_request(
        &fixture.host,
        &fixture.store,
        "host-run-1",
        TrustedHostExecutionEvidence {
            actual_request_material: snapshot.request_material.clone(),
            environment_digest: snapshot.environment_digest.clone(),
            host_surface_digest: snapshot.host_surface_digest.clone(),
            host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
            offered: vec![],
            attached: vec![],
            used: vec![],
            execution_receipt_id: None,
            truncated: false,
        },
    )
    .await
    .unwrap();
    assert!(application.receipt.is_none());
    let before = raw_state(&fixture).await;
    // No approved release, no trusted applied receipt: refused, nothing
    // written.
    assert_conflict_naming(
        coordinator
            .record_applied_learning_asset(
                STATE_ID,
                "host-run-1",
                &application.id,
                "host-execution-1",
            )
            .await,
        &["zero-skill"],
    );
    // A receipt that does not exist is refused as well.
    assert!(
        coordinator
            .record_applied_learning_asset(
                STATE_ID,
                "host-run-1",
                "host-application-missing",
                "host-execution-1"
            )
            .await
            .is_err()
    );
    assert_eq!(raw_state(&fixture).await, before);
    let state = load_state(&fixture).await;
    assert!(state.applied_assets.is_empty());
    assert!(state.retest_priority_marks.is_empty());
    // The selection is not influenced: no retest_due, no consumed asset, the
    // adaptive arm equals the frozen arm.
    let decision = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(decision.priority_category, SelectionPriorityCategory::Other);
    assert_eq!(decision.consumed_asset, None);
    assert_eq!(
        decision.chosen_task_id,
        decision.frozen_state_choice_task_id
    );
}

#[tokio::test]
async fn asset_influence_ends_with_its_release_and_an_unreadable_release_is_refused() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates = eligible_candidates(
        &coordinator,
        &facts,
        &[
            ("task-a", "family-a", 1, None),
            ("task-b", "family-b", 7, None),
        ],
    )
    .await;
    plant_failures(&fixture, "planted-cycle-1", &["family-b"]).await;
    let candidate = stage_candidate(&fixture, "candidate-b", &["run-b"]).await;
    plant_applied_asset(&fixture, "release-b", &candidate).await;
    // A mark whose release record is unreadable fails closed.
    assert_conflict_naming(
        coordinator.select_next_task(STATE_ID, &candidates).await,
        &["release-b", "unreadable"],
    );
    // A rolled-back or revoked release has no influence any more (V109.c:
    // 撤销资产后影响失效): no retest_due, no consumed asset; its retest-due
    // record is a current gap again and its mark stays unconsumed.
    for void in [
        PersistentReleaseState::RolledBack,
        PersistentReleaseState::Revoked,
    ] {
        put_release(&fixture, "release-b", &candidate, void).await;
        let decision = coordinator
            .select_next_task(STATE_ID, &candidates)
            .await
            .unwrap();
        assert_eq!(decision.chosen_task_id, "task-a", "{void:?}");
        assert_ne!(
            decision.priority_category,
            SelectionPriorityCategory::RetestDue
        );
        assert_eq!(decision.consumed_asset, None);
        let state = raw_state(&fixture).await;
        assert_eq!(find_record(&state, "family-b")["status"], json!("active"));
        assert_eq!(
            find_record(&state, "family-b")["transition_note"],
            json!("retest_void:release-b")
        );
        assert_eq!(
            state["payload"]["retest_priority_marks"],
            json!([{"family": "family-b", "release_id": "release-b", "consumed": false}])
        );
    }
    // Live again: the unconsumed mark grants its one-shot priority.
    put_release(
        &fixture,
        "release-b",
        &candidate,
        PersistentReleaseState::Active,
    )
    .await;
    let decision = coordinator
        .select_next_task(STATE_ID, &candidates)
        .await
        .unwrap();
    assert_eq!(decision.chosen_task_id, "task-b");
    assert_eq!(
        decision.priority_category,
        SelectionPriorityCategory::RetestDue
    );
}

#[tokio::test]
async fn selection_record_holds_no_formal_evaluation_detail_or_broker_seed_and_a_live_basis() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, Some(d(SEED)), &[]).await;
    let facts = real_cycle(&fixture, &coordinator, 1).await;
    let candidates = eligible_candidates(
        &coordinator,
        &facts,
        &[
            ("task-a", "family-a", 1, None),
            ("task-b", "family-b", 7, None),
        ],
    )
    .await;
    let candidate = stage_candidate(&fixture, "candidate-b", &["run-b"]).await;
    put_release(
        &fixture,
        "release-b",
        &candidate,
        PersistentReleaseState::Active,
    )
    .await;
    plant_applied_asset(&fixture, "release-b", &candidate).await;
    let first = serde_json::to_value(
        coordinator
            .select_next_task(STATE_ID, &candidates)
            .await
            .unwrap(),
    )
    .unwrap();
    let second = serde_json::to_value(
        coordinator
            .select_next_task(STATE_ID, &candidates)
            .await
            .unwrap(),
    )
    .unwrap();
    // The record holds exactly the §8.4.1 fields.
    let keys: BTreeSet<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "budget_status",
            "candidate_set_digest",
            "chosen_task_id",
            "consumed_asset",
            "consumed_coverage_bucket_ids",
            "consumed_failure_record_ids",
            "fixed_random_choice_task_id",
            "fixed_random_seed_commitment",
            "frozen_state_choice_task_id",
            "id",
            "priority_category",
            "proposal_id",
            "schema_version",
            "selection_basis",
            "stable_choice_seq",
            "state_digest",
            "state_id",
        ])
    );
    // No formal evaluation detail of the release and no holdout, exposure or
    // broker material reaches it.
    for record in [&first, &second] {
        let text = record.to_string();
        for needle in [
            "formal-report-must-not-leak",
            d("formal-report-digest-must-not-leak").as_str(),
            "formal-evaluator-must-not-leak",
            "holdout",
            "exposure",
            "broker",
        ] {
            assert!(!text.contains(needle), "{needle} leaked into {text}");
        }
    }
    // The basis and the budget status are derived, never fixed literals.
    assert_eq!(first["priority_category"], json!("retest_due"));
    let basis = first["selection_basis"].as_str().unwrap();
    assert!(
        basis.starts_with("priority=retest_due;asset_release=release-b;"),
        "{basis}"
    );
    assert!(basis.contains("live_retest_marks=1"), "{basis}");
    assert!(basis.ends_with("eligible=2;offered=2"), "{basis}");
    let next_basis = second["selection_basis"].as_str().unwrap();
    assert!(
        next_basis.starts_with("priority=other;asset_release=none;"),
        "{next_basis}"
    );
    assert_ne!(basis, next_basis);
    assert_eq!(
        first["budget_status"],
        json!("runtime_status=disabled;monetary_limit_micros=0")
    );
    assert_ne!(first["state_digest"], second["state_digest"]);
}

async fn wait_terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..400 {
        let job = dispatcher.status(ctx, job_id).await.unwrap();
        if matches!(
            job.state,
            ManagementJobState::Succeeded
                | ManagementJobState::Failed
                | ManagementJobState::Cancelled
                | ManagementJobState::Blocked
        ) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("management job did not finish");
}

#[tokio::test]
async fn a_failure_gap_without_a_registered_bucket_ends_in_a_queryable_unsupported_scope_terminal()
{
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &["family-a"]).await;
    // Only an orphan cluster (no registered task bucket) holds an active
    // absolute failure.
    plant_failures(&fixture, "planted-cycle-1", &["family-orphan"]).await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &fixture.admin,
            "curriculum.step",
            json!({
                "schema_version": "rsia.management.curriculum_step.v1",
                "request_key": "unsupported-scope-step",
                "profile_id": PROFILE_ID,
                "state_id": STATE_ID,
                "root_budget_limit_micros": 1_000_000u64,
            }),
        )
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &fixture.admin, &queued.id).await;
    // I4: the management job succeeds with the queryable unsupported_scope
    // terminal instead of failing on an error string.
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "probe_scheduled");
    let Some(ManagementResult::ProbeScheduled {
        probe_job_id,
        terminal,
        ..
    }) = &done.result
    else {
        panic!("unexpected result: {:?}", done.result);
    };
    assert_eq!(*terminal, Some(ProbeTerminal::UnsupportedScope));
    let view = verified_probe_job_view(&fixture.admin, &fixture.store, probe_job_id)
        .await
        .unwrap();
    assert_eq!(view.terminal, Some(ProbeTerminal::UnsupportedScope));
    assert_eq!(
        view.trigger_digest,
        fingerprint(&PlateauSignalV1::NotTriggered {
            reasons: vec![evo_core::curriculum::UNSUPPORTED_SCOPE_REASON.into()],
        })
        .unwrap()
    );
    // Not a probe trigger: no cooldown starts.
    assert_eq!(load_state(&fixture).await.cooldown_remaining_cycles, 0);
    // With a registered cluster failing next to the orphan, the failure gap
    // fires for the registered cluster only.
    plant_failures(&fixture, "planted-cycle-2", &["family-a"]).await;
    let job = probe_job(&coordinator).await;
    assert_eq!(
        job.trigger_digest,
        fingerprint(&PlateauSignalV1::FailureGap {
            failure_cluster_ids: vec!["family-a".into()],
        })
        .unwrap()
    );
    assert_eq!(job.terminal, Some(ProbeTerminal::BudgetExhausted));
}

#[tokio::test]
async fn a_stored_v2_state_keeps_its_failure_clusters_as_legacy_diagnostics_across_cycles() {
    let fixture = Fixture::new().await;
    let coordinator = curriculum(&fixture, None, &["family-legacy"]).await;
    // A learner state stored by the v2 writer, with a bare cluster list.
    let mut legacy = registered_v2_state(&fixture, &["family-legacy"]);
    legacy.failure_clusters = vec!["family-legacy".into()];
    legacy.validate().unwrap();
    overwrite_state(&fixture.store, &fixture.admin, &legacy).await;
    for n in 1..=2 {
        real_cycle(&fixture, &coordinator, n).await;
        let state = raw_state(&fixture).await;
        assert_eq!(
            state["payload"]["schema_version"],
            json!("rsia.learner_state.v3")
        );
        // I3: the legacy cluster survives every later cycle as an untyped
        // diagnostic record and never counts as a current gap.
        let record = find_record(&state, "family-legacy");
        assert_eq!(record["status"], json!("diagnostic"));
        assert_eq!(record["transition_note"], json!(LEGACY_V2_UNTYPED_NOTE));
        assert_eq!(record["record_type"], Value::Null);
        assert_eq!(
            record["record_id"],
            json!(legacy_failure_record_id("family-legacy"))
        );
        assert_eq!(state["payload"]["failure_clusters"], json!([]));
    }
    let state = load_state(&fixture).await;
    assert_eq!(
        state
            .failure_records
            .iter()
            .filter(|record| record.status == FailureRecordStatus::Diagnostic)
            .count(),
        1
    );
    let job = probe_job(&coordinator).await;
    assert_ne!(
        job.trigger_digest,
        fingerprint(&PlateauSignalV1::FailureGap {
            failure_cluster_ids: vec!["family-legacy".into()],
        })
        .unwrap()
    );
}
