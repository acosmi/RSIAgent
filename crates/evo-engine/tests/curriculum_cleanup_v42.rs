//! AG-025 (E12/E08): the `rsia.curriculum_artifact_envelope.v1` records join the
//! source-revocation cleanup closure (plan §11.4, §11.5, E08). Real SQLite store,
//! no model, no provider, zero monetary cost.
//!
//! A run that a development cycle depends on reaches the learner state and the
//! cycle receipt through the stage facts (`record_cycle` writes the edges), and
//! from the state every probe job, schedule receipt, proposal, validity report,
//! attempt and selection. Before this change the cleanup of such a run stopped in
//! `blocked_unknown_scope:artifact:rsia.curriculum_artifact_envelope.v1:learner_state_v2`.
//!
//! Classification under test (per record_kind):
//! - redacted: learner_state_v2, development_cycle_receipt_v1,
//!   curriculum_proposal_attempt_v1, structured_test_proposal_v1,
//!   curriculum_validity_report_v1, curriculum_selection_v1;
//! - preserved: curriculum_profile_v1, coverage_probe_job_v1,
//!   probe_schedule_receipt_v1;
//! - any other record_kind (or envelope schema) still fails the job closed.
//!
//! The registered-runner fixtures below are copied from
//! `tests/development_receipts.rs` and `tests/dispatch_management.rs` (test crates
//! cannot import each other); the originals are untouched.
//!
//! The offline profile never activates a probe (every job is terminal) and never
//! deems a report development-eligible, so `record_probe_attempt` and
//! `select_next_task` cannot produce a record through the public API. Those two
//! kinds are written in the exact envelope shape and with the exact edges the
//! production code writes (see `Chain::build`); the attempt is read back through
//! the production idempotent path to show the shape is accepted.
//!
//! Boundary: a `curriculum.step` submitted through the management dispatcher also
//! leaves a `rsia.management_private_input.v1` artifact and a
//! `rsia.management_job.v1` job that depend on the learner state. They are not
//! curriculum envelopes and have no cleanup classification here, so the chains
//! below schedule through `schedule_probe_idempotent` (the dispatcher is only used
//! where no cleanup job is driven to the end, or where nothing was submitted
//! before the revocation).

use evo_core::curriculum::{
    CurriculumControlProfileV1, LearnerStateV2, ProbeJobV1, ProbeTerminal, ProposalAttemptOutcome,
    RegisteredProperty, StructuredTestProposalV1,
};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::curriculum::{
    CurriculumSourceArtifactV1, CurriculumSourceKindV1, DevelopmentCycleReceiptV1,
    PersistentCurriculumCoordinator, ProposalAttemptReceiptV1, TaskSelectionDecisionV1,
    curriculum_probe_job_storage_id, curriculum_profile_storage_id, curriculum_state_storage_id,
    probe_schedule_receipt_id, verified_probe_job_view,
};
use evo_engine::curriculum_profiles::{
    CLAMP_TARGET_ID, ClampOutput, RegisteredPureFunctionProfileV1, check_target_outputs,
    registered_profile_source_bodies, runtime_validity_report,
};
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
    registered_runner_digest, typed_receipt_closure,
};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::optimization::{
    DevRunner, DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";
const PROFILE_ID: &str = "pure_function_test_proposal.v1";
const STATE_ID: &str = "learner-state";
const ENVELOPE: &str = "rsia.curriculum_artifact_envelope.v1";
const REDACTED: &str = "rsia.redacted.v1";

const PROFILE_KIND: &str = "curriculum_profile_v1";
const STATE_KIND: &str = "learner_state_v2";
const CYCLE_KIND: &str = "development_cycle_receipt_v1";
const JOB_KIND: &str = "coverage_probe_job_v1";
const ATTEMPT_KIND: &str = "curriculum_proposal_attempt_v1";
const PROPOSAL_KIND: &str = "structured_test_proposal_v1";
const VALIDITY_KIND: &str = "curriculum_validity_report_v1";
const SELECTION_KIND: &str = "curriculum_selection_v1";
const RECEIPT_KIND: &str = "probe_schedule_receipt_v1";

/// The plan-ruled split: content derived from sources is redacted; the control
/// registration, the scheduling/quota facts and the idempotent receipt survive.
const REDACTED_KINDS: [&str; 6] = [
    STATE_KIND,
    CYCLE_KIND,
    ATTEMPT_KIND,
    PROPOSAL_KIND,
    VALIDITY_KIND,
    SELECTION_KIND,
];
const PRESERVED_KINDS: [&str; 3] = [PROFILE_KIND, JOB_KIND, RECEIPT_KIND];

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn storage_id(record_kind: &str, id: &str) -> String {
    format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
}

// ---------------------------------------------------------------------------
// Fixtures (copied from tests/development_receipts.rs)
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
        DevelopmentTaskSpecV1::registered(
            "task-b",
            "family-b",
            RegisteredTargetInputV1::ClampI64 {
                value: 5,
                min: 0,
                max: 3,
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
        candidate_bundle_digest: d("candidate-bundle"),
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
    path: PathBuf,
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
        let path = dir.path().join("curriculum-cleanup.sqlite3");
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
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        Self {
            _dir: dir,
            path,
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

    /// Commits the request/observation pair the way the E03 step and its
    /// recovery journal would: with run, watermark, and artifact extras.
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
            dependencies: extras.clone(),
            payload: serde_json::to_value(request).unwrap(),
        }
        .seal()
        .unwrap();
        let observed_fact = observed_fact(request, report, extras);
        journal.commit(request_fact.clone()).await.unwrap();
        journal.commit(observed_fact.clone()).await.unwrap();
        (request_fact.artifact_id, observed_fact.artifact_id)
    }
}

fn observed_fact(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
    extras: Vec<StageDependency>,
) -> StageFact {
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
    dependencies.extend(typed_receipt_closure(report).unwrap());
    dependencies.extend(extras);
    StageFact {
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
    .unwrap()
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

/// Registers the typed sources, the offline profile and the initial learner state.
async fn curriculum(fixture: &Fixture) -> PersistentCurriculumCoordinator {
    let coordinator =
        PersistentCurriculumCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
            .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets":["clamp-boundary"]}),
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
    let profile = CurriculumControlProfileV1::offline_default(
        PROFILE_ID,
        d("task-space"),
        registered.oracle_digest.clone(),
        registered.runner_digest.clone(),
    )
    .unwrap();
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

fn curriculum_source_ids() -> Vec<String> {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    vec![
        registered.oracle_source_id,
        registered.runner_source_id,
        registered.target_source_id,
        "task-space-source".into(),
    ]
}

// ---------------------------------------------------------------------------
// The full chain: every one of the nine record kinds
// ---------------------------------------------------------------------------

struct Chain {
    fixture: Fixture,
    coordinator: PersistentCurriculumCoordinator,
    /// (request fact id, observed fact id) of the two development cycles.
    facts: Vec<(String, String)>,
    job: ProbeJobV1,
    proposal: StructuredTestProposalV1,
    validity_id: String,
    attempt: ProposalAttemptReceiptV1,
    selection: TaskSelectionDecisionV1,
}

impl Chain {
    /// profile + state (registration), two real registered development cycles
    /// through `record_cycle`, one `schedule_probe_idempotent` (two cycles are a
    /// cold start, so the zero-budget profile ends the probe in
    /// `BudgetExhausted`), one stored proposal and its validity report, plus the
    /// two kinds the offline profile cannot produce (attempt, selection).
    async fn build() -> Self {
        let fixture = Fixture::new().await;
        let coordinator = curriculum(&fixture).await;

        let mut facts = Vec::new();
        for (n, episode) in ["episode-1", "episode-2"].into_iter().enumerate() {
            let request =
                build_request(&fixture.control, &format!("dev-request-{}", n + 1), episode);
            let report = fixture.runner().run(request.clone()).await.unwrap();
            let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
            let state = coordinator
                .record_cycle(DevelopmentCycleReceiptV1 {
                    schema_version: "rsia.development_cycle_receipt.v1".into(),
                    id: format!("cycle-{}", n + 1),
                    state_id: STATE_ID.into(),
                    development_request_fact_id: request_fact_id.clone(),
                    development_observed_fact_id: observed_fact_id.clone(),
                })
                .await
                .unwrap();
            assert_eq!(state.completed_cycles.len(), n + 1);
            facts.push((request_fact_id, observed_fact_id));
        }

        let job = coordinator
            .schedule_probe_idempotent("probe-step-1", PROFILE_ID, STATE_ID, 1_000_000)
            .await
            .unwrap();
        assert_eq!(job.terminal, Some(ProbeTerminal::BudgetExhausted));

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
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        let offline = check_target_outputs(
            &registered,
            &proposal,
            ClampOutput { clamped: -1 },
            ClampOutput { clamped: -1 },
        )
        .unwrap();
        let report = runtime_validity_report(&registered, &proposal, &offline).unwrap();
        let validity_id = coordinator
            .record_validity_report(STATE_ID, report)
            .await
            .unwrap();

        // Attempt: edges exactly as `record_probe_attempt` writes them
        // (attempt -> job, attempt -> each source artifact).
        let attempt = ProposalAttemptReceiptV1 {
            schema_version: "rsia.curriculum_proposal_attempt.v1".into(),
            id: "attempt-1".into(),
            job_id: job.id.clone(),
            outcome: ProposalAttemptOutcome::Malformed,
            source_artifact_ids: vec!["task-space-source".into()],
        };
        forge_record(
            &fixture,
            ATTEMPT_KIND,
            &attempt.id,
            &attempt,
            &[
                curriculum_probe_job_storage_id(&job.id).unwrap(),
                "task-space-source".into(),
            ],
        )
        .await;
        // The production idempotent path reads the forged attempt back: same
        // envelope shape, same payload.
        let replayed = coordinator
            .record_probe_attempt(attempt.clone())
            .await
            .unwrap();
        assert_eq!(replayed.id, job.id);

        // Selection: edges exactly as `select_next_task` writes them
        // (selection -> state, proposal, validity report).
        let selection = TaskSelectionDecisionV1 {
            schema_version: "rsia.curriculum_selection.v1".into(),
            id: "selection-1".into(),
            state_id: STATE_ID.into(),
            state_digest: d("state"),
            chosen_task_id: "candidate-task".into(),
            proposal_id: proposal.id.clone(),
            stable_choice_seq: 1,
            reason: "runner_verified".into(),
            budget_status: "disabled_zero_budget".into(),
        };
        forge_record(
            &fixture,
            SELECTION_KIND,
            &selection.id,
            &selection,
            &[
                curriculum_state_storage_id(STATE_ID).unwrap(),
                storage_id(PROPOSAL_KIND, &proposal.id),
                storage_id(VALIDITY_KIND, &validity_id),
            ],
        )
        .await;

        Self {
            fixture,
            coordinator,
            facts,
            job,
            proposal,
            validity_id,
            attempt,
            selection,
        }
    }

    fn cycle_ids(&self) -> [&'static str; 2] {
        ["cycle-1", "cycle-2"]
    }

    /// (record_kind, logical id) of every curriculum record the chain holds.
    fn records(&self) -> Vec<(&'static str, String)> {
        vec![
            (PROFILE_KIND, PROFILE_ID.into()),
            (STATE_KIND, STATE_ID.into()),
            (CYCLE_KIND, "cycle-1".into()),
            (CYCLE_KIND, "cycle-2".into()),
            (JOB_KIND, self.job.id.clone()),
            (
                RECEIPT_KIND,
                probe_schedule_receipt_id("probe-step-1").unwrap(),
            ),
            (ATTEMPT_KIND, self.attempt.id.clone()),
            (PROPOSAL_KIND, self.proposal.id.clone()),
            (VALIDITY_KIND, self.validity_id.clone()),
            (SELECTION_KIND, self.selection.id.clone()),
        ]
    }
}

/// Writes a curriculum envelope the way `put_record` does, plus its edges.
async fn forge_record<T: Serialize>(
    fixture: &Fixture,
    record_kind: &str,
    id: &str,
    payload: &T,
    depends_on: &[String],
) {
    let storage = storage_id(record_kind, id);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &storage,
            "admin",
            &json!({
                "schema_version": ENVELOPE,
                "id": storage,
                "record_kind": record_kind,
                "payload": payload,
            }),
        )
        .await
        .unwrap();
    for dependency in depends_on {
        session
            .put_edge(&fixture.admin, "artifact", &storage, "artifact", dependency)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

async fn body(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// Every curriculum envelope, live or redacted, keyed by its storage id.
async fn curriculum_bodies(store: &Store, ctx: &Context) -> BTreeMap<String, Value> {
    let mut session = store.session().await.unwrap();
    let all: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    all.into_iter()
        .filter(|value| {
            value["schema_version"] == ENVELOPE
                || (value["schema_version"] == REDACTED && value["original_schema"] == ENVELOPE)
        })
        .map(|value| (value["id"].as_str().unwrap().to_string(), value))
        .collect()
}

/// Everything reachable from `source` over "depends on" edges (what the
/// cleanup frontier expands).
async fn closure(store: &Store, ctx: &Context, source: (&str, &str)) -> BTreeSet<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![(source.0.to_string(), source.1.to_string())];
    let mut session = store.session().await.unwrap();
    while let Some((kind, id)) = pending.pop() {
        for dependent in session.dependents(ctx, &kind, &id).await.unwrap() {
            if seen.insert(dependent.clone()) {
                pending.push(dependent);
            }
        }
    }
    session.commit().await.unwrap();
    seen
}

async fn dependents_of(
    store: &Store,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Vec<(String, String)> {
    let mut session = store.session().await.unwrap();
    let dependents = session.dependents(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    dependents
}

fn has_edge(dependents: &[(String, String)], src_id: &str) -> bool {
    dependents
        .iter()
        .any(|(kind, id)| kind == "artifact" && id == src_id)
}

async fn begin(fixture: &Fixture, source: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: source.into(),
        },
        "source revoked",
        500,
    )
    .await
    .unwrap()
}

/// Drives the cleanup job until it stops moving.
async fn drive(
    store: &Store,
    admin: &Context,
    mut status: CleanupStatus,
    page: usize,
) -> CleanupStatus {
    for now in 501..1_500 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(admin, store, &status.job_id, page, now)
            .await
            .unwrap();
    }
    status
}

async fn revoke_and_clean(fixture: &Fixture, source: &str) -> CleanupStatus {
    let status = begin(fixture, source).await;
    drive(&fixture.store, &fixture.admin, status, 8).await
}

fn assert_redacted_envelope(body: &Value, storage: &str, record_kind: &str) {
    let digest = body["original_digest"].as_str().unwrap_or_default();
    assert_eq!(digest.len(), 64, "{body}");
    assert_eq!(
        body,
        &json!({
            "id": storage,
            "schema_version": REDACTED,
            "state": "source_revoked",
            "original_kind": "artifact",
            "original_schema": ENVELOPE,
            "original_digest": digest,
            "metadata": {"record_kind": record_kind},
        }),
        "a redacted curriculum record keeps its identity and kind and nothing derived"
    );
}

// ---------------------------------------------------------------------------
// Edge review: every record is reachable from the revoked source run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_content_derived_curriculum_record_is_reachable_from_the_source_run() {
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;

    // Direct edges each write path records (plan §11.4: a record depends on the
    // inputs it was derived from).
    let state_storage = curriculum_state_storage_id(STATE_ID).unwrap();
    let profile_storage = curriculum_profile_storage_id(PROFILE_ID).unwrap();
    let job_storage = curriculum_probe_job_storage_id(&chain.job.id).unwrap();
    let proposal_storage = storage_id(PROPOSAL_KIND, &chain.proposal.id);
    let validity_storage = storage_id(VALIDITY_KIND, &chain.validity_id);
    let receipt_storage = storage_id(
        RECEIPT_KIND,
        &probe_schedule_receipt_id("probe-step-1").unwrap(),
    );
    let sources = curriculum_source_ids();

    // register_profile_and_state: state and profile -> every typed source.
    for source in &sources {
        let dependents = dependents_of(&fixture.store, ctx, "artifact", source).await;
        assert!(has_edge(&dependents, &state_storage), "state -> {source}");
        assert!(
            has_edge(&dependents, &profile_storage),
            "profile -> {source}"
        );
    }
    // record_cycle: the cycle receipt and the state -> both development facts.
    for (n, (request_fact, observed_fact)) in chain.facts.iter().enumerate() {
        let cycle_storage = storage_id(CYCLE_KIND, chain.cycle_ids()[n]);
        for fact in [request_fact, observed_fact] {
            let dependents = dependents_of(&fixture.store, ctx, "artifact", fact).await;
            assert!(
                has_edge(&dependents, &cycle_storage),
                "cycle-{} -> {fact}",
                n + 1
            );
            assert!(has_edge(&dependents, &state_storage), "state -> {fact}");
            // The facts themselves hang off the source run.
            let run_dependents = dependents_of(&fixture.store, ctx, "run", "run-a").await;
            assert!(has_edge(&run_dependents, fact), "{fact} -> run-a");
        }
    }
    // schedule_probe: job -> state and every state source.
    let state_dependents = dependents_of(&fixture.store, ctx, "artifact", &state_storage).await;
    assert!(has_edge(&state_dependents, &job_storage), "job -> state");
    for source in &sources {
        let dependents = dependents_of(&fixture.store, ctx, "artifact", source).await;
        assert!(has_edge(&dependents, &job_storage), "job -> {source}");
    }
    // schedule_probe_idempotent: receipt -> job, state, profile.
    for (dependency, name) in [
        (&job_storage, "job"),
        (&state_storage, "state"),
        (&profile_storage, "profile"),
    ] {
        let dependents = dependents_of(&fixture.store, ctx, "artifact", dependency).await;
        assert!(has_edge(&dependents, &receipt_storage), "receipt -> {name}");
    }
    // store_proposal: proposal -> job and its sources.
    let job_dependents = dependents_of(&fixture.store, ctx, "artifact", &job_storage).await;
    assert!(
        has_edge(&job_dependents, &proposal_storage),
        "proposal -> job"
    );
    let task_space = dependents_of(&fixture.store, ctx, "artifact", "task-space-source").await;
    assert!(
        has_edge(&task_space, &proposal_storage),
        "proposal -> source"
    );
    // record_validity_report: validity -> proposal and job.
    let proposal_dependents =
        dependents_of(&fixture.store, ctx, "artifact", &proposal_storage).await;
    assert!(
        has_edge(&proposal_dependents, &validity_storage),
        "validity -> proposal"
    );
    assert!(
        has_edge(&job_dependents, &validity_storage),
        "validity -> job"
    );

    // Transitive closure of the source run: every record that carries content
    // derived from the run is in it, so the cleanup frontier reaches it. (The
    // administrator's profile depends on the typed sources only and holds nothing
    // run-derived, so it is the one record the closure does not have to reach.)
    let reach = closure(&fixture.store, ctx, ("run", "run-a")).await;
    for (record_kind, id) in chain.records() {
        if record_kind == PROFILE_KIND {
            continue;
        }
        assert!(
            reach.contains(&("artifact".to_string(), storage_id(record_kind, &id))),
            "{record_kind} {id} is not reachable from run-a"
        );
    }
    // The typed sources are roots: no edge makes one depend on a run, so a run
    // revocation never reaches them. They have no cleanup classification of
    // their own; if an edge ever reached one, the job would fail closed.
    for source in &sources {
        assert!(
            !reach.contains(&("artifact".to_string(), source.clone())),
            "typed source {source} must stay outside every run closure"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. The full chain: cleanup runs to Complete; redact/preserve per kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_a_source_run_cleans_the_whole_curriculum_chain_to_complete() {
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;

    // All nine kinds are present before the revocation, each in its real
    // envelope.
    let before = curriculum_bodies(&fixture.store, ctx).await;
    let kinds_before: BTreeSet<&str> = before
        .values()
        .map(|value| value["record_kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds_before,
        BTreeSet::from([
            PROFILE_KIND,
            STATE_KIND,
            CYCLE_KIND,
            JOB_KIND,
            ATTEMPT_KIND,
            PROPOSAL_KIND,
            VALIDITY_KIND,
            SELECTION_KIND,
            RECEIPT_KIND,
        ])
    );
    assert_eq!(
        before.len(),
        10,
        "profile, state, 2 cycles, job, receipt, attempt, proposal, validity, selection"
    );
    // The forged records have exactly the shape of a production-written one.
    let real_keys: BTreeSet<&String> = before[&storage_id(JOB_KIND, &chain.job.id)]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    for (record_kind, id) in [
        (ATTEMPT_KIND, chain.attempt.id.as_str()),
        (SELECTION_KIND, chain.selection.id.as_str()),
    ] {
        let forged = &before[&storage_id(record_kind, id)];
        assert_eq!(
            forged.as_object().unwrap().keys().collect::<BTreeSet<_>>(),
            real_keys
        );
    }
    // The fixture really holds source-derived content that must not survive.
    assert!(
        before[&storage_id(STATE_KIND, STATE_ID)]["payload"]["completed_cycles"]
            .as_array()
            .is_some_and(|cycles| cycles.len() == 2)
    );
    assert_eq!(
        before[&storage_id(PROPOSAL_KIND, &chain.proposal.id)]["payload"]["reason"],
        "exercise registered lower boundary"
    );

    let status = revoke_and_clean(fixture, "run-a").await;
    // Defect: this used to end in
    // `blocked_unknown_scope:artifact:rsia.curriculum_artifact_envelope.v1:learner_state_v2`.
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    let after = curriculum_bodies(&fixture.store, ctx).await;
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "no curriculum record is deleted: redaction keeps identity, preservation keeps the record"
    );
    let mut redacted = 0;
    let mut preserved = 0;
    for (storage, original) in &before {
        let record_kind = original["record_kind"].as_str().unwrap();
        let now = &after[storage];
        if REDACTED_KINDS.contains(&record_kind) {
            assert_redacted_envelope(now, storage, record_kind);
            redacted += 1;
        } else {
            assert!(
                PRESERVED_KINDS.contains(&record_kind),
                "unclassified {record_kind}"
            );
            assert_eq!(now, original, "{record_kind} is preserved byte for byte");
            preserved += 1;
        }
    }
    assert_eq!(
        redacted, 7,
        "state, 2 cycles, attempt, proposal, validity, selection"
    );
    assert_eq!(preserved, 3, "profile, job, receipt");

    // Nothing derived from the run's content is left in the redacted kinds.
    let redacted_text = after
        .values()
        .filter(|value| value["schema_version"] == REDACTED)
        .map(Value::to_string)
        .collect::<String>();
    for needle in [
        "family-a",
        "family-b",
        "exercise registered lower boundary",
        "development-cycle-episode-1-1-1",
        "completed_cycles",
        "task-space-source",
    ] {
        assert!(
            !redacted_text.contains(needle),
            "{needle} survived redaction"
        );
    }

    // The preserved scheduling facts are still exactly what was recorded.
    let job = &after[&storage_id(JOB_KIND, &chain.job.id)]["payload"];
    assert_eq!(job["terminal"], "budget_exhausted");
    assert_eq!(job["provider_dispatch_count"], 0);
    assert_eq!(job["root_budget_limit_micros"], 1_000_000);
    let receipt = &after[&storage_id(
        RECEIPT_KIND,
        &probe_schedule_receipt_id("probe-step-1").unwrap(),
    )]["payload"];
    assert_eq!(receipt["probe_job_id"], json!(chain.job.id));
    assert_eq!(receipt["idempotency_key"], "probe-step-1");

    // The path that led here: the run's content and the stage facts are gone.
    for (request_fact, observed_fact) in &chain.facts {
        for fact in [request_fact, observed_fact] {
            let fact_body = body(&fixture.store, ctx, "artifact", fact).await;
            assert_eq!(fact_body["schema_version"], REDACTED, "{fact_body}");
        }
    }
    // The typed sources and the other run are not part of the closure.
    let mut session = fixture.store.session().await.unwrap();
    for source in curriculum_source_ids() {
        let typed: Value = session.need(ctx, "artifact", &source).await.unwrap();
        assert_eq!(typed["schema_version"], CurriculumSourceArtifactV1::SCHEMA);
    }
    let other: Value = session.need(ctx, "run", "run-b").await.unwrap();
    assert_ne!(other["schema_version"], REDACTED);
    session.commit().await.unwrap();
}

// ---------------------------------------------------------------------------
// 2. Fail closed after the revocation
// ---------------------------------------------------------------------------

/// Every coordinator entry point that reads the learner state, each with an input
/// a live chain would accept: new writes and replays of records already stored.
///
/// `schedule_probe_idempotent` with an already recorded key is deliberately not in
/// this list. That crash-recovery path reloads the recorded probe job through its
/// schedule receipt and never reads the learner state, so it does not derive
/// anything new; it is outside what this change classifies.
async fn state_entry_points(chain: &Chain) -> Vec<(&'static str, Result<(), Error>)> {
    let coordinator = &chain.coordinator;
    let cycle = |id: &str| DevelopmentCycleReceiptV1 {
        schema_version: "rsia.development_cycle_receipt.v1".into(),
        id: id.into(),
        state_id: STATE_ID.into(),
        development_request_fact_id: chain.facts[1].0.clone(),
        development_observed_fact_id: chain.facts[1].1.clone(),
    };
    let late_proposal = StructuredTestProposalV1 {
        id: "proposal-late".into(),
        ..chain.proposal.clone()
    };
    let late_attempt = ProposalAttemptReceiptV1 {
        id: "attempt-late".into(),
        ..chain.attempt.clone()
    };
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let offline = check_target_outputs(
        &registered,
        &chain.proposal,
        ClampOutput { clamped: -1 },
        ClampOutput { clamped: -1 },
    )
    .unwrap();
    let report = runtime_validity_report(&registered, &chain.proposal, &offline).unwrap();
    vec![
        (
            "record_cycle (new cycle)",
            coordinator.record_cycle(cycle("cycle-3")).await.map(|_| ()),
        ),
        (
            "record_cycle (replay)",
            coordinator.record_cycle(cycle("cycle-2")).await.map(|_| ()),
        ),
        (
            "schedule_probe",
            coordinator
                .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .map(|_| ()),
        ),
        (
            "schedule_probe_idempotent (new key)",
            coordinator
                .schedule_probe_idempotent("probe-step-2", PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .map(|_| ()),
        ),
        (
            "record_probe_attempt (new)",
            coordinator
                .record_probe_attempt(late_attempt)
                .await
                .map(|_| ()),
        ),
        (
            "record_probe_attempt (replay)",
            coordinator
                .record_probe_attempt(chain.attempt.clone())
                .await
                .map(|_| ()),
        ),
        (
            "store_proposal (new)",
            coordinator.store_proposal(STATE_ID, late_proposal).await,
        ),
        (
            "store_proposal (replay)",
            coordinator
                .store_proposal(STATE_ID, chain.proposal.clone())
                .await,
        ),
        (
            "record_validity_report",
            coordinator
                .record_validity_report(STATE_ID, report)
                .await
                .map(|_| ()),
        ),
        (
            "select_next_task",
            coordinator
                .select_next_task(STATE_ID, &[])
                .await
                .map(|_| ()),
        ),
        (
            "record_applied_learning_asset",
            coordinator
                .record_applied_learning_asset(STATE_ID, "run-b", "application", "execution")
                .await
                .map(|_| ()),
        ),
        // The read side of `curriculum.step` status: reloads the probe job and
        // re-verifies its learner state against the live source closure.
        (
            "verified_probe_job_view",
            verified_probe_job_view(&chain.fixture.admin, &chain.fixture.store, &chain.job.id)
                .await
                .map(|_| ()),
        ),
    ]
}

#[tokio::test]
async fn a_revoked_curriculum_fails_closed_before_and_after_the_cleanup() {
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;

    // Live chain: the same calls are accepted (replays) or at least not blocked by
    // the revocation machinery, so the assertions below are about the revocation.
    assert!(
        chain
            .coordinator
            .record_cycle(DevelopmentCycleReceiptV1 {
                schema_version: "rsia.development_cycle_receipt.v1".into(),
                id: "cycle-2".into(),
                state_id: STATE_ID.into(),
                development_request_fact_id: chain.facts[1].0.clone(),
                development_observed_fact_id: chain.facts[1].1.clone(),
            })
            .await
            .is_ok()
    );
    assert!(
        verified_probe_job_view(ctx, &fixture.store, &chain.job.id)
            .await
            .is_ok()
    );

    // Phase 1: the logical block. The watermark bump alone fails every entry
    // point that reads the learner state, before a single cleanup step ran.
    let status = begin(fixture, "run-a").await;
    assert_eq!(status.state, CleanupState::Pending);
    for (name, result) in state_entry_points(&chain).await {
        assert!(
            matches!(&result, Err(Error::Conflict(message)) if message.contains("watermark")),
            "{name} after begin_revoke: {result:?}"
        );
    }

    // Phase 2: the cleanup ran to Complete; the learner state is redacted and
    // every entry point still fails (no silent success, no panic).
    let status = drive(&fixture.store, ctx, status, 8).await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    let revoked_state = body(
        &fixture.store,
        ctx,
        "artifact",
        &curriculum_state_storage_id(STATE_ID).unwrap(),
    )
    .await;
    assert_eq!(revoked_state["schema_version"], REDACTED);
    let records_before_retry = curriculum_bodies(&fixture.store, ctx).await;
    for (name, result) in state_entry_points(&chain).await {
        assert!(
            result.is_err(),
            "{name} must not succeed on a redacted state"
        );
    }

    // curriculum.step through the management dispatcher: the job ends Failed,
    // never Succeeded, and schedules nothing.
    let dispatcher = ManagementDispatcher::new(fixture.store.clone(), vec![ctx.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            ctx,
            "curriculum.step",
            step_request("curriculum-after-revocation"),
        )
        .await
        .unwrap();
    let finished = wait_terminal(&dispatcher, ctx, &queued.id).await;
    assert_eq!(finished.state, ManagementJobState::Failed, "{finished:?}");
    assert!(finished.error_code.is_some());

    // None of the refused calls wrote a curriculum record or rewrote the
    // redacted state.
    assert_eq!(
        curriculum_bodies(&fixture.store, ctx).await,
        records_before_retry
    );
}

fn step_request(request_key: &str) -> Value {
    json!({
        "schema_version": "rsia.management.curriculum_step.v1",
        "request_key": request_key,
        "profile_id": PROFILE_ID,
        "state_id": STATE_ID,
        "root_budget_limit_micros": 1_000_000u64,
    })
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
async fn a_management_curriculum_step_status_fails_closed_once_its_source_is_revoked() {
    // A curriculum.step that succeeded before the revocation: its status reloads
    // the probe job and re-verifies the learner state against the live closure.
    // The logical block (the watermark bump of begin_revoke) stops it before any
    // cleanup step ran.
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;
    let dispatcher = ManagementDispatcher::new(fixture.store.clone(), vec![ctx.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            ctx,
            "curriculum.step",
            step_request("curriculum-before-revocation"),
        )
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, ctx, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "probe_scheduled");

    let status = begin(fixture, "run-a").await;
    assert_eq!(status.state, CleanupState::Pending);
    let blocked = dispatcher.status(ctx, &queued.id).await;
    assert!(
        matches!(&blocked, Err(Error::Conflict(message)) if message.contains("watermark")),
        "{blocked:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Unknown kinds still fail closed
// ---------------------------------------------------------------------------

async fn hang_on_state(chain: &Chain, envelope: Value, id: &str, object_kind: &str) {
    let fixture = &chain.fixture;
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, object_kind, id, "admin", &envelope)
        .await
        .unwrap();
    session
        .put_edge(
            &fixture.admin,
            object_kind,
            id,
            "artifact",
            &curriculum_state_storage_id(STATE_ID).unwrap(),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn assert_blocked(chain: &Chain, expected_error: &str) {
    let fixture = &chain.fixture;
    let status = revoke_and_clean(fixture, "run-a").await;
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    assert_eq!(
        status.last_error.as_deref(),
        Some(expected_error),
        "{status:?}"
    );
    // The job does not claim completion on a further step either.
    let again =
        LifecycleStore::cleanup_step(&fixture.admin, &fixture.store, &status.job_id, 8, 2_000)
            .await
            .unwrap();
    assert_eq!(again.state, CleanupState::Failed);
    assert_eq!(again.last_error.as_deref(), Some(expected_error));
}

#[tokio::test]
async fn an_unknown_curriculum_record_kind_still_fails_the_cleanup_job_closed() {
    let chain = Chain::build().await;
    let forged = storage_id("unknown_v1", "forged");
    hang_on_state(
        &chain,
        json!({
            "schema_version": ENVELOPE,
            "id": forged,
            "record_kind": "unknown_v1",
            "payload": {"note": "a record nobody classified"},
        }),
        &forged,
        "artifact",
    )
    .await;
    assert_blocked(
        &chain,
        "blocked_unknown_scope:artifact:rsia.curriculum_artifact_envelope.v1:unknown_v1",
    )
    .await;
}

#[tokio::test]
async fn an_unknown_envelope_version_or_object_kind_is_not_classified_by_record_kind_alone() {
    // A known record_kind under another envelope schema.
    let chain = Chain::build().await;
    let forged = storage_id(STATE_KIND, "forged-v2");
    hang_on_state(
        &chain,
        json!({
            "schema_version": "rsia.curriculum_artifact_envelope.v2",
            "id": forged,
            "record_kind": STATE_KIND,
            "payload": {},
        }),
        &forged,
        "artifact",
    )
    .await;
    assert_blocked(
        &chain,
        "blocked_unknown_scope:artifact:rsia.curriculum_artifact_envelope.v2:learner_state_v2",
    )
    .await;

    // The same envelope and a known record_kind stored under another object
    // kind: the arms are for artifacts only.
    let chain = Chain::build().await;
    hang_on_state(
        &chain,
        json!({
            "schema_version": ENVELOPE,
            "id": "forged-receipt",
            "record_kind": STATE_KIND,
            "payload": {},
        }),
        "forged-receipt",
        "receipt",
    )
    .await;
    assert_blocked(
        &chain,
        "blocked_unknown_scope:receipt:rsia.curriculum_artifact_envelope.v1:learner_state_v2",
    )
    .await;
}

// ---------------------------------------------------------------------------
// 4. Isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_a_run_the_curriculum_does_not_depend_on_leaves_it_unchanged() {
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;
    store_trace_authority(
        &fixture.store,
        &fixture.host,
        &authority("run-unrelated", "family-x"),
    )
    .await
    .unwrap();
    let before = curriculum_bodies(&fixture.store, ctx).await;
    assert_eq!(before.len(), 10);
    let fact_before = body(&fixture.store, ctx, "artifact", &chain.facts[0].0).await;

    let status = revoke_and_clean(fixture, "run-unrelated").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);

    assert_eq!(curriculum_bodies(&fixture.store, ctx).await, before);
    // The stage facts of the cycles the run never fed are untouched as well.
    assert_eq!(
        body(&fixture.store, ctx, "artifact", &chain.facts[0].0).await,
        fact_before
    );
    // Every live record is still typed and unredacted.
    assert!(
        before
            .values()
            .all(|value| value["schema_version"] == ENVELOPE)
    );
}

// ---------------------------------------------------------------------------
// A profile the closure does reach is preserved (forced edge)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_profile_that_the_closure_reaches_is_preserved_byte_for_byte() {
    // No production write path makes a profile depend on a run. The edge is
    // written directly (same technique as the monitoring closure test) so the
    // profile arm of the classification is exercised.
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let ctx = &fixture.admin;
    let profile_storage = curriculum_profile_storage_id(PROFILE_ID).unwrap();
    let profile_before = body(&fixture.store, ctx, "artifact", &profile_storage).await;
    let mut session = fixture.store.session().await.unwrap();
    session
        .put_edge(ctx, "artifact", &profile_storage, "run", "run-a")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        closure(&fixture.store, ctx, ("run", "run-a"))
            .await
            .contains(&("artifact".to_string(), profile_storage.clone()))
    );

    let status = revoke_and_clean(fixture, "run-a").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(
        body(&fixture.store, ctx, "artifact", &profile_storage).await,
        profile_before
    );
    assert_eq!(profile_before["record_kind"], PROFILE_KIND);
    assert_eq!(profile_before["payload"]["profile_id"], PROFILE_ID);
    // The learner state next to it is redacted all the same.
    assert_redacted_envelope(
        &body(
            &fixture.store,
            ctx,
            "artifact",
            &curriculum_state_storage_id(STATE_ID).unwrap(),
        )
        .await,
        &curriculum_state_storage_id(STATE_ID).unwrap(),
        STATE_KIND,
    );
}

// ---------------------------------------------------------------------------
// Paged cleanup resumes after a restart (E08: crash, then continue)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_curriculum_cleanup_is_paged_and_resumes_after_a_restart() {
    let chain = Chain::build().await;
    let admin = chain.fixture.admin.clone();
    let path = chain.fixture.path.clone();
    let store = chain.fixture.store.clone();
    let before = curriculum_bodies(&store, &admin).await;
    let state_storage = curriculum_state_storage_id(STATE_ID).unwrap();

    let mut status = begin(&chain.fixture, "run-a").await;
    // One frontier node per step, one edge per page: stop as soon as the learner
    // state is redacted but the job has not finished.
    let mut steps = 0;
    for now in 501..1_500 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 1, now)
            .await
            .unwrap();
        steps += 1;
        let state_now = body(&store, &admin, "artifact", &state_storage).await;
        if state_now["schema_version"] == REDACTED {
            break;
        }
        assert_eq!(status.state, CleanupState::Running, "{status:?}");
    }
    assert_ne!(
        status.state,
        CleanupState::Complete,
        "{status:?} after {steps} steps"
    );
    assert!(status.pending_nodes > 0);
    // Half way: some curriculum records are redacted, others are not yet.
    let half = curriculum_bodies(&store, &admin).await;
    assert!(
        half.values()
            .any(|value| value["schema_version"] == REDACTED)
    );
    assert!(
        half.values()
            .any(|value| value["schema_version"] == ENVELOPE)
    );

    // Restart: the job is reloaded from the store and continues.
    let job_id = status.job_id.clone();
    store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let resumed = LifecycleStore::cleanup_status(&admin, &reopened, &job_id)
        .await
        .unwrap();
    assert_eq!(resumed.state, CleanupState::Running);
    let done = drive(&reopened, &admin, resumed, 1).await;
    assert_eq!(done.state, CleanupState::Complete, "{done:?}");
    assert_eq!(done.last_error, None);

    let after = curriculum_bodies(&reopened, &admin).await;
    for (storage, original_body) in &before {
        let record_kind = original_body["record_kind"].as_str().unwrap();
        if REDACTED_KINDS.contains(&record_kind) {
            assert_redacted_envelope(&after[storage], storage, record_kind);
        } else {
            assert_eq!(&after[storage], original_body);
        }
    }
}
