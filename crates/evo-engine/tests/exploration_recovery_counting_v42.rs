//! AG-042 (E09 PR-B step B4a; plan §7.1 "at most one dispatched recovery per failed
//! episode", §7.1.1 rule 5, §7.2.1 "the recovery queue is a derived view of the observed
//! failures and the dispatches that really happened, not a fact source that can be edited
//! on its own"): how the coordinator counts recoveries and when it offers one, over a real
//! SQLite store with no provider and zero monetary cost.
//!
//! * the repairs a failure has used are derived, never read from the stored node: the
//!   repairs of an episode are the completed (observed or uncertain) `Recover` dispatch
//!   facts whose failed node belongs to that episode, the recoveries a world has used are
//!   its completed `Recover` facts, and a node's repair failures follow its lineage the way
//!   the replay counts them (a child repeats its parent's count, plus one when a `Recover`
//!   ended in a repairable failure). A `Recover` no longer rewrites the failed node it
//!   repairs, and rewriting the counters a node is stored with changes neither what the
//!   world offers nor what it decides (a repair that was dispatched is not given back);
//! * a `Recover` is offered only for a failure of a kind that can be repaired (compile,
//!   implementation, type, output shape: the kinds `PrefixViewV2::validate` accepts), whose
//!   episode has repairs left and whose own repair failures are below `MAX_REPAIR`, so a
//!   repairable failure of the repair never leaves a prefix the check refuses for good. A
//!   stored failure of another kind (environment, resource, safety, unknown) stays stored
//!   as it is, and is handed to the decision as a hard failure instead of refusing the
//!   whole prefix;
//! * a claimed `Recover` is not spent: it stays legal and resumes, like a claimed `Deepen`;
//! * a completed `Recover` fact is held to its target: the node it repairs is a repairable
//!   failure of a supported kind, as the replay requires. A root dispatch rewritten into
//!   the `Recover` of a hard failure, with the counters that would pay for it, is refused;
//! * the dispatch facts are read only when a repairable failure is stored (by stored status,
//!   of any kind): a world that holds none has no recovery to derive, and its decision does
//!   not depend on its facts being readable; once one is stored every fact the world lists
//!   is read, and one that cannot be read is the reader's own error, never skipped;
//! * the repairs of an episode are counted per episode where the replay counts them per
//!   node. The two differ only when one episode has failures on two nodes (the history
//!   `replay_v41` pins), and a test pins exactly that difference.
//!
//! No step produces a repairable failure yet (B4b), so the failures here are written by
//! hand: a failed root is rewritten into one, as `exploration_legal_actions_v42` does. Not
//! covered (and not claimed): a repairable failure produced by a step, the end to end
//! recovery of a production world, and the NoChange / KeepIncumbent terminal states (B4c).
//! The replay of the same history is run through `run_replay`. The fixtures are copies
//! (the world's own from `exploration_legal_actions_v42.rs` and
//! `exploration_postpaid_v42.rs`, the replay's from `replay_v41.rs`): this file changes none
//! of the files it copies from.
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
use evo_core::replay::{
    DEFAULT_LAMBDA_MICROS, HistoricalUsage, PrefixCoverageV1, REPLAY_MANIFEST_SCHEMA,
    REPLAY_WORLD_SCHEMA, ReplayActionSpecV1, ReplayObjective, ReplayReportV2,
    ReplaySimulationProfile, ReplaySourceRef, ReplayTerminal, ReplayTransitionOutcome,
    ReplayTransitionV2, ReplayWorldManifestV2, ReplayWorldV2, SIMULATION_VERSION, WorldPartition,
    replay_baseline_observation_digest, replay_observation_digest,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind, LegalActionV1,
    LegalActionsV1, ObservedStatus, PrefixNodeV2, PrefixViewV2, SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
use evo_engine::development::{
    DEVELOPMENT_RUN_RECEIPT_SCHEMA, DevelopmentControlV1, DevelopmentEvidenceScope,
    DevelopmentExecutionReceiptV1, DevelopmentRunReceiptV1, DevelopmentSide, DevelopmentTaskSpecV1,
    EXECUTION_RECEIPT_KIND, ExecutionReceiptIssueRequest, GraderReceiptIssueRequest,
    RUN_RECEIPT_KIND, RegisteredExecutionRequestV1, RegisteredTargetInputV1,
    execution_budget_call_id, execution_request_digest, issue_execution_receipt,
    issue_grader_receipt, load_development_control, register_development_control,
    registered_runner_digest, run_receipt_id, storage_id,
};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, PersistentCoordinator,
    RegisterWorldOutcome, RootOpportunity, WorldState, verified_world_decision_view,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, OptimizationJournal, OptimizationStepRequest,
    PairedTaskResult, StageFact, StageFactKind, StoreOptimizationJournal,
};
use evo_engine::replay::{LiveWorldAuthority, run_replay};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RootBudgetAuthorization,
    UsageCharge,
};
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
const OBSERVED_REASON: &str = "candidate_observed";

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

    /// The parent a `Deepen` of a node at `depth` is requested against: the root
    /// skill with the fixture model's two edits applied once for each step of the
    /// node's chain, and the candidate bundle the node recorded.
    fn evolved_parent(&self, depth: u8, bundle: String) -> Parent {
        let mut skill = parent_skill();
        for _ in 0..depth {
            skill.content.push_str(" [repair]");
            skill.applicability.push_str(" [repair]");
        }
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

// ---------------------------------------------------------------------------
// Worlds, steps, and what a world offers
// ---------------------------------------------------------------------------

/// Quality of a candidate that answers every task, and the baseline the worlds are
/// registered over.
const FULL: u32 = 1_000_000;
const HALF: u32 = 500_000;
/// The reason of a decision that finds nothing legal.
const STOP_REASON: &str = "no_legal_action_with_authorized_cost";
/// More rounds than any world of these tests can dispatch: one that has not stopped by
/// then is stuck.
const MAX_ROUNDS: u32 = 8;
/// The episode of the failures written by hand, unless a test needs another.
const EPISODE: &str = "episode-1";
/// What every action of these worlds costs, roots and successors alike, and what a
/// world is registered with.
const COST: u64 = 10;
const REGISTERED_ROOT_MICROS: u64 = 1_000;
const REGISTERED_RECOVERY_DISPATCHES: u64 = 2;

/// A world whose roots are the given slots, in that order (a slot is also its branch
/// and its action sequence), over `baseline_micros`.
fn world(id: &str, slots: &[u32], baseline_micros: u32) -> ExplorationWorldV1 {
    let mut world = world_for(id);
    world.root_opportunities = slots
        .iter()
        .map(|slot| RootOpportunity {
            root_slot: *slot,
            branch_seq: *slot,
            action_seq: *slot,
            estimated_cost_upper_micros: COST,
        })
        .collect();
    world.initial_baseline_quality_micros = baseline_micros;
    world
}

/// A candidate that answers every task: quality 1000000.
fn answers_all(_: &str) -> bool {
    true
}

/// Counts the runs of the runner it wraps: a recovered step must not reach it.
struct CallCounting<R> {
    inner: R,
    calls: AtomicUsize,
}

impl<R> CallCounting<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl<R: DevRunner> DevRunner for CallCounting<R> {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.run(request).await
    }
}

/// What the step that repairs a failure ends in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Repair {
    /// A trusted valid candidate (the runner answers every task).
    Valid,
    /// The model suggests nothing: a plain failure.
    HardFailure,
    /// The model fails once the dispatch was claimed: an uncertain outcome.
    Uncertain,
    /// The failed node is not where the world lists it, so the repair that was paid for
    /// cannot be recorded against it.
    TargetGone,
}

impl Repair {
    fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::HardFailure => "hard-failure",
            Self::Uncertain => "uncertain",
            Self::TargetGone => "target-gone",
        }
    }

    fn model(self) -> &'static dyn ModelPort {
        match self {
            Self::Valid | Self::TargetGone => &EditingFixtureModel,
            Self::HardFailure => &NoChangeModel,
            Self::Uncertain => &FailingModel,
        }
    }

    /// The status the node of the dispatch is stored with.
    fn status(self) -> Value {
        match self {
            Self::Valid => json!({"status": "valid", "quality_micros": FULL}),
            Self::HardFailure => json!({"status": "hard_failure"}),
            Self::Uncertain | Self::TargetGone => json!({"status": "usage_uncertain"}),
        }
    }

    /// The state the dispatch fact ends in.
    fn dispatch_state(self) -> &'static str {
        match self {
            Self::Valid | Self::HardFailure => "observed",
            Self::Uncertain | Self::TargetGone => "uncertain",
        }
    }

    /// The label the gate gives the node.
    fn evidence(self) -> &'static str {
        match self {
            Self::Valid => "trusted",
            Self::HardFailure | Self::Uncertain | Self::TargetGone => "not_observed",
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
    /// The Admin management surface `exploration.start` runs on.
    dispatcher: ManagementDispatcher,
}

impl Env {
    async fn new() -> Self {
        let (dir, store) = seeded_store(true).await;
        Self {
            coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
            journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            model: CountingModel::default(),
            fixture: Fixture::new(),
            dispatcher: ManagementDispatcher::new(store.clone(), vec![admin()]).unwrap(),
            store,
            _dir: dir,
        }
    }

    async fn register(&self, world: ExplorationWorldV1) {
        self.coordinator.register_world(world).await.unwrap();
    }

    /// The registration a started world is compared with (what `exploration.start` and
    /// the registration of the same world again make).
    async fn register_again(&self, world: &ExplorationWorldV1) -> Result<RegisterWorldOutcome> {
        self.coordinator
            .register_world_idempotent(world.clone())
            .await
    }

    /// Submits `exploration.start` for `world` and waits for the terminal job: it
    /// registers the world, and its `status` re-verifies the registration later.
    async fn start(&self, request_key: &str, world: &ExplorationWorldV1) -> ManagementJob {
        start_job(&self.dispatcher, &admin(), request_key, world).await
    }

    /// A runner whose candidate answers the tasks `candidate_correct` names correctly.
    fn runner(&self, candidate_correct: fn(&str) -> bool) -> ContrastRunner {
        ContrastRunner {
            store: self.store.clone(),
            executor: Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap(),
            grader: Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap(),
            candidate_correct,
        }
    }

    /// The decisions the world has completed: the number of its nodes.
    async fn round(&self, world: &str) -> u32 {
        let stored = payload(&self.store, WORLD, world).await;
        u32::try_from(stored["decision_round"].as_u64().unwrap()).unwrap()
    }

    /// The stored world.
    async fn world(&self, world: &str) -> Value {
        payload(&self.store, WORLD, world).await
    }

    /// The stored node `node_seq` of `world`.
    async fn node(&self, world: &str, node_seq: u32) -> Value {
        payload(&self.store, NODE, &node_id(world, node_seq)).await
    }

    /// The parent a dispatch of `action_seq` is requested against: the world's own skill
    /// for a root; for a `Deepen` (`1000000 + 2 * node_seq`) or a `Recover` (that, plus
    /// one) the candidate of the nearest node on the line of the node it starts from that
    /// names one, or the world's own when none does. Each node that names a candidate
    /// adds one round of the fixture model's edits to its parent's skill, so the skill is
    /// the world's own with as many rounds as such nodes are on the line.
    async fn parent_of(&self, world: &str, action_seq: u32) -> Parent {
        if action_seq < 1_000_000 {
            return self.fixture.root_parent();
        }
        let mut node_seq = (action_seq - 1_000_000) / 2;
        let mut bundle: Option<String> = None;
        let mut rounds = 0u8;
        loop {
            let node = self.node(world, node_seq).await;
            if let Some(candidate) = node["candidate_bundle_digest"].as_str() {
                bundle.get_or_insert_with(|| candidate.to_string());
                rounds += 1;
            }
            match node["node"]["search_parent_seq"].as_u64() {
                Some(parent) => node_seq = u32::try_from(parent).unwrap(),
                None => break,
            }
        }
        match bundle {
            Some(bundle) => self.fixture.evolved_parent(rounds, bundle),
            None => self.fixture.root_parent(),
        }
    }

    /// One dispatch of whatever the world decides, through `run_next`: the request is
    /// built against the parent that decision names. When the decision is Stop, nothing
    /// is dispatched and `run_next` says so.
    async fn step(
        &self,
        world: &str,
        model: &dyn ModelPort,
        runner: &dyn DevRunner,
    ) -> Result<CoordinatorStepResult> {
        let decision = self.coordinator.decide_next(world).await?;
        let parent = match &decision.action {
            BatchActionV1::Dispatch { action_seqs, .. } => {
                self.parent_of(world, action_seqs[0]).await
            }
            BatchActionV1::Stop { .. } => self.fixture.root_parent(),
        };
        self.step_against(world, model, runner, &parent).await
    }

    /// One dispatch through `run_next` against `parent`.
    async fn step_against(
        &self,
        world: &str,
        model: &dyn ModelPort,
        runner: &dyn DevRunner,
        parent: &Parent,
    ) -> Result<CoordinatorStepResult> {
        let step = self.round(world).await + 1;
        self.coordinator
            .run_next(
                Some(model),
                Some(runner),
                Some(&self.journal),
                self.fixture
                    .request(parent, world, step, &format!("{world}-{step}")),
            )
            .await
    }

    /// `count` roots opened in a row, each of which fails without a candidate.
    async fn open_failed_roots(&self, world: &str, count: usize) -> Vec<CoordinatorStepResult> {
        let mut steps = Vec::new();
        for _ in 0..count {
            steps.push(
                self.step(world, &NoChangeModel, &self.runner(answers_all))
                    .await
                    .unwrap(),
            );
        }
        steps
    }
}

fn node_id(world: &str, node_seq: u32) -> String {
    format!("node-{world}-{node_seq}")
}

fn dispatched_seqs(action: &BatchActionV1) -> Vec<u32> {
    match action {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs.clone(),
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

/// The status a stored repairable failure carries.
fn repairable_status(
    episode: &str,
    kind: &str,
    environment_reset: bool,
    dispatched_repairs: u64,
) -> Value {
    json!({
        "status": "repairable_failure",
        "episode_id": episode,
        "failure_kind": kind,
        "repair_template_digest": d("repair-template"),
        "environment_reset": environment_reset,
        "dispatched_repairs": dispatched_repairs,
    })
}

/// Rewrites the stored node `node_seq` of `world` into a repairable failure of `kind`
/// (a failed node of the world is the only way to have one: no step produces it yet).
async fn make_repairable(
    env: &Env,
    world: &str,
    node_seq: u32,
    kind: &str,
    episode: &str,
    environment_reset: bool,
) {
    rewrite_node(&env.store, &node_id(world, node_seq), |node| {
        node["node"]["status"] = repairable_status(episode, kind, environment_reset, 0);
    })
    .await;
}

// What the coordinator derives, written out independently.

fn widen(slot: u32) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("root-{slot}"),
        action_seq: slot,
        branch_seq: slot,
        target_depth: 1,
        kind: ActionKindV1::Widen { root_slot: slot },
        estimated_cost_upper_micros: Some(COST),
    }
}

/// The Deepen of node `parent_seq`, which sits on `branch_seq` at `parent_depth`.
fn deepen(parent_seq: u32, branch_seq: u32, parent_depth: u8) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("deepen-{parent_seq}"),
        action_seq: 1_000_000 + parent_seq * 2,
        branch_seq,
        target_depth: parent_depth + 1,
        kind: ActionKindV1::Deepen {
            parent_node_seq: parent_seq,
        },
        estimated_cost_upper_micros: Some(COST),
    }
}

/// The Recover of node `failed_seq`, which sits on `branch_seq` at `failed_depth`.
fn recover(failed_seq: u32, branch_seq: u32, failed_depth: u8, episode_id: &str) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("recover-{failed_seq}"),
        action_seq: 1_000_000 + failed_seq * 2 + 1,
        branch_seq,
        target_depth: failed_depth + 1,
        kind: ActionKindV1::Recover {
            failed_node_seq: failed_seq,
            episode_id: episode_id.into(),
        },
        estimated_cost_upper_micros: Some(COST),
    }
}

/// The digest a decision records for these legal actions.
fn legal_digest(actions: &[LegalActionV1]) -> String {
    fingerprint(&LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: actions.to_vec(),
    })
    .unwrap()
}

fn ids(actions: &[LegalActionV1]) -> Vec<&str> {
    actions
        .iter()
        .map(|action| action.action_id.as_str())
        .collect()
}

/// Every action a world with these nodes could offer: its roots, then the Deepen of
/// each node and the Recover of each node that is stored as a repairable failure, in
/// the order the coordinator derives them.
async fn universe(env: &Env, world: &str) -> Vec<LegalActionV1> {
    let stored = env.world(world).await;
    let mut all: Vec<LegalActionV1> = stored["root_opportunities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| widen(u32::try_from(root["root_slot"].as_u64().unwrap()).unwrap()))
        .collect();
    for node_id in stored["node_ids"].as_array().unwrap() {
        let node = payload(&env.store, NODE, node_id.as_str().unwrap()).await;
        let node = &node["node"];
        let seq = u32::try_from(node["node_seq"].as_u64().unwrap()).unwrap();
        let branch = u32::try_from(node["branch_seq"].as_u64().unwrap()).unwrap();
        let depth = u8::try_from(node["depth"].as_u64().unwrap()).unwrap();
        all.push(deepen(seq, branch, depth));
        if node["status"]["status"] == "repairable_failure" {
            let episode = node["status"]["episode_id"].as_str().unwrap();
            all.push(recover(seq, branch, depth, episode));
        }
    }
    all
}

/// The legal actions the world offers now, read back from the digest its decision
/// records: the subset of every action it could offer whose digest that is.
async fn offered(env: &Env, world: &str) -> Vec<String> {
    let decision = env
        .coordinator
        .decide_next(world)
        .await
        .unwrap_or_else(|error| panic!("{world}: decide_next failed: {error:?}"));
    let universe = universe(env, world).await;
    assert!(universe.len() <= 16, "{world}: too many candidate actions");
    for mask in 0u32..(1 << universe.len()) {
        let subset: Vec<LegalActionV1> = universe
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, action)| action.clone())
            .collect();
        if legal_digest(&subset) == decision.legal_actions_digest {
            return ids(&subset).into_iter().map(str::to_owned).collect();
        }
    }
    panic!(
        "{world}: the offered legal actions are outside {:?}",
        ids(&universe)
    );
}

/// The world offers exactly `expected`, field for field.
async fn assert_offered(env: &Env, world: &str, expected: &[LegalActionV1]) {
    assert_eq!(
        offered(env, world).await,
        ids(expected),
        "legal actions of {world}"
    );
    assert_eq!(
        env.coordinator
            .decide_next(world)
            .await
            .unwrap()
            .legal_actions_digest,
        legal_digest(expected),
        "legal actions of {world}, field for field"
    );
}

/// The prefix the stored world and nodes make, as evo-core's own check reads it: the
/// counters are the ones the nodes are stored with.
async fn stored_prefix(env: &Env, world: &str) -> PrefixViewV2 {
    let stored = env.world(world).await;
    let mut nodes = Vec::new();
    for node_id in stored["node_ids"].as_array().unwrap() {
        let node = payload(&env.store, NODE, node_id.as_str().unwrap()).await;
        nodes.push(serde_json::from_value::<PrefixNodeV2>(node["node"].clone()).unwrap());
    }
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: stored["context_signature"].as_str().unwrap().into(),
        approved_parent_digest: stored["approved_parent_digest"].as_str().unwrap().into(),
        initial_baseline_quality_micros: u32::try_from(
            stored["initial_baseline_quality_micros"].as_u64().unwrap(),
        )
        .unwrap(),
        nodes_used: u8::try_from(nodes.len()).unwrap(),
        nodes,
        current_branch_seq: stored["current_branch_seq"]
            .as_u64()
            .map(|branch| u32::try_from(branch).unwrap()),
        current_branch_focus_actions: u8::try_from(
            stored["current_branch_focus_actions"].as_u64().unwrap(),
        )
        .unwrap(),
        decisions_completed: u32::try_from(stored["decision_round"].as_u64().unwrap()).unwrap(),
        waits: serde_json::from_value(stored["waits"].clone()).unwrap(),
        recovery_dispatches_used: 0,
    }
}

// ---------------------------------------------------------------------------
// The management surface and the registration (copied from
// `exploration_postpaid_v42.rs`)
// ---------------------------------------------------------------------------

fn exploration_start_payload(request_key: &str, world: &ExplorationWorldV1) -> Value {
    json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": request_key,
        "world": serde_json::to_value(world).unwrap(),
    })
}

/// Submits `exploration.start` for `world` and waits for the terminal job.
async fn start_job(
    dispatcher: &ManagementDispatcher,
    admin: &Context,
    request_key: &str,
    world: &ExplorationWorldV1,
) -> ManagementJob {
    let queued = dispatcher
        .submit(
            admin,
            "exploration.start",
            exploration_start_payload(request_key, world),
        )
        .await
        .unwrap();
    wait_terminal(dispatcher, admin, &queued.id).await
}

async fn wait_terminal(
    dispatcher: &ManagementDispatcher,
    context: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..400 {
        let job = dispatcher.status(context, job_id).await.unwrap();
        if matches!(
            job.state,
            ManagementJobState::Succeeded
                | ManagementJobState::Failed
                | ManagementJobState::Cancelled
                | ManagementJobState::Blocked
        ) {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("management job did not finish");
}

fn expect_already_registered(result: Result<RegisterWorldOutcome>, what: &str) {
    match result {
        Ok(RegisterWorldOutcome::AlreadyRegistered) => {}
        other => panic!("{what}: expected AlreadyRegistered, got {other:?}"),
    }
}

fn expect_conflict(result: Result<RegisterWorldOutcome>, what: &str) {
    assert!(
        matches!(result, Err(Error::Conflict(_))),
        "{what}: expected a Conflict, got {result:?}"
    );
}

/// `status` on the job holds and serves the stored result unchanged.
async fn expect_status_ok(dispatcher: &ManagementDispatcher, job: &ManagementJob, when: &str) {
    match dispatcher.status(&admin(), &job.id).await {
        Ok(status) => {
            assert_eq!(status.state, ManagementJobState::Succeeded, "{when}");
            assert_eq!(
                serde_json::to_value(&status.result).unwrap(),
                serde_json::to_value(&job.result).unwrap(),
                "{when}: the stored result is served unchanged"
            );
        }
        Err(error) => panic!("status {when}: {error:?}"),
    }
}

async fn expect_status_conflict(
    dispatcher: &ManagementDispatcher,
    job: &ManagementJob,
    what: &str,
) {
    let result = dispatcher.status(&admin(), &job.id).await;
    assert!(
        matches!(result, Err(Error::Conflict(_))),
        "{what}: expected a Conflict, got {:?}",
        result.map(|job| job.state)
    );
}

/// The registration of the world and the `status` of its start job both hold.
async fn assert_still_registered(
    env: &Env,
    world: &ExplorationWorldV1,
    job: &ManagementJob,
    when: &str,
) {
    expect_already_registered(env.register_again(world).await, when);
    expect_status_ok(&env.dispatcher, job, when).await;
}

/// The world still reads: its decision and the `status` read side compute, and the
/// registration holds.
async fn assert_decisions_read(env: &Env, world: &str, job: &ManagementJob) {
    env.coordinator
        .decide_next(world)
        .await
        .unwrap_or_else(|error| panic!("{world}: decide_next failed: {error:?}"));
    verified_world_decision_view(&admin(), &env.store, world)
        .await
        .unwrap_or_else(|error| panic!("{world}: the decision view failed: {error:?}"));
    expect_status_ok(&env.dispatcher, job, world).await;
}

/// The world still reads, and the nodes it stored make a prefix evo-core's own check
/// accepts.
async fn assert_world_reads(env: &Env, world: &str, job: &ManagementJob) {
    assert_decisions_read(env, world, job).await;
    stored_prefix(env, world)
        .await
        .validate()
        .unwrap_or_else(|error| {
            panic!("{world}: the prefix check refuses the stored nodes: {error:?}")
        });
}

/// Runs `world` round by round (every step ends in a hard failure) until the decision
/// is Stop, reading the world after each dispatch. Returns the actions it dispatched. A
/// world that fails to read, or does not stop within `MAX_ROUNDS`, is stuck.
async fn run_to_stop(env: &Env, world: &str, job: &ManagementJob) -> Vec<u32> {
    let mut dispatched = Vec::new();
    for round in 1..=MAX_ROUNDS {
        let decision = env
            .coordinator
            .decide_next(world)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{world}: round {round}: decide_next failed with {error:?} after {dispatched:?}"
                )
            });
        match &decision.action {
            BatchActionV1::Stop { reason } => {
                assert_eq!(reason, STOP_REASON, "{world}");
                return dispatched;
            }
            BatchActionV1::Dispatch { action_seqs, .. } => dispatched.push(action_seqs[0]),
        }
        env.step(world, &NoChangeModel, &env.runner(answers_all))
            .await
            .unwrap_or_else(|error| {
                panic!("{world}: round {round} could not dispatch ({error:?}) after {dispatched:?}")
            });
        assert_world_reads(env, world, job).await;
    }
    panic!("{world} did not stop within {MAX_ROUNDS} rounds: {dispatched:?}");
}

/// The stored records of a world that dispatched, to put edited and to put back: the
/// world, each dispatch fact and each node, as raw JSON.
#[derive(Clone, PartialEq)]
struct Stored {
    world_id: String,
    dispatch_ids: Vec<String>,
    node_ids: Vec<String>,
    world: Value,
    facts: Vec<Value>,
    nodes: Vec<Value>,
}

impl Stored {
    async fn load(env: &Env, world_id: &str, steps: &[CoordinatorStepResult]) -> Self {
        let dispatch_ids: Vec<String> = steps
            .iter()
            .map(|step| step.dispatch_id.clone().unwrap())
            .collect();
        let node_ids: Vec<String> = steps
            .iter()
            .map(|step| step.node_id.clone().unwrap())
            .collect();
        let mut facts = Vec::new();
        for id in &dispatch_ids {
            facts.push(raw_record(&env.store, DISPATCH, id).await);
        }
        let mut nodes = Vec::new();
        for id in &node_ids {
            nodes.push(raw_record(&env.store, NODE, id).await);
        }
        Self {
            world_id: world_id.into(),
            dispatch_ids,
            node_ids,
            world: raw_record(&env.store, WORLD, world_id).await,
            facts,
            nodes,
        }
    }

    async fn put(&self, env: &Env) {
        put_raw_record(&env.store, WORLD, &self.world_id, &self.world).await;
        for (id, fact) in self.dispatch_ids.iter().zip(&self.facts) {
            put_raw_record(&env.store, DISPATCH, id, fact).await;
        }
        for (id, node) in self.node_ids.iter().zip(&self.nodes) {
            put_raw_record(&env.store, NODE, id, node).await;
        }
    }
}

/// One edit of the stored records of a world that dispatched.
type StoredTamper = Box<dyn Fn(&mut Stored)>;

/// Registration, `status` and a second job for the same world all refuse what
/// `tamper` made of the records, and accept the untouched ones again.
async fn expect_refused_everywhere(
    env: &Env,
    world: &ExplorationWorldV1,
    done: &ManagementJob,
    original: &Stored,
    key: &str,
    label: &str,
    tamper: &StoredTamper,
) {
    let mut tampered = original.clone();
    tamper(&mut tampered);
    assert!(&tampered != original, "{label}: the tamper changed nothing");
    tampered.put(env).await;
    expect_conflict(env.register_again(world).await, label);
    expect_status_conflict(&env.dispatcher, done, label).await;
    // A job for the same world under another request key does not converge on it
    // either: it fails on registering, it does not report a first decision.
    let again = env.start(key, world).await;
    assert_eq!(
        again.state,
        ManagementJobState::Failed,
        "{label}: {again:?}"
    );
    assert_eq!(again.error_code.as_deref(), Some("conflict"), "{label}");
    assert!(again.result.is_none(), "{label}");
    original.put(env).await;
    assert_still_registered(env, world, done, &format!("restored: {label}")).await;
}

/// The stored fact of a root dispatch rewritten into the fact of the `Recover` of node
/// `failed_seq` (of `episode`), on `branch_seq` and `target_depth`, at the successor
/// cost: what `derive_legal_actions` would have derived for a repairable failure.
fn rewrite_as_recover(
    fact: &mut Value,
    failed_seq: u32,
    episode: &str,
    branch_seq: u32,
    target_depth: u32,
) {
    let action_id = format!("recover-{failed_seq}");
    let action_seq = 1_000_000 + 2 * failed_seq + 1;
    let payload = &mut fact["payload"];
    payload["action_id"] = json!(action_id);
    payload["action_seq"] = json!(action_seq);
    payload["selected_action"] = json!({
        "action_id": action_id,
        "action_seq": action_seq,
        "branch_seq": branch_seq,
        "target_depth": target_depth,
        "kind": {"action": "recover", "failed_node_seq": failed_seq, "episode_id": episode},
        "estimated_cost_upper_micros": COST,
    });
    payload["decision"]["action"]["action_ids"] = json!([action_id]);
    payload["decision"]["action"]["action_seqs"] = json!([action_seq]);
    payload["decision"]["action"]["estimated_cost_upper_micros"] = json!(COST);
}

/// Places the stored node `node` as the child of node `parent_seq`, on `branch_seq`
/// and `depth`: where the dispatch of a `Recover` of that node derives it.
fn place_under(node: &mut Value, parent_seq: u32, branch_seq: u32, depth: u32) {
    node["payload"]["node"]["search_parent_seq"] = json!(parent_seq);
    node["payload"]["node"]["branch_seq"] = json!(branch_seq);
    node["payload"]["node"]["depth"] = json!(depth);
}

// ---------------------------------------------------------------------------
// The replay of the same history (fixtures copied from `replay_v41.rs`)
// ---------------------------------------------------------------------------

fn refresh_evidence_digests(world: &mut ReplayWorldV2) {
    let digest = replay_baseline_observation_digest(&world.manifest).unwrap();
    let source_id = world.manifest.baseline_observation_source_id.clone();
    world
        .manifest
        .source_closure
        .iter_mut()
        .find(|source| source.source_id == source_id)
        .unwrap()
        .content_digest = digest;
    let observation_digests: Vec<_> = world
        .transitions
        .iter()
        .map(|transition| {
            (
                transition.observation_source_id.clone(),
                replay_observation_digest(&world.manifest, transition).unwrap(),
            )
        })
        .collect();
    for (source_id, digest) in observation_digests {
        world
            .manifest
            .source_closure
            .iter_mut()
            .find(|source| source.source_id == source_id)
            .unwrap()
            .content_digest = digest;
    }
}

/// One record of a replay world: the action available once the context `parent` was
/// revealed, and what it ended in; `next` is the context it reveals.
struct Replayed {
    parent: &'static str,
    next: &'static str,
    branch_seq: u32,
    /// The depth the node lands on.
    depth: u8,
    kind: ActionKindV1,
    outcome: ReplayTransitionOutcome,
}

impl Replayed {
    /// The root of `slot`, opened from the baseline.
    fn root(slot: u32, next: &'static str, outcome: ReplayTransitionOutcome) -> Self {
        Self {
            parent: "baseline",
            next,
            branch_seq: slot,
            depth: 1,
            kind: ActionKindV1::Widen { root_slot: slot },
            outcome,
        }
    }

    /// The `Recover` of node `failed_seq` (of `episode`) on `branch_seq`, available once
    /// `contexts.0` was revealed, which reveals `contexts.1` and lands on `depth`.
    fn recover(
        failed_seq: u32,
        episode: &str,
        branch_seq: u32,
        contexts: (&'static str, &'static str),
        depth: u8,
        outcome: ReplayTransitionOutcome,
    ) -> Self {
        Self {
            parent: contexts.0,
            next: contexts.1,
            branch_seq,
            depth,
            kind: ActionKindV1::Recover {
                failed_node_seq: failed_seq,
                episode_id: episode.into(),
            },
            outcome,
        }
    }
}

fn replayed_repairable(episode: &str) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::RepairableFailure {
            episode_id: episode.into(),
            failure_kind: FailureKind::Compile,
            repair_template_digest: d("repair-template"),
            environment_reset: true,
            dispatched_repairs: 0,
        },
    }
}

fn replayed_hard_failure() -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::HardFailure,
    }
}

fn replayed_valid(quality_micros: u32) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::Valid { quality_micros },
    }
}

/// The replay world of `records`, numbered in order: the record `n` is the node `n` of
/// the world it describes.
fn replay_world(records: &[Replayed]) -> ReplayWorldV2 {
    let contexts: BTreeSet<&str> = std::iter::once("baseline")
        .chain(
            records
                .iter()
                .flat_map(|record| [record.parent, record.next]),
        )
        .collect();
    let actions: Vec<ReplayActionSpecV1> = records
        .iter()
        .enumerate()
        .map(|(index, record)| ReplayActionSpecV1 {
            record_seq: u32::try_from(index).unwrap() + 1,
            generation_signature: d("generation-v1"),
            parent_context_signature: d(record.parent),
            branch_seq: record.branch_seq,
            target_depth: record.depth,
            action_kind: record.kind.clone(),
            estimated_cost_upper_micros: Some(COST),
            writes_shared_workspace: false,
        })
        .collect();
    let transitions: Vec<ReplayTransitionV2> = records
        .iter()
        .enumerate()
        .map(|(index, record)| ReplayTransitionV2 {
            record_id: format!("record-{}", index + 1),
            record_seq: u32::try_from(index).unwrap() + 1,
            generation_signature: d("generation-v1"),
            parent_context_signature: d(record.parent),
            action_kind: record.kind.clone(),
            next_context_signature: d(record.next),
            outcome: record.outcome.clone(),
            actual_usage: HistoricalUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: Some(1),
                latency_millis: Some(1),
            },
            source_ids: vec![format!("source-{}", index + 1)],
            observation_source_id: format!("source-{}", index + 1),
        })
        .collect();
    let mut source_closure: Vec<ReplaySourceRef> = (1..=records.len())
        .map(|index| ReplaySourceRef {
            source_id: format!("source-{index}"),
            content_digest: String::new(),
        })
        .collect();
    source_closure.push(ReplaySourceRef {
        source_id: "baseline-source-1".into(),
        content_digest: String::new(),
    });
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: "world-1".into(),
            cluster_id: "cluster-1".into(),
            partition: WorldPartition::Select,
            purpose: Purpose::Development,
            generation_signature: d("generation-v1"),
            world_context_signature: d("world-context"),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("approved-parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: HALF,
            baseline_observation_source_id: "baseline-source-1".into(),
            source_closure,
            revoke_watermark: 7,
            prefix_coverage: contexts
                .iter()
                .map(|context| PrefixCoverageV1 {
                    context_signature: d(context),
                    exhausted: true,
                })
                .collect(),
            action_catalog: actions,
        },
        transitions,
        sealed_digest: None,
    };
    refresh_evidence_digests(&mut world);
    world.seal().unwrap();
    world
}

/// What the replay does with `records` when it may dispatch `recovery_limit` recoveries
/// in all. It follows the history: it ends by exhausting the world, its budget or its
/// policy, never as an invalid, out of support, censored or revoked world.
fn replayed_report(records: &[Replayed], recovery_limit: u8) -> ReplayReportV2 {
    let probes = u8::try_from(records.len()).unwrap() + 1;
    let report = run_replay(
        &replay_world(records),
        &ElasticPolicyV1::default(),
        &ReplaySimulationProfile {
            simulation_version: SIMULATION_VERSION.into(),
            objective: ReplayObjective::ParetoAttainmentV2,
            w_sim: 1,
            probe_budget: probes,
            horizon: probes,
            lambda_work_micros: DEFAULT_LAMBDA_MICROS,
            lambda_round_micros: DEFAULT_LAMBDA_MICROS,
            fixed_seed: 9,
            global_recovery_dispatch_limit: recovery_limit,
            pool_digest: d("pool"),
            purpose: Purpose::Development,
            target_runtime_profile: "simulation-only".into(),
        },
        &ExplorationCapsV1::online(),
        &LiveWorldAuthority {
            revoke_watermark: 7,
            revoked_source_ids: BTreeSet::new(),
        },
    )
    .unwrap();
    assert!(
        matches!(
            report.terminal,
            ReplayTerminal::WorldExhausted
                | ReplayTerminal::BudgetExhausted
                | ReplayTerminal::PolicyStop
        ),
        "the replay did not follow the history: {:?} ({})",
        report.terminal,
        report.terminal_reason
    );
    report
}

/// The prefix the replay reveals for `records`, every one of which it reveals.
fn replayed_prefix(records: &[Replayed], recovery_limit: u8) -> PrefixViewV2 {
    let report = replayed_report(records, recovery_limit);
    assert_eq!(
        report.coverage.observed_actions,
        u32::try_from(records.len()).unwrap(),
        "every record of the history is revealed"
    );
    report.revealed_prefix.expect("a replay reveals its prefix")
}

/// What a prefix counts for each node, in the order of its nodes: the sequence, the
/// repairs dispatched on it when it is a repairable failure, and its repair failures.
fn counts(prefix: &PrefixViewV2) -> Vec<(u32, Option<u8>, u8)> {
    prefix
        .nodes
        .iter()
        .map(|node| {
            let repairs = match &node.status {
                ObservedStatus::RepairableFailure {
                    dispatched_repairs, ..
                } => Some(*dispatched_repairs),
                _ => None,
            };
            (node.node_seq, repairs, node.repair_failures_dispatched)
        })
        .collect()
}

/// The prefix the stored nodes of `world` make, with the counters of the replay in place
/// of the ones the nodes are stored with: what the world would hand its decision if it
/// counted the history as the replay does.
async fn prefix_counted_as(env: &Env, world: &str, replayed: &PrefixViewV2) -> PrefixViewV2 {
    let mut expected = stored_prefix(env, world).await;
    assert_eq!(
        expected.nodes.len(),
        replayed.nodes.len(),
        "{world}: the same nodes"
    );
    for (mine, theirs) in expected.nodes.iter_mut().zip(&replayed.nodes) {
        assert_eq!(mine.node_seq, theirs.node_seq, "{world}");
        match (&mut mine.status, &theirs.status) {
            (
                ObservedStatus::RepairableFailure {
                    dispatched_repairs: mine,
                    ..
                },
                ObservedStatus::RepairableFailure {
                    dispatched_repairs: theirs,
                    ..
                },
            ) => *mine = *theirs,
            (mine, theirs) => assert_eq!(
                serde_json::to_value(&*mine).unwrap(),
                serde_json::to_value(theirs).unwrap(),
                "{world}: the same status"
            ),
        }
        mine.repair_failures_dispatched = theirs.repair_failures_dispatched;
    }
    expected.recovery_dispatches_used = replayed.recovery_dispatches_used;
    expected
}

/// The prefix the world hands its decision counts as the replay counts the same history.
async fn assert_counted_as_replayed(env: &Env, world: &str, replayed: &PrefixViewV2) {
    let expected = prefix_counted_as(env, world, replayed).await;
    let decision = env.coordinator.decide_next(world).await.unwrap();
    assert_eq!(
        decision.prefix_digest,
        fingerprint(&expected).unwrap(),
        "{world}: the prefix of the decision does not count as the replay counts it; the replay's counts are {:?}, \
         it used {} recoveries",
        counts(replayed),
        replayed.recovery_dispatches_used
    );
}

// ---------------------------------------------------------------------------
// 1. A Recover never rewrites its failed node, and what it writes counts as the
//    replay counts it
// ---------------------------------------------------------------------------

/// The Recover of the only failed root of a world, ending as `repair` says, leaves that
/// root exactly as it was and writes a node that counts as the replay counts it.
async fn assert_repair_settled(repair: Repair) {
    let env = Env::new().await;
    let id = format!("world-repair-{}", repair.name());
    let registered = world(&id, &[1], HALF);
    let job = env.start(&format!("start-{id}"), &registered).await;
    assert_eq!(job.state, ManagementJobState::Succeeded, "{id}: {job:?}");
    // The only root fails without a candidate and is rewritten into a repairable
    // failure whose environment was reset: no step produces one yet.
    env.open_failed_roots(&id, 1).await;
    make_repairable(&env, &id, 1, "compile", EPISODE, true).await;
    let gone = repair == Repair::TargetGone;
    let failed_seq = if gone { 2 } else { 1 };
    if gone {
        // The node is numbered past the world's list of nodes: the Recover derived
        // from it names a node (2) the world does not list at that position.
        rewrite_node(&env.store, &node_id(&id, 1), |node| {
            node["node"]["node_seq"] = json!(2);
        })
        .await;
    }
    let failed_node = node_id(&id, 1);
    let failed_before = raw_record(&env.store, NODE, &failed_node).await;
    let decision = env.coordinator.decide_next(&id).await.unwrap();
    assert_eq!(
        dispatched_seqs(&decision.action),
        vec![1_000_001 + 2 * failed_seq],
        "{id}: the repair is what the world decides"
    );

    let step = if gone {
        let parent = env.fixture.root_parent();
        env.step_against(&id, repair.model(), &env.runner(answers_all), &parent)
            .await
    } else {
        env.step(&id, repair.model(), &env.runner(answers_all))
            .await
    }
    .expect("a step that was paid for never ends in an error");

    // The failed node is exactly what it was: no counter, no status, nothing moved.
    assert_eq!(
        raw_record(&env.store, NODE, &failed_node).await,
        failed_before,
        "{id}: the failed node was rewritten"
    );

    // The node of the dispatch is the child of the failed node, one level down, on
    // its branch, and it counts its repair failures as the replay does: the failed
    // node's (none) and one more only if the repair is itself a repairable failure.
    // None of these is, so none has any.
    let new_node = node_id(&id, 2);
    assert_eq!(step.node_id.as_deref(), Some(new_node.as_str()), "{id}");
    let child = env.node(&id, 2).await;
    assert_eq!(child["node"]["status"], repair.status(), "{id}");
    assert_eq!(child["node"]["search_parent_seq"], failed_seq, "{id}");
    assert_eq!(child["node"]["branch_seq"], 1, "{id}");
    assert_eq!(child["node"]["depth"], 2, "{id}");
    assert_eq!(child["node"]["repair_failures_dispatched"], 0, "{id}");
    assert_eq!(child["evidence"], repair.evidence(), "{id}");

    // The dispatch spent a recovery dispatch (whatever the outcome) and its cost.
    let stored = env.world(&id).await;
    assert_eq!(stored["node_ids"], json!([failed_node, new_node]), "{id}");
    assert_eq!(
        stored["remaining_recovery_dispatches"],
        REGISTERED_RECOVERY_DISPATCHES - 1,
        "{id}"
    );
    assert_eq!(
        stored["remaining_root_micros"],
        REGISTERED_ROOT_MICROS - 2 * COST,
        "{id}"
    );
    assert_eq!(stored["decision_round"], 2, "{id}");

    // The fact names the Recover, its node and the state the outcome ends it in.
    let fact = payload(&env.store, DISPATCH, step.dispatch_id.as_deref().unwrap()).await;
    assert_eq!(fact["state"], repair.dispatch_state(), "{id}");
    assert_eq!(fact["node_id"], json!(new_node), "{id}");
    assert_eq!(fact["evidence"], repair.evidence(), "{id}");
    assert_eq!(
        fact["selected_action"]["kind"],
        json!({"action": "recover", "failed_node_seq": failed_seq, "episode_id": EPISODE}),
        "{id}"
    );
    if gone {
        // Nothing to count on: the outcome is the fixed uncertain one. (The world was
        // rewritten to make its target vanish, so it is no registered world, and two
        // of its nodes carry sequence 2: no prefix check or registration is asked.)
        assert_eq!(fact["outcome_reason"], "recover_target_missing", "{id}");
        return;
    }

    // The prefix the stored nodes make passes evo-core's check, and the world still
    // reads, registers and is served by `status`.
    assert_world_reads(&env, &id, &job).await;
    assert_still_registered(&env, &registered, &job, &id).await;
    // The repair is spent: only a valid repair has a successor.
    let expected = match repair {
        Repair::Valid => vec![deepen(2, 1, 2)],
        _ => vec![],
    };
    assert_offered(&env, &id, &expected).await;
}

#[tokio::test]
async fn a_valid_repair_leaves_its_failed_node_as_it_was_and_counts_as_the_replay_counts_it() {
    assert_repair_settled(Repair::Valid).await;
}

#[tokio::test]
async fn a_failed_repair_leaves_its_failed_node_as_it_was_and_counts_as_the_replay_counts_it() {
    assert_repair_settled(Repair::HardFailure).await;
}

#[tokio::test]
async fn an_uncertain_repair_leaves_its_failed_node_as_it_was_and_counts_as_the_replay_counts_it() {
    assert_repair_settled(Repair::Uncertain).await;
}

#[tokio::test]
async fn a_repair_whose_target_is_gone_leaves_the_failed_node_as_it_was_and_ends_uncertain() {
    assert_repair_settled(Repair::TargetGone).await;
}

// ---------------------------------------------------------------------------
// 2. The same history, counted as the replay counts it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repair_that_ends_in_a_hard_failure_is_counted_as_the_replay_counts_it() {
    // S2: the repair of a repairable failure ends in a hard failure. The coordinator
    // used to count one repair failure on both the failed node and the repair (1, 1);
    // the replay counts none on either (0, 0), so the same history gave another prefix.
    let env = Env::new().await;
    let id = "world-replay-hard";
    let registered = world(id, &[1], HALF);
    let job = env.start("start-replay-hard", &registered).await;
    env.open_failed_roots(id, 1).await;
    make_repairable(&env, id, 1, "compile", EPISODE, true).await;
    env.step(id, Repair::HardFailure.model(), &env.runner(answers_all))
        .await
        .unwrap();

    let replayed = replayed_prefix(
        &[
            Replayed::root(1, "ctx-1", replayed_repairable(EPISODE)),
            Replayed::recover(
                1,
                EPISODE,
                1,
                ("ctx-1", "ctx-2"),
                2,
                replayed_hard_failure(),
            ),
        ],
        1,
    );
    assert_eq!(counts(&replayed), vec![(1, Some(1), 0), (2, None, 0)]);
    assert_eq!(replayed.recovery_dispatches_used, 1);
    assert_counted_as_replayed(&env, id, &replayed).await;
    assert_still_registered(&env, &registered, &job, id).await;
}

#[tokio::test]
async fn two_repairs_and_a_success_are_counted_as_the_replay_counts_them() {
    // S3: a repairable failure, repaired into another repairable failure, repaired into
    // a valid node. The replay's valid node inherits the repair failure of the node it
    // repaired (1); the coordinator used to count none, which moved the value of the
    // `Deepen` of that node by 50000 and could change the next action.
    //
    // The coordinator no longer offers the second repair (the repair of a repair is a
    // failure of the same line), so this is the world a coordinator that did might have
    // stored: three real dispatches, rewritten into that history.
    let env = Env::new().await;
    let id = "world-replay-two";
    let registered = world(id, &[1, 2, 3], HALF);
    let job = env.start("start-replay-two", &registered).await;
    let mut steps = env.open_failed_roots(id, 2).await;
    steps.push(
        env.step(id, &EditingFixtureModel, &env.runner(answers_all))
            .await
            .unwrap(),
    );
    let original = Stored::load(&env, id, &steps).await;
    let mut history = original.clone();
    // The first node is a repairable failure of the first episode; the second, written as
    // the repair of it, is a repairable failure of another episode; the third, written as
    // the repair of that, is the valid and trusted one the third root produced.
    history.nodes[0]["payload"]["node"]["status"] = repairable_status(EPISODE, "compile", true, 0);
    history.nodes[1]["payload"]["node"]["status"] =
        repairable_status("episode-2", "compile", true, 0);
    place_under(&mut history.nodes[1], 1, 1, 2);
    place_under(&mut history.nodes[2], 2, 1, 3);
    rewrite_as_recover(&mut history.facts[1], 1, EPISODE, 1, 2);
    rewrite_as_recover(&mut history.facts[2], 2, "episode-2", 1, 3);
    history.world["payload"]["remaining_recovery_dispatches"] = json!(0);
    history.world["payload"]["current_branch_seq"] = json!(1);
    history.world["payload"]["waits"] = json!([]);
    history.put(&env).await;

    let replayed = replayed_prefix(
        &[
            Replayed::root(1, "ctx-1", replayed_repairable(EPISODE)),
            Replayed::recover(
                1,
                EPISODE,
                1,
                ("ctx-1", "ctx-2"),
                2,
                replayed_repairable("episode-2"),
            ),
            Replayed::recover(
                2,
                "episode-2",
                1,
                ("ctx-2", "ctx-3"),
                3,
                replayed_valid(FULL),
            ),
        ],
        2,
    );
    assert_eq!(
        counts(&replayed),
        vec![(1, Some(1), 0), (2, Some(1), 1), (3, None, 1)]
    );
    assert_eq!(replayed.recovery_dispatches_used, 2);
    assert_counted_as_replayed(&env, id, &replayed).await;
    assert_still_registered(&env, &registered, &job, id).await;
}

// ---------------------------------------------------------------------------
// 3. The stored counters are not the source of the count
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rewriting_the_stored_counters_of_a_node_neither_gives_a_repair_back_nor_takes_one_away() {
    let env = Env::new().await;

    // A repair that was dispatched is spent: putting the counters of its failed node back
    // to 0 (the repair counter of its status and its repair failures) does not offer it
    // again. It used to: the counter was the only thing that held it, and the second
    // Recover of the node has the action sequence of the first, so it met the dispatch id
    // that was spent.
    let id = "world-counters-spent";
    let registered = world(id, &[1], HALF);
    let job = env.start("start-counters-spent", &registered).await;
    env.open_failed_roots(id, 1).await;
    make_repairable(&env, id, 1, "compile", EPISODE, true).await;
    env.step(id, Repair::HardFailure.model(), &env.runner(answers_all))
        .await
        .unwrap();
    let spent = env.coordinator.decide_next(id).await.unwrap();
    assert!(
        matches!(&spent.action, BatchActionV1::Stop { reason } if reason == STOP_REASON),
        "{spent:?}"
    );
    rewrite_node(&env.store, &node_id(id, 1), |node| {
        node["node"]["status"]["dispatched_repairs"] = json!(0);
        node["node"]["repair_failures_dispatched"] = json!(0);
    })
    .await;
    let after = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&spent).unwrap(),
        "the decision moved with the stored counters"
    );
    assert_offered(&env, id, &[]).await;
    // `run_next` reports the stop: the world does not meet the spent dispatch id.
    let stopped = env
        .step(id, &NoChangeModel, &env.runner(answers_all))
        .await
        .expect("the world stops, it does not meet the dispatch id of the repair");
    assert_eq!(stopped.dispatch_id, None);
    assert_eq!(stopped.node_id, None);
    assert_eq!(stopped.outcome, STOP_REASON);
    assert_eq!(run_to_stop(&env, id, &job).await, Vec::<u32>::new());
    assert_still_registered(&env, &registered, &job, id).await;

    // Counters raised on a failure that was never repaired do not take its repair away.
    let id = "world-counters-open";
    let registered = world(id, &[1], HALF);
    let job = env.start("start-counters-open", &registered).await;
    env.open_failed_roots(id, 1).await;
    rewrite_node(&env.store, &node_id(id, 1), |node| {
        node["node"]["status"] = repairable_status(EPISODE, "compile", true, 1);
        node["node"]["repair_failures_dispatched"] = json!(1);
    })
    .await;
    assert_offered(&env, id, &[recover(1, 1, 1, EPISODE)]).await;
    let step = env
        .step(id, Repair::HardFailure.model(), &env.runner(answers_all))
        .await
        .unwrap();
    assert_eq!(dispatched_seqs(&step.decision.action), vec![1_000_003]);
    // Now it is spent for real: the world stops.
    assert_offered(&env, id, &[]).await;
    assert_eq!(run_to_stop(&env, id, &job).await, Vec::<u32>::new());
    assert_still_registered(&env, &registered, &job, id).await;
}

// ---------------------------------------------------------------------------
// 4. A repair is never offered where the prefix could not hold its outcome (S1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repair_that_fails_again_in_a_repairable_way_is_offered_no_second_repair_and_the_world_stops()
 {
    // The repair of a repairable failure ends in a failure that is repairable again.
    // Repairing that one would give a node two repair failures, more than the prefix
    // check accepts, and `decide_next`, the decision view and `status` would all fail
    // for good. Nothing of the kind is offered: the world stops. Whether the repair kept
    // the name of its episode or renamed it, the repair failures of its line close it.
    let env = Env::new().await;
    for (case, again) in [("same-episode", EPISODE), ("renamed-episode", "episode-2")] {
        let id = format!("world-again-{case}");
        let registered = world(&id, &[1], HALF);
        let job = env.start(&format!("start-{id}"), &registered).await;
        env.open_failed_roots(&id, 1).await;
        make_repairable(&env, &id, 1, "compile", EPISODE, true).await;
        // The repair is dispatched for real; what it produced is rewritten into a
        // repairable failure (no step produces one yet).
        env.step(&id, Repair::HardFailure.model(), &env.runner(answers_all))
            .await
            .unwrap();
        make_repairable(&env, &id, 2, "compile", again, true).await;

        // Whatever the world dispatches from here (nothing, once it is fixed) leaves a
        // world that reads and stops.
        let dispatched = run_to_stop(&env, &id, &job).await;
        assert_eq!(
            dispatched,
            Vec::<u32>::new(),
            "{id}: a second repair was offered"
        );
        assert_offered(&env, &id, &[]).await;
        assert_world_reads(&env, &id, &job).await;
        assert_still_registered(&env, &registered, &job, &id).await;
    }
}

#[tokio::test]
async fn an_episode_is_repaired_once_whichever_of_its_failures_is_repaired() {
    // Two roots fail the same episode. Repairing either spends the episode's one repair
    // (plan §7.1: at most one dispatched recovery per failed episode), and neither has
    // a repair failure of its own, so only the episode's count can close the other.
    let env = Env::new().await;
    let id = "world-episode-once";
    let registered = world(id, &[1, 2], HALF);
    let job = env.start("start-episode-once", &registered).await;
    env.open_failed_roots(id, 2).await;
    make_repairable(&env, id, 1, "compile", EPISODE, true).await;
    make_repairable(&env, id, 2, "type", EPISODE, true).await;
    assert_offered(
        &env,
        id,
        &[recover(1, 1, 1, EPISODE), recover(2, 2, 1, EPISODE)],
    )
    .await;
    let step = env
        .step(id, Repair::HardFailure.model(), &env.runner(answers_all))
        .await
        .unwrap();
    assert_eq!(dispatched_seqs(&step.decision.action), vec![1_000_003]);
    assert_eq!(run_to_stop(&env, id, &job).await, Vec::<u32>::new());
    assert_offered(&env, id, &[]).await;
    assert_world_reads(&env, id, &job).await;
    assert_still_registered(&env, &registered, &job, id).await;

    // Another episode keeps its own repair.
    let id = "world-episode-apart";
    let registered = world(id, &[1, 2], HALF);
    let job = env.start("start-episode-apart", &registered).await;
    env.open_failed_roots(id, 2).await;
    make_repairable(&env, id, 1, "compile", EPISODE, true).await;
    make_repairable(&env, id, 2, "compile", "episode-2", true).await;
    env.step(id, Repair::HardFailure.model(), &env.runner(answers_all))
        .await
        .unwrap();
    assert_offered(&env, id, &[recover(2, 2, 1, "episode-2")]).await;
    assert_world_reads(&env, id, &job).await;
}

// ---------------------------------------------------------------------------
// 5. A failure of a kind that cannot be repaired is no Recover and no stuck prefix
//    (S4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failure_of_a_kind_that_cannot_be_repaired_is_offered_no_recover_and_stops_the_world() {
    let env = Env::new().await;
    // The kinds the wire format stores but no repair addresses (plan §7.2.1: an
    // environment, resource, safety or unknown failure cannot be renamed into a
    // repairable one). The stored prefix check refuses them, so a world holding one used
    // to fail `decide_next`, the decision view and `status` for good.
    for kind in ["environment", "resource", "safety", "unknown"] {
        let id = format!("world-kind-{kind}");
        let registered = world(&id, &[1], HALF);
        let job = env.start(&format!("start-{id}"), &registered).await;
        env.open_failed_roots(&id, 1).await;
        make_repairable(&env, &id, 1, kind, EPISODE, true).await;
        let stored = raw_record(&env.store, NODE, &node_id(&id, 1)).await;

        assert_decisions_read(&env, &id, &job).await;
        assert_offered(&env, &id, &[]).await;
        let stopped = env
            .step(&id, &NoChangeModel, &env.runner(answers_all))
            .await
            .unwrap();
        assert_eq!(stopped.dispatch_id, None, "{id}");
        assert_eq!(stopped.node_id, None, "{id}");
        assert_eq!(stopped.outcome, STOP_REASON, "{id}");
        assert_eq!(run_to_stop(&env, &id, &job).await, Vec::<u32>::new());

        // The record is kept as it was stored, whatever the decision made of it.
        assert_eq!(stored["payload"]["node"]["status"]["failure_kind"], kind);
        assert_eq!(
            raw_record(&env.store, NODE, &node_id(&id, 1)).await,
            stored,
            "{id}: the stored failure was rewritten"
        );
        assert_still_registered(&env, &registered, &job, &id).await;
    }

    // The kinds a repair addresses are offered one.
    for kind in ["compile", "implementation", "type", "output_shape"] {
        let id = format!("world-kind-{kind}");
        env.register(world(&id, &[1], HALF)).await;
        env.open_failed_roots(&id, 1).await;
        make_repairable(&env, &id, 1, kind, EPISODE, true).await;
        assert_offered(&env, &id, &[recover(1, 1, 1, EPISODE)]).await;
    }

    // Such a failure does not stop the rest of the world: the other root is still
    // offered, dispatched, and then the world stops.
    let id = "world-kind-continues";
    let registered = world(id, &[1, 2], HALF);
    let job = env.start("start-kind-continues", &registered).await;
    env.open_failed_roots(id, 1).await;
    make_repairable(&env, id, 1, "safety", EPISODE, true).await;
    assert_offered(&env, id, &[widen(2)]).await;
    env.open_failed_roots(id, 1).await;
    assert_decisions_read(&env, id, &job).await;
    assert_eq!(run_to_stop(&env, id, &job).await, Vec::<u32>::new());
}

// ---------------------------------------------------------------------------
// 6. A claimed Recover is not spent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_claimed_recover_stays_legal_and_resumes_without_paying_again() {
    let env = Env::new().await;
    let id = "world-claimed-recover";
    let registered = world(id, &[1], HALF);
    let job = env.start("start-claimed-recover", &registered).await;
    env.open_failed_roots(id, 1).await;
    make_repairable(&env, id, 1, "compile", EPISODE, true).await;
    let before = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(dispatched_seqs(&before.action), vec![1_000_003]);

    // The Recover is dispatched and the process is lost after the runner's report was
    // saved and before the step observed it: the dispatch stays claimed and no node
    // names the failed node as its parent yet.
    let parent = env.parent_of(id, 1_000_003).await;
    let (hanging, entered) = HangingJournal::new(&env.store, StageFactKind::DevelopmentObserved);
    let runner = CallCounting::new(env.runner(answers_all));
    tokio::select! {
        biased;
        result = env.coordinator.run_next(
            Some(&env.model),
            Some(&runner),
            Some(&hanging),
            env.fixture.request(&parent, id, 2, "claimed-recover-2"),
        ) => panic!("the hanging journal must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    assert_eq!(runner.calls(), 1, "the paid step ran once");
    let stored = env.world(id).await;
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 1);
    let dispatch_ids = stored["dispatch_ids"].as_array().unwrap();
    assert_eq!(dispatch_ids.len(), 2);
    let claimed = payload(&env.store, DISPATCH, dispatch_ids[1].as_str().unwrap()).await;
    assert_eq!(claimed["state"], "claimed");
    assert_eq!(claimed["action_seq"], 1_000_003);

    // The claimed Recover is not spent: it is still legal, and still what the world
    // decides, with the same digests the claim recorded. Registration holds, too: a claim
    // has spent nothing.
    let during = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(dispatched_seqs(&during.action), vec![1_000_003]);
    assert_eq!(during.legal_actions_digest, before.legal_actions_digest);
    assert_eq!(during.prefix_digest, before.prefix_digest);
    assert_offered(&env, id, &[recover(1, 1, 1, EPISODE)]).await;
    assert_still_registered(&env, &registered, &job, id).await;

    // A new `run_next` of the same request finishes the claimed dispatch: the saved
    // report and the model answers are replayed, nothing is paid for again, and the step
    // is recorded as observed, not as a claim whose inputs changed.
    let model_calls = env.model.calls();
    let replay = CallCounting::new(env.runner(answers_all));
    let finished = env
        .coordinator
        .run_next(
            Some(&env.model),
            Some(&replay),
            Some(&env.journal),
            env.fixture.request(&parent, id, 2, "claimed-recover-2"),
        )
        .await
        .unwrap();
    assert_eq!(replay.calls(), 0, "the recovery journal replays the report");
    assert_eq!(env.model.calls(), model_calls);
    assert_eq!(finished.outcome, OBSERVED_REASON);
    assert_eq!(finished.dispatch_id.as_deref(), claimed["id"].as_str());

    // The resumed step wrote the node the claimed Recover was to derive, and the dispatch
    // is consumed.
    let child = env.node(id, 2).await;
    assert_eq!(child["node"]["search_parent_seq"], 1);
    assert_eq!(child["node"]["depth"], 2);
    assert_eq!(
        child["node"]["status"],
        json!({"status": "valid", "quality_micros": FULL})
    );
    assert_eq!(child["evidence"], "trusted");
    assert_eq!(child["node"]["repair_failures_dispatched"], 0);
    let consumed = payload(&env.store, DISPATCH, dispatch_ids[1].as_str().unwrap()).await;
    assert_eq!(consumed["state"], "observed");
    assert_eq!(consumed["node_id"], finished.node_id.clone().unwrap());

    // Now the repair is spent, and the valid repair has its Deepen.
    assert_offered(&env, id, &[deepen(2, 1, 2)]).await;
    assert_world_reads(&env, id, &job).await;
    assert_still_registered(&env, &registered, &job, id).await;
}

// ---------------------------------------------------------------------------
// 7. A completed Recover fact is held to its target
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recover_fact_is_held_to_a_repairable_failure_of_a_supported_kind() {
    // A root dispatch rewritten into the Recover of a node that is no repairable failure,
    // together with the counters that would pay for it, balanced the books: the replay
    // refuses such a history ("recover action does not target a repairable failure"),
    // and so do registration and `status` now.
    let env = Env::new().await;
    let id = "world-bind";
    let registered = world(id, &[2, 1], HALF);
    let done = env.start("bind-start", &registered).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    // Two real dispatches: the roots of slot 1 and slot 2, both of which fail without a
    // candidate.
    let steps = env.open_failed_roots(id, 2).await;
    let original = Stored::load(&env, id, &steps).await;
    assert_still_registered(&env, &registered, &done, "two root dispatches").await;

    // The second root dispatch becomes the Recover of the first node: its fact, the
    // counters that pay for it (a recovery dispatch; every action costs the same) and
    // its node, which a Recover derives as the child of the failed node, one level
    // down, on its branch. What varies is what the failed node is stored as.
    let recovering = |status: Value| -> StoredTamper {
        Box::new(move |stored| {
            rewrite_as_recover(&mut stored.facts[1], 1, EPISODE, 1, 2);
            stored.world["payload"]["remaining_recovery_dispatches"] =
                json!(REGISTERED_RECOVERY_DISPATCHES - 1);
            place_under(&mut stored.nodes[1], 1, 1, 2);
            stored.nodes[0]["payload"]["node"]["status"] = status.clone();
        })
    };

    // Nothing that is no repairable failure of a supported kind is repaired.
    let mut refused: Vec<(String, Value)> = vec![
        ("a hard failure".into(), json!({"status": "hard_failure"})),
        (
            "a valid node".into(),
            json!({"status": "valid", "quality_micros": FULL}),
        ),
        (
            "an uncertain node".into(),
            json!({"status": "usage_uncertain"}),
        ),
        (
            "an environment failure".into(),
            json!({"status": "environment_failure"}),
        ),
        (
            "a safety rejection".into(),
            json!({"status": "safety_rejected"}),
        ),
        ("a cancelled node".into(), json!({"status": "cancelled"})),
    ];
    for kind in ["environment", "resource", "safety", "unknown"] {
        refused.push((
            format!("a repairable failure of kind {kind}"),
            repairable_status(EPISODE, kind, true, 0),
        ));
    }
    for (index, (label, status)) in refused.iter().enumerate() {
        expect_refused_everywhere(
            &env,
            &registered,
            &done,
            &original,
            &format!("bind-job-{index}"),
            &format!("a Recover of {label}"),
            &recovering(status.clone()),
        )
        .await;
    }

    // A repairable failure of a kind a repair addresses is repaired: registration and
    // `status` hold, and another job for the world converges on it.
    for kind in ["compile", "implementation", "type", "output_shape"] {
        let mut accepted = original.clone();
        recovering(repairable_status(EPISODE, kind, true, 0))(&mut accepted);
        accepted.put(&env).await;
        let label = format!("a Recover of a repairable failure of kind {kind}");
        assert_still_registered(&env, &registered, &done, &label).await;
        let again = env
            .start(&format!("bind-accepted-{kind}"), &registered)
            .await;
        assert_eq!(
            again.state,
            ManagementJobState::Succeeded,
            "{label}: {again:?}"
        );
        original.put(&env).await;
        assert_still_registered(&env, &registered, &done, &format!("restored: {label}")).await;
    }
}

// ---------------------------------------------------------------------------
// 8. The dispatch facts are read once a repairable failure is stored, never skipped
// ---------------------------------------------------------------------------

/// What the reader says of a stored dispatch fact it cannot read.
enum Unreadable {
    /// A `Conflict` whose message holds all of these.
    Conflict(Vec<String>),
    /// A listed fact that is not stored.
    NotFound,
}

/// Each way a stored dispatch fact cannot be read, with what the reader answers for it:
/// a schema it does not know, the schema before the digests (E14), the tombstone the
/// revocation cleanup leaves, and a fact that is not there at all. `None` is a fact that
/// is deleted.
fn unreadable_facts(
    original: &Value,
    dispatch_id: &str,
) -> Vec<(&'static str, Option<Value>, Unreadable)> {
    let mut unknown = original.clone();
    unknown["payload"]["schema_version"] = json!("rsia.exploration_dispatch.v3");
    let mut before_digests = original.clone();
    before_digests["payload"]["schema_version"] = json!("rsia.exploration_dispatch.v1");
    vec![
        (
            "a schema the reader does not know",
            Some(unknown),
            Unreadable::Conflict(vec!["unsupported exploration dispatch fact schema".into()]),
        ),
        (
            "the schema before the digests",
            Some(before_digests),
            Unreadable::Conflict(vec!["pre-E14 dispatch fact without policy digest".into()]),
        ),
        (
            "the tombstone of a revocation",
            Some(json!({"schema_version": "rsia.redacted.v1"})),
            Unreadable::Conflict(vec!["redacted".into(), DISPATCH.into(), dispatch_id.into()]),
        ),
        ("a fact that is gone", None, Unreadable::NotFound),
    ]
}

/// Puts the stored dispatch fact `dispatch_id` back to `body`, or deletes it.
async fn replace_fact(env: &Env, dispatch_id: &str, body: Option<&Value>) {
    match body {
        Some(body) => put_raw_record(&env.store, DISPATCH, dispatch_id, body).await,
        None => {
            let mut session = env.store.session().await.unwrap();
            session
                .delete(
                    &worker(),
                    "artifact",
                    &exploration_storage_id(DISPATCH, dispatch_id),
                )
                .await
                .unwrap();
            session.commit().await.unwrap();
        }
    }
}

/// The read side answers with the error the reader names: not an answer, not an
/// `Internal`.
fn expect_unreadable<T: std::fmt::Debug>(result: Result<T>, expected: &Unreadable, what: &str) {
    match (result, expected) {
        (Err(Error::Conflict(message)), Unreadable::Conflict(parts)) => {
            for part in parts {
                assert!(message.contains(part.as_str()), "{what}: {message}");
            }
        }
        (Err(Error::NotFound), Unreadable::NotFound) => {}
        (other, _) => panic!("{what}: expected the reader's own error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_world_that_holds_a_repairable_failure_reads_every_dispatch_fact_or_names_the_one_it_cannot()
 {
    // Once a repairable failure is stored, whatever its kind (one that cannot be repaired
    // is stored as the repairable failure it was), the recovery state is derived from the
    // dispatch facts the world lists, so the decision, the decision view and the status
    // read side need every one of them. A fact that cannot be read is neither skipped
    // (a spent repair would be given back) nor answered with a decision: the read fails
    // with the error the reader names, and reads again once the fact is back.
    let env = Env::new().await;
    for kind in ["compile", "environment"] {
        let id = format!("world-facts-{kind}");
        let registered = world(&id, &[1], HALF);
        let job = env.start(&format!("start-{id}"), &registered).await;
        let steps = env.open_failed_roots(&id, 1).await;
        make_repairable(&env, &id, 1, kind, EPISODE, true).await;
        let dispatch_id = steps[0].dispatch_id.clone().unwrap();
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        let original = raw_record(&env.store, DISPATCH, &dispatch_id).await;

        for (what, body, expected) in unreadable_facts(&original, &dispatch_id) {
            let case = format!("{id}, {what}");
            replace_fact(&env, &dispatch_id, body.as_ref()).await;
            expect_unreadable(
                env.coordinator.decide_next(&id).await,
                &expected,
                &format!("{case}: decide_next"),
            );
            expect_unreadable(
                env.coordinator.decision_view(&id).await,
                &expected,
                &format!("{case}: decision_view"),
            );
            expect_unreadable(
                verified_world_decision_view(&admin(), &env.store, &id).await,
                &expected,
                &format!("{case}: the decision view of the status read side"),
            );
            replace_fact(&env, &dispatch_id, Some(&original)).await;
            assert_eq!(
                serde_json::to_value(env.coordinator.decide_next(&id).await.unwrap()).unwrap(),
                serde_json::to_value(&decision).unwrap(),
                "{case}: restoring the fact restores the decision"
            );
            env.coordinator.decision_view(&id).await.unwrap();
        }
        assert_decisions_read(&env, &id, &job).await;
        assert_still_registered(&env, &registered, &job, &id).await;
    }
}

#[tokio::test]
async fn a_world_without_a_repairable_failure_does_not_depend_on_its_dispatch_facts_being_readable()
{
    // A world that holds no repairable failure has no recovery to derive: no completed
    // Recover can name a node that is not there, so its decision never needed its facts
    // and still does not, as it always was. Whatever else the nodes are (a hard failure,
    // a valid and trusted node offered a Deepen, an uncertain one), the same decision is
    // taken with a fact that cannot be read. `run_next` still reads every fact the world
    // lists (it checks the request against them), and says so.
    let env = Env::new().await;
    for repair in [Repair::HardFailure, Repair::Valid, Repair::Uncertain] {
        let id = format!("world-no-failure-{}", repair.name());
        let registered = world(&id, &[1], HALF);
        let job = env.start(&format!("start-{id}"), &registered).await;
        let step = env
            .step(&id, repair.model(), &env.runner(answers_all))
            .await
            .unwrap();
        let dispatch_id = step.dispatch_id.clone().unwrap();
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        let original = raw_record(&env.store, DISPATCH, &dispatch_id).await;
        let reconnect = || async {
            let parent = env.fixture.root_parent();
            env.coordinator
                .run_next(
                    None,
                    None,
                    None,
                    env.fixture.request(&parent, &id, 1, &format!("{id}-1")),
                )
                .await
        };

        for (what, body, expected) in unreadable_facts(&original, &dispatch_id) {
            let case = format!("{id}, {what}");
            replace_fact(&env, &dispatch_id, body.as_ref()).await;
            assert_eq!(
                serde_json::to_value(env.coordinator.decide_next(&id).await.unwrap()).unwrap(),
                serde_json::to_value(&decision).unwrap(),
                "{case}: the decision depends on the fact"
            );
            env.coordinator.decision_view(&id).await.unwrap();
            verified_world_decision_view(&admin(), &env.store, &id)
                .await
                .unwrap();
            expect_unreadable(
                reconnect().await,
                &expected,
                &format!("{case}: run_next reads every listed fact"),
            );
            replace_fact(&env, &dispatch_id, Some(&original)).await;
        }
        assert_still_registered(&env, &registered, &job, &id).await;
    }
}

// ---------------------------------------------------------------------------
// 9. An episode that spans two nodes: counted per episode, where the replay counts per node
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_episode_that_spans_two_nodes_is_counted_per_episode_where_the_replay_counts_per_node() {
    // The history `replay_v41` pins (`recovery_is_counted_only_when_dispatched_and_global_
    // limit_spans_episodes`): a repairable failure of episode-1 is repaired into another
    // repairable failure of episode-1, a second root fails episode-2, and the global limit
    // of one recovery keeps the second repair from being dispatched.
    //
    // The replay counts the repairs dispatched on each node: the failure that was repaired
    // has one, the repair that failed again in a repairable way has none. The coordinator
    // counts the repairs of an episode (plan §7.1: at most one dispatched recovery per
    // failed episode, the administrator's hard limit), wherever the failures of the
    // episode sit, so both nodes of episode-1 have one: the repair is not repaired
    // again. They differ on that one counter, and on no other: the repair failures and
    // the recoveries used are the replay's. No production step produces a repairable
    // failure yet, so a history that spans an episode over two nodes is written by hand;
    // aligning the two is left to the step that mints the episode of a failure (B4b).
    let env = Env::new().await;
    let id = "world-episode-spans";
    let mut registered = world(id, &[1, 2], HALF);
    // The second root costs more than a successor, so that the repair of the first root
    // is what the world decides second (a `Widen` wins an equal price against a
    // `Recover`), which is the order of the replay's history.
    registered.root_opportunities[1].estimated_cost_upper_micros = COST + 1;
    let job = env.start("start-episode-spans", &registered).await;
    let mut dispatched = Vec::new();
    for round in 0..3 {
        let step = env
            .step(id, &NoChangeModel, &env.runner(answers_all))
            .await
            .unwrap();
        dispatched.extend(dispatched_seqs(&step.decision.action));
        match round {
            // The first root fails a repairable episode ...
            0 => make_repairable(&env, id, 1, "compile", EPISODE, true).await,
            // ... its repair fails again in a repairable way, under the same episode ...
            1 => make_repairable(&env, id, 2, "compile", EPISODE, true).await,
            // ... and the second root fails another episode.
            _ => make_repairable(&env, id, 3, "compile", "episode-2", true).await,
        }
    }
    assert_eq!(
        dispatched,
        vec![1, 1_000_003, 2],
        "the order of the history"
    );

    let report = replayed_report(
        &[
            Replayed::root(1, "ctx-1", replayed_repairable("episode-1")),
            Replayed::recover(
                1,
                "episode-1",
                1,
                ("ctx-1", "ctx-2"),
                2,
                replayed_repairable("episode-1"),
            ),
            Replayed::root(2, "ctx-3", replayed_repairable("episode-2")),
            Replayed::recover(
                3,
                "episode-2",
                2,
                ("ctx-3", "ctx-4"),
                2,
                replayed_repairable("episode-2"),
            ),
        ],
        1,
    );
    let selected: Vec<u32> = report
        .batches
        .iter()
        .flat_map(|batch| batch.action_seqs.clone())
        .collect();
    assert!(
        selected.contains(&2) && !selected.contains(&4),
        "{selected:?}"
    );
    assert_eq!(report.coverage.observed_actions, 3);
    let replayed = report.revealed_prefix.expect("a replay reveals its prefix");
    // The expectations `replay_v41` pins: the failure that was repaired has one repair
    // dispatched, the repair has one repair failure, the ordinary failure has none.
    assert_eq!(
        counts(&replayed),
        vec![(1, Some(1), 0), (2, Some(0), 1), (3, Some(0), 0)]
    );
    assert_eq!(replayed.recovery_dispatches_used, 1);

    // The coordinator's prefix is the replay's but for the repairs dispatched on the repair
    // (node 2): its episode has one, it is not a repair of its own.
    let counted_by_node = prefix_counted_as(&env, id, &replayed).await;
    let mut counted_by_episode = counted_by_node.clone();
    match &mut counted_by_episode.nodes[1].status {
        ObservedStatus::RepairableFailure {
            dispatched_repairs, ..
        } => *dispatched_repairs = 1,
        other => panic!("the repair is a repairable failure: {other:?}"),
    }
    let decision = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(
        decision.prefix_digest,
        fingerprint(&counted_by_episode).unwrap(),
        "the prefix of the decision is the replay's, with the repairs of the episode on node 2"
    );
    assert_ne!(
        decision.prefix_digest,
        fingerprint(&counted_by_node).unwrap(),
        "the replay's count on node 2 (none) is not the coordinator's"
    );
    // What the difference is for: the repair of episode-1 is not offered a second repair,
    // while episode-2 still has its own.
    assert_offered(&env, id, &[recover(3, 2, 1, "episode-2")]).await;
    assert_world_reads(&env, id, &job).await;
    assert_still_registered(&env, &registered, &job, id).await;
}
