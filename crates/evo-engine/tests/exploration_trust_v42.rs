//! AG-033 (E09 trust; plan E09 acceptance "a successor is chosen only after a
//! real per-node evaluation", §7.1.1, E03 and §6.7.4 "a self-reported provenance or
//! score grants nothing", §14 R35): `run_next` records a candidate as a valid node
//! only when E03's observation gate verified the development report behind it.
//!
//! * a Fixture report is a terminal `HardFailure` labelled `FixtureDeclared`, the
//!   gate is not asked, and its node can be neither deepened nor offered as a final
//!   candidate;
//! * a report that claims a registered execution without receipts, or whose scores
//!   differ from the ones recomputed from the receipts, is refused by the gate and
//!   is a terminal `HardFailure` labelled `EvidenceRejected`;
//! * both refusals happen after the step was paid for and its dispatch claimed, so
//!   they are terminal records, never errors: the dispatch is consumed, a reconnect
//!   returns the same terminal state and the world goes on;
//! * a report the gate verifies is a `Valid` node labelled `Trusted` whose quality
//!   is the mean of the recomputed scores, and only such a node can be deepened and
//!   proposed as a final candidate;
//! * an outcome that is no candidate (no change, a rejected selection, uncertain) is
//!   not a verdict: its node and dispatch fact are `NotObserved`, as is every record
//!   written before the label existed.
//!
//! The registered runner executes parent and candidate with the same pure function,
//! so on its own it produces `KeepIncumbent`, never a candidate. The trusted path is
//! covered with `ContrastRunner`, which writes honest E03 evidence with chosen
//! outputs, as `practice_v42` does: the budget rows, settlements, execution receipts
//! and grader receipts through the public issuing functions, and the run receipt as
//! a typed envelope (the registered runner's own issuer is private). Real SQLite
//! store, no provider, zero monetary cost.
//!
//! Not covered (and not claimed): a trusted node from a runner that really executes
//! the candidate bundle (none exists), the repairable-failure origins and the
//! recovery end to end (E09 PR-B), optimization history in the request (PR-C), and a
//! gate that fails on storage (the error mapping is unit-tested in `exploration.rs`).
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, SimulationContext};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
use evo_engine::development::{
    DEVELOPMENT_RUN_RECEIPT_SCHEMA, DevelopmentControlV1, DevelopmentEvidenceScope,
    DevelopmentExecutionReceiptV1, DevelopmentRunReceiptV1, DevelopmentSide, DevelopmentTaskSpecV1,
    EXECUTION_RECEIPT_KIND, ExecutionReceiptIssueRequest, GraderReceiptIssueRequest,
    RUN_RECEIPT_KIND, RegisteredDevelopmentRunner, RegisteredExecutionRequestV1,
    RegisteredTargetInputV1, execution_budget_call_id, execution_request_digest,
    issue_execution_receipt, issue_grader_receipt, load_development_control,
    register_development_control, registered_runner_digest, run_receipt_id, storage_id,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationEvidenceV1, ExplorationWorldV1,
    MechanismUsageState, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationStepRequest, PairedTaskResult, StageFact, StageFactKind,
    StoreOptimizationJournal, VerifiedDevelopmentObservationView, verify_development_observation,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";
/// The runs of the world's source closure, and of the control's (sorted, unique).
const RUNS: [&str; 2] = ["run-failure", "run-success"];
const WORLD: &str = "exploration_world_v1";
const NODE: &str = "exploration_node_v1";
const DISPATCH: &str = "exploration_dispatch_v1";
/// An answer the grader scores 0 for every registered task.
const WRONG_ANSWER: &str = "{\"answer\":{\"clamped\":12345}}";

const FIXTURE_REASON: &str = "fixture_evidence_not_accepted";
const REJECTED_REASON: &str = "development_evidence_rejected";
const OBSERVED_REASON: &str = "candidate_observed";
const KEEP_INCUMBENT_REASON: &str =
    "candidate did not strictly improve while preserving all incumbent passes";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

// ---------------------------------------------------------------------------
// The registered control, the world and the trusted source closure
// ---------------------------------------------------------------------------

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
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

/// The Admin-registered control the world's development requests bind to: its
/// environment, grader, rules and tools digests are the world's, and its source
/// closure is the world's.
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
        source_ids: RUNS.iter().map(|run| (*run).into()).collect(),
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

fn trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

fn parent_skill() -> SkillSnapshot {
    SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    }
}

fn world_for(id: &str) -> ExplorationWorldV1 {
    let parent_skill_digest = skill_snapshot_digest(&parent_skill()).unwrap();
    let parent_bundle = d("parent-bundle");
    let environment = d("environment");
    let model = d("model");
    let tools = d("tools");
    let grader = fingerprint(&grader_spec()).unwrap();
    let rules = d("rules");
    let context_signature = fingerprint(&(
        &parent_skill_digest,
        &parent_bundle,
        &environment,
        &model,
        &tools,
        &grader,
        &rules,
        1u64,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: id.into(),
        approved_parent_digest: d("approved-parent"),
        context_signature,
        parent_skill_digest,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark: 1,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        root_opportunities: vec![
            RootOpportunity {
                root_slot: 2,
                branch_seq: 2,
                action_seq: 2,
                estimated_cost_upper_micros: 10,
            },
            RootOpportunity {
                root_slot: 1,
                branch_seq: 1,
                action_seq: 1,
                estimated_cost_upper_micros: 10,
            },
        ],
        dependencies: RUNS
            .iter()
            .map(|run| ExplorationDependency {
                kind: "run".into(),
                id: (*run).into(),
            })
            .collect(),
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 2,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    }
}

fn admin() -> Context {
    Context::new(NAMESPACE, "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NAMESPACE, "worker", Role::Worker).unwrap()
}

/// The two Host-issued trusted runs and one watermark bump (`source_watermark ==
/// 1`), and, when asked, the root budget and the Admin-registered development
/// control that E03's receipts bind to.
async fn seeded_store(with_control: bool) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("exploration-trust.sqlite3"))
        .await
        .unwrap();
    let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        (RUNS[0], "family-a", TraceOutcome::TaskFailure),
        (RUNS[1], "family-b", TraceOutcome::Success),
    ] {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(&store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        &store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    if with_control {
        let admin = admin();
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
        register_development_control(&admin, &store, control())
            .await
            .unwrap();
    }
    (dir, store)
}

// ---------------------------------------------------------------------------
// The model port and the development runners
// ---------------------------------------------------------------------------

struct EditingFixtureModel;

#[async_trait]
impl ModelPort for EditingFixtureModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let parent: SkillSnapshot = serde_json::from_str(
            &request
                .input
                .iter()
                .find(|part| part.label == "parent-skill")
                .ok_or_else(|| Error::Invalid("fixture parent input missing".into()))?
                .content,
        )
        .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
        let source = request.source_closure[0].clone();
        let (id, batch_id, field, start) = match request.stage {
            ModelStage::ReflectFailure => (
                "repair-content",
                "reflection-failure",
                SkillTextField::Content,
                parent.content.len(),
            ),
            ModelStage::ReflectSuccess => (
                "preserve-applicability",
                "reflection-success",
                SkillTextField::Applicability,
                parent.applicability.len(),
            ),
            _ => return Err(Error::Invalid("unexpected fixture stage".into())),
        };
        let suggestion = EditSuggestion {
            id: id.into(),
            hypothesis: "bounded deterministic fixture repair".into(),
            batch_ids: vec![batch_id.into()],
            support: vec![source.clone()],
            counterexamples: (request.stage == ModelStage::ReflectSuccess)
                .then_some(source.clone())
                .into_iter()
                .collect(),
            dependencies: vec![source],
            edit: SkillTextEdit {
                field,
                start,
                end: start,
                expected_text_digest: hash(b""),
                exact_anchor: None,
                operation: TextEditOperation::Insert {
                    text: " [repair]".into(),
                },
            },
        };
        let output = serde_json::to_string(&vec![suggestion]).unwrap();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: format!("fixture-edit-response-{id}"),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: format!("fixture-edit-call-{id}"),
                dispatch_id: format!("fixture-edit-dispatch-{id}"),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: format!("fixture-provider-{id}"),
                usage_record_id: format!("fixture-usage-{id}"),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

/// Counts every model dispatch: a reconnect and a replay must not reach one.
#[derive(Default)]
struct CountingModel(AtomicUsize);

impl CountingModel {
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelPort for CountingModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        EditingFixtureModel.dispatch(request).await
    }
}

/// A model port that fails once the dispatch was claimed.
struct FailingModel;

#[async_trait]
impl ModelPort for FailingModel {
    async fn dispatch(&self, _: ModelRequest) -> Result<ModelResponse> {
        Err(Error::NotFound)
    }
}

/// A model port that suggests nothing: the step ends without a candidate.
struct NoChangeModel;

#[async_trait]
impl ModelPort for NoChangeModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let output = "[]".to_string();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: "response".into(),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: "call".into(),
                dispatch_id: "dispatch".into(),
                root_budget_id: "budget".into(),
                provider_request_id: "provider".into(),
                usage_record_id: "usage".into(),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

/// A report that improves every task and names receipts that do not exist: the
/// ids are made up, nothing was issued or settled under them.
fn unreceipted_report(
    request: DevelopmentRunRequest,
    provenance: DevelopmentExecutionProvenance,
) -> DevelopmentRunReport {
    let results = request
        .manifest
        .tasks
        .iter()
        .map(|task| PairedTaskResult {
            task_id: task.id.clone(),
            parent_score_micros: 500_000,
            candidate_score_micros: 900_000,
            parent_passed: true,
            candidate_passed: true,
            parent_execution_id: format!("claimed-parent-{}", task.id),
            candidate_execution_id: format!("claimed-candidate-{}", task.id),
            grader_receipt_digest: d(&format!("claimed-grade-{}", task.id)),
        })
        .collect();
    DevelopmentRunReport {
        request_id: request.request_id,
        manifest_digest: request.manifest.digest,
        parent_bundle_digest: request.parent_bundle_digest,
        candidate_bundle_digest: request.candidate_bundle_digest,
        environment_digest: request.environment_digest,
        grader_digest: request.grader_digest,
        results,
        execution_receipt_id: "claimed-development-run".into(),
        usage_record_ids: vec!["claimed-development-usage".into()],
        provenance,
    }
}

/// The improving report an optimizer fixture would hand over, honest about being a
/// fixture.
struct FixtureRunner;

#[async_trait]
impl DevRunner for FixtureRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Ok(unreceipted_report(
            request,
            DevelopmentExecutionProvenance::Fixture,
        ))
    }
}

/// The same report claiming a registered pure-function execution: a forged claim,
/// there is no receipt behind it.
struct ClaimingRunner;

#[async_trait]
impl DevRunner for ClaimingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Ok(unreceipted_report(
            request,
            DevelopmentExecutionProvenance::RegisteredPureFunction,
        ))
    }
}

/// Wraps a runner: counts its calls and, if asked, forges its report after the
/// honest run (the `ForgingRunner` of `practice_v42`).
struct Spy<R> {
    inner: R,
    forge: fn(&mut DevelopmentRunReport),
    calls: AtomicUsize,
}

impl<R> Spy<R> {
    fn new(inner: R) -> Self {
        Self::forging(inner, |_| {})
    }

    fn forging(inner: R, forge: fn(&mut DevelopmentRunReport)) -> Self {
        Self {
            inner,
            forge,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl<R: DevRunner> DevRunner for Spy<R> {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut report = self.inner.run(request).await?;
        (self.forge)(&mut report);
        Ok(report)
    }
}

/// A runner whose parent never answers a task correctly and whose candidate
/// answers the tasks `candidate_correct` names correctly. It issues honest E03
/// evidence (budget rows, settlements, execution and grader receipts through the
/// public issuing functions, the run receipt as a typed envelope), so the gate
/// verifies it exactly like a registered runner's; only the outputs, and so the
/// scores, differ between the sides. The deterministic registered runner cannot do
/// that (`practice_v42`'s `ContrastRunner` is the same device).
struct ContrastRunner {
    store: Store,
    executor: Context,
    grader: Context,
    candidate_correct: fn(&str) -> bool,
}

impl ContrastRunner {
    async fn issue_side(
        &self,
        control: &DevelopmentControlV1,
        request: &DevelopmentRunRequest,
        task: &DevelopmentTaskSpecV1,
        side: DevelopmentSide,
        output: &str,
    ) -> Result<DevelopmentExecutionReceiptV1> {
        let bundle_digest = match side {
            DevelopmentSide::Parent => request.parent_bundle_digest.clone(),
            DevelopmentSide::Candidate => request.candidate_bundle_digest.clone(),
        };
        let request_digest = execution_request_digest(control, request, task, side)?;
        let call_id = execution_budget_call_id(&request.request_id, &task.task_id, side)?;
        let stored = RegisteredExecutionRequestV1 {
            schema_version: REGISTERED_EXECUTION_REQUEST_SCHEMA.into(),
            control_id: control.id.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            task_id: task.task_id.clone(),
            side,
            input: task.input,
            input_digest: task.input_digest.clone(),
            bundle_digest,
            environment_digest: request.environment_digest.clone(),
            request_digest: request_digest.clone(),
            target_id: task.target_id.clone(),
            target_digest: control.target_digest.clone(),
            runner_digest: control.runner_digest.clone(),
            source_ids: control.source_ids.clone(),
        };
        let call = self
            .store
            .reserve_budget_call_with_sources(
                &self.executor,
                &BudgetCallReservation {
                    billing_scope: control.billing_scope.clone(),
                    call_id: call_id.clone(),
                    dispatch_group_id: request.episode_id.clone(),
                    stage: BudgetStage::DevelopmentExecution,
                    actual_input_digest: request_digest.clone(),
                    request_artifact: Some(BudgetArtifact::from_serializable(
                        REGISTERED_EXECUTION_REQUEST_SCHEMA,
                        &stored,
                    )?),
                    max_cost_micros: 1,
                    lease_token: format!("lease-{}", &fingerprint(&call_id)?[..32]),
                    lease_until: 700,
                    now: 100,
                },
                &control.source_ids,
            )
            .await?;
        let fence = BudgetCallFence {
            billing_scope: control.billing_scope.clone(),
            call_id: call_id.clone(),
            actual_input_digest: request_digest.clone(),
            lease_token: call.lease_token.clone(),
            lease_epoch: call.lease_epoch,
            now: 101,
        };
        let dispatch_id = self
            .store
            .begin_budget_dispatch(&self.executor, &fence)
            .await?
            .call
            .dispatch_id
            .ok_or(Error::Internal)?;
        let output_digest = hash(output.as_bytes());
        self.store
            .settle_registered_execution_call(
                &self.executor,
                &fence,
                &UsageCharge {
                    amount_micros: 0,
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    provider_request_id: format!("registered-pure-function:{dispatch_id}"),
                    usage_record_id: format!("usage-{}", &fingerprint(&call_id)?[..32]),
                    output_digest: output_digest.clone(),
                },
                &RegisteredExecutionSettlement {
                    schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
                    provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
                    call_id: call_id.clone(),
                    dispatch_id,
                    request_digest,
                    output_digest,
                    target_id: task.target_id.clone(),
                    target_digest: control.target_digest.clone(),
                    runner_digest: control.runner_digest.clone(),
                },
            )
            .await?;
        issue_execution_receipt(
            &self.executor,
            &self.store,
            ExecutionReceiptIssueRequest {
                control_id: control.id.clone(),
                request: request.clone(),
                task_id: task.task_id.clone(),
                side,
                output_utf8: output.into(),
                budget_call_id: call_id,
                issued_at_unix_seconds: 100,
            },
        )
        .await
    }
}

#[async_trait]
impl DevRunner for ContrastRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        let control = load_development_control(&self.store, &self.executor, CONTROL_ID).await?;
        let mut results = Vec::new();
        let mut execution_ids = Vec::new();
        let mut grader_ids = Vec::new();
        let mut call_ids = Vec::new();
        let mut usage_ids = Vec::new();
        for manifest_task in &request.manifest.tasks {
            let task = control.task(&manifest_task.id)?;
            let candidate_output = if (self.candidate_correct)(&task.task_id) {
                format!("{{\"answer\":{}}}", task.expected_answer_json)
            } else {
                WRONG_ANSWER.to_string()
            };
            let parent = self
                .issue_side(
                    &control,
                    &request,
                    task,
                    DevelopmentSide::Parent,
                    WRONG_ANSWER,
                )
                .await?;
            let candidate = self
                .issue_side(
                    &control,
                    &request,
                    task,
                    DevelopmentSide::Candidate,
                    &candidate_output,
                )
                .await?;
            let grader = issue_grader_receipt(
                &self.grader,
                &self.store,
                GraderReceiptIssueRequest {
                    control_id: control.id.clone(),
                    request: request.clone(),
                    task_id: task.task_id.clone(),
                    parent_execution_receipt_id: parent.receipt_id.clone(),
                    candidate_execution_receipt_id: candidate.receipt_id.clone(),
                    scoring_budget_call_id: None,
                    issued_at_unix_seconds: 100,
                },
            )
            .await?;
            results.push(PairedTaskResult {
                task_id: task.task_id.clone(),
                parent_score_micros: grader.parent_score_micros,
                candidate_score_micros: grader.candidate_score_micros,
                parent_passed: grader.parent_passed,
                candidate_passed: grader.candidate_passed,
                parent_execution_id: parent.receipt_id.clone(),
                candidate_execution_id: candidate.receipt_id.clone(),
                grader_receipt_digest: fingerprint(&grader)?,
            });
            for receipt in [&parent, &candidate] {
                execution_ids.push(receipt.receipt_id.clone());
                call_ids.push(receipt.budget_call_id.clone());
                usage_ids.push(receipt.usage_record_id.clone());
            }
            grader_ids.push(grader.receipt_id.clone());
        }
        let run_id = run_receipt_id(&request.request_id)?;
        let run = DevelopmentRunReceiptV1 {
            schema_version: DEVELOPMENT_RUN_RECEIPT_SCHEMA.into(),
            id: run_id.clone(),
            control_id: control.id.clone(),
            namespace: control.namespace.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            execution_receipt_ids: execution_ids.clone(),
            grader_receipt_ids: grader_ids,
            budget_call_ids: call_ids,
            usage_record_ids: usage_ids.clone(),
            executor_actor: self.executor.actor().into(),
            issued_at_unix_seconds: 100,
        };
        let run_storage = storage_id(RUN_RECEIPT_KIND, &run_id)?;
        let mut session = self.store.session().await?;
        session
            .put(
                &self.executor,
                "artifact",
                &run_storage,
                self.executor.actor(),
                &json!({
                    "schema_version": "rsia.typed_artifact_envelope.v1",
                    "id": run_storage,
                    "record_kind": RUN_RECEIPT_KIND,
                    "payload": run,
                }),
            )
            .await?;
        for source in &control.source_ids {
            session
                .put_edge(&self.executor, "artifact", &run_storage, "run", source)
                .await?;
        }
        for id in &execution_ids {
            session
                .put_edge(
                    &self.executor,
                    "artifact",
                    &run_storage,
                    "artifact",
                    &storage_id(EXECUTION_RECEIPT_KIND, id)?,
                )
                .await?;
        }
        session.commit().await?;
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results,
            execution_receipt_id: run_id,
            usage_record_ids: usage_ids,
            provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
        })
    }
}

/// A journal that never finishes the commit of a fact of kind `hang_on`. Dropping
/// the `run_next` future once it was entered simulates a crash at that point: the
/// dispatch stays claimed and the step has no terminal fact. `StepPrepared` is the
/// first fact of a step (nothing was dispatched yet); `DevelopmentObserved` comes
/// after the runner, whose report the recovery journal has already saved.
struct HangingJournal {
    inner: StoreOptimizationJournal,
    hang_on: StageFactKind,
    entered: Arc<Notify>,
}

impl HangingJournal {
    fn new(store: &Store, hang_on: StageFactKind) -> (Self, Arc<Notify>) {
        let entered = Arc::new(Notify::new());
        let journal = Self {
            inner: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            hang_on,
            entered: entered.clone(),
        };
        (journal, entered)
    }
}

#[async_trait]
impl OptimizationJournal for HangingJournal {
    async fn commit(&self, fact: StageFact) -> Result<()> {
        if fact.kind == self.hang_on {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        self.inner.commit(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(artifact_id).await
    }

    async fn claim(&self, fact: StageFact) -> Result<bool> {
        self.inner.claim(fact).await
    }

    async fn verify_sources(
        &self,
        selection: &SourceSelection,
        evidence: &EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> Result<()> {
        self.inner
            .verify_sources(selection, evidence, bindings, traces, watermark)
            .await
    }

    async fn check_live(&self, source_ids: &[String], watermark: u64) -> Result<()> {
        self.inner.check_live(source_ids, watermark).await
    }
}

// ---------------------------------------------------------------------------
// The step request
// ---------------------------------------------------------------------------

/// The skill and bundle a step is run against: the world's own for a `Widen`, the
/// parent node's candidate for a `Deepen`.
struct Parent {
    skill: SkillSnapshot,
    edit_context: TrustedEditContext,
    bundle: String,
}

/// Everything an `OptimizationStepRequest` borrows besides its parent.
struct Fixture {
    evidence: EvidenceSet,
    source_selection: SourceSelection,
    bindings: Vec<TrustedSourceBinding>,
    profile: Profile,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
    allowed: Vec<EvidenceRef>,
    manifest: DevelopmentManifest,
}

impl Fixture {
    fn new() -> Self {
        let evidence = EvidenceSet::build(
            "evidence",
            vec![
                EvidenceMember {
                    source_id: RUNS[0].into(),
                    content_digest: hash(RUNS[0].as_bytes()),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                EvidenceMember {
                    source_id: RUNS[1].into(),
                    content_digest: hash(RUNS[1].as_bytes()),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
            ],
            SourceCoverage {
                discovery_exhausted: true,
                files_known: true,
                parsed_ok: 2,
                bytes_read: 22,
                ..SourceCoverage::default()
            },
            ["family-a".into(), "family-b".into()].into_iter().collect(),
        )
        .unwrap();
        Self {
            evidence,
            source_selection: SourceSelection {
                roots: vec![],
                run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            },
            bindings: vec![
                TrustedSourceBinding {
                    source_id: RUNS[0].into(),
                    source_digest: hash(RUNS[0].as_bytes()),
                    parent_family: "family-a".into(),
                },
                TrustedSourceBinding {
                    source_id: RUNS[1].into(),
                    source_digest: hash(RUNS[1].as_bytes()),
                    parent_family: "family-b".into(),
                },
            ],
            profile: Profile {
                id: "profile".into(),
                evolution_enabled: true,
                parent_digest: d("approved-parent"),
                baseline_digest: d("baseline"),
            },
            baseline: parent_skill(),
            parent_strategy: Strategy::default(),
            baseline_strategy: Strategy::default(),
            improver: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
            allowed: vec![
                EvidenceRef {
                    id: RUNS[0].into(),
                    digest: hash(RUNS[0].as_bytes()),
                },
                EvidenceRef {
                    id: RUNS[1].into(),
                    digest: hash(RUNS[1].as_bytes()),
                },
            ],
            manifest: control().manifest().unwrap(),
        }
    }

    fn parent(&self, skill: SkillSnapshot, bundle: String) -> Parent {
        let edit_context = TrustedEditContext::new(
            NAMESPACE,
            "profile",
            "skill",
            "v1",
            d("approved-parent"),
            d("baseline"),
            &skill,
            self.allowed.clone(),
        )
        .unwrap();
        Parent {
            skill,
            edit_context,
            bundle,
        }
    }

    /// The world's own parent: what a `Widen` is requested against.
    fn root_parent(&self) -> Parent {
        self.parent(parent_skill(), d("parent-bundle"))
    }

    /// The parent a `Deepen` of a first-step candidate is requested against: the
    /// root skill with the fixture model's two edits applied, and the candidate
    /// bundle the node recorded.
    fn evolved_parent(&self, bundle: String) -> Parent {
        let mut skill = parent_skill();
        skill.content.push_str(" [repair]");
        skill.applicability.push_str(" [repair]");
        self.parent(skill, bundle)
    }

    fn request<'a>(
        &'a self,
        parent: &'a Parent,
        world_id: &str,
        step: u32,
        tag: &str,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.bindings,
            traces: vec![
                trace(RUNS[0], "family-a", TraceOutcome::TaskFailure),
                trace(RUNS[1], "family-b", TraceOutcome::Success),
            ],
            model_context: ModelRequestContext {
                request_id: format!("optimizer-{tag}"),
                namespace: NAMESPACE.into(),
                purpose: Purpose::Development,
                stage: ModelStage::ReflectFailure,
                episode_id: world_id.into(),
                step,
                attempt: 1,
                parent_skill_digest: skill_snapshot_digest(&parent.skill).unwrap(),
                bundle_digest: parent.bundle.clone(),
                source_closure: self.allowed.clone(),
                model_digest: d("model"),
                tools_digest: d("tools"),
                rules_digest: d("rules"),
                sampling_digest: d("sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &parent.skill,
            edit_context: &parent.edit_context,
            edit_batch_template: SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: NAMESPACE.into(),
                profile_id: "profile".into(),
                skill_id: "skill".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(&parent.skill).unwrap(),
                approved_parent_digest: d("approved-parent"),
                safe_baseline_digest: d("baseline"),
                evidence: EvidenceClosure {
                    support: vec![],
                    counterexamples: vec![],
                    dependencies: vec![],
                },
                edits: vec![],
            },
            protected_ranges: &[],
            bundle_context: BundleCompileContext {
                profile: &self.profile,
                baseline: &self.baseline,
                parent_strategy: &self.parent_strategy,
                baseline_strategy: &self.baseline_strategy,
                improver_patch: &self.improver,
                caps: &self.caps,
                revoked: &self.revoked,
            },
            development_request: DevelopmentRunRequest {
                request_id: format!("dev-{tag}"),
                namespace: NAMESPACE.into(),
                purpose: Purpose::Development,
                episode_id: world_id.into(),
                step,
                attempt: 1,
                manifest: self.manifest.clone(),
                parent_bundle_digest: parent.bundle.clone(),
                candidate_bundle_digest: d(&format!("candidate-{tag}")),
                environment_digest: d("environment"),
                grader_digest: fingerprint(&grader_spec()).unwrap(),
                rules_digest: d("rules"),
                tools_digest: d("tools"),
                revoke_watermark: 1,
                idempotency_key: format!("dev-idempotency-{tag}"),
            },
            allow_rank_call: false,
        }
    }
}

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    model: CountingModel,
    fixture: Fixture,
    root: Parent,
}

impl Env {
    async fn new(with_control: bool) -> Self {
        let (dir, store) = seeded_store(with_control).await;
        let fixture = Fixture::new();
        let root = fixture.root_parent();
        Self {
            coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
            journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            model: CountingModel::default(),
            fixture,
            root,
            store,
            _dir: dir,
        }
    }

    async fn register(&self, id: &str) {
        self.coordinator
            .register_world(world_for(id))
            .await
            .unwrap();
    }

    fn registered_runner(&self) -> RegisteredDevelopmentRunner {
        RegisteredDevelopmentRunner::with_clock(
            self.store.clone(),
            Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap(),
            Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap(),
            CONTROL_ID,
            600,
            Arc::new(|| 100),
        )
        .unwrap()
    }

    fn contrast_runner(&self, candidate_correct: fn(&str) -> bool) -> ContrastRunner {
        ContrastRunner {
            store: self.store.clone(),
            executor: Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap(),
            grader: Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap(),
            candidate_correct,
        }
    }

    /// One real dispatch through `run_next`.
    async fn run(
        &self,
        world: &str,
        step: u32,
        tag: &str,
        parent: &Parent,
        runner: &dyn DevRunner,
    ) -> CoordinatorStepResult {
        self.coordinator
            .run_next(
                Some(&self.model),
                Some(runner),
                Some(&self.journal),
                self.fixture.request(parent, world, step, tag),
            )
            .await
            .unwrap()
    }

    /// The same request again, with no port: the coordinator can only return what the
    /// dispatch fact stored.
    async fn reconnect(
        &self,
        coordinator: &PersistentCoordinator,
        world: &str,
        step: u32,
        tag: &str,
    ) -> CoordinatorStepResult {
        coordinator
            .run_next(
                None,
                None,
                None,
                self.fixture.request(&self.root, world, step, tag),
            )
            .await
            .unwrap()
    }
}

// ---------------------------------------------------------------------------
// Reading what the coordinator stored
// ---------------------------------------------------------------------------

fn exploration_storage_id(record_kind: &str, id: &str) -> String {
    format!("e09-{}", fingerprint(&(record_kind, id)).unwrap())
}

async fn raw_record(store: &Store, record_kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value: Value = session
        .need(
            &worker(),
            "artifact",
            &exploration_storage_id(record_kind, id),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
}

async fn put_raw_record(store: &Store, record_kind: &str, id: &str, value: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(
            &worker(),
            "artifact",
            &exploration_storage_id(record_kind, id),
            "worker",
            value,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn payload(store: &Store, record_kind: &str, id: &str) -> Value {
    raw_record(store, record_kind, id).await["payload"].clone()
}

/// Rewrites one field set of a stored node (status and label) the way a store
/// holding another record would.
async fn rewrite_node(store: &Store, node_id: &str, edit: impl FnOnce(&mut Value)) {
    let mut node = raw_record(store, NODE, node_id).await;
    edit(&mut node["payload"]);
    put_raw_record(store, NODE, node_id, &node).await;
}

/// What a step left behind, as one comparable value: the stored node and the
/// stored dispatch fact, without any id.
#[derive(Debug, PartialEq)]
struct Seen {
    outcome: String,
    status: Value,
    node_evidence: Value,
    names_candidate: bool,
    fact_state: Value,
    fact_evidence: Value,
    fact_reason: Value,
}

async fn seen(env: &Env, step: &CoordinatorStepResult) -> Seen {
    let node = payload(&env.store, NODE, step.node_id.as_deref().unwrap()).await;
    let fact = payload(&env.store, DISPATCH, step.dispatch_id.as_deref().unwrap()).await;
    Seen {
        outcome: step.outcome.clone(),
        status: node["node"]["status"].clone(),
        node_evidence: node["evidence"].clone(),
        names_candidate: [
            "candidate_bundle_digest",
            "candidate_skill_digest",
            "development_selection_digest",
        ]
        .iter()
        .all(|field| node[*field].is_string()),
        fact_state: fact["state"].clone(),
        fact_evidence: fact["evidence"].clone(),
        fact_reason: fact["outcome_reason"].clone(),
    }
}

/// A terminal failure of the node: a `HardFailure` the dispatch has consumed, with
/// the label and the fixed reason, whatever the step ran.
fn assert_terminal_failure(seen: &Seen, label: &str, reason: &str) {
    assert_eq!(seen.outcome, reason);
    assert_eq!(seen.status, json!({"status": "hard_failure"}));
    assert_eq!(seen.node_evidence, json!(label));
    assert_eq!(
        seen.fact_state,
        json!("observed"),
        "the dispatch is consumed"
    );
    assert_eq!(seen.fact_evidence, json!(label));
    assert_eq!(seen.fact_reason, json!(reason));
}

fn dispatched_seqs(action: &BatchActionV1) -> Vec<u32> {
    match action {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs.clone(),
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

/// Every stage fact of one episode and step, read from the store.
async fn stage_facts(store: &Store, episode: &str, step: u32) -> Vec<StageFact> {
    let mut session = store.session().await.unwrap();
    let values = session.list::<Value>(&worker(), "artifact").await.unwrap();
    session.commit().await.unwrap();
    values
        .into_iter()
        .filter(|value| {
            value.get("schema_version").and_then(Value::as_str)
                == Some(OPTIMIZATION_STAGE_FACT_SCHEMA)
        })
        .map(|value| serde_json::from_value::<StageFact>(value).unwrap())
        .filter(|fact| fact.episode_id == episode && fact.step == step)
        .collect()
}

async fn the_fact(store: &Store, episode: &str, step: u32, kind: StageFactKind) -> StageFact {
    let mut found: Vec<_> = stage_facts(store, episode, step)
        .await
        .into_iter()
        .filter(|fact| fact.kind == kind)
        .collect();
    assert_eq!(found.len(), 1, "{kind:?} of {episode} step {step}");
    found.remove(0)
}

/// E03's own gate, asked directly about the development stage a step journaled: the
/// verdict the coordinator's gate call had to reach.
async fn gate_verdict(
    store: &Store,
    episode: &str,
    step: u32,
) -> Result<VerifiedDevelopmentObservationView> {
    let request = the_fact(
        store,
        episode,
        step,
        StageFactKind::DevelopmentRequestPrepared,
    )
    .await;
    let observed = the_fact(store, episode, step, StageFactKind::DevelopmentObserved).await;
    verify_development_observation(store, &admin(), &request.artifact_id, &observed.artifact_id)
        .await
}

async fn stored_report(store: &Store, episode: &str, step: u32) -> DevelopmentRunReport {
    let observed = the_fact(store, episode, step, StageFactKind::DevelopmentObserved).await;
    serde_json::from_value(observed.payload).unwrap()
}

// ---------------------------------------------------------------------------
// 1. A Fixture report
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fixture_report_is_a_labelled_terminal_failure_that_drives_no_successor() {
    let env = Env::new(false).await;
    env.register("world-fixture").await;
    let runner = Spy::new(FixtureRunner);
    let first = env
        .run("world-fixture", 1, "fixture-1", &env.root, &runner)
        .await;
    assert_eq!(runner.calls(), 1, "the step ran and was paid for once");
    let first_seen = seen(&env, &first).await;
    assert_terminal_failure(&first_seen, "fixture_declared", FIXTURE_REASON);
    assert!(
        first_seen.names_candidate,
        "the node still names the candidate it dispatched"
    );
    let node_id = first.node_id.clone().unwrap();

    // The gate would refuse a Fixture report too; the coordinator did not need to
    // ask, and the report it left is exactly what the gate refuses.
    assert!(matches!(
        gate_verdict(&env.store, "world-fixture", 1).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        stored_report(&env.store, "world-fixture", 1)
            .await
            .provenance,
        DevelopmentExecutionProvenance::Fixture
    );

    // The dispatch is a real use that ended observed, and says what evidence it had.
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-fixture")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Observed);
    assert_eq!(usage[0].evidence(), ExplorationEvidenceV1::FixtureDeclared);
    assert_eq!(
        serde_json::to_value(&usage[0]).unwrap()["evidence"],
        "fixture_declared"
    );

    // The node cannot be deepened: the only legal action is the other root.
    let next = env.coordinator.decide_next("world-fixture").await.unwrap();
    assert_eq!(dispatched_seqs(&next.action), vec![2]);
    // And it is no final candidate.
    match env
        .coordinator
        .final_candidate_request("world-fixture", &node_id)
        .await
    {
        Err(Error::Conflict(message)) => {
            assert!(
                message.contains("not a valid observed candidate"),
                "{message}"
            );
        }
        other => panic!("a fixture node is no final candidate, got {other:?}"),
    }

    // A reconnect, from this coordinator and from a new one over the same store,
    // returns the same terminal state without reaching a port.
    let model_calls = env.model.calls();
    let resumed = PersistentCoordinator::new(env.store.clone(), worker(), "worker").unwrap();
    for coordinator in [&env.coordinator, &resumed] {
        let again = env
            .reconnect(coordinator, "world-fixture", 1, "fixture-1")
            .await;
        assert_eq!(again.node_id, first.node_id);
        assert_eq!(again.dispatch_id, first.dispatch_id);
        assert_eq!(again.outcome, FIXTURE_REASON);
    }
    assert_eq!(runner.calls(), 1);
    assert_eq!(env.model.calls(), model_calls);
    assert_eq!(seen(&env, &first).await, first_seen);
}

// ---------------------------------------------------------------------------
// 2. A forged claim of a registered execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_claim_without_receipts_is_refused_and_the_consumed_dispatch_never_blocks_the_world() {
    // A registered control exists, so the claim is refused for what it lacks, not
    // for there being nothing to check it against.
    let env = Env::new(true).await;
    env.register("world-claim").await;
    let runner = Spy::new(ClaimingRunner);
    let first = env
        .run("world-claim", 1, "claim-1", &env.root, &runner)
        .await;
    assert_eq!(runner.calls(), 1);
    let first_seen = seen(&env, &first).await;
    assert_terminal_failure(&first_seen, "evidence_rejected", REJECTED_REASON);
    assert!(first_seen.names_candidate);

    // What the gate says about the same facts: no such receipt.
    assert!(matches!(
        gate_verdict(&env.store, "world-claim", 1).await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        stored_report(&env.store, "world-claim", 1).await.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );

    let usage = env
        .coordinator
        .verified_mechanism_usage("world-claim")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].evidence(), ExplorationEvidenceV1::EvidenceRejected);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Observed);

    // The dispatch was consumed, not stranded: the same request returns the same
    // terminal state (not "dispatch idempotency conflict") and reaches no port.
    let model_calls = env.model.calls();
    let again = env
        .reconnect(&env.coordinator, "world-claim", 1, "claim-1")
        .await;
    assert_eq!(again.node_id, first.node_id);
    assert_eq!(again.dispatch_id, first.dispatch_id);
    assert_eq!(again.outcome, REJECTED_REASON);
    assert_eq!(runner.calls(), 1);
    assert_eq!(env.model.calls(), model_calls);

    // The refused node drives nothing, and the world goes on with the other root.
    let node_id = first.node_id.clone().unwrap();
    assert!(matches!(
        env.coordinator
            .final_candidate_request("world-claim", &node_id)
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        dispatched_seqs(
            &env.coordinator
                .decide_next("world-claim")
                .await
                .unwrap()
                .action
        ),
        vec![2]
    );
    let honest = env.registered_runner();
    let second = env
        .run("world-claim", 2, "claim-2", &env.root, &honest)
        .await;
    assert_ne!(second.node_id, first.node_id);
    assert_ne!(second.dispatch_id, first.dispatch_id);
    assert_eq!(second.outcome, KEEP_INCUMBENT_REASON);

    // Whatever else a runner claims, it is the receipts the gate reloads: a report
    // that is not a fixture and has none is refused just the same.
    env.register("world-isolated").await;
    let isolated = Spy::forging(ClaimingRunner, |report| {
        report.provenance = DevelopmentExecutionProvenance::IsolatedRunner;
    });
    let step = env
        .run("world-isolated", 1, "isolated-1", &env.root, &isolated)
        .await;
    assert_terminal_failure(
        &seen(&env, &step).await,
        "evidence_rejected",
        REJECTED_REASON,
    );
}

// ---------------------------------------------------------------------------
// 3. The registered runner, and a candidate with real receipts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_registered_runner_executes_both_sides_alike_so_it_never_yields_a_candidate() {
    let env = Env::new(true).await;
    env.register("world-registered").await;
    let runner = Spy::new(env.registered_runner());
    let step = env
        .run("world-registered", 1, "registered-1", &env.root, &runner)
        .await;
    assert_eq!(runner.calls(), 1);

    // Both sides ran the same registered function on the same input: equal scores,
    // so the selection keeps the incumbent and there is no candidate to judge.
    let report = stored_report(&env.store, "world-registered", 1).await;
    assert_eq!(
        report.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    assert!(!report.results.is_empty());
    for result in &report.results {
        assert_eq!(result.parent_score_micros, result.candidate_score_micros);
        assert_eq!(result.parent_score_micros, 1_000_000);
        assert!(result.parent_passed && result.candidate_passed);
    }
    // The evidence itself is genuine: the gate accepts it when asked.
    gate_verdict(&env.store, "world-registered", 1)
        .await
        .unwrap();

    // The node is the plain failure of a step without a candidate. The gate was not
    // misused on it: the label is not a verdict, and the reason is the selection's,
    // not the refusal literal.
    let step_seen = seen(&env, &step).await;
    assert_eq!(step_seen.outcome, KEEP_INCUMBENT_REASON);
    assert_eq!(step_seen.status, json!({"status": "hard_failure"}));
    assert_eq!(step_seen.node_evidence, json!("not_observed"));
    assert_eq!(step_seen.fact_state, json!("observed"));
    assert_eq!(step_seen.fact_evidence, json!("not_observed"));
    assert_eq!(step_seen.fact_reason, json!(KEEP_INCUMBENT_REASON));
    assert!(!step_seen.names_candidate);
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-registered")
            .await
            .unwrap()[0]
            .evidence(),
        ExplorationEvidenceV1::NotObserved
    );
}

#[tokio::test]
async fn a_candidate_with_real_receipts_is_a_trusted_valid_node_that_can_be_deepened_and_proposed()
{
    let env = Env::new(true).await;
    env.register("world-trusted").await;
    // The candidate answers every task correctly, the parent none: a strict
    // improvement, backed by receipts the gate reloads.
    let runner = Spy::new(env.contrast_runner(|_| true));
    let first = env
        .run("world-trusted", 1, "trusted-1", &env.root, &runner)
        .await;
    assert_eq!(first.outcome, OBSERVED_REASON);
    let first_seen = seen(&env, &first).await;
    assert_eq!(
        first_seen.status,
        json!({"status": "valid", "quality_micros": 1_000_000})
    );
    assert_eq!(first_seen.node_evidence, json!("trusted"));
    assert!(first_seen.names_candidate);
    assert_eq!(first_seen.fact_state, json!("observed"));
    assert_eq!(first_seen.fact_evidence, json!("trusted"));
    assert_eq!(first_seen.fact_reason, json!(OBSERVED_REASON));

    // E03's own gate verifies the very facts the coordinator's call used, and the
    // recomputed scores are what the node's quality is the mean of.
    let view = gate_verdict(&env.store, "world-trusted", 1).await.unwrap();
    assert_eq!(view.outcomes.len(), 2);
    for outcome in &view.outcomes {
        assert_eq!(outcome.parent_score_micros, 0);
        assert_eq!(outcome.candidate_score_micros, 1_000_000);
    }
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-trusted")
        .await
        .unwrap();
    assert_eq!(usage[0].evidence(), ExplorationEvidenceV1::Trusted);
    // A reconnect returns the stored dispatch: same node, same outcome, no new call.
    let again = env
        .reconnect(&env.coordinator, "world-trusted", 1, "trusted-1")
        .await;
    assert_eq!(again.node_id, first.node_id);
    assert_eq!(again.outcome, OBSERVED_REASON);
    assert_eq!(runner.calls(), 1);

    // A trusted valid node is a final candidate ...
    let node_id = first.node_id.clone().unwrap();
    let node = payload(&env.store, NODE, &node_id).await;
    let selected = env
        .coordinator
        .final_candidate_request("world-trusted", &node_id)
        .await
        .unwrap();
    assert_eq!(selected.selected_node_id, node_id);
    assert_eq!(
        json!(selected.selected_bundle_digest),
        node["candidate_bundle_digest"]
    );
    assert!(selected.requires_recompile_against_approved_parent);
    assert!(selected.requires_independent_formal_evaluation);

    // ... and the world deepens it: the gain over the baseline is significant.
    let next = env.coordinator.decide_next("world-trusted").await.unwrap();
    assert_eq!(dispatched_seqs(&next.action), vec![1_000_002]);
    let evolved = env
        .fixture
        .evolved_parent(selected.selected_bundle_digest.clone());
    let second = env
        .run("world-trusted", 2, "trusted-2", &evolved, &runner)
        .await;
    assert_eq!(second.outcome, OBSERVED_REASON);
    let second_node = payload(&env.store, NODE, second.node_id.as_deref().unwrap()).await;
    assert_eq!(
        second_node["node"]["status"],
        json!({"status": "valid", "quality_micros": 1_000_000})
    );
    assert_eq!(second_node["evidence"], "trusted");
    assert_eq!(second_node["node"]["search_parent_seq"], 1);
    assert_eq!(second_node["node"]["depth"], 2);
    env.coordinator
        .final_candidate_request("world-trusted", second.node_id.as_deref().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_node_quality_is_the_mean_of_the_scores_the_gate_recomputed() {
    let env = Env::new(true).await;
    env.register("world-partial").await;
    // The candidate answers task-a only: 1000000 and 0, so the mean is 500000.
    let runner = env.contrast_runner(|task| task == "task-a");
    let step = env
        .run("world-partial", 1, "partial-1", &env.root, &runner)
        .await;
    assert_eq!(step.outcome, OBSERVED_REASON);
    let step_seen = seen(&env, &step).await;
    assert_eq!(
        step_seen.status,
        json!({"status": "valid", "quality_micros": 500_000})
    );
    assert_eq!(step_seen.node_evidence, json!("trusted"));
    let view = gate_verdict(&env.store, "world-partial", 1).await.unwrap();
    let total: u64 = view
        .outcomes
        .iter()
        .map(|outcome| u64::from(outcome.candidate_score_micros))
        .sum();
    assert_eq!(total / view.outcomes.len() as u64, 500_000);
    // The mean equals the world's baseline, so there is no gain: that does not matter
    // for what the node is. It is valid and trusted, so it can be proposed.
    env.coordinator
        .final_candidate_request("world-partial", step.node_id.as_deref().unwrap())
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 4. Scores the receipts do not bear out
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_report_whose_scores_differ_from_its_receipts_is_rejected_whatever_it_claims() {
    let env = Env::new(true).await;

    // The parent's score is understated over honest receipts of two equal sides:
    // the report claims an improvement the receipts do not show.
    env.register("world-understated").await;
    let understated = Spy::forging(env.registered_runner(), |report| {
        for result in &mut report.results {
            result.parent_score_micros = 0;
            result.parent_passed = false;
        }
    });
    let step = env
        .run(
            "world-understated",
            1,
            "understated-1",
            &env.root,
            &understated,
        )
        .await;
    // The step trusted the report and found a candidate; the gate did not.
    assert_terminal_failure(
        &seen(&env, &step).await,
        "evidence_rejected",
        REJECTED_REASON,
    );
    assert!(matches!(
        gate_verdict(&env.store, "world-understated", 1).await,
        Err(Error::Conflict(_))
    ));
    // The self-reported 1000000 is not the node's quality, and nothing deepens it.
    assert_eq!(
        dispatched_seqs(
            &env.coordinator
                .decide_next("world-understated")
                .await
                .unwrap()
                .action
        ),
        vec![2]
    );

    // The candidate's score is overstated over honest receipts in which it lost.
    env.register("world-overstated").await;
    let overstated = Spy::forging(env.contrast_runner(|_| false), |report| {
        for result in &mut report.results {
            result.candidate_score_micros = 1_000_000;
            result.candidate_passed = true;
        }
    });
    let step = env
        .run(
            "world-overstated",
            1,
            "overstated-1",
            &env.root,
            &overstated,
        )
        .await;
    assert_terminal_failure(
        &seen(&env, &step).await,
        "evidence_rejected",
        REJECTED_REASON,
    );
    assert!(matches!(
        gate_verdict(&env.store, "world-overstated", 1).await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        env.coordinator
            .final_candidate_request("world-overstated", step.node_id.as_deref().unwrap())
            .await,
        Err(Error::Conflict(_))
    ));

    // A forged Fixture claim over honest receipts is a fixture, not a refusal: the
    // report says what it is, and what it says is never trusted.
    env.register("world-relabelled").await;
    let relabelled = Spy::forging(env.contrast_runner(|_| true), |report| {
        report.provenance = DevelopmentExecutionProvenance::Fixture;
    });
    let step = env
        .run(
            "world-relabelled",
            1,
            "relabelled-1",
            &env.root,
            &relabelled,
        )
        .await;
    assert_terminal_failure(&seen(&env, &step).await, "fixture_declared", FIXTURE_REASON);
}

// ---------------------------------------------------------------------------
// 5. Recovery
// ---------------------------------------------------------------------------

/// Runs one step of `world` with `runner`. With `crash`, the process is lost after
/// the runner's report was saved and before the step observed it, and a new
/// `run_next` finishes the claimed dispatch with a runner that must not be called:
/// the recovery journal replays the saved report.
async fn step_with_or_without_a_crash(
    env: &Env,
    world: &str,
    runner: &Spy<impl DevRunner>,
    replay: &Spy<impl DevRunner>,
    crash: bool,
) -> CoordinatorStepResult {
    env.register(world).await;
    if !crash {
        return env.run(world, 1, "recovery-1", &env.root, runner).await;
    }
    let (journal, entered) = HangingJournal::new(&env.store, StageFactKind::DevelopmentObserved);
    tokio::select! {
        biased;
        result = env.coordinator.run_next(
            Some(&env.model),
            Some(runner),
            Some(&journal),
            env.fixture.request(&env.root, world, 1, "recovery-1"),
        ) => panic!("the hanging journal must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    // The paid step ran once; its dispatch is claimed and no node exists.
    assert_eq!(runner.calls(), 1);
    let stored = raw_record(&env.store, WORLD, world).await;
    let dispatch_ids = stored["payload"]["dispatch_ids"].as_array().unwrap();
    assert_eq!(dispatch_ids.len(), 1);
    let claimed = payload(&env.store, DISPATCH, dispatch_ids[0].as_str().unwrap()).await;
    assert_eq!(claimed["state"], "claimed");
    assert_eq!(claimed["evidence"], "not_observed");
    assert!(stored["payload"]["node_ids"].as_array().unwrap().is_empty());

    let model_calls = env.model.calls();
    let finished = env
        .coordinator
        .run_next(
            Some(&env.model),
            Some(replay),
            Some(&env.journal),
            env.fixture.request(&env.root, world, 1, "recovery-1"),
        )
        .await
        .unwrap();
    // Nothing was paid for again: the report and the model answers were replayed.
    assert_eq!(replay.calls(), 0, "the recovery journal replays the report");
    assert_eq!(env.model.calls(), model_calls);
    finished
}

#[tokio::test]
async fn a_recovered_step_carries_the_same_evidence_label_as_an_uninterrupted_one() {
    let env = Env::new(true).await;

    // An honest candidate: trusted, with the same quality, after the crash.
    let honest = || Spy::new(env.contrast_runner(|_| true));
    let straight =
        step_with_or_without_a_crash(&env, "world-honest-straight", &honest(), &honest(), false)
            .await;
    let recovered =
        step_with_or_without_a_crash(&env, "world-honest-recovered", &honest(), &honest(), true)
            .await;
    let straight_seen = seen(&env, &straight).await;
    assert_eq!(
        straight_seen.status,
        json!({"status": "valid", "quality_micros": 1_000_000})
    );
    assert_eq!(straight_seen.node_evidence, json!("trusted"));
    assert_eq!(seen(&env, &recovered).await, straight_seen);
    gate_verdict(&env.store, "world-honest-recovered", 1)
        .await
        .unwrap();

    // A forged report: refused the first time, refused the same way when replayed.
    fn forge(report: &mut DevelopmentRunReport) {
        for result in &mut report.results {
            result.parent_score_micros = 0;
            result.parent_passed = false;
        }
    }
    let forging = || Spy::forging(env.registered_runner(), forge);
    let straight =
        step_with_or_without_a_crash(&env, "world-forged-straight", &forging(), &forging(), false)
            .await;
    let recovered =
        step_with_or_without_a_crash(&env, "world-forged-recovered", &forging(), &forging(), true)
            .await;
    let straight_seen = seen(&env, &straight).await;
    assert_terminal_failure(&straight_seen, "evidence_rejected", REJECTED_REASON);
    assert_eq!(seen(&env, &recovered).await, straight_seen);
    assert!(matches!(
        gate_verdict(&env.store, "world-forged-recovered", 1).await,
        Err(Error::Conflict(_))
    ));

    // A fixture report likewise.
    let fixture = || Spy::new(FixtureRunner);
    let straight = step_with_or_without_a_crash(
        &env,
        "world-fixture-straight",
        &fixture(),
        &fixture(),
        false,
    )
    .await;
    let recovered = step_with_or_without_a_crash(
        &env,
        "world-fixture-recovered",
        &fixture(),
        &fixture(),
        true,
    )
    .await;
    let straight_seen = seen(&env, &straight).await;
    assert_terminal_failure(&straight_seen, "fixture_declared", FIXTURE_REASON);
    assert_eq!(seen(&env, &recovered).await, straight_seen);
}

// ---------------------------------------------------------------------------
// 6. Records written before the label existed, and the final candidate guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn records_without_an_evidence_label_read_as_not_observed_and_are_no_final_candidate() {
    let env = Env::new(false).await;
    env.register("world-legacy").await;
    let first = env
        .run(
            "world-legacy",
            1,
            "legacy-1",
            &env.root,
            &Spy::new(FixtureRunner),
        )
        .await;
    let node_id = first.node_id.clone().unwrap();
    let dispatch_id = first.dispatch_id.clone().unwrap();

    // Turn both records into what an earlier build stored: no `evidence` field.
    for (kind, id) in [(NODE, node_id.as_str()), (DISPATCH, dispatch_id.as_str())] {
        let mut record = raw_record(&env.store, kind, id).await;
        assert!(record["payload"]["evidence"].is_string(), "{kind}");
        record["payload"]
            .as_object_mut()
            .unwrap()
            .remove("evidence");
        put_raw_record(&env.store, kind, id, &record).await;
    }

    // They read, as `not_observed`, and the stored fact is still the v2 schema.
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-legacy")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].evidence(), ExplorationEvidenceV1::NotObserved);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Observed);
    assert_eq!(
        payload(&env.store, DISPATCH, &dispatch_id).await["schema_version"],
        "rsia.exploration_dispatch.v2"
    );
    let again = env
        .reconnect(&env.coordinator, "world-legacy", 1, "legacy-1")
        .await;
    assert_eq!(again.node_id, first.node_id);
    assert_eq!(again.outcome, FIXTURE_REASON);
    assert_eq!(
        dispatched_seqs(
            &env.coordinator
                .decide_next("world-legacy")
                .await
                .unwrap()
                .action
        ),
        vec![2]
    );

    // A legacy node that was a failure is no final candidate. A legacy node that
    // was `valid` (it rested on whatever the runner reported) is none either: nothing
    // verified it.
    assert!(matches!(
        env.coordinator
            .final_candidate_request("world-legacy", &node_id)
            .await,
        Err(Error::Conflict(_))
    ));
    rewrite_node(&env.store, &node_id, |node| {
        node["node"]["status"] = json!({"status": "valid", "quality_micros": 900_000});
    })
    .await;
    match env
        .coordinator
        .final_candidate_request("world-legacy", &node_id)
        .await
    {
        Err(Error::Conflict(message)) => {
            assert!(message.contains("not_observed"), "{message}");
        }
        other => panic!("a legacy valid node is not trusted evidence, got {other:?}"),
    }

    // A dispatch that was claimed when the old build stopped is finished by this
    // one and gets its label then.
    env.register("world-legacy-claimed").await;
    let model_calls = env.model.calls();
    let (hanging, entered) = HangingJournal::new(&env.store, StageFactKind::StepPrepared);
    tokio::select! {
        biased;
        result = env.coordinator.run_next(
            Some(&env.model),
            Some(&FixtureRunner),
            Some(&hanging),
            env.fixture.request(&env.root, "world-legacy-claimed", 1, "legacy-claimed-1"),
        ) => panic!("the hanging journal must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    assert_eq!(
        env.model.calls(),
        model_calls,
        "the claim came before any model call"
    );
    let world = raw_record(&env.store, WORLD, "world-legacy-claimed").await;
    let claimed_id = world["payload"]["dispatch_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let mut claimed = raw_record(&env.store, DISPATCH, &claimed_id).await;
    assert_eq!(claimed["payload"]["state"], "claimed");
    claimed["payload"]
        .as_object_mut()
        .unwrap()
        .remove("evidence");
    put_raw_record(&env.store, DISPATCH, &claimed_id, &claimed).await;
    let finished = env
        .run(
            "world-legacy-claimed",
            1,
            "legacy-claimed-1",
            &env.root,
            &FixtureRunner,
        )
        .await;
    assert_eq!(finished.dispatch_id.as_deref(), Some(claimed_id.as_str()));
    assert_terminal_failure(
        &seen(&env, &finished).await,
        "fixture_declared",
        FIXTURE_REASON,
    );
}

#[tokio::test]
async fn a_final_candidate_needs_a_valid_node_and_trusted_evidence() {
    let env = Env::new(false).await;
    env.register("world-guard").await;
    let first = env
        .run(
            "world-guard",
            1,
            "guard-1",
            &env.root,
            &Spy::new(FixtureRunner),
        )
        .await;
    let node_id = first.node_id.clone().unwrap();
    let bundle = payload(&env.store, NODE, &node_id).await["candidate_bundle_digest"].clone();

    // Every combination of status and label. Only a valid node with trusted
    // evidence is proposed; the refusal names which of the two is missing.
    let valid = json!({"status": "valid", "quality_micros": 900_000});
    let failure = json!({"status": "hard_failure"});
    let cases = [
        (&failure, "trusted", Some("not a valid observed candidate")),
        (
            &failure,
            "fixture_declared",
            Some("not a valid observed candidate"),
        ),
        (&valid, "fixture_declared", Some("fixture_declared")),
        (&valid, "evidence_rejected", Some("evidence_rejected")),
        (&valid, "not_observed", Some("not_observed")),
        (&valid, "trusted", None),
    ];
    for (status, label, refusal) in cases {
        rewrite_node(&env.store, &node_id, |node| {
            node["node"]["status"] = status.clone();
            node["evidence"] = json!(label);
        })
        .await;
        let result = env
            .coordinator
            .final_candidate_request("world-guard", &node_id)
            .await;
        match (result, refusal) {
            (Err(Error::Conflict(message)), Some(expected)) => {
                assert!(message.contains(expected), "{status} {label}: {message}");
            }
            (Ok(selected), None) => {
                // The positive path of a node stored as a trusted valid one.
                assert_eq!(json!(selected.selected_bundle_digest), bundle);
                assert!(selected.requires_recompile_against_approved_parent);
            }
            (other, _) => panic!("{status} {label}: unexpected {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Outcomes that are no candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_outcome_without_a_candidate_carries_no_verdict() {
    let env = Env::new(false).await;

    // The model fails after the dispatch was claimed: uncertain, nothing for the
    // gate to judge, and the node names no candidate.
    env.register("world-uncertain").await;
    let uncertain = env
        .coordinator
        .run_next(
            Some(&FailingModel),
            Some(&FixtureRunner),
            Some(&env.journal),
            env.fixture
                .request(&env.root, "world-uncertain", 1, "uncertain-1"),
        )
        .await
        .unwrap();
    let node = payload(&env.store, NODE, uncertain.node_id.as_deref().unwrap()).await;
    assert_eq!(node["node"]["status"], json!({"status": "usage_uncertain"}));
    assert_eq!(node["evidence"], "not_observed");
    assert_eq!(node["candidate_bundle_digest"], Value::Null);
    let fact = payload(
        &env.store,
        DISPATCH,
        uncertain.dispatch_id.as_deref().unwrap(),
    )
    .await;
    assert_eq!(fact["state"], "uncertain");
    assert_eq!(fact["evidence"], "not_observed");
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-uncertain")
        .await
        .unwrap();
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Uncertain);
    assert_eq!(usage[0].evidence(), ExplorationEvidenceV1::NotObserved);

    // The model suggests nothing: a plain failure, not a verdict on any evidence.
    env.register("world-nochange").await;
    let nochange = env
        .coordinator
        .run_next(
            Some(&NoChangeModel),
            Some(&FixtureRunner),
            Some(&env.journal),
            env.fixture
                .request(&env.root, "world-nochange", 1, "nochange-1"),
        )
        .await
        .unwrap();
    assert_eq!(nochange.outcome, "model returned no edit suggestions");
    let node = payload(&env.store, NODE, nochange.node_id.as_deref().unwrap()).await;
    assert_eq!(node["node"]["status"], json!({"status": "hard_failure"}));
    assert_eq!(node["evidence"], "not_observed");
    let fact = payload(
        &env.store,
        DISPATCH,
        nochange.dispatch_id.as_deref().unwrap(),
    )
    .await;
    assert_eq!(fact["state"], "observed");
    assert_eq!(fact["evidence"], "not_observed");
}

// ---------------------------------------------------------------------------
// 7. Revocation cleanup
// ---------------------------------------------------------------------------

async fn finish_cleanup(store: &Store, mut status: CleanupStatus, page: usize) {
    let admin = admin();
    for now in 201..4_000 {
        if status.state == CleanupState::Complete {
            break;
        }
        assert_ne!(
            status.state,
            CleanupState::Failed,
            "cleanup blocked: {:?}",
            status.last_error
        );
        status = LifecycleCoordinator::continue_cleanup(&admin, store, &status.job_id, page, now)
            .await
            .unwrap();
    }
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "{:?}",
        status.last_error
    );
}

#[tokio::test]
async fn revoking_a_source_still_redacts_labelled_nodes_and_dispatch_facts() {
    let env = Env::new(false).await;
    env.register("world-revoke").await;
    let fixture = env
        .run(
            "world-revoke",
            1,
            "revoke-1",
            &env.root,
            &Spy::new(FixtureRunner),
        )
        .await;
    let claim = env
        .run(
            "world-revoke",
            2,
            "revoke-2",
            &env.root,
            &Spy::new(ClaimingRunner),
        )
        .await;
    let records = [
        (NODE, fixture.node_id.clone().unwrap(), "fixture_declared"),
        (
            DISPATCH,
            fixture.dispatch_id.clone().unwrap(),
            "fixture_declared",
        ),
        (NODE, claim.node_id.clone().unwrap(), "evidence_rejected"),
        (
            DISPATCH,
            claim.dispatch_id.clone().unwrap(),
            "evidence_rejected",
        ),
    ];

    // Before the revocation each record is a live envelope carrying its label and
    // the candidate it names; remember what the cleanup must remove.
    let mut content = Vec::new();
    for (kind, id, label) in &records {
        let envelope = raw_record(&env.store, kind, id).await;
        assert_eq!(envelope["payload"]["evidence"], *label, "{kind} {id}");
        let text = envelope.to_string();
        let digests: Vec<String> = if *kind == NODE {
            [
                "candidate_bundle_digest",
                "candidate_skill_digest",
                "development_selection_digest",
            ]
            .iter()
            .map(|field| envelope["payload"][*field].as_str().unwrap().to_owned())
            .collect()
        } else {
            vec![
                envelope["payload"]["idempotency_key"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ]
        };
        for needle in &digests {
            assert!(text.contains(needle.as_str()), "{kind} {id}");
        }
        content.push((*kind, id.clone(), digests));
    }

    let started =
        LifecycleCoordinator::revoke_source(&admin(), &env.store, RUNS[0], "privacy", 200)
            .await
            .unwrap();
    finish_cleanup(&env.store, started, 2).await;

    // Each node and dispatch fact is a tombstone that keeps neither its candidate,
    // nor its label, nor the verdict the label recorded.
    for (kind, id, digests) in &content {
        let value = raw_record(&env.store, kind, id).await;
        assert_eq!(value["schema_version"], "rsia.redacted.v1", "{kind} {id}");
        assert_eq!(value["state"], "source_revoked", "{kind} {id}");
        assert!(value.get("payload").is_none(), "{kind} {id}");
        let text = value.to_string();
        for needle in digests
            .iter()
            .map(String::as_str)
            .chain(["fixture_declared", "evidence_rejected"])
        {
            assert!(!text.contains(needle), "{kind} {id} still carries {needle}");
        }
    }

    // The coordinator names the redaction instead of reporting a storage failure.
    let node_id = fixture.node_id.clone().unwrap();
    for error in [
        env.coordinator
            .final_candidate_request("world-revoke", &node_id)
            .await
            .unwrap_err(),
        env.coordinator
            .verified_mechanism_usage("world-revoke")
            .await
            .unwrap_err(),
    ] {
        match error {
            Error::Conflict(message) => assert!(message.contains("redacted"), "{message}"),
            other => panic!("expected a Conflict naming the redaction, got {other:?}"),
        }
    }
}
