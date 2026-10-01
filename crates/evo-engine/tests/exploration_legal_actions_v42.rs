//! AG-039 (E09 legal actions and prefix hygiene; plan E09 acceptance "a successor is
//! chosen only after a real per-node evaluation", §7.1.1 "no legal action stops, the
//! world is never stuck", §7.2.1): what the coordinator offers as a legal action and
//! what `run_next` writes into the prefix, over a real SQLite store with no provider
//! and zero monetary cost.
//!
//! * a `Valid` node is deepened only when its evidence is `Trusted`: a record written
//!   before the gate existed (`NotObserved`) and a forged row labelled `FixtureDeclared`
//!   or `EvidenceRejected` are offered no successor, as they are offered as no final
//!   candidate, and a trusted one is;
//! * a parent whose Deepen was dispatched is not offered again. A Deepen's `action_seq`,
//!   and so its dispatch id, is fixed by the world and the parent, so a second selection
//!   would meet "dispatch idempotency conflict" before anything is paid and, the decision
//!   being deterministic, the world could neither advance nor stop. The Deepen is spent
//!   when a node names the parent as its search parent, not when the dispatch is claimed:
//!   a claimed Deepen stays legal and resumes;
//! * a node's best valid ancestor is what `PrefixViewV2::validate` recomputes from its
//!   parent (the parent's best valid ancestor, the baseline for a root), not the
//!   parent's own quality. A trusted root below the baseline, or a parent that regressed
//!   below its own ancestor, would otherwise leave a node the prefix check refuses, and
//!   `decide_next` and `status` would fail for good. The gains a node carries stay
//!   relative to its parent's own quality, as the replay computes them.
//!
//! The trusted path uses `ContrastRunner`, which writes honest E03 evidence with chosen
//! outputs (the budget rows, settlements, execution and grader receipts through the
//! public issuing functions, the run receipt as a typed envelope), as
//! `exploration_trust_v42` does. A candidate answers either every task (quality
//! 1000000) or `task-a` only (500000) over the two registered tasks.
//!
//! Not covered (and not claimed): the error paths after a paid step (a failure between
//! the paid step and the node's write leaves the dispatch claimed), the recovery end to
//! end (the legal-action conditions of `Recover` are only pinned unchanged), and a
//! trusted node from a runner that really executes the candidate bundle (none exists).
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
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, LegalActionV1, LegalActionsV1,
    PrefixNodeV2, PrefixViewV2, SimulationContext,
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
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, PersistentCoordinator,
    RootOpportunity, WorldState, verified_world_decision_view,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, OptimizationJournal, OptimizationStepRequest,
    PairedTaskResult, StageFact, StageFactKind, StoreOptimizationJournal,
};
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

/// Quality of a candidate that answers every task, and of one that answers `task-a`
/// only, over the two registered tasks.
const FULL: u32 = 1_000_000;
const HALF: u32 = 500_000;
/// The reason of a decision that finds nothing legal.
const STOP_REASON: &str = "no_legal_action_with_authorized_cost";
/// One more round than the 12 nodes of the online caps, so the round that stops the
/// world is seen too.
const MAX_ROUNDS: u32 = 13;

/// A world whose roots are the given slots, in that order (a slot is also its
/// branch and its action sequence), over `baseline_micros`.
fn world(id: &str, slots: &[u32], baseline_micros: u32) -> ExplorationWorldV1 {
    let mut world = world_for(id);
    world.root_opportunities = slots
        .iter()
        .map(|slot| RootOpportunity {
            root_slot: *slot,
            branch_seq: *slot,
            action_seq: *slot,
            estimated_cost_upper_micros: 10,
        })
        .collect();
    world.initial_baseline_quality_micros = baseline_micros;
    world
}

/// A candidate that answers every task: quality 1000000.
fn answers_all(_: &str) -> bool {
    true
}

/// A candidate that answers `task-a` only: quality 500000.
fn answers_task_a(task: &str) -> bool {
    task == "task-a"
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

/// What the step that derives a node ends in: a trusted valid candidate (the
/// half-answering runner makes it 500000), a plain failure (the model suggests
/// nothing) or an uncertain outcome (the model fails once the dispatch was claimed).
#[derive(Clone, Copy)]
enum Child {
    Valid,
    HardFailure,
    Uncertain,
}

impl Child {
    const ALL: [Child; 3] = [Child::Valid, Child::HardFailure, Child::Uncertain];

    fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::HardFailure => "hard-failure",
            Self::Uncertain => "uncertain",
        }
    }

    fn model(self) -> &'static dyn ModelPort {
        match self {
            Self::Valid => &EditingFixtureModel,
            Self::HardFailure => &NoChangeModel,
            Self::Uncertain => &FailingModel,
        }
    }

    /// The status the node is stored with.
    fn status(self) -> Value {
        match self {
            Self::Valid => json!({"status": "valid", "quality_micros": HALF}),
            Self::HardFailure => json!({"status": "hard_failure"}),
            Self::Uncertain => json!({"status": "usage_uncertain"}),
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
}

impl Env {
    async fn new() -> Self {
        let (dir, store) = seeded_store(true).await;
        Self {
            coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
            journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            model: CountingModel::default(),
            fixture: Fixture::new(),
            store,
            _dir: dir,
        }
    }

    async fn register(&self, world: ExplorationWorldV1) {
        self.coordinator.register_world(world).await.unwrap();
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

    /// The stored node `node_seq` of `world`.
    async fn node(&self, world: &str, node_seq: u32) -> Value {
        payload(&self.store, NODE, &format!("node-{world}-{node_seq}")).await
    }

    /// The parent a dispatch of `action_seq` is requested against: the world's own
    /// skill for a root, the candidate of the parent node for a Deepen (whose
    /// sequence is `1000000 + 2 * node_seq`).
    async fn parent_of(&self, world: &str, action_seq: u32) -> Parent {
        if action_seq < 1_000_000 {
            return self.fixture.root_parent();
        }
        assert_eq!(
            action_seq % 2,
            0,
            "only roots and Deepen are dispatched here"
        );
        let parent = self.node(world, (action_seq - 1_000_000) / 2).await;
        self.fixture.evolved_parent(
            u8::try_from(parent["node"]["depth"].as_u64().unwrap()).unwrap(),
            parent["candidate_bundle_digest"].as_str().unwrap().into(),
        )
    }

    /// One dispatch of whatever the world decides, through `run_next`: the request
    /// is built against the parent that decision names. When the decision is Stop,
    /// nothing is dispatched and `run_next` says so.
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
        let step = self.round(world).await + 1;
        self.coordinator
            .run_next(
                Some(model),
                Some(runner),
                Some(&self.journal),
                self.fixture
                    .request(&parent, world, step, &format!("{world}-{step}")),
            )
            .await
    }
}

fn dispatched_seqs(action: &BatchActionV1) -> Vec<u32> {
    match action {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs.clone(),
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

/// What a round dispatched: the action's sequence and the reason of the decision.
type Round = (u32, String);

/// Runs `world` round by round, the runner of each round chosen by `script`, until
/// the decision is Stop. Returns what each round dispatched and the reason it
/// stopped. A round that cannot dispatch what was decided is a world that can
/// neither advance nor stop.
async fn run_to_stop(
    env: &Env,
    world: &str,
    script: fn(u32) -> fn(&str) -> bool,
) -> (Vec<Round>, String) {
    let mut rounds: Vec<Round> = Vec::new();
    for round in 1..=MAX_ROUNDS {
        let decision = env
            .coordinator
            .decide_next(world)
            .await
            .unwrap_or_else(|error| {
                panic!("{world}: round {round}: decide_next failed with {error:?} after {rounds:?}")
            });
        let (action_seq, reason) = match &decision.action {
            BatchActionV1::Dispatch {
                action_seqs,
                reason,
                ..
            } => (action_seqs[0], reason.clone()),
            BatchActionV1::Stop { reason } => return (rounds, reason.clone()),
        };
        if let Err(error) = env
            .step(world, &env.model, &env.runner(script(round)))
            .await
        {
            panic!(
                "{world}: round {round} could not dispatch action {action_seq} chosen for \
                 {reason:?} ({error:?}); dispatched so far {rounds:?}"
            );
        }
        rounds.push((action_seq, reason));
    }
    panic!("{world} did not stop within {MAX_ROUNDS} rounds: {rounds:?}");
}

/// A world that stops after at least five rounds, each of which dispatched a
/// different opportunity, and that still reads.
async fn assert_runs_to_stop(env: &Env, world: &str, rounds: &[Round], reason: &str) {
    assert!(
        rounds.len() >= 5,
        "{world}: only {} rounds before the stop: {rounds:?}",
        rounds.len()
    );
    let distinct: BTreeSet<_> = rounds.iter().map(|(action_seq, _)| action_seq).collect();
    assert_eq!(distinct.len(), rounds.len(), "{world}: {rounds:?}");
    assert_eq!(reason, STOP_REASON, "{world}");
    // `run_next` reports the stop and dispatches nothing.
    let stopped = env
        .step(world, &env.model, &env.runner(answers_all))
        .await
        .unwrap();
    assert_eq!(stopped.dispatch_id, None, "{world}");
    assert_eq!(stopped.node_id, None, "{world}");
    assert_eq!(stopped.outcome, STOP_REASON, "{world}");
    assert_world_reads(env, world).await;
}

// What the coordinator derives, in the order it derives it (the roots in
// registration order, then each node's successors), written out independently.

fn widen(slot: u32) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("root-{slot}"),
        action_seq: slot,
        branch_seq: slot,
        target_depth: 1,
        kind: ActionKindV1::Widen { root_slot: slot },
        estimated_cost_upper_micros: Some(10),
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
        estimated_cost_upper_micros: Some(10),
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
        estimated_cost_upper_micros: Some(10),
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
/// each node, in the order the coordinator derives them.
async fn universe(env: &Env, world: &str) -> Vec<LegalActionV1> {
    let stored = payload(&env.store, WORLD, world).await;
    let mut all: Vec<LegalActionV1> = stored["root_opportunities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| widen(u32::try_from(root["root_slot"].as_u64().unwrap()).unwrap()))
        .collect();
    for node_id in stored["node_ids"].as_array().unwrap() {
        let node = payload(&env.store, NODE, node_id.as_str().unwrap()).await;
        let node = &node["node"];
        all.push(deepen(
            u32::try_from(node["node_seq"].as_u64().unwrap()).unwrap(),
            u32::try_from(node["branch_seq"].as_u64().unwrap()).unwrap(),
            u8::try_from(node["depth"].as_u64().unwrap()).unwrap(),
        ));
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

/// The prefix the stored world and nodes make, as evo-core's own check reads it.
async fn stored_prefix(env: &Env, world: &str) -> PrefixViewV2 {
    let stored = payload(&env.store, WORLD, world).await;
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

/// The world still reads: its decision and its `status` view are computed, and the
/// nodes it stored make a prefix evo-core's own check accepts.
async fn assert_world_reads(env: &Env, world: &str) {
    env.coordinator
        .decide_next(world)
        .await
        .unwrap_or_else(|error| panic!("{world}: decide_next failed: {error:?}"));
    verified_world_decision_view(&admin(), &env.store, world)
        .await
        .unwrap_or_else(|error| panic!("{world}: the status view failed: {error:?}"));
    stored_prefix(env, world)
        .await
        .validate()
        .unwrap_or_else(|error| {
            panic!("{world}: the prefix check refuses the stored nodes: {error:?}")
        });
}

// ---------------------------------------------------------------------------
// 1. Deepen needs trusted evidence
// ---------------------------------------------------------------------------

/// The label the stored node carries: `None` is a record written before the gate
/// existed (the field is absent), the next two are forged rows, the last is the
/// label the gate gives.
const LABELS: [(&str, Option<&str>); 4] = [
    ("legacy", None),
    ("fixture", Some("fixture_declared")),
    ("rejected", Some("evidence_rejected")),
    ("trusted", Some("trusted")),
];

/// Rewrites the label of a stored node.
async fn relabel(env: &Env, node_id: &str, label: Option<&str>) {
    rewrite_node(&env.store, node_id, |node| match label {
        Some(label) => node["evidence"] = json!(label),
        None => {
            node.as_object_mut().unwrap().remove("evidence");
        }
    })
    .await;
}

#[tokio::test]
async fn a_valid_node_is_deepened_only_when_its_evidence_is_trusted() {
    let env = Env::new().await;
    for (case, label) in LABELS {
        let id = format!("world-gate-{case}");
        env.register(world(&id, &[1], HALF)).await;
        // A genuine trusted valid node (1000000 over the 500000 baseline) whose label
        // is rewritten to what the row under test carries: the label is the only
        // difference between the cases.
        let first = env
            .step(&id, &env.model, &env.runner(answers_all))
            .await
            .unwrap();
        let node_id = first.node_id.clone().unwrap();
        relabel(&env, &node_id, label).await;
        assert_eq!(
            payload(&env.store, NODE, &node_id).await["node"]["status"],
            json!({"status": "valid", "quality_micros": FULL}),
            "{case}"
        );

        // Only the trusted one is offered a successor ...
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        if label == Some("trusted") {
            assert_eq!(dispatched_seqs(&decision.action), vec![1_000_002], "{case}");
            assert_offered(&env, &id, &[deepen(1, 1, 1)]).await;
        } else {
            match &decision.action {
                BatchActionV1::Stop { reason } => assert_eq!(reason, STOP_REASON, "{case}"),
                other => panic!(
                    "{case}: a valid node without trusted evidence was offered a successor: {other:?}"
                ),
            }
            assert_offered(&env, &id, &[]).await;
        }

        // ... and only the trusted one is proposed as a final candidate (unchanged).
        let proposed = env.coordinator.final_candidate_request(&id, &node_id).await;
        if label == Some("trusted") {
            assert_eq!(proposed.unwrap().selected_node_id, node_id, "{case}");
        } else {
            match proposed {
                Err(Error::Conflict(message)) => assert!(
                    message.contains(label.unwrap_or("not_observed")),
                    "{case}: {message}"
                ),
                other => panic!("{case}: expected a Conflict naming the label, got {other:?}"),
            }
        }
    }
}

#[tokio::test]
async fn a_valid_row_without_trusted_evidence_leaves_the_other_root_offered() {
    let env = Env::new().await;
    for (case, label) in LABELS.iter().filter(|(_, label)| *label != Some("trusted")) {
        let id = format!("world-gate-roots-{case}");
        env.register(world(&id, &[2, 1], HALF)).await;
        // The first root (slot 1, the lowest sequence) is a valid node whose
        // significant gain would focus the world on deepening it, were it trusted.
        let first = env
            .step(&id, &env.model, &env.runner(answers_all))
            .await
            .unwrap();
        assert_eq!(dispatched_seqs(&first.decision.action), vec![1], "{case}");
        relabel(&env, first.node_id.as_deref().unwrap(), *label).await;

        // The row drives nothing: the world goes on with the other root.
        assert_offered(&env, &id, &[widen(2)]).await;
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        assert_eq!(dispatched_seqs(&decision.action), vec![2], "{case}");
    }
}

// ---------------------------------------------------------------------------
// 2. A Deepen that was dispatched is not offered again
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deepen_that_was_dispatched_is_not_offered_again_so_the_world_stops() {
    let env = Env::new().await;
    for child in [Child::HardFailure, Child::Uncertain] {
        let id = format!("world-spent-{}", child.name());
        env.register(world(&id, &[1], HALF)).await;
        env.step(&id, &env.model, &env.runner(answers_all))
            .await
            .unwrap();
        assert_offered(&env, &id, &[deepen(1, 1, 1)]).await;

        // The root's Deepen is the only opportunity left, and its step ends without a
        // valid node: nothing can be deepened below it.
        let second = env
            .step(&id, child.model(), &env.runner(answers_all))
            .await
            .unwrap();
        let node = payload(&env.store, NODE, second.node_id.as_deref().unwrap()).await;
        assert_eq!(node["node"]["status"], child.status(), "{id}");
        assert_eq!(node["node"]["search_parent_seq"], 1, "{id}");

        // That Deepen was dispatched, so nothing is left to decide and the world
        // stops: in the decision, and in `run_next`, which names the stop instead of
        // meeting the spent dispatch id.
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        match &decision.action {
            BatchActionV1::Stop { reason } => assert_eq!(reason, STOP_REASON, "{id}"),
            other => {
                let attempt = env.step(&id, child.model(), &env.runner(answers_all)).await;
                panic!(
                    "{id}: the spent Deepen was offered again: {other:?}; dispatching it gives {attempt:?}"
                );
            }
        }
        assert_offered(&env, &id, &[]).await;
        let stopped = env
            .step(&id, child.model(), &env.runner(answers_all))
            .await
            .unwrap();
        assert_eq!(stopped.dispatch_id, None, "{id}");
        assert_eq!(stopped.node_id, None, "{id}");
        assert_eq!(stopped.outcome, STOP_REASON, "{id}");
        assert_world_reads(&env, &id).await;
    }
}

#[tokio::test]
async fn each_node_of_a_chain_is_offered_one_successor_up_to_the_depth_cap() {
    let env = Env::new().await;
    let id = "world-chain";
    env.register(world(id, &[1], HALF)).await;
    assert_offered(&env, id, &[widen(1)]).await;
    // Every node is trusted and valid, so each is offered a Deepen; once that was
    // dispatched only the newest node's is left, and at the depth cap (4) none.
    for depth in 1..=4u8 {
        env.step(id, &env.model, &env.runner(answers_all))
            .await
            .unwrap();
        if depth < 4 {
            assert_offered(&env, id, &[deepen(u32::from(depth), 1, depth)]).await;
        } else {
            assert_offered(&env, id, &[]).await;
        }
    }
    let decision = env.coordinator.decide_next(id).await.unwrap();
    assert!(
        matches!(&decision.action, BatchActionV1::Stop { reason } if reason == STOP_REASON),
        "{decision:?}"
    );
}

/// Every round improves on the last.
fn improves_every_round(_: u32) -> fn(&str) -> bool {
    answers_all
}

#[tokio::test]
async fn a_world_runs_many_rounds_to_stop_without_a_dispatch_conflict() {
    let env = Env::new().await;
    let id = "world-rounds";
    env.register(world(id, &[2, 1], HALF)).await;
    let (rounds, reason) = run_to_stop(&env, id, improves_every_round).await;
    // Two branches, four levels each (the depth cap), every node trusted and valid:
    // each node is deepened once, and then nothing is left to decide.
    assert_eq!(rounds.len(), 8, "{rounds:?}");
    assert_runs_to_stop(&env, id, &rounds, &reason).await;
}

/// Every round stays at the baseline: no gain anywhere, so the roots and the
/// successors wait for each other until the fairness rule forces them.
fn stays_at_the_baseline(_: u32) -> fn(&str) -> bool {
    answers_task_a
}

#[tokio::test]
async fn a_world_whose_rounds_are_forced_by_fairness_runs_to_the_node_cap_and_stops() {
    let env = Env::new().await;
    let id = "world-fairness";
    env.register(world(id, &[5, 4, 3, 2, 1], HALF)).await;
    let (rounds, reason) = run_to_stop(&env, id, stays_at_the_baseline).await;
    // The waiting actions are forced after four rounds, as a spent Deepen would be
    // had it stayed offered; the world runs to the 12 nodes of the caps instead.
    assert!(
        rounds
            .iter()
            .any(|(_, reason)| reason == "fairness_wait_threshold"),
        "no round was forced by fairness: {rounds:?}"
    );
    assert_eq!(rounds.len(), 12, "{rounds:?}");
    assert_runs_to_stop(&env, id, &rounds, &reason).await;
}

// ---------------------------------------------------------------------------
// 3. A node's best valid ancestor is the one the prefix check recomputes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_trusted_root_below_the_baseline_can_be_deepened_without_bricking_the_world() {
    let env = Env::new().await;
    for child in Child::ALL {
        let id = format!("world-below-{}", child.name());
        // The baseline is 900000 and the trusted root scores 500000, below it: the
        // best valid ancestor its successors inherit (900000) is not the root's own
        // quality.
        env.register(world(&id, &[1], 900_000)).await;
        env.step(&id, &env.model, &env.runner(answers_task_a))
            .await
            .unwrap();
        let root = env.node(&id, 1).await;
        assert_eq!(root["evidence"], "trusted", "{id}");
        assert_eq!(
            root["node"]["status"],
            json!({"status": "valid", "quality_micros": HALF}),
            "{id}"
        );
        assert_eq!(root["node"]["best_valid_ancestor_micros"], 900_000, "{id}");
        assert_eq!(root["node"]["recent_valid_gains_micros"], json!([-400_000]));

        // Deepen it, the one legal action; the step ends as `child` says.
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        assert_eq!(dispatched_seqs(&decision.action), vec![1_000_002], "{id}");
        env.step(&id, child.model(), &env.runner(answers_task_a))
            .await
            .unwrap();

        // The world still reads, and the node inherits the root's best valid ancestor.
        assert_world_reads(&env, &id).await;
        let node = env.node(&id, 2).await;
        assert_eq!(node["node"]["status"], child.status(), "{id}");
        assert_eq!(
            node["node"]["best_valid_ancestor_micros"], root["node"]["best_valid_ancestor_micros"],
            "{id}: the child's best valid ancestor is its parent's"
        );
        // The gains stay relative to the parent's own quality, as the replay computes
        // them: a valid child adds `500000 - 500000`, any other child adds nothing.
        let gains = match child {
            Child::Valid => json!([-400_000, 0]),
            Child::HardFailure | Child::Uncertain => json!([-400_000]),
        };
        assert_eq!(node["node"]["recent_valid_gains_micros"], gains, "{id}");
    }
}

#[tokio::test]
async fn a_regressing_parent_does_not_brick_its_children() {
    let env = Env::new().await;
    for child in Child::ALL {
        let id = format!("world-regress-{}", child.name());
        // Grandparent: 1000000 over the 500000 baseline.
        env.register(world(&id, &[1], HALF)).await;
        env.step(&id, &env.model, &env.runner(answers_all))
            .await
            .unwrap();
        // Parent: deepens the grandparent and regresses to 500000, below the best
        // valid ancestor it inherits (1000000).
        env.step(&id, &env.model, &env.runner(answers_task_a))
            .await
            .unwrap();
        let parent = env.node(&id, 2).await;
        assert_eq!(
            parent["node"]["status"],
            json!({"status": "valid", "quality_micros": HALF}),
            "{id}"
        );
        assert_eq!(parent["node"]["best_valid_ancestor_micros"], FULL, "{id}");
        assert_eq!(
            parent["node"]["recent_valid_gains_micros"],
            json!([500_000, -500_000]),
            "{id}"
        );
        assert_world_reads(&env, &id).await;

        // The grandparent's Deepen is spent, so the regressed parent's is the next
        // decision; its step ends as `child` says.
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        assert_eq!(dispatched_seqs(&decision.action), vec![1_000_004], "{id}");
        env.step(&id, child.model(), &env.runner(answers_task_a))
            .await
            .unwrap();

        // The world still reads, and the grandchild inherits the parent's best valid
        // ancestor, not the parent's own (lower) quality.
        assert_world_reads(&env, &id).await;
        let grandchild = env.node(&id, 3).await;
        assert_eq!(grandchild["node"]["status"], child.status(), "{id}");
        assert_eq!(
            grandchild["node"]["best_valid_ancestor_micros"],
            parent["node"]["best_valid_ancestor_micros"],
            "{id}: the child's best valid ancestor is its parent's"
        );
        let gains = match child {
            Child::Valid => json!([-500_000, 0]),
            Child::HardFailure | Child::Uncertain => json!([500_000, -500_000]),
        };
        assert_eq!(
            grandchild["node"]["recent_valid_gains_micros"], gains,
            "{id}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. What stays as it was
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_legal_actions_of_a_world_without_old_nodes_are_what_they_were() {
    let env = Env::new().await;
    // A fresh world offers its roots in registration order.
    env.register(world("world-fresh", &[2, 1], HALF)).await;
    assert_offered(&env, "world-fresh", &[widen(2), widen(1)]).await;
    // A trusted first root offers its Deepen after the roots still open.
    env.step("world-fresh", &env.model, &env.runner(answers_all))
        .await
        .unwrap();
    assert_offered(&env, "world-fresh", &[widen(2), deepen(1, 1, 1)]).await;
    // A root that failed offers no successor.
    for child in [Child::HardFailure, Child::Uncertain] {
        let id = format!("world-fresh-{}", child.name());
        env.register(world(&id, &[2, 1], HALF)).await;
        env.step(&id, child.model(), &env.runner(answers_all))
            .await
            .unwrap();
        assert_offered(&env, &id, &[widen(2)]).await;
    }
}

#[tokio::test]
async fn the_legal_action_conditions_of_recover_are_unchanged() {
    let env = Env::new().await;
    // A failed root rewritten into a repairable failure: Recover is offered while the
    // environment was reset and the episode has repairs left, and only then.
    let cases = [
        ("open", true, 0, true),
        ("not-reset", false, 0, false),
        ("spent", true, 1, false),
    ];
    for (case, environment_reset, dispatched_repairs, offered) in cases {
        let id = format!("world-recover-{case}");
        env.register(world(&id, &[1], HALF)).await;
        let first = env
            .step(&id, Child::HardFailure.model(), &env.runner(answers_all))
            .await
            .unwrap();
        rewrite_node(&env.store, first.node_id.as_deref().unwrap(), |node| {
            node["node"]["status"] = json!({
                "status": "repairable_failure",
                "episode_id": "episode-1",
                "failure_kind": "compile",
                "repair_template_digest": d("repair-template"),
                "environment_reset": environment_reset,
                "dispatched_repairs": dispatched_repairs,
            });
        })
        .await;
        let expected = if offered {
            vec![recover(1, 1, 1, "episode-1")]
        } else {
            vec![]
        };
        let decision = env.coordinator.decide_next(&id).await.unwrap();
        assert_eq!(
            decision.legal_actions_digest,
            legal_digest(&expected),
            "{case}"
        );
        assert_eq!(
            matches!(decision.action, BatchActionV1::Dispatch { .. }),
            offered,
            "{case}"
        );
    }
}

#[tokio::test]
async fn a_claimed_deepen_stays_legal_and_resumes_without_paying_again() {
    let env = Env::new().await;
    let id = "world-claimed-deepen";
    env.register(world(id, &[1], HALF)).await;
    env.step(id, &env.model, &env.runner(answers_all))
        .await
        .unwrap();
    let before = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(dispatched_seqs(&before.action), vec![1_000_002]);

    // The Deepen is dispatched and the process is lost after the runner's report
    // was saved and before the step observed it: the dispatch stays claimed and no
    // node names the parent yet.
    let parent = env.parent_of(id, 1_000_002).await;
    let (hanging, entered) = HangingJournal::new(&env.store, StageFactKind::DevelopmentObserved);
    let runner = CallCounting::new(env.runner(answers_all));
    tokio::select! {
        biased;
        result = env.coordinator.run_next(
            Some(&env.model),
            Some(&runner),
            Some(&hanging),
            env.fixture.request(&parent, id, 2, "claimed-deepen-2"),
        ) => panic!("the hanging journal must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    assert_eq!(runner.calls(), 1, "the paid step ran once");
    let stored = payload(&env.store, WORLD, id).await;
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 1);
    let dispatch_ids = stored["dispatch_ids"].as_array().unwrap();
    assert_eq!(dispatch_ids.len(), 2);
    let claimed = payload(&env.store, DISPATCH, dispatch_ids[1].as_str().unwrap()).await;
    assert_eq!(claimed["state"], "claimed");
    assert_eq!(claimed["action_seq"], 1_000_002);

    // The claimed Deepen is not spent: it is still legal, and still what the world
    // decides, with the same digests the claim recorded.
    let during = env.coordinator.decide_next(id).await.unwrap();
    assert_eq!(dispatched_seqs(&during.action), vec![1_000_002]);
    assert_eq!(during.legal_actions_digest, before.legal_actions_digest);
    assert_eq!(during.prefix_digest, before.prefix_digest);
    assert_offered(&env, id, &[deepen(1, 1, 1)]).await;

    // A new `run_next` finishes the claimed dispatch: the saved report and the
    // model answers are replayed, nothing is paid for again.
    let model_calls = env.model.calls();
    let replay = CallCounting::new(env.runner(answers_all));
    let finished = env
        .coordinator
        .run_next(
            Some(&env.model),
            Some(&replay),
            Some(&env.journal),
            env.fixture.request(&parent, id, 2, "claimed-deepen-2"),
        )
        .await
        .unwrap();
    assert_eq!(replay.calls(), 0, "the recovery journal replays the report");
    assert_eq!(env.model.calls(), model_calls);
    assert_eq!(finished.outcome, OBSERVED_REASON);
    assert_eq!(finished.dispatch_id.as_deref(), claimed["id"].as_str());

    // The resumed step wrote the node the claimed Deepen was to derive, and the
    // dispatch is consumed.
    let child = env.node(id, 2).await;
    assert_eq!(child["node"]["search_parent_seq"], 1);
    assert_eq!(child["node"]["depth"], 2);
    assert_eq!(
        child["node"]["status"],
        json!({"status": "valid", "quality_micros": FULL})
    );
    assert_eq!(child["evidence"], "trusted");
    let consumed = payload(&env.store, DISPATCH, dispatch_ids[1].as_str().unwrap()).await;
    assert_eq!(consumed["state"], "observed");
    assert_eq!(consumed["node_id"], finished.node_id.clone().unwrap());
}
