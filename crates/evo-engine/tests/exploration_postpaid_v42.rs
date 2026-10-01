//! AG-041 (E09 PR-B step B1; plan §7.2.1 "after a dispatch a cancel, a crash and an
//! uncertain usage all count as a used opportunity, never repeated automatically",
//! §6.7.2 "a mismatch ends in an explicit terminal state", V086.d, V096.a "a fault
//! inside a stage is never a dangling state"): every path after the step was paid for
//! ends in a terminal state, and a failure that can be known beforehand is refused
//! before anything is paid.
//!
//! * the claim of a dispatch is committed before the step is paid for, and the step is
//!   not undone by an error afterwards: an `Err` there left the claim open for good
//!   (the dispatch id depends only on the world and the action, so a retry meets
//!   "dispatch idempotency conflict" and a resume meets the same guard again). So a
//!   world rewritten while the step ran, a claim whose inputs changed before it
//!   resumed and a Recover target that is gone each end in a terminal `Uncertain` node
//!   (exactly one node, the immutable cost of its action spent, the node, its edge to
//!   the world, the dispatch fact and the world written together). The lagging one of
//!   two resumers of the same request returns the winner's terminal state;
//! * a node id that would not be an identifier (`node-{world}-{seq}` past 128 bytes) is
//!   refused before the claim is written: nothing is paid for a step whose node cannot
//!   be written. Registration refuses a world id past 120 bytes, the longest that
//!   names every node a world may hold;
//! * a new world starts at the initial scheduling state: registration refuses a
//!   pre-filled decision round, current branch, branch focus and waits, which no
//!   registration fingerprint covers and which change the first decision it reports;
//! * the registration and `status` of a started world hold each completed `Widen` or
//!   `Deepen` fact to its node: the search parent, the depth and the branch are the
//!   ones the fact's action derives. A root dispatch rewritten into a `Deepen`
//!   together with the counters that would pay for it is refused (it balanced the
//!   books before);
//! * none of the new terminal states leaves a world `register_world_idempotent` or
//!   the management `status` of its `exploration.start` job refuses (the Recover
//!   target test excepted: its world was tampered with to make the target vanish).
//!
//! Not covered (and not claimed): the recovery of a repairable failure end to end
//! (AG-042: no step produces a repairable failure yet, so a `Recover` fact is not yet
//! held to its node, and the Recover here is made by rewriting a stored node), and a
//! transient storage error after the step was paid for, which still leaves the
//! dispatch claimed until the original request resumes it. Real SQLite store, fixture
//! model and runner, no provider, zero monetary cost.
//!
//! Fixtures are copied from `exploration_start_after_dispatch_v42.rs` and
//! `exploration_trust_v42.rs`; those files are unchanged.
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
    BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, OpportunityWait, SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, MechanismUsageState,
    PersistentCoordinator, RegisterWorldOutcome, RootOpportunity, WorldState,
    exploration_world_storage_id,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationStepRequest,
    PairedTaskResult, StoreOptimizationJournal,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

/// What the world is registered with. The two root opportunities cost different
/// amounts on purpose: what a dispatch spent is read from its own fact, never
/// computed from a flat rate.
const REGISTERED_ROOT_MICROS: u64 = 1_000;
const REGISTERED_RECOVERY_DISPATCHES: u8 = 2;
const FIRST_ROOT_COST: u64 = 10;
const SECOND_ROOT_COST: u64 = 25;
/// The cost of a `Deepen` or a `Recover`. It differs from both root costs, so that a
/// cost bound to the wrong kind of action shows.
const SUCCESSOR_COST: u64 = 15;
const NODE_KIND: &str = "exploration_node_v1";
const WORLD_KIND: &str = "exploration_world_v1";
const RUNS: [&str; 2] = ["run-failure", "run-success"];

// ---------------------------------------------------------------------------
// Fixtures: the trusted source closure, the world and the optimization step
// ---------------------------------------------------------------------------

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

/// One Host-issued trusted run. A run stored after the watermark was bumped is
/// live under it too: the world's source closure is checked by id.
async fn store_trusted_run(store: &Store, id: &str, family: &str, outcome: TraceOutcome) {
    let host = Context::new("n", "host", Role::Host).unwrap();
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
    store_trace_authority(store, &host, &authority)
        .await
        .unwrap();
}

async fn seeded_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("exploration-start-after-dispatch.sqlite3"))
        .await
        .unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        store_trusted_run(&store, id, family, outcome).await;
    }
    store_source_selection(
        &store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: vec!["run-failure".into(), "run-success".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    (dir, store)
}

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

/// A model port that never answers. Dropping the `run_next` future once it was
/// entered simulates a crash after the dispatch was claimed and before it was
/// observed: the fact stays `claimed`.
struct HangingModel {
    entered: Arc<Notify>,
}

#[async_trait]
impl ModelPort for HangingModel {
    async fn dispatch(&self, _: ModelRequest) -> Result<ModelResponse> {
        self.entered.notify_one();
        std::future::pending().await
    }
}

struct ImprovingFixtureRunner;

#[async_trait]
impl DevRunner for ImprovingFixtureRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results: vec![PairedTaskResult {
                task_id: "task".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: 900_000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: "fixture-parent-execution".into(),
                candidate_execution_id: "fixture-candidate-execution".into(),
                grader_receipt_digest: hash(b"fixture-grade"),
            }],
            execution_receipt_id: "fixture-development-execution".into(),
            usage_record_ids: vec!["fixture-development-usage".into()],
            provenance: DevelopmentExecutionProvenance::Fixture,
        })
    }
}

/// Everything an `OptimizationStepRequest` borrows, built once.
struct Fixture {
    evidence: EvidenceSet,
    source_selection: SourceSelection,
    bindings: Vec<TrustedSourceBinding>,
    parent: SkillSnapshot,
    edit_context: TrustedEditContext,
    profile: Profile,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
    allowed: Vec<EvidenceRef>,
    parent_bundle: String,
}

impl Fixture {
    fn new() -> Self {
        let evidence = EvidenceSet::build(
            "evidence",
            vec![
                EvidenceMember {
                    source_id: "run-failure".into(),
                    content_digest: hash(b"run-failure"),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                EvidenceMember {
                    source_id: "run-success".into(),
                    content_digest: hash(b"run-success"),
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
        let parent = parent_skill();
        let allowed = vec![
            EvidenceRef {
                id: "run-failure".into(),
                digest: hash(b"run-failure"),
            },
            EvidenceRef {
                id: "run-success".into(),
                digest: hash(b"run-success"),
            },
        ];
        let edit_context = TrustedEditContext::new(
            "n",
            "profile",
            "skill",
            "v1",
            hash(b"approved-parent"),
            hash(b"baseline"),
            &parent,
            allowed.clone(),
        )
        .unwrap();
        Self {
            evidence,
            source_selection: SourceSelection {
                roots: vec![],
                run_ids: vec!["run-failure".into(), "run-success".into()],
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            },
            bindings: vec![
                TrustedSourceBinding {
                    source_id: "run-failure".into(),
                    source_digest: hash(b"run-failure"),
                    parent_family: "family-a".into(),
                },
                TrustedSourceBinding {
                    source_id: "run-success".into(),
                    source_digest: hash(b"run-success"),
                    parent_family: "family-b".into(),
                },
            ],
            baseline: parent.clone(),
            parent,
            edit_context,
            profile: Profile {
                id: "profile".into(),
                evolution_enabled: true,
                parent_digest: hash(b"approved-parent"),
                baseline_digest: hash(b"baseline"),
            },
            parent_strategy: Strategy::default(),
            baseline_strategy: Strategy::default(),
            improver: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
            allowed,
            parent_bundle: hash(b"parent-bundle"),
        }
    }

    /// The edit context of a skill other than the world's own parent: the parent
    /// of a `Deepen` is the candidate of an earlier node.
    fn edit_context_for(&self, skill: &SkillSnapshot) -> TrustedEditContext {
        TrustedEditContext::new(
            "n",
            "profile",
            "skill",
            "v1",
            hash(b"approved-parent"),
            hash(b"baseline"),
            skill,
            self.allowed.clone(),
        )
        .unwrap()
    }

    /// A request against the world's own parent skill/bundle, i.e. for a
    /// `Widen` action. `tag` keeps the request/idempotency ids distinct.
    fn request(&self, world_id: &str, step: u32, tag: &str) -> OptimizationStepRequest<'_> {
        self.request_for(
            &self.parent,
            &self.edit_context,
            &self.parent_bundle,
            world_id,
            step,
            tag,
        )
    }

    /// A request against `parent` (skill, its edit context and bundle): the world's
    /// own for a `Widen`, the candidate of the parent node for a `Deepen`.
    fn request_for<'a>(
        &'a self,
        parent: &'a SkillSnapshot,
        edit_context: &'a TrustedEditContext,
        bundle: &str,
        world_id: &str,
        step: u32,
        tag: &str,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.bindings,
            traces: vec![
                trace("run-failure", "family-a", TraceOutcome::TaskFailure),
                trace("run-success", "family-b", TraceOutcome::Success),
            ],
            model_context: ModelRequestContext {
                request_id: format!("optimizer-{tag}"),
                namespace: "n".into(),
                purpose: Purpose::Development,
                stage: ModelStage::ReflectFailure,
                episode_id: world_id.into(),
                step,
                attempt: 1,
                parent_skill_digest: skill_snapshot_digest(parent).unwrap(),
                bundle_digest: bundle.to_string(),
                source_closure: self.allowed.clone(),
                model_digest: hash(b"model"),
                tools_digest: hash(b"tools"),
                rules_digest: hash(b"rules"),
                sampling_digest: hash(b"sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: parent,
            edit_context,
            edit_batch_template: SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: "n".into(),
                profile_id: "profile".into(),
                skill_id: "skill".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(parent).unwrap(),
                approved_parent_digest: hash(b"approved-parent"),
                safe_baseline_digest: hash(b"baseline"),
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
                namespace: "n".into(),
                purpose: Purpose::Development,
                episode_id: world_id.into(),
                step,
                attempt: 1,
                manifest: DevelopmentManifest::build(
                    format!("manifest-{step}"),
                    vec![DevelopmentTask {
                        id: "task".into(),
                        parent_family: "family-a".into(),
                        input_digest: hash(b"task"),
                    }],
                )
                .unwrap(),
                parent_bundle_digest: bundle.to_string(),
                candidate_bundle_digest: hash(format!("candidate-{tag}").as_bytes()),
                environment_digest: hash(b"environment"),
                grader_digest: hash(b"grader"),
                rules_digest: hash(b"rules"),
                tools_digest: hash(b"tools"),
                revoke_watermark: 1,
                idempotency_key: format!("dev-idempotency-{tag}"),
            },
            allow_rank_call: false,
        }
    }
}

/// The world of `dispatch_management::exploration_world`, with the root budget
/// and the root costs this file measures.
fn world_for(id: &str) -> ExplorationWorldV1 {
    let parent_skill_digest = skill_snapshot_digest(&parent_skill()).unwrap();
    let parent_bundle = hash(b"parent-bundle");
    let environment = hash(b"environment");
    let model = hash(b"model");
    let tools = hash(b"tools");
    let grader = hash(b"grader");
    let rules = hash(b"rules");
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
        approved_parent_digest: hash(b"approved-parent"),
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
                estimated_cost_upper_micros: SECOND_ROOT_COST,
            },
            RootOpportunity {
                root_slot: 1,
                branch_seq: 1,
                action_seq: 1,
                estimated_cost_upper_micros: FIRST_ROOT_COST,
            },
        ],
        dependencies: RUNS
            .iter()
            .map(|run| ExplorationDependency {
                kind: "run".into(),
                id: (*run).into(),
            })
            .collect(),
        successor_cost_upper_micros: SUCCESSOR_COST,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: REGISTERED_ROOT_MICROS,
        remaining_recovery_dispatches: REGISTERED_RECOVERY_DISPATCHES,
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
    Context::new("n", "admin", Role::Admin).unwrap()
}

fn dispatched_seq(decision: &BatchActionV1) -> u32 {
    match decision {
        BatchActionV1::Dispatch { action_seqs, .. } => {
            assert_eq!(action_seqs.len(), 1);
            action_seqs[0]
        }
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    /// Registers worlds and drives `run_next`, as the exploration worker does.
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    /// The Admin management surface `exploration.start` runs on.
    dispatcher: ManagementDispatcher,
    fixture: Fixture,
}

async fn env() -> Env {
    let (dir, store) = seeded_store().await;
    Env {
        coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
        journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
        dispatcher: ManagementDispatcher::new(store.clone(), vec![admin()]).unwrap(),
        fixture: Fixture::new(),
        store,
        _dir: dir,
    }
}

impl Env {
    async fn register(&self, world: &ExplorationWorldV1) -> Result<RegisterWorldOutcome> {
        self.coordinator
            .register_world_idempotent(world.clone())
            .await
    }

    /// One real dispatch through `run_next`, observed with a fixture runner: the
    /// node is a terminal fixture-declared one, which is all these tests need,
    /// because what they measure is that the dispatch spent budget.
    async fn step(&self, world_id: &str, step: u32, tag: &str) -> CoordinatorStepResult {
        self.run(
            &EditingFixtureModel,
            &ImprovingFixtureRunner,
            world_id,
            step,
            tag,
        )
        .await
        .unwrap()
    }

    /// One dispatch through `run_next` with the given ports, the result as it is: a
    /// paid step is never to end in an `Err`, so the tests that read it do not unwrap.
    async fn run(
        &self,
        model: &dyn ModelPort,
        runner: &dyn DevRunner,
        world_id: &str,
        step: u32,
        tag: &str,
    ) -> Result<CoordinatorStepResult> {
        self.coordinator
            .run_next(
                Some(model),
                Some(runner),
                Some(&self.journal),
                self.fixture.request(world_id, step, tag),
            )
            .await
    }

    /// The same request again with no ports at all: what a client reconnecting to a
    /// dispatch that already has its terminal state sends.
    async fn reconnect(
        &self,
        world_id: &str,
        step: u32,
        tag: &str,
    ) -> Result<CoordinatorStepResult> {
        self.coordinator
            .run_next(None, None, None, self.fixture.request(world_id, step, tag))
            .await
    }

    /// `count` real dispatches in a row (at most two: the world has two roots and
    /// a fixture node is terminal, so nothing else is legal afterwards).
    async fn dispatches(&self, world_id: &str, count: u32) -> Vec<CoordinatorStepResult> {
        let mut steps = Vec::new();
        for step in 1..=count {
            steps.push(
                self.step(world_id, step, &format!("{world_id}-{step}"))
                    .await,
            );
        }
        steps
    }

    /// Rewrites a stored node as the observation of a trusted development report:
    /// valid at `quality_micros`, with that gain over the baseline of the world. A
    /// `Deepen` is only offered over such a node, and the fixture runner's report is
    /// not one (it is a fixture); the node is the only thing rewritten.
    async fn promote_to_trusted_valid(&self, node_id: &str, quality_micros: u32, gain_micros: i32) {
        let mut node = raw_record(&self.store, NODE_KIND, node_id).await;
        let payload = &mut node["payload"];
        payload["evidence"] = json!("trusted");
        payload["node"]["status"] = json!({"status": "valid", "quality_micros": quality_micros});
        payload["node"]["best_valid_ancestor_micros"] = json!(quality_micros);
        payload["node"]["recent_valid_gains_micros"] = json!([gain_micros]);
        put_raw_record(&self.store, NODE_KIND, node_id, &node).await;
    }

    /// One real `Deepen` of the node `parent_node_id` through `run_next`. Its parent is
    /// the candidate of that node: the world's skill with the fixture model's two
    /// edits applied, and the bundle the node recorded.
    async fn deepen(
        &self,
        world_id: &str,
        parent_node_id: &str,
        step: u32,
        tag: &str,
    ) -> CoordinatorStepResult {
        self.successor_with(
            Some(&EditingFixtureModel),
            Some(&ImprovingFixtureRunner),
            world_id,
            parent_node_id,
            step,
            tag,
        )
        .await
        .unwrap()
    }

    /// A step against the candidate of node `parent_node_id` (the parent of a `Deepen`
    /// or of a `Recover`) with the given ports (none at all is a reconnect), the
    /// result as it is.
    async fn successor_with(
        &self,
        model: Option<&dyn ModelPort>,
        runner: Option<&dyn DevRunner>,
        world_id: &str,
        parent_node_id: &str,
        step: u32,
        tag: &str,
    ) -> Result<CoordinatorStepResult> {
        let node = raw_record(&self.store, NODE_KIND, parent_node_id).await;
        let bundle = node["payload"]["candidate_bundle_digest"]
            .as_str()
            .unwrap()
            .to_string();
        let mut skill = parent_skill();
        skill.content.push_str(" [repair]");
        skill.applicability.push_str(" [repair]");
        let edit_context = self.fixture.edit_context_for(&skill);
        self.coordinator
            .run_next(
                model,
                runner,
                Some(&self.journal),
                self.fixture
                    .request_for(&skill, &edit_context, &bundle, world_id, step, tag),
            )
            .await
    }

    /// The two dispatches of a world that deepened: a `Widen` of the first root, whose
    /// node is then taken for a trusted valid observation, and the `Deepen` of it.
    async fn widen_then_deepen(&self, world_id: &str) -> Vec<CoordinatorStepResult> {
        let widen = self.step(world_id, 1, &format!("{world_id}-1")).await;
        let node_id = widen.node_id.clone().unwrap();
        self.promote_to_trusted_valid(&node_id, 900_000, 400_000)
            .await;
        let deepen = self
            .deepen(world_id, &node_id, 2, &format!("{world_id}-2"))
            .await;
        assert_eq!(dispatched_seq(&deepen.decision.action), 1_000_002);
        vec![widen, deepen]
    }

    async fn start_job(&self, request_key: &str, world: &ExplorationWorldV1) -> ManagementJob {
        start_job(&self.dispatcher, &admin(), request_key, world).await
    }

    /// The stored world, as the store holds it.
    async fn world(&self, world_id: &str) -> Value {
        raw_record(&self.store, WORLD_KIND, world_id).await["payload"].clone()
    }

    /// Everything the namespace holds under the kinds a registration or a status
    /// read could write: if a call changed none of it, it wrote nothing.
    async fn snapshot(&self) -> Vec<Value> {
        let mut session = self.store.session().await.unwrap();
        let mut all = Vec::new();
        for kind in ["artifact", "job", "tombstone"] {
            all.push(json!(session.list::<Value>(&admin(), kind).await.unwrap()));
        }
        session.commit().await.unwrap();
        all
    }

    /// The edges that make the world reach its source runs and its dispatch
    /// records (what a revocation cleanup walks).
    async fn world_dependents(&self, world_id: &str) -> BTreeSet<(String, String)> {
        let mut session = self.store.session().await.unwrap();
        let storage = exploration_world_storage_id(world_id).unwrap();
        let mut edges: BTreeSet<(String, String)> = session
            .dependents(&admin(), "artifact", &storage)
            .await
            .unwrap()
            .into_iter()
            .collect();
        for run in RUNS {
            edges.extend(session.dependents(&admin(), "run", run).await.unwrap());
        }
        session.commit().await.unwrap();
        edges
    }
}

/// The counters of a stored world: `(remaining_root_micros,
/// remaining_recovery_dispatches)`.
fn counters(world: &Value) -> (u64, u64) {
    (
        world["remaining_root_micros"].as_u64().unwrap(),
        world["remaining_recovery_dispatches"].as_u64().unwrap(),
    )
}

/// The result a succeeded `exploration.start` job carries, as the wire shows it.
fn started_result(job: &ManagementJob) -> Value {
    assert!(
        matches!(
            job.result,
            Some(ManagementResult::ExplorationStarted { .. })
        ),
        "not an exploration.start result: {job:?}"
    );
    serde_json::to_value(&job.result).unwrap()
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

// ---------------------------------------------------------------------------
// Raw store access, to tamper with persisted records the way a corrupt store or
// a hostile writer could.
// ---------------------------------------------------------------------------

fn storage_id(record_kind: &str, id: &str) -> String {
    format!("e09-{}", fingerprint(&(record_kind, id)).unwrap())
}

fn worker() -> Context {
    Context::new("n", "worker", Role::Worker).unwrap()
}

async fn raw_record(store: &Store, record_kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value: Value = session
        .need(&worker(), "artifact", &storage_id(record_kind, id))
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
            &storage_id(record_kind, id),
            "worker",
            value,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn raw_fact(store: &Store, dispatch_id: &str) -> Value {
    raw_record(store, "exploration_dispatch_v1", dispatch_id).await
}

async fn put_raw_fact(store: &Store, dispatch_id: &str, value: &Value) {
    put_raw_record(store, "exploration_dispatch_v1", dispatch_id, value).await;
}

// ---------------------------------------------------------------------------
// The management surface
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

// ---------------------------------------------------------------------------
// Ports that count their calls, rewrite the world, or hold their answer
// ---------------------------------------------------------------------------

/// Counts every model dispatch: a reconnect and a claim that ended without its step
/// being run again must not reach one.
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

/// Counts every development run.
#[derive(Default)]
struct CountingRunner(AtomicUsize);

impl CountingRunner {
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DevRunner for CountingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.0.fetch_add(1, Ordering::SeqCst);
        ImprovingFixtureRunner.run(request).await
    }
}

/// A runner that edits the stored world while the step is being paid for, then
/// answers like the fixture runner. The claim of the dispatch is committed by then,
/// so this is the world moving under a dispatch in flight (a second writer, a repair
/// tool, a restore): whatever the step produces has to be recorded on the world as it
/// is, as a terminal state.
struct RewritingRunner {
    store: Store,
    world_id: String,
    edit: fn(&mut Value),
    calls: AtomicUsize,
}

impl RewritingRunner {
    fn new(store: &Store, world_id: &str, edit: fn(&mut Value)) -> Self {
        Self {
            store: store.clone(),
            world_id: world_id.into(),
            edit,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DevRunner for RewritingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut world = raw_record(&self.store, WORLD_KIND, &self.world_id).await;
        (self.edit)(&mut world["payload"]);
        put_raw_record(&self.store, WORLD_KIND, &self.world_id, &world).await;
        ImprovingFixtureRunner.run(request).await
    }
}

/// A model that holds its first answer until the test releases it, so that the step
/// stays in flight while another resumer of the same request runs to its end. Later
/// calls (the step asks once per reflection batch) answer at once.
struct GatedModel {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    calls: AtomicUsize,
}

impl GatedModel {
    fn new(entered: Arc<Notify>, release: Arc<Notify>) -> Self {
        Self {
            entered,
            release,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelPort for GatedModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        EditingFixtureModel.dispatch(request).await
    }
}

// ---------------------------------------------------------------------------
// Worlds and what a paid dispatch leaves behind
// ---------------------------------------------------------------------------

/// The reasons a paid dispatch is settled with when its normal outcome cannot be
/// recorded. Fixed literals: they are part of what the dispatch fact stores.
const PREFIX_CHANGED: &str = "exploration_prefix_changed_after_dispatch";
const CLAIM_INPUTS_CHANGED: &str = "claim_inputs_changed_after_dispatch";
const RECOVER_TARGET_MISSING: &str = "recover_target_missing";

/// A world with the first root only, so that nothing but the successors of its node
/// is left to dispatch.
fn single_root_world(id: &str) -> ExplorationWorldV1 {
    let mut world = world_for(id);
    world.root_opportunities.retain(|root| root.root_slot == 1);
    world
}

/// A world id of exactly `len` bytes.
fn long_world_id(len: usize) -> String {
    let id = format!("long-{}", "x".repeat(len - "long-".len()));
    assert_eq!(id.len(), len);
    id
}

/// Persists `world` straight into the store, the way a world that was registered
/// before the id bound existed is there: registration refuses it now.
async fn seed_world_record(store: &Store, world: &ExplorationWorldV1) {
    put_raw_record(
        store,
        WORLD_KIND,
        &world.id,
        &json!({
            "schema_version": "rsia.exploration_artifact_envelope.v1",
            "id": storage_id(WORLD_KIND, &world.id),
            "record_kind": WORLD_KIND,
            "payload": serde_json::to_value(world).unwrap(),
        }),
    )
    .await;
}

/// The stored node of sequence `seq` of a world (`node-{world}-{seq}`).
async fn stored_node(store: &Store, world_id: &str, seq: u32) -> Value {
    raw_record(store, NODE_KIND, &format!("node-{world_id}-{seq}")).await["payload"].clone()
}

/// The stored dispatch fact.
async fn stored_fact(store: &Store, dispatch_id: &str) -> Value {
    raw_fact(store, dispatch_id).await["payload"].clone()
}

/// The node a dispatch settled as a terminal `Uncertain`: where it sits in the prefix.
struct Settled<'a> {
    reason: &'a str,
    node_seq: u32,
    search_parent: Option<u32>,
    branch_seq: u32,
    depth: u32,
}

/// What a dispatch that was settled as a terminal `Uncertain` leaves behind: the
/// step result names the node and the reason, the dispatch fact is `uncertain` and
/// names the same node and reason, the node is `UsageUncertain` with no gate verdict
/// and no candidate, it sits where the dispatched action derives it, and it has its
/// edge to the world (the revocation cleanup reaches a record through that edge).
async fn assert_settled_uncertain(
    env: &Env,
    world_id: &str,
    step: &CoordinatorStepResult,
    settled: Settled<'_>,
) {
    assert_eq!(step.outcome, settled.reason);
    let node_id = step
        .node_id
        .clone()
        .expect("a settled dispatch names its node");
    assert_eq!(node_id, format!("node-{world_id}-{}", settled.node_seq));
    let dispatch_id = step
        .dispatch_id
        .clone()
        .expect("a settled dispatch names itself");
    let fact = stored_fact(&env.store, &dispatch_id).await;
    assert_eq!(fact["state"], "uncertain");
    assert_eq!(fact["node_id"], json!(node_id));
    assert_eq!(fact["outcome_reason"], settled.reason);
    assert_eq!(fact["evidence"], "not_observed");
    let node = stored_node(&env.store, world_id, settled.node_seq).await;
    assert_eq!(node["node"]["status"], json!({"status": "usage_uncertain"}));
    assert_eq!(node["evidence"], "not_observed");
    assert!(
        node["candidate_bundle_digest"].is_null()
            && node["candidate_skill_digest"].is_null()
            && node["development_selection_digest"].is_null(),
        "an uncertain node names no candidate: {node}"
    );
    assert_eq!(node["node"]["node_seq"], settled.node_seq);
    assert_eq!(
        node["node"]["search_parent_seq"],
        json!(settled.search_parent)
    );
    assert_eq!(node["node"]["branch_seq"], settled.branch_seq);
    assert_eq!(node["node"]["depth"], settled.depth);
    let edges = env.world_dependents(world_id).await;
    assert!(
        edges.contains(&("artifact".to_string(), storage_id(NODE_KIND, &node_id))),
        "the node has no edge to its world: {edges:?}"
    );
}

/// The registration and the management `status` of the world's `exploration.start`
/// job still hold (AG-040): the new terminal states are states a started world may be
/// in.
async fn assert_still_registered(
    env: &Env,
    world: &ExplorationWorldV1,
    job: &ManagementJob,
    when: &str,
) {
    expect_already_registered(env.register(world).await, when);
    expect_status_ok(&env.dispatcher, job, when).await;
}

// ---------------------------------------------------------------------------
// 1. A world rewritten while the step was paid for
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_world_rewritten_while_the_step_was_paid_for_ends_in_a_terminal_uncertain_node() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("drift-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");

    // The claim is committed, the step is paid for, and while it runs the stored
    // world is rewritten: the decision round is not the one the decision was taken at.
    let model = CountingModel::default();
    let runner = RewritingRunner::new(&env.store, "world-1", |world| {
        world["decision_round"] = json!(3);
    });
    let step = env
        .run(&model, &runner, "world-1", 1, "drift-1")
        .await
        .expect("a step that was paid for never ends in an error: its claim has to be settled");
    // The step was paid for once: the model answered (once per reflection batch) and
    // the runner ran. Nothing below may reach either again.
    let paid = (model.calls(), runner.calls());
    assert!(paid.0 >= 1 && paid.1 == 1, "{paid:?}");
    assert_eq!(dispatched_seq(&step.decision.action), 1);
    assert_settled_uncertain(
        &env,
        "world-1",
        &step,
        Settled {
            reason: PREFIX_CHANGED,
            node_seq: 1,
            search_parent: None,
            branch_seq: 1,
            depth: 1,
        },
    )
    .await;

    // Exactly one node, the immutable cost of the action spent, the round advanced
    // from where the world was found, and the waits taken from the legal set as it is.
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"], json!(["node-world-1-1"]));
    assert_eq!(stored["dispatch_ids"], json!([step.dispatch_id.clone()]));
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        )
    );
    assert_eq!(stored["decision_round"], 4);
    assert_eq!(stored["current_branch_seq"], 1);
    assert_eq!(stored["current_branch_focus_actions"], 1);
    assert_eq!(
        stored["waits"],
        json!([{"action_seq": 2, "waited_rounds": 1}])
    );

    // The claim is consumed: a reconnect gets the same terminal state, with and
    // without ports, and no port is reached again.
    let again = env.reconnect("world-1", 1, "drift-1").await.unwrap();
    assert_eq!(again.dispatch_id, step.dispatch_id);
    assert_eq!(again.node_id, step.node_id);
    assert_eq!(again.outcome, PREFIX_CHANGED);
    let with_ports = env
        .run(&model, &runner, "world-1", 1, "drift-1")
        .await
        .unwrap();
    assert_eq!(with_ports.node_id, step.node_id);
    assert_eq!(with_ports.outcome, PREFIX_CHANGED);
    assert_eq!((model.calls(), runner.calls()), paid);

    // The usage view counts the dispatch (it was paid for), and the world goes on
    // with the next root instead of meeting the spent dispatch again.
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-1")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Uncertain);
    assert_eq!(usage[0].node_id(), step.node_id.as_deref());
    let next = env.coordinator.decide_next("world-1").await.unwrap();
    assert_eq!(dispatched_seq(&next.action), 2);

    // AG-040: the registration and the status of the job still hold, and so they do
    // after the next dispatch (its request is the round after the rewritten one).
    assert_still_registered(&env, &world, &done, "after the settled dispatch").await;
    let second = env.step("world-1", 5, "drift-2").await;
    assert_eq!(dispatched_seq(&second.decision.action), 2);
    assert_eq!(
        counters(&env.world("world-1").await).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SECOND_ROOT_COST
    );
    assert_still_registered(&env, &world, &done, "after the next dispatch").await;
}

#[tokio::test]
async fn a_deepen_settled_after_the_world_moved_is_spent_like_any_other() {
    // The settled node sits where the dispatched action derives it, whatever the
    // kind of the action: a Deepen is spent (its parent has a child) and costs the
    // successor cost, not a root's.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("deepen-drift-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let widen = env.step("world-1", 1, "deepen-drift-1").await;
    let parent = widen.node_id.clone().unwrap();
    env.promote_to_trusted_valid(&parent, 900_000, 400_000)
        .await;

    let model = CountingModel::default();
    let runner = RewritingRunner::new(&env.store, "world-1", |world| {
        world["decision_round"] = json!(7);
    });
    let step = env
        .successor_with(
            Some(&model),
            Some(&runner),
            "world-1",
            &parent,
            2,
            "deepen-drift-2",
        )
        .await
        .expect("a step that was paid for never ends in an error");
    assert_eq!(dispatched_seq(&step.decision.action), 1_000_002);
    assert_settled_uncertain(
        &env,
        "world-1",
        &step,
        Settled {
            reason: PREFIX_CHANGED,
            node_seq: 2,
            search_parent: Some(1),
            branch_seq: 1,
            depth: 2,
        },
    )
    .await;
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 2);
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SUCCESSOR_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        )
    );
    // The Deepen is not offered again: the world offers the second root.
    let next = env.coordinator.decide_next("world-1").await.unwrap();
    assert_eq!(dispatched_seq(&next.action), 2);
    assert_still_registered(&env, &world, &done, "after a settled Deepen").await;
}

// ---------------------------------------------------------------------------
// 2. Two resumers of one request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_lagging_one_of_two_resumers_of_a_request_returns_the_terminal_state_of_the_winner() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("race-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");

    // The first run claims the dispatch and stays in flight at its model. The second
    // is the same request found again (a retry, a second worker): it resumes the claim
    // and runs to its end first. The first run finds, when its step is done, that the
    // dispatch is already terminal.
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first_model = GatedModel::new(entered.clone(), release.clone());
    let second_model = CountingModel::default();
    let first = env.run(
        &first_model,
        &ImprovingFixtureRunner,
        "world-1",
        1,
        "race-1",
    );
    let second = async {
        entered.notified().await;
        let result = env
            .run(
                &second_model,
                &ImprovingFixtureRunner,
                "world-1",
                1,
                "race-1",
            )
            .await;
        release.notify_one();
        result
    };
    let (first, second) = tokio::join!(first, second);
    let second = second.expect("the resumer that wrote the node returns it");
    let first = first.expect("the lagging resumer returns the terminal state of the winner");
    assert_eq!(first.dispatch_id, second.dispatch_id);
    assert_eq!(first.node_id, second.node_id);
    assert_eq!(first.outcome, second.outcome);
    assert!(second.node_id.is_some());
    assert!(first_model.calls() >= 1, "the first run reached its model");
    assert_eq!(
        second_model.calls(),
        0,
        "the resumer found the model dispatch already begun and did not repeat it"
    );

    // One dispatch, one node: the lagging run wrote nothing of its own.
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"], json!([second.node_id.clone()]));
    assert_eq!(stored["dispatch_ids"], json!([second.dispatch_id.clone()]));
    assert_eq!(stored["decision_round"], 1);
    assert_eq!(
        counters(&stored).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    let fact = stored_fact(&env.store, second.dispatch_id.as_deref().unwrap()).await;
    assert_ne!(fact["state"], "claimed");
    assert_eq!(fact["node_id"], json!(second.node_id));
    assert_still_registered(&env, &world, &done, "after the race").await;
}

// ---------------------------------------------------------------------------
// 3. A failure that can be known beforehand is refused before the claim
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_world_that_cannot_name_its_nodes_is_refused_before_anything_is_paid_for() {
    let env = env().await;
    for len in [122usize, 121] {
        let id = long_world_id(len);
        // Registered before the id bound existed (registration refuses it now): the
        // node id of its first node is `node-{id}-1`, 129 bytes at 122, which could not
        // be written once the step was paid for (the claim stayed open for good), and
        // 128 at 121, which can. What no world with such an id can do is name every
        // node it may hold, so both are refused whole before the claim.
        seed_world_record(&env.store, &world_for(&id)).await;
        let before = env.snapshot().await;
        let model = CountingModel::default();
        let runner = CountingRunner::default();
        let result = env
            .run(&model, &runner, &id, 1, &format!("long-{len}"))
            .await;
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "{len} bytes: expected Invalid, got {result:?}"
        );
        assert_eq!(model.calls(), 0, "{len} bytes: nothing was paid for");
        assert_eq!(runner.calls(), 0, "{len} bytes: nothing was paid for");
        assert_eq!(
            env.snapshot().await,
            before,
            "{len} bytes: no dispatch fact, no claim, nothing was written"
        );
        let stored = env.world(&id).await;
        assert_eq!(stored["dispatch_ids"], json!([]));
        assert_eq!(counters(&stored).0, REGISTERED_ROOT_MICROS);
        // Another request meets the same refusal, not a claim it conflicts with.
        let again = env
            .run(&model, &runner, &id, 1, &format!("long-{len}-again"))
            .await;
        assert!(matches!(again, Err(Error::Invalid(_))), "{len}: {again:?}");
        assert_eq!((model.calls(), runner.calls()), (0, 0));
        assert_eq!(env.snapshot().await, before);
    }
}

#[tokio::test]
async fn registration_refuses_a_world_id_past_120_bytes_and_accepts_120() {
    let env = env().await;
    for len in [121usize, 122, 128] {
        let world = world_for(&long_world_id(len));
        let registered = env.coordinator.register_world(world.clone()).await;
        assert!(
            matches!(registered, Err(Error::Invalid(_))),
            "{len} bytes (register_world): {registered:?}"
        );
        let idempotent = env.register(&world).await;
        assert!(
            matches!(idempotent, Err(Error::Invalid(_))),
            "{len} bytes (register_world_idempotent): {idempotent:?}"
        );
        // The management surface refuses it as an invalid input, and writes no world.
        let job = env.start_job(&format!("long-start-{len}"), &world).await;
        assert_eq!(job.state, ManagementJobState::Failed, "{len}: {job:?}");
        assert_eq!(job.error_code.as_deref(), Some("invalid_input"), "{len}");
        assert!(matches!(
            env.coordinator.decide_next(&world.id).await,
            Err(Error::NotFound)
        ));
    }

    // 120 bytes is the longest: `node-{id}-{seq}` holds 128 bytes at the twelfth node.
    let id = long_world_id(120);
    let world = world_for(&id);
    assert_eq!(
        env.register(&world).await.unwrap(),
        RegisterWorldOutcome::Registered
    );
    let step = env.step(&id, 1, "long-120").await;
    assert_eq!(step.node_id.as_deref().map(str::len), Some(127));
    assert_eq!(
        step.node_id,
        Some(format!("node-{id}-1")),
        "the node is named after its world"
    );
}

// ---------------------------------------------------------------------------
// 4. A new world starts at the initial scheduling state
// ---------------------------------------------------------------------------

type WorldEdit = (&'static str, fn(&mut ExplorationWorldV1));

/// The scheduling state `run_next` and `record_history` move. None of it is part of
/// the registration fingerprint, and all of it changes the first decision a request
/// reports.
fn prefilled_scheduling() -> Vec<WorldEdit> {
    vec![
        ("a decision round taken", |world| world.decision_round = 1),
        ("a current branch", |world| {
            world.current_branch_seq = Some(1);
        }),
        ("branch focus actions", |world| {
            world.current_branch_focus_actions = 1;
        }),
        ("a wait", |world| {
            world.waits = vec![OpportunityWait {
                action_seq: 2,
                waited_rounds: 1,
            }];
        }),
    ]
}

#[tokio::test]
async fn a_world_with_prefilled_scheduling_state_is_refused_at_registration() {
    let env = env().await;
    for (index, (label, edit)) in prefilled_scheduling().into_iter().enumerate() {
        let mut world = world_for("world-prefilled");
        edit(&mut world);
        let registered = env.coordinator.register_world(world.clone()).await;
        assert!(
            matches!(registered, Err(Error::Invalid(_))),
            "{label} (register_world): {registered:?}"
        );
        let idempotent = env.register(&world).await;
        assert!(
            matches!(idempotent, Err(Error::Invalid(_))),
            "{label} (register_world_idempotent): {idempotent:?}"
        );
        let job = env
            .start_job(&format!("prefilled-start-{index}"), &world)
            .await;
        assert_eq!(job.state, ManagementJobState::Failed, "{label}: {job:?}");
        assert_eq!(job.error_code.as_deref(), Some("invalid_input"), "{label}");
        assert!(
            matches!(
                env.coordinator.decide_next("world-prefilled").await,
                Err(Error::NotFound)
            ),
            "{label}: no world was written"
        );
    }
}

#[tokio::test]
async fn a_request_with_prefilled_scheduling_state_does_not_converge_on_the_registered_world() {
    // The registration fingerprint leaves the scheduling state out, so a request that
    // carries one used to converge on the registered world and report another first
    // decision. It is refused now, also for a world that is already registered.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("converge-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let first_decision = started_result(&done);
    let before = env.snapshot().await;
    for (label, edit) in prefilled_scheduling() {
        let mut prefilled = world.clone();
        edit(&mut prefilled);
        let result = env.register(&prefilled).await;
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "{label}: expected Invalid, got {result:?}"
        );
    }
    assert_eq!(env.snapshot().await, before, "a refusal writes nothing");
    let again = env.start_job("converge-restart", &world).await;
    assert_eq!(again.state, ManagementJobState::Succeeded, "{again:?}");
    assert_eq!(started_result(&again), first_decision);
}

// ---------------------------------------------------------------------------
// 5. A claim whose inputs changed before it resumed
// ---------------------------------------------------------------------------

/// Claims the dispatch of the request and loses the process once the model was
/// entered (the future is dropped): the dispatch stays claimed, and nobody knows
/// whether its step was paid for.
async fn leave_claimed(env: &Env, world_id: &str, step: u32, tag: &str) {
    let entered = Arc::new(Notify::new());
    let hanging = HangingModel {
        entered: entered.clone(),
    };
    tokio::select! {
        biased;
        result = env.coordinator.run_next(
            Some(&hanging),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request(world_id, step, tag),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
}

#[tokio::test]
async fn a_claim_whose_inputs_changed_before_it_resumed_ends_in_a_terminal_uncertain_node() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("claim-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    leave_claimed(&env, "world-1", 1, "claim-1").await;
    let claimed = env.world("world-1").await;
    assert_eq!(claimed["dispatch_ids"].as_array().unwrap().len(), 1);
    assert_eq!(claimed["node_ids"], json!([]));
    assert_eq!(counters(&claimed).0, REGISTERED_ROOT_MICROS);

    // While nobody ran the claim the stored world was rewritten: the request of the
    // claim (the round after the decision round it was taken at) no longer binds it.
    let mut rewritten = raw_record(&env.store, WORLD_KIND, "world-1").await;
    rewritten["payload"]["decision_round"] = json!(1);
    put_raw_record(&env.store, WORLD_KIND, "world-1", &rewritten).await;

    // The claim is the truth about what was dispatched: it is settled as a used
    // opportunity, not run again, and never an error that would leave it open.
    let model = CountingModel::default();
    let runner = CountingRunner::default();
    let step = env
        .run(&model, &runner, "world-1", 1, "claim-1")
        .await
        .expect("a dispatched claim is settled, whatever became of its inputs");
    assert_eq!(
        (model.calls(), runner.calls()),
        (0, 0),
        "a claim whose inputs changed is not run again"
    );
    assert_eq!(dispatched_seq(&step.decision.action), 1);
    assert_settled_uncertain(
        &env,
        "world-1",
        &step,
        Settled {
            reason: CLAIM_INPUTS_CHANGED,
            node_seq: 1,
            search_parent: None,
            branch_seq: 1,
            depth: 1,
        },
    )
    .await;
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"], json!(["node-world-1-1"]));
    assert_eq!(stored["dispatch_ids"], json!([step.dispatch_id.clone()]));
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        )
    );
    assert_eq!(stored["decision_round"], 2);

    // A reconnect gets the same terminal state, with no port reached.
    let again = env.reconnect("world-1", 1, "claim-1").await.unwrap();
    assert_eq!(again.node_id, step.node_id);
    assert_eq!(again.outcome, CLAIM_INPUTS_CHANGED);
    assert_eq!(model.calls() + runner.calls(), 0);

    // The world goes on with the next root, and the registration still holds.
    let next = env.coordinator.decide_next("world-1").await.unwrap();
    assert_eq!(dispatched_seq(&next.action), 2);
    assert_still_registered(&env, &world, &done, "after the settled claim").await;
    let second = env.step("world-1", 3, "claim-2").await;
    assert_eq!(dispatched_seq(&second.decision.action), 2);
    assert_still_registered(&env, &world, &done, "after the next dispatch").await;
}

#[tokio::test]
async fn a_claimed_dispatch_without_ports_is_not_found_and_a_claim_that_did_not_move_resumes() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("regress-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    leave_claimed(&env, "world-1", 1, "regress-1").await;

    // No ports: a caller that cannot run the step does not settle the claim.
    let before = env.snapshot().await;
    assert!(matches!(
        env.reconnect("world-1", 1, "regress-1").await,
        Err(Error::NotFound)
    ));
    assert_eq!(env.snapshot().await, before, "nothing was written");

    // A claim whose world did not move is resumed, and the step's own journal says
    // what became of the model dispatch that was begun and never answered: it is not
    // repeated, and neither settlement reason of a moved world is given.
    let model = CountingModel::default();
    let runner = CountingRunner::default();
    let step = env
        .run(&model, &runner, "world-1", 1, "regress-1")
        .await
        .unwrap();
    assert_eq!((model.calls(), runner.calls()), (0, 0));
    assert_ne!(step.outcome, PREFIX_CHANGED);
    assert_ne!(step.outcome, CLAIM_INPUTS_CHANGED);
    let fact = stored_fact(&env.store, step.dispatch_id.as_deref().unwrap()).await;
    assert_eq!(fact["state"], "uncertain");
    assert_eq!(fact["node_id"], json!(step.node_id));
    assert_eq!(fact["outcome_reason"], json!(step.outcome));
    assert_still_registered(&env, &world, &done, "after the resumed claim").await;
}

#[tokio::test]
async fn another_request_never_settles_a_claim_it_did_not_make() {
    // The claim belongs to the request that made it. A different request that meets
    // it (the claimed action is still the one the world decides) is an idempotency
    // conflict before it pays for anything, and the claim stays as it was: only the
    // request that made it resumes it, and only that one may settle it.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("other-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    leave_claimed(&env, "world-1", 1, "other-1").await;
    let claimed = env.world("world-1").await;
    let claim_id = claimed["dispatch_ids"][0].as_str().unwrap().to_string();
    let before = env.snapshot().await;

    let model = CountingModel::default();
    let runner = CountingRunner::default();
    let other = env.run(&model, &runner, "world-1", 1, "other-2").await;
    match other {
        Err(Error::Conflict(message)) => {
            assert_eq!(message, "dispatch idempotency conflict");
        }
        unexpected => panic!("expected the idempotency conflict, got {unexpected:?}"),
    }
    assert_eq!((model.calls(), runner.calls()), (0, 0));
    assert_eq!(env.snapshot().await, before, "nothing was written");
    assert_eq!(stored_fact(&env.store, &claim_id).await["state"], "claimed");

    // The request that made the claim still resumes it.
    let resumed = env
        .run(
            &CountingModel::default(),
            &CountingRunner::default(),
            "world-1",
            1,
            "other-1",
        )
        .await
        .unwrap();
    assert_eq!(resumed.dispatch_id.as_deref(), Some(claim_id.as_str()));
    assert_ne!(stored_fact(&env.store, &claim_id).await["state"], "claimed");
    assert_still_registered(&env, &world, &done, "after the resumed claim").await;
}

// ---------------------------------------------------------------------------
// 6. A Recover target that is gone, and a settlement that must not overwrite a node
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recover_target_that_is_gone_ends_in_a_terminal_uncertain_node() {
    // No step produces a repairable failure yet (AG-042), so one is made: the node of
    // the only root is rewritten into a repairable failure whose environment was reset
    // and numbered past the world's list of node ids. The Recover derived from it
    // names a node (`node_seq` 2) the world does not list at that position, which is
    // where `run_next` finds the id of the node to update: the target is gone.
    let env = env().await;
    env.register(&single_root_world("world-1")).await.unwrap();
    let widen = env.step("world-1", 1, "recover-1").await;
    let failed = widen.node_id.clone().unwrap();
    let mut node = raw_record(&env.store, NODE_KIND, &failed).await;
    node["payload"]["node"]["node_seq"] = json!(2);
    node["payload"]["node"]["status"] = json!({
        "status": "repairable_failure",
        "episode_id": "episode-1",
        "failure_kind": "compile",
        "repair_template_digest": hash(b"repair-template"),
        "environment_reset": true,
        "dispatched_repairs": 0,
    });
    put_raw_record(&env.store, NODE_KIND, &failed, &node).await;
    let decision = env.coordinator.decide_next("world-1").await.unwrap();
    assert_eq!(dispatched_seq(&decision.action), 1_000_005);

    let model = CountingModel::default();
    let runner = CountingRunner::default();
    let step = env
        .successor_with(
            Some(&model),
            Some(&runner),
            "world-1",
            &failed,
            2,
            "recover-2",
        )
        .await
        .expect("a step that was paid for never ends in an error");
    assert!(model.calls() >= 1 && runner.calls() == 1);
    assert_eq!(dispatched_seq(&step.decision.action), 1_000_005);
    assert_settled_uncertain(
        &env,
        "world-1",
        &step,
        Settled {
            reason: RECOVER_TARGET_MISSING,
            node_seq: 2,
            search_parent: Some(2),
            branch_seq: 1,
            depth: 2,
        },
    )
    .await;
    // The dispatch spent what a Recover spends: the successor cost and one recovery
    // dispatch, once.
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 2);
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SUCCESSOR_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES) - 1
        )
    );
    assert_eq!(stored["decision_round"], 2);
    let again = env
        .successor_with(None, None, "world-1", &failed, 2, "recover-2")
        .await
        .unwrap();
    assert_eq!(again.node_id, step.node_id);
    assert_eq!(again.outcome, RECOVER_TARGET_MISSING);
    // Not asserted: that the registration of this world still holds. Its node
    // numbering was rewritten to make the target vanish (two nodes now carry sequence
    // 2), which is a world the registration is right to refuse.
}

#[tokio::test]
async fn a_settlement_never_overwrites_a_node_and_leaves_the_claim_resumable() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("clobber-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.step("world-1", 1, "clobber-1").await;
    let node_one = raw_record(&env.store, NODE_KIND, "node-world-1-1").await;

    // While the second dispatch is paid for the world forgets its node: the prefix is
    // not the one the decision saw, and the node this dispatch would write is
    // numbered 1, the id of a node that exists. The settlement cannot be written
    // without overwriting it, so it is refused, with the error the world gave before
    // there was a settlement, and the claim stays open.
    let runner = RewritingRunner::new(&env.store, "world-1", |world| {
        world["node_ids"] = json!([]);
    });
    let refused = env
        .run(&EditingFixtureModel, &runner, "world-1", 2, "clobber-2")
        .await;
    match refused {
        Err(Error::Conflict(message)) => assert!(message.contains("prefix changed"), "{message}"),
        other => panic!("expected the prefix conflict, got {other:?}"),
    }
    assert_eq!(
        raw_record(&env.store, NODE_KIND, "node-world-1-1").await,
        node_one,
        "no node was overwritten"
    );
    let stored = env.world("world-1").await;
    let second_dispatch = stored["dispatch_ids"][1].as_str().unwrap().to_string();
    assert_eq!(
        stored_fact(&env.store, &second_dispatch).await["state"],
        "claimed"
    );

    // The world is put right and the same request resumes the claim: its step is
    // replayed from the journal (nothing is paid for again) and the node is written.
    let mut restored = raw_record(&env.store, WORLD_KIND, "world-1").await;
    restored["payload"]["node_ids"] = json!(["node-world-1-1"]);
    put_raw_record(&env.store, WORLD_KIND, "world-1", &restored).await;
    let model = CountingModel::default();
    let replay = CountingRunner::default();
    let resumed = env
        .run(&model, &replay, "world-1", 2, "clobber-2")
        .await
        .unwrap();
    assert_eq!((model.calls(), replay.calls()), (0, 0));
    assert_eq!(resumed.node_id.as_deref(), Some("node-world-1-2"));
    assert_eq!(
        stored_fact(&env.store, &second_dispatch).await["state"],
        "observed"
    );
    assert_still_registered(&env, &world, &done, "after the resumed claim").await;
}

// ---------------------------------------------------------------------------
// 7. A completed dispatch fact is held to its node
//
// The budget rebuilt from the dispatch facts (AG-040) holds each fact to the cost
// of the action it records. It did not hold the fact to the node the dispatch
// produced: a root dispatch rewritten into a `Deepen`, together with the counters
// that would pay for it, balanced the books. The search parent, the depth and the
// branch of the node are the ones the action of its fact derives
// (`derive_legal_actions`): a root has no parent, depth 1 and the branch of its
// root; a `Deepen` has the node it deepens for parent, on its branch, one deeper.
// ---------------------------------------------------------------------------

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
            facts.push(raw_fact(&env.store, id).await);
        }
        let mut nodes = Vec::new();
        for id in &node_ids {
            nodes.push(raw_record(&env.store, NODE_KIND, id).await);
        }
        Self {
            world_id: world_id.into(),
            dispatch_ids,
            node_ids,
            world: raw_record(&env.store, WORLD_KIND, world_id).await,
            facts,
            nodes,
        }
    }

    async fn put(&self, env: &Env) {
        put_raw_record(&env.store, WORLD_KIND, &self.world_id, &self.world).await;
        for (id, fact) in self.dispatch_ids.iter().zip(&self.facts) {
            put_raw_fact(&env.store, id, fact).await;
        }
        for (id, node) in self.node_ids.iter().zip(&self.nodes) {
            put_raw_record(&env.store, NODE_KIND, id, node).await;
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
    expect_conflict(env.register(world).await, label);
    expect_status_conflict(&env.dispatcher, done, label).await;
    // A job for the same world under another request key does not converge on it
    // either: it fails on registering, it does not report a first decision.
    let again = env.start_job(key, world).await;
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

/// The stored fact of a root dispatch rewritten into the fact of the `Deepen` of
/// node `parent_seq`, on `branch_seq` and `target_depth`, at the successor cost: what
/// `derive_legal_actions` would have derived for that node.
fn rewrite_as_deepen(fact: &mut Value, parent_seq: u32, branch_seq: u32, target_depth: u32) {
    let action_id = format!("deepen-{parent_seq}");
    let action_seq = 1_000_000 + 2 * parent_seq;
    let payload = &mut fact["payload"];
    payload["action_id"] = json!(action_id);
    payload["action_seq"] = json!(action_seq);
    payload["selected_action"] = json!({
        "action_id": action_id,
        "action_seq": action_seq,
        "branch_seq": branch_seq,
        "target_depth": target_depth,
        "kind": {"action": "deepen", "parent_node_seq": parent_seq},
        "estimated_cost_upper_micros": SUCCESSOR_COST,
    });
    payload["decision"]["action"]["action_ids"] = json!([action_id]);
    payload["decision"]["action"]["action_seqs"] = json!([action_seq]);
    payload["decision"]["action"]["estimated_cost_upper_micros"] = json!(SUCCESSOR_COST);
}

/// Sets the remaining root budget of a stored world record.
fn set_remaining_root(world: &mut Value, micros: u64) {
    world["payload"]["remaining_root_micros"] = json!(micros);
}

#[tokio::test]
async fn a_root_dispatch_rewritten_into_a_deepen_with_its_counters_is_refused_everywhere() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("bind-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let steps = env.dispatches("world-1", 2).await;
    let original = Stored::load(&env, "world-1", &steps).await;
    assert_still_registered(&env, &world, &done, "untouched").await;

    let tampers: Vec<(&str, StoredTamper)> = vec![
        (
            "the first root dispatch as a Deepen of its own node, the counters moved to match",
            Box::new(|stored| {
                rewrite_as_deepen(&mut stored.facts[0], 1, 1, 2);
                set_remaining_root(
                    &mut stored.world,
                    REGISTERED_ROOT_MICROS - SUCCESSOR_COST - SECOND_ROOT_COST,
                );
            }),
        ),
        (
            "the second root dispatch as a Deepen of the first node, the counters moved to match",
            Box::new(|stored| {
                rewrite_as_deepen(&mut stored.facts[1], 1, 1, 2);
                set_remaining_root(
                    &mut stored.world,
                    REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SUCCESSOR_COST,
                );
            }),
        ),
        (
            "a root dispatch whose node sits on another branch",
            Box::new(|stored| {
                stored.nodes[0]["payload"]["node"]["branch_seq"] = json!(2);
            }),
        ),
        (
            "a root dispatch whose node names a search parent",
            Box::new(|stored| {
                stored.nodes[1]["payload"]["node"]["search_parent_seq"] = json!(1);
                stored.nodes[1]["payload"]["node"]["depth"] = json!(2);
            }),
        ),
        (
            "a root dispatch whose node is not a root by its depth",
            Box::new(|stored| {
                stored.nodes[0]["payload"]["node"]["depth"] = json!(2);
            }),
        ),
        (
            "the nodes of the two root dispatches swapped",
            Box::new(|stored| {
                stored.facts[0]["payload"]["node_id"] = json!(stored.node_ids[1].clone());
                stored.facts[1]["payload"]["node_id"] = json!(stored.node_ids[0].clone());
            }),
        ),
    ];
    for (index, (label, tamper)) in tampers.iter().enumerate() {
        expect_refused_everywhere(
            &env,
            &world,
            &done,
            &original,
            &format!("bind-job-{index}"),
            label,
            tamper,
        )
        .await;
    }
}

#[tokio::test]
async fn a_deepen_fact_is_held_to_the_node_it_produced() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("deepen-bind-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let steps = env.widen_then_deepen("world-1").await;
    let original = Stored::load(&env, "world-1", &steps).await;
    // The real Deepen: its node is the child of the first node, one deeper, on its
    // branch, and the untouched records are accepted by registration and `status`.
    assert_eq!(
        original.nodes[1]["payload"]["node"]["search_parent_seq"],
        json!(1)
    );
    assert_eq!(original.nodes[1]["payload"]["node"]["depth"], json!(2));
    assert_eq!(original.nodes[1]["payload"]["node"]["branch_seq"], json!(1));
    assert_still_registered(&env, &world, &done, "after a real Deepen").await;

    let tampers: Vec<(&str, StoredTamper)> = vec![
        (
            "a Deepen whose node is a root",
            Box::new(|stored| {
                stored.nodes[1]["payload"]["node"]["search_parent_seq"] = Value::Null;
                stored.nodes[1]["payload"]["node"]["depth"] = json!(1);
            }),
        ),
        (
            "a Deepen whose node is deeper than its parent's child",
            Box::new(|stored| {
                stored.nodes[1]["payload"]["node"]["depth"] = json!(3);
            }),
        ),
        (
            "a Deepen whose node sits on another branch",
            Box::new(|stored| {
                stored.nodes[1]["payload"]["node"]["branch_seq"] = json!(2);
            }),
        ),
        (
            "a Deepen whose node names another parent",
            Box::new(|stored| {
                stored.nodes[1]["payload"]["node"]["search_parent_seq"] = json!(2);
            }),
        ),
        (
            "a Deepen whose fact names the node itself for parent",
            Box::new(|stored| {
                stored.facts[1]["payload"]["selected_action"]["kind"]["parent_node_seq"] = json!(2);
            }),
        ),
        (
            "a Deepen whose fact names another depth",
            Box::new(|stored| {
                stored.facts[1]["payload"]["selected_action"]["target_depth"] = json!(3);
            }),
        ),
        (
            "a Deepen whose fact names another branch",
            Box::new(|stored| {
                stored.facts[1]["payload"]["selected_action"]["branch_seq"] = json!(2);
            }),
        ),
        (
            "the first Widen rewritten into a Deepen of the second node",
            Box::new(|stored| {
                rewrite_as_deepen(&mut stored.facts[0], 2, 1, 3);
                set_remaining_root(
                    &mut stored.world,
                    REGISTERED_ROOT_MICROS - SUCCESSOR_COST - SUCCESSOR_COST,
                );
            }),
        ),
    ];
    for (index, (label, tamper)) in tampers.iter().enumerate() {
        expect_refused_everywhere(
            &env,
            &world,
            &done,
            &original,
            &format!("deepen-bind-job-{index}"),
            label,
            tamper,
        )
        .await;
    }
}
