//! AG-040 (E09 PR-B step B3, E07 management consumer; plan §7.1.1: the facts a
//! world is registered with are immutable and its state is derived): a world
//! that already dispatched is still the world that was registered.
//!
//! * the registration fingerprint covers the immutable registration facts only.
//!   The two budget counters `run_next` spends are not in it; they are compared
//!   apart, against the value the world was *registered* with (its current value
//!   plus what its own dispatch facts say was spent), exactly;
//! * re-registering a world after dispatches is `AlreadyRegistered` and writes
//!   nothing; an immutable field that differs, or a different counter
//!   declaration (the registered one is the only one that is accepted, not the
//!   current one), is a `Conflict`;
//! * an `exploration.start` job re-run after a dispatch (crash recovery, or
//!   another request key for the same world) reports the first decision of the
//!   world as it was requested, not the live one, and `status` of a started
//!   world compares the stored result with that first decision and always with
//!   the world's id, context, policy and caps;
//! * (R1) `status` of a started world also re-checks that the stored world is
//!   still the registration the job made, the way registration compares it: the
//!   fingerprint of its immutable facts (a source closure replaced by other live
//!   runs, a cost, a seed, a digest) and the budget it was registered with,
//!   rebuilt from its dispatch facts, against the world in the job's private
//!   input. A world that never started keeps comparing its own decision;
//! * (R2) the budget rebuilt from the dispatch facts takes what each fact spent from
//!   the world's registration, not from the fact: the cost of an action is its root
//!   opportunity's (a root action) or the successor cost (a Deepen or a Recover), and
//!   every fact, a claimed one included, has to record that cost in its decision and
//!   in its selected action. A cost edited together with the counter that paid for it
//!   balances the books (the controller's probe K1) and is refused by registration,
//!   by `status` and for a second job alike. A real `Deepen` is dispatched for that.
//!
//! Not covered (and not claimed): the paid `Err` paths of `run_next` (AG-041),
//! that MetaTrial no longer needs its own guard against re-registering a stored
//! world (it keeps it), and the recovery of a repairable failure end to end (the
//! recovery counter is exercised on a tampered fact, not on a real recovery).
//! Real SQLite store, fixture model and runner, no provider, zero monetary cost.
//!
//! Fixtures are copied from `dispatch_management.rs`, `exploration_trust_v42.rs`
//! and `meta_inheritance_v42.rs`; those files are unchanged.
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
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, PersistentCoordinator,
    RegisterWorldOutcome, RootOpportunity, WorldState, exploration_world_storage_id,
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
const DISPATCH_KIND: &str = "exploration_dispatch_v1";
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

/// A model port that fails after the dispatch was claimed: the coordinator
/// must record the dispatch as uncertain, not lose it.
struct FailingModel;

#[async_trait]
impl ModelPort for FailingModel {
    async fn dispatch(&self, _: ModelRequest) -> Result<ModelResponse> {
        Err(Error::NotFound)
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

fn policy_focus_one() -> ElasticPolicyV1 {
    ElasticPolicyV1 {
        max_focus_actions: 1,
        ..ElasticPolicyV1::default()
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

/// Re-derives the context signature a world has to carry once one of the digests
/// it is made of (or its watermark) changed, so that the edited world is still a
/// valid request and differs from the registered one only in that digest.
fn resign(world: &mut ExplorationWorldV1) {
    world.context_signature = fingerprint(&(
        &world.parent_skill_digest,
        &world.parent_bundle_digest,
        &world.environment_digest,
        &world.model_digest,
        &world.tools_digest,
        &world.grader_digest,
        &world.rules_digest,
        world.source_watermark,
    ))
    .unwrap();
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
        self.coordinator
            .run_next(
                Some(&EditingFixtureModel),
                Some(&ImprovingFixtureRunner),
                Some(&self.journal),
                self.fixture.request(world_id, step, tag),
            )
            .await
            .unwrap()
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
                Some(&EditingFixtureModel),
                Some(&ImprovingFixtureRunner),
                Some(&self.journal),
                self.fixture
                    .request_for(&skill, &edit_context, &bundle, world_id, step, tag),
            )
            .await
            .unwrap()
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

/// Raises the remaining root budget a stored world record holds by `micros`.
fn raise_remaining_root(world: &mut Value, micros: u64) {
    let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
    world["payload"]["remaining_root_micros"] = json!(root + micros);
}

/// The cost a stored dispatch fact records for the action its decision dispatched.
fn set_decision_cost(fact: &mut Value, micros: u64) {
    fact["payload"]["decision"]["action"]["estimated_cost_upper_micros"] = json!(micros);
}

/// The cost a stored dispatch fact records for the action it selected.
fn set_selected_cost(fact: &mut Value, micros: u64) {
    fact["payload"]["selected_action"]["estimated_cost_upper_micros"] = json!(micros);
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

async fn raw_job(store: &Store, admin: &Context, job_id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value: Value = session.need(admin, "job", job_id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn put_raw_job(store: &Store, admin: &Context, job_id: &str, value: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(admin, "job", job_id, admin.actor(), value)
        .await
        .unwrap();
    session.commit().await.unwrap();
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
// 1. The fingerprint covers the immutable facts and nothing `run_next` moves
// ---------------------------------------------------------------------------

type Edit = (&'static str, fn(&mut ExplorationWorldV1));
/// An edit of one stored record, as raw JSON.
type ValueEdit = fn(&mut Value);

/// Every field the fingerprint covers, each edited on its own. The S0 digests are
/// re-signed so that the edited world is still a valid request.
fn immutable_edits() -> Vec<Edit> {
    vec![
        ("schema_version", |w| {
            w.schema_version = "rsia.exploration_world.v0".into()
        }),
        ("id", |w| w.id = "another-world".into()),
        ("approved_parent_digest", |w| {
            w.approved_parent_digest = hash(b"other-approved-parent");
        }),
        ("context_signature", |w| {
            w.context_signature = hash(b"other-context")
        }),
        ("parent_skill_digest", |w| {
            w.parent_skill_digest = hash(b"other-parent-skill");
            resign(w);
        }),
        ("parent_bundle_digest", |w| {
            w.parent_bundle_digest = hash(b"other-parent-bundle");
            resign(w);
        }),
        ("environment_digest", |w| {
            w.environment_digest = hash(b"other-environment");
            resign(w);
        }),
        ("model_digest", |w| {
            w.model_digest = hash(b"other-model");
            resign(w);
        }),
        ("tools_digest", |w| {
            w.tools_digest = hash(b"other-tools");
            resign(w);
        }),
        ("grader_digest", |w| {
            w.grader_digest = hash(b"other-grader");
            resign(w);
        }),
        ("rules_digest", |w| {
            w.rules_digest = hash(b"other-rules");
            resign(w);
        }),
        ("source_watermark", |w| {
            w.source_watermark = 2;
            resign(w);
        }),
        ("caps", |w| w.caps.max_nodes = 6),
        ("policy", |w| w.policy = policy_focus_one()),
        ("simulation", |w| {
            w.simulation = SimulationContext::Online { fixed_seed: 8 };
        }),
        ("root_opportunities (a cost)", |w| {
            w.root_opportunities[0].estimated_cost_upper_micros += 1;
        }),
        ("root_opportunities (one more)", |w| {
            w.root_opportunities.push(RootOpportunity {
                root_slot: 3,
                branch_seq: 3,
                action_seq: 3,
                estimated_cost_upper_micros: 10,
            });
        }),
        ("dependencies", |w| {
            w.dependencies.pop();
        }),
        ("successor_cost_upper_micros", |w| {
            w.successor_cost_upper_micros += 1;
        }),
        ("initial_baseline_quality_micros", |w| {
            w.initial_baseline_quality_micros = 400_000;
        }),
    ]
}

#[test]
fn the_registration_fingerprint_ignores_what_run_next_moves_and_nothing_else() {
    let registered = world_for("world-1");
    let fingerprint = registered.registration_fingerprint().unwrap();

    // Everything a run, a history entry or a dispatch changes leaves the
    // fingerprint alone: the world's state, its ids, its scheduling state and the
    // two counters `run_next` spends.
    let mut moved = registered.clone();
    moved.remaining_root_micros -= FIRST_ROOT_COST + SECOND_ROOT_COST;
    moved.remaining_recovery_dispatches -= 1;
    moved.state = WorldState::Sealed;
    moved.node_ids = vec!["node-world-1-1".into(), "node-world-1-2".into()];
    moved.dispatch_ids = vec!["dispatch-1".into(), "dispatch-2".into()];
    moved.history_ids = vec!["history-1".into()];
    moved.current_branch_seq = Some(2);
    moved.current_branch_focus_actions = 3;
    moved.decision_round = 2;
    moved.waits = vec![OpportunityWait {
        action_seq: 2,
        waited_rounds: 1,
    }];
    assert_eq!(
        moved.registration_fingerprint().unwrap(),
        fingerprint,
        "a field run_next or record_history moves is not part of the registration"
    );
    // One counter at a time, in both directions.
    let counter_edits: [fn(&mut ExplorationWorldV1); 4] = [
        |w| w.remaining_root_micros = 0,
        |w| w.remaining_root_micros += 1,
        |w| w.remaining_recovery_dispatches = 0,
        |w| w.remaining_recovery_dispatches += 1,
    ];
    for edit in counter_edits {
        let mut edited = registered.clone();
        edit(&mut edited);
        assert_eq!(edited.registration_fingerprint().unwrap(), fingerprint);
    }

    // Every immutable field is covered.
    for (label, edit) in immutable_edits() {
        let mut edited = registered.clone();
        edit(&mut edited);
        assert_ne!(
            edited.registration_fingerprint().unwrap(),
            fingerprint,
            "{label} is part of the registration"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Re-registering a world after dispatches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn re_registering_a_world_after_dispatches_is_already_registered_and_writes_nothing() {
    let env = env().await;
    let world = world_for("world-1");
    assert_eq!(
        env.register(&world).await.unwrap(),
        RegisterWorldOutcome::Registered
    );
    // Before any dispatch the same registration converges, as it always did.
    expect_already_registered(env.register(&world).await, "before a dispatch");

    let steps = env.dispatches("world-1", 2).await;
    assert_eq!(dispatched_seq(&steps[0].decision.action), 1);
    assert_eq!(dispatched_seq(&steps[1].decision.action), 2);
    // The world moved on: both dispatches spent, the world lists them.
    let stored = env.world("world-1").await;
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SECOND_ROOT_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        ),
        "each dispatch spent the cost its own fact names"
    );
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 2);
    assert_eq!(stored["dispatch_ids"].as_array().unwrap().len(), 2);
    assert_eq!(stored["decision_round"], 2);

    // The same registration is still that registration.
    let before = env.snapshot().await;
    let edges = env.world_dependents("world-1").await;
    expect_already_registered(env.register(&world).await, "after two dispatches");
    expect_already_registered(env.register(&world).await, "again");
    assert_eq!(env.snapshot().await, before, "nothing was written");
    assert_eq!(env.world_dependents("world-1").await, edges);
    assert_eq!(counters(&env.world("world-1").await), counters(&stored));
}

#[tokio::test]
async fn every_immutable_field_is_still_compared_before_and_after_a_dispatch() {
    let env = env().await;
    let world = world_for("world-1");
    env.register(&world).await.unwrap();

    for stage in ["registered, not started", "after two dispatches"] {
        if stage == "after two dispatches" {
            env.dispatches("world-1", 2).await;
        }
        let before = env.snapshot().await;
        for (label, edit) in immutable_edits() {
            let mut edited = world.clone();
            edit(&mut edited);
            if edited.id != world.id {
                // Another id is another world, not another registration of this
                // one: it is not a conflict (it registers on its own).
                continue;
            }
            let result = env.register(&edited).await;
            assert!(
                matches!(result, Err(Error::Conflict(_) | Error::Invalid(_))),
                "{stage}: a different {label} must be refused, got {result:?}"
            );
        }
        assert_eq!(
            env.snapshot().await,
            before,
            "{stage}: a refusal writes nothing"
        );
        expect_already_registered(env.register(&world).await, stage);
    }
}

#[tokio::test]
async fn a_different_counter_declaration_is_a_conflict_and_only_the_registered_one_is_accepted() {
    let env = env().await;
    let world = world_for("world-1");
    env.register(&world).await.unwrap();

    // Not started: the counters must equal the request's.
    for (label, root, recovery) in [
        ("a larger root budget", REGISTERED_ROOT_MICROS + 1_000, 2),
        ("a smaller root budget", REGISTERED_ROOT_MICROS - 1, 2),
        ("no root budget", 0, 2),
        ("more recovery dispatches", REGISTERED_ROOT_MICROS, 3),
        ("fewer recovery dispatches", REGISTERED_ROOT_MICROS, 1),
        ("no recovery dispatches", REGISTERED_ROOT_MICROS, 0),
    ] {
        let mut declared = world.clone();
        declared.remaining_root_micros = root;
        declared.remaining_recovery_dispatches = recovery;
        expect_conflict(
            env.register(&declared).await,
            &format!("not started, {label}"),
        );
    }

    // Started: the same, against the value the world was registered with, which
    // is what its dispatch facts add back to the stored one. A loose rule (the
    // request must not be below the current value) would take 2_000 for 1_000;
    // the current value is not a registration either.
    env.dispatches("world-1", 2).await;
    let current = counters(&env.world("world-1").await);
    assert_eq!(
        current,
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SECOND_ROOT_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        )
    );
    let before = env.snapshot().await;
    for (label, root, recovery) in [
        ("2_000 for the registered 1_000", 2_000, 2),
        ("the current value", current.0, 2),
        ("one micro more", REGISTERED_ROOT_MICROS + 1, 2),
        ("one micro less", REGISTERED_ROOT_MICROS - 1, 2),
        ("one more recovery dispatch", REGISTERED_ROOT_MICROS, 3),
        ("one fewer recovery dispatch", REGISTERED_ROOT_MICROS, 1),
        ("no recovery dispatches", REGISTERED_ROOT_MICROS, 0),
    ] {
        let mut declared = world.clone();
        declared.remaining_root_micros = root;
        declared.remaining_recovery_dispatches = recovery;
        expect_conflict(env.register(&declared).await, &format!("started, {label}"));
    }
    assert_eq!(env.snapshot().await, before, "a refusal writes nothing");
    expect_already_registered(env.register(&world).await, "the registered declaration");
}

// ---------------------------------------------------------------------------
// 3. exploration.start after a dispatch: status, crash re-run, a second job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_of_an_exploration_start_job_holds_after_the_world_dispatched() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("start-1", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let result = started_result(&done);
    assert_eq!(result["action"]["action_seqs"], json!([1]));
    expect_status_ok(&env.dispatcher, &done, "before any dispatch").await;

    env.step("world-1", 1, "start-1-1").await;
    expect_status_ok(&env.dispatcher, &done, "after one dispatch").await;
    env.step("world-1", 2, "start-1-2").await;
    expect_status_ok(&env.dispatcher, &done, "after two dispatches").await;

    // The job is the record of the first decision: nothing rewrote it.
    let persisted = raw_job(&env.store, &admin(), &done.id).await;
    assert_eq!(persisted["state"], "succeeded");
    assert_eq!(
        serde_json::to_value(&done.result).unwrap(),
        persisted["result"]
    );
    // Reconnecting with the same key returns the same terminal job.
    let again = env
        .dispatcher
        .submit(
            &admin(),
            "exploration.start",
            exploration_start_payload("start-1", &world),
        )
        .await
        .unwrap();
    assert_eq!(again.id, done.id);
    assert_eq!(started_result(&again), result);
}

#[tokio::test]
async fn a_crash_re_run_after_a_dispatch_reports_the_first_decision() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("crash-1", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let first_decision = started_result(&done);
    let steps = env.dispatches("world-1", 2).await;
    assert_eq!(dispatched_seq(&steps[0].decision.action), 1);
    // The live decision is another one by now: both roots are taken, so it is a
    // stop. Reporting it would not be reporting the first decision.
    let live = env.coordinator.decide_next("world-1").await.unwrap();
    assert!(
        matches!(live.action, BatchActionV1::Stop { .. }),
        "{live:?}"
    );

    // The process died after the world registered and dispatched, before the
    // job's terminal was written (or: the job is simply run again).
    let mut crashed = done.clone();
    crashed.state = ManagementJobState::Running;
    crashed.step = "before_exploration_start".into();
    crashed.result = None;
    crashed.error_code = None;
    crashed.lease_token = Some("dead-process-lease".into());
    crashed.lease_until = 0;
    let mut session = env.store.session().await.unwrap();
    session
        .put(&admin(), "job", &crashed.id, admin().actor(), &crashed)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let restarted = ManagementDispatcher::new(env.store.clone(), vec![admin()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let recovered = wait_terminal(&restarted, &admin(), &done.id).await;
    assert_eq!(
        recovered.state,
        ManagementJobState::Succeeded,
        "the re-run converges on the registered world: {recovered:?}"
    );
    assert_eq!(recovered.step, "exploration_started");
    assert_eq!(recovered.generation, done.generation + 1);
    // The result is the first decision, not the live one (the live prefix has two
    // nodes now, and the live decision is a stop).
    assert_eq!(started_result(&recovered), first_decision);
    assert_eq!(first_decision["action"]["decision"], "dispatch");

    // Still one world, still the one that dispatched; the re-run moved nothing.
    let stored = env.world("world-1").await;
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 2);
    assert_eq!(stored["dispatch_ids"].as_array().unwrap().len(), 2);
    assert_eq!(stored["decision_round"], 2);
    expect_status_ok(&restarted, &recovered, "after the re-run").await;
}

#[tokio::test]
async fn another_request_key_for_a_world_that_dispatched_reports_the_first_decision() {
    let env = env().await;
    let world = world_for("world-1");
    let first = env.start_job("key-1", &world).await;
    assert_eq!(first.state, ManagementJobState::Succeeded, "{first:?}");
    env.dispatches("world-1", 2).await;
    let live = env.coordinator.decide_next("world-1").await.unwrap();
    assert!(
        matches!(live.action, BatchActionV1::Stop { .. }),
        "{live:?}"
    );

    // The same registration under a new key converges on the persisted world.
    let second = env.start_job("key-2", &world).await;
    assert_ne!(second.id, first.id);
    assert_eq!(second.state, ManagementJobState::Succeeded, "{second:?}");
    assert_eq!(second.step, "exploration_started");
    assert_eq!(started_result(&second), started_result(&first));
    expect_status_ok(&env.dispatcher, &first, "the first job").await;
    expect_status_ok(&env.dispatcher, &second, "the second job").await;
    assert_eq!(
        env.world("world-1").await["node_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn a_world_dispatched_through_the_coordinator_can_still_be_reported_by_a_job() {
    // The world was registered and run by the worker (as MetaTrial's streams are);
    // the management surface reports it afterwards.
    let env = env().await;
    let world = world_for("world-1");
    env.register(&world).await.unwrap();
    env.dispatches("world-1", 1).await;

    let job = env.start_job("report-1", &world).await;
    assert_eq!(job.state, ManagementJobState::Succeeded, "{job:?}");
    let result = started_result(&job);
    // The first decision of the registered world: the lowest root.
    assert_eq!(result["action"]["decision"], "dispatch");
    assert_eq!(result["action"]["action_seqs"], json!([1]));
    expect_status_ok(&env.dispatcher, &job, "a job started after the dispatch").await;
    env.step("world-1", 2, "report-1-2").await;
    expect_status_ok(&env.dispatcher, &job, "after the second dispatch").await;
}

#[tokio::test]
async fn a_job_declaring_other_counters_for_a_world_that_dispatched_fails_with_a_conflict() {
    let env = env().await;
    let world = world_for("world-1");
    let first = env.start_job("counters-1", &world).await;
    assert_eq!(first.state, ManagementJobState::Succeeded, "{first:?}");
    env.dispatches("world-1", 2).await;
    let stored = env.world("world-1").await;
    let before = env.snapshot().await;

    let current = counters(&stored);
    let declarations: [(&str, u64, u8); 3] = [
        ("2_000 for the registered 1_000", 2_000, 2),
        ("the current counters", current.0, 2),
        ("a different recovery budget", REGISTERED_ROOT_MICROS, 1),
    ];
    for (index, (label, root, recovery)) in declarations.into_iter().enumerate() {
        let mut declared = world.clone();
        declared.remaining_root_micros = root;
        declared.remaining_recovery_dispatches = recovery;
        let job = env
            .start_job(&format!("counters-{}", index + 2), &declared)
            .await;
        assert_eq!(job.state, ManagementJobState::Failed, "{label}: {job:?}");
        assert_eq!(job.error_code.as_deref(), Some("conflict"), "{label}");
        assert!(job.result.is_none(), "{label}");
    }
    // Only the three failed jobs and their private inputs were written; the world
    // is what the dispatches left.
    assert_eq!(env.world("world-1").await, stored);
    assert_ne!(env.snapshot().await, before);
    expect_status_ok(&env.dispatcher, &first, "the registering job").await;
}

// ---------------------------------------------------------------------------
// 4. What status still refuses, before and after a dispatch
// ---------------------------------------------------------------------------

/// The ways a stored `exploration.start` result can disagree with the world: each
/// edits one field of the stored job.
fn result_tampers() -> Vec<(&'static str, ValueEdit)> {
    vec![
        ("policy_digest", |job| {
            job["result"]["policy_digest"] = json!(hash(b"forged"));
        }),
        ("caps_digest", |job| {
            job["result"]["caps_digest"] = json!(hash(b"forged"));
        }),
        ("context_signature", |job| {
            job["result"]["context_signature"] = json!(hash(b"forged"));
        }),
        ("prefix_digest", |job| {
            job["result"]["prefix_digest"] = json!(hash(b"forged"));
        }),
        ("legal_actions_digest", |job| {
            job["result"]["legal_actions_digest"] = json!(hash(b"forged"));
        }),
        ("action", |job| {
            job["result"]["action"]["action_seqs"] = json!([2]);
        }),
        ("action cost", |job| {
            job["result"]["action"]["estimated_cost_upper_micros"] = json!(1);
        }),
        // A result stored before E14 has no digests: it must fail closed, not
        // be taken for a match.
        ("pre-E14 digests", |job| {
            let result = job["result"].as_object_mut().unwrap();
            result.remove("policy_digest");
            result.remove("caps_digest");
        }),
    ]
}

#[tokio::test]
async fn status_of_a_started_world_still_refuses_a_stored_result_that_does_not_match() {
    let env = env().await;
    let done = env.start_job("tamper-1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.step("world-1", 1, "tamper-1-1").await;
    expect_status_ok(&env.dispatcher, &done, "before tampering").await;

    let original = raw_job(&env.store, &admin(), &done.id).await;
    for (label, tamper) in result_tampers() {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, original, "{label}: the tamper changed nothing");
        put_raw_job(&env.store, &admin(), &done.id, &tampered).await;
        expect_status_conflict(&env.dispatcher, &done, &format!("a tampered {label}")).await;
        put_raw_job(&env.store, &admin(), &done.id, &original).await;
        expect_status_ok(&env.dispatcher, &done, &format!("after restoring {label}")).await;
    }
}

#[tokio::test]
async fn status_of_a_started_world_refuses_a_stored_world_that_changed_its_policy_caps_or_context()
{
    let env = env().await;
    let done = env.start_job("world-tamper-1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.step("world-1", 1, "world-tamper-1-1").await;
    expect_status_ok(&env.dispatcher, &done, "before tampering").await;

    let original = raw_record(&env.store, WORLD_KIND, "world-1").await;
    let tampers: Vec<(&str, ValueEdit)> = vec![
        ("policy", |world| {
            world["payload"]["policy"]["max_focus_actions"] = json!(1);
        }),
        ("caps", |world| {
            world["payload"]["caps"]["max_nodes"] = json!(6);
        }),
        ("context signature", |world| {
            world["payload"]["context_signature"] = json!(hash(b"forged"));
        }),
    ];
    for (label, tamper) in tampers {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, original, "{label}: the tamper changed nothing");
        put_raw_record(&env.store, WORLD_KIND, "world-1", &tampered).await;
        expect_status_conflict(
            &env.dispatcher,
            &done,
            &format!("a stored world with another {label}"),
        )
        .await;
        put_raw_record(&env.store, WORLD_KIND, "world-1", &original).await;
        expect_status_ok(
            &env.dispatcher,
            &done,
            &format!("after restoring the {label}"),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// 4b. AG-040 R1: status of a started world re-checks the registration
//
// The stored world of a started job has moved on, so its decision cannot be
// compared; what can be is the registration it must still be: the fingerprint of
// its immutable facts and the budget it was registered with (rebuilt from its
// dispatch facts), both against the world in the job's private input, exactly as
// the registration compares them. Each tamper below leaves the world valid and
// live and leaves its context, policy and caps alone, so nothing else sees it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_of_a_started_world_refuses_a_stored_world_whose_source_closure_was_replaced() {
    let env = env().await;
    // Another trusted run that is alive under the same watermark.
    store_trusted_run(&env.store, "run-other", "family-c", TraceOutcome::Success).await;
    let done = env.start_job("closure-r1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.dispatches("world-1", 2).await;
    expect_status_ok(&env.dispatcher, &done, "before tampering").await;

    let original = raw_record(&env.store, WORLD_KIND, "world-1").await;
    let tampers: Vec<(&str, ValueEdit)> = vec![
        ("one run replaced by another live run", |world| {
            world["payload"]["dependencies"][1]["id"] = json!("run-other");
        }),
        ("a live run added", |world| {
            let closure = world["payload"]["dependencies"].as_array_mut().unwrap();
            closure.push(json!({"kind": "run", "id": "run-other"}));
        }),
        ("a run dropped", |world| {
            world["payload"]["dependencies"]
                .as_array_mut()
                .unwrap()
                .pop();
        }),
    ];
    for (label, tamper) in tampers {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, original, "{label}: the tamper changed nothing");
        put_raw_record(&env.store, WORLD_KIND, "world-1", &tampered).await;
        expect_status_conflict(
            &env.dispatcher,
            &done,
            &format!("a stored world with {label}"),
        )
        .await;
        put_raw_record(&env.store, WORLD_KIND, "world-1", &original).await;
        expect_status_ok(&env.dispatcher, &done, &format!("after restoring: {label}")).await;
    }
}

#[tokio::test]
async fn status_of_a_started_world_refuses_rewritten_counters_and_registration_facts() {
    let env = env().await;
    let done = env.start_job("facts-r1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.dispatches("world-1", 2).await;
    expect_status_ok(&env.dispatcher, &done, "before tampering").await;

    let original = raw_record(&env.store, WORLD_KIND, "world-1").await;
    let tampers: Vec<(&str, ValueEdit)> = vec![
        ("the root budget raised by one micro", |world| {
            let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
            world["payload"]["remaining_root_micros"] = json!(root + 1);
        }),
        ("the root budget lowered by one micro", |world| {
            let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
            world["payload"]["remaining_root_micros"] = json!(root - 1);
        }),
        ("the root budget zeroed", |world| {
            world["payload"]["remaining_root_micros"] = json!(0);
        }),
        ("the recovery budget raised", |world| {
            world["payload"]["remaining_recovery_dispatches"] = json!(3);
        }),
        ("the recovery budget lowered", |world| {
            world["payload"]["remaining_recovery_dispatches"] = json!(1);
        }),
        ("the cost of a root opportunity", |world| {
            let root = &mut world["payload"]["root_opportunities"][0];
            let cost = root["estimated_cost_upper_micros"].as_u64().unwrap();
            root["estimated_cost_upper_micros"] = json!(cost + 1);
        }),
        ("the successor cost", |world| {
            world["payload"]["successor_cost_upper_micros"] = json!(11);
        }),
        ("the baseline quality", |world| {
            world["payload"]["initial_baseline_quality_micros"] = json!(400_000);
        }),
        ("the simulation seed", |world| {
            world["payload"]["simulation"]["online"]["fixed_seed"] = json!(8);
        }),
        ("the approved parent digest", |world| {
            world["payload"]["approved_parent_digest"] = json!(hash(b"forged"));
        }),
    ];
    for (label, tamper) in tampers {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, original, "{label}: the tamper changed nothing");
        put_raw_record(&env.store, WORLD_KIND, "world-1", &tampered).await;
        expect_status_conflict(
            &env.dispatcher,
            &done,
            &format!("a stored world with {label} rewritten"),
        )
        .await;
        put_raw_record(&env.store, WORLD_KIND, "world-1", &original).await;
        expect_status_ok(&env.dispatcher, &done, &format!("after restoring: {label}")).await;
    }
}

#[tokio::test]
async fn status_of_a_started_world_refuses_facts_that_do_not_account_for_its_counters() {
    let env = env().await;
    let done = env.start_job("accounts-r1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let steps = env.dispatches("world-1", 2).await;
    let records = DispatchedRecords::load(&env, "world-1", &steps).await;
    expect_status_ok(&env.dispatcher, &done, "before tampering").await;

    // The same tampers the registration refuses.
    for (label, tamper) in unaccounted_tampers(records.first["payload"]["node_id"].clone()) {
        records.put(&env, &records.tampered(&tamper)).await;
        expect_status_conflict(&env.dispatcher, &done, label).await;
        records.restore(&env).await;
        expect_status_ok(&env.dispatcher, &done, &format!("after restoring: {label}")).await;
    }

    // A dispatch fact that is gone, and one the revocation cleanup redacted.
    let mut session = env.store.session().await.unwrap();
    session
        .delete(
            &worker(),
            "artifact",
            &storage_id(DISPATCH_KIND, &records.second_id),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    expect_status_conflict(&env.dispatcher, &done, "a missing dispatch fact").await;
    put_raw_fact(
        &env.store,
        &records.second_id,
        &json!({"schema_version": "rsia.redacted.v1"}),
    )
    .await;
    expect_status_conflict(&env.dispatcher, &done, "a redacted dispatch fact").await;
    records.restore(&env).await;
    expect_status_ok(&env.dispatcher, &done, "after restoring the facts").await;
}

#[tokio::test]
async fn status_of_a_world_that_never_started_still_compares_the_stored_worlds_decision() {
    // No dispatch yet: the stored world's own first decision is compared, so a
    // stored world whose root cost changed no longer matches the result.
    let env = env().await;
    let done = env.start_job("idle-1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    expect_status_ok(&env.dispatcher, &done, "untouched").await;

    let original = raw_record(&env.store, WORLD_KIND, "world-1").await;
    let mut tampered = original.clone();
    let roots = tampered["payload"]["root_opportunities"]
        .as_array_mut()
        .unwrap();
    for root in roots {
        if root["action_seq"] == 1 {
            root["estimated_cost_upper_micros"] = json!(11);
        }
    }
    put_raw_record(&env.store, WORLD_KIND, "world-1", &tampered).await;
    expect_status_conflict(
        &env.dispatcher,
        &done,
        "a stored world with another root cost",
    )
    .await;
    put_raw_record(&env.store, WORLD_KIND, "world-1", &original).await;
    expect_status_ok(&env.dispatcher, &done, "after restoring the root cost").await;
}

#[tokio::test]
async fn status_and_re_registration_hold_while_a_dispatch_is_claimed_and_not_completed() {
    // A claimed dispatch has written no node and spent nothing, but the world has
    // started: its first decision is taken from the request, and agrees.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("claimed-1", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
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
            env.fixture.request("world-1", 1, "claimed-1"),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let stored = env.world("world-1").await;
    assert_eq!(stored["dispatch_ids"].as_array().unwrap().len(), 1);
    assert_eq!(stored["node_ids"], json!([]));
    assert_eq!(counters(&stored).0, REGISTERED_ROOT_MICROS);

    expect_status_ok(&env.dispatcher, &done, "while a dispatch is claimed").await;
    expect_already_registered(env.register(&world).await, "while a dispatch is claimed");
    let again = env.start_job("claimed-2", &world).await;
    assert_eq!(again.state, ManagementJobState::Succeeded, "{again:?}");
    assert_eq!(started_result(&again), started_result(&done));
    expect_status_ok(&env.dispatcher, &again, "the second job").await;
}

#[tokio::test]
async fn status_of_a_started_world_whose_record_was_redacted_is_a_named_conflict() {
    // The cleanup replaces a world of a revoked source with a tombstone. The job's
    // private input is still readable here, so the registration read is what
    // names it: a Conflict that says so, never an internal error.
    let env = env().await;
    let done = env.start_job("redacted-1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.dispatches("world-1", 1).await;
    let original = raw_record(&env.store, WORLD_KIND, "world-1").await;
    put_raw_record(
        &env.store,
        WORLD_KIND,
        "world-1",
        &json!({"schema_version": "rsia.redacted.v1"}),
    )
    .await;
    match env.dispatcher.status(&admin(), &done.id).await {
        Err(Error::Conflict(message)) => assert!(message.contains("redacted"), "{message}"),
        other => panic!("a redacted world must be a named Conflict, got {other:?}"),
    }
    put_raw_record(&env.store, WORLD_KIND, "world-1", &original).await;
    expect_status_ok(&env.dispatcher, &done, "after restoring the world").await;
}

#[tokio::test]
async fn status_of_a_started_world_still_fails_closed_on_a_revoked_source() {
    let env = env().await;
    let done = env.start_job("revoke-1", &world_for("world-1")).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.dispatches("world-1", 2).await;
    expect_status_ok(&env.dispatcher, &done, "before the revocation").await;

    // One trusted run of the source closure is tombstoned: Forbidden.
    let mut session = env.store.session().await.unwrap();
    session
        .put(
            &admin(),
            "tombstone",
            "run-failure",
            "admin",
            &json!({"id": "run-failure"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let result = env.dispatcher.status(&admin(), &done.id).await;
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "a tombstoned source must be Forbidden, got {:?}",
        result.map(|job| job.state)
    );
    // And the persisted terminal is never rewritten.
    let persisted = raw_job(&env.store, &admin(), &done.id).await;
    assert_eq!(persisted["state"], "succeeded");

    // A watermark bump (the source closure changed) is a Conflict.
    let mut session = env.store.session().await.unwrap();
    session
        .bump_watermark(&admin(), "e09-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    expect_status_conflict(&env.dispatcher, &done, "a watermark bump").await;
}

/// The raw stored records `unaccounted_tampers` edit, for a world that
/// dispatched twice, to put edited and to put back.
struct DispatchedRecords {
    world_id: String,
    first_id: String,
    second_id: String,
    world: Value,
    first: Value,
    second: Value,
}

impl DispatchedRecords {
    async fn load(env: &Env, world_id: &str, steps: &[CoordinatorStepResult]) -> Self {
        let first_id = steps[0].dispatch_id.clone().unwrap();
        let second_id = steps[1].dispatch_id.clone().unwrap();
        Self {
            world_id: world_id.into(),
            world: raw_record(&env.store, WORLD_KIND, world_id).await,
            first: raw_fact(&env.store, &first_id).await,
            second: raw_fact(&env.store, &second_id).await,
            first_id,
            second_id,
        }
    }

    /// The three records with one edit applied.
    fn tampered(&self, tamper: &FactTamper) -> (Value, Value, Value) {
        let mut records = (self.world.clone(), self.first.clone(), self.second.clone());
        tamper(&mut records.0, &mut records.1, &mut records.2);
        records
    }

    async fn put(&self, env: &Env, records: &(Value, Value, Value)) {
        put_raw_record(&env.store, WORLD_KIND, &self.world_id, &records.0).await;
        put_raw_fact(&env.store, &self.first_id, &records.1).await;
        put_raw_fact(&env.store, &self.second_id, &records.2).await;
    }

    async fn restore(&self, env: &Env) {
        self.put(
            env,
            &(self.world.clone(), self.first.clone(), self.second.clone()),
        )
        .await;
    }
}

/// One edit of the three stored records of a world that dispatched twice: the
/// world, its first dispatch fact and its second one, as raw JSON.
type FactTamper = Box<dyn Fn(&mut Value, &mut Value, &mut Value)>;

/// The ways a world and its dispatch facts can stop accounting for the budget the
/// world was registered with. Each is an edit of the stored records; both the
/// registration and `status` have to refuse the world it makes (fail closed).
fn unaccounted_tampers(first_node: Value) -> Vec<(&'static str, FactTamper)> {
    vec![
        (
            "a counter lowered by one micro",
            Box::new(|world, _, _| {
                let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
                world["payload"]["remaining_root_micros"] = json!(root - 1);
            }),
        ),
        (
            "a counter raised by one micro",
            Box::new(|world, _, _| {
                let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
                world["payload"]["remaining_root_micros"] = json!(root + 1);
            }),
        ),
        (
            "a recovery counter lowered",
            Box::new(|world, _, _| {
                world["payload"]["remaining_recovery_dispatches"] = json!(1);
            }),
        ),
        (
            "a dispatch fact that names another cost",
            Box::new(|_, first, _| {
                first["payload"]["decision"]["action"]["estimated_cost_upper_micros"] =
                    json!(FIRST_ROOT_COST + 1);
            }),
        ),
        (
            "a dispatch fact of another world",
            Box::new(|_, _, second| {
                second["payload"]["world_id"] = json!("another-world");
            }),
        ),
        (
            "a dispatch fact whose decision is a stop",
            Box::new(|_, _, second| {
                second["payload"]["decision"]["action"] =
                    json!({"decision": "stop", "reason": "forged"});
            }),
        ),
        (
            "a dispatch that is no longer listed (its node is not accounted for)",
            Box::new(|world, _, _| {
                let listed = world["payload"]["dispatch_ids"].as_array_mut().unwrap();
                listed.pop();
            }),
        ),
        (
            "a dispatch listed twice",
            Box::new(|world, _, _| {
                let listed = world["payload"]["dispatch_ids"].as_array_mut().unwrap();
                let first = listed[0].clone();
                listed[1] = first;
            }),
        ),
        (
            "a completed dispatch whose node the world does not list",
            Box::new(|world, _, _| {
                let nodes = world["payload"]["node_ids"].as_array_mut().unwrap();
                nodes.pop();
            }),
        ),
        (
            "a completed dispatch that names no node",
            Box::new(|_, _, second| {
                second["payload"]["node_id"] = Value::Null;
            }),
        ),
        (
            "a dispatch listed twice, under counters that paid for it twice",
            Box::new(|world, _, _| {
                let listed = world["payload"]["dispatch_ids"].as_array_mut().unwrap();
                let first = listed[0].clone();
                listed.push(first);
                let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
                world["payload"]["remaining_root_micros"] = json!(root - FIRST_ROOT_COST);
            }),
        ),
        (
            "a claimed dispatch that names a node",
            Box::new(|_, _, second| {
                second["payload"]["state"] = json!("claimed");
            }),
        ),
        (
            "two completed dispatches that name the same node",
            Box::new(move |_, _, second| {
                second["payload"]["node_id"] = first_node.clone();
            }),
        ),
        // The cost a fact records is the world's own cost of that action: a root
        // opportunity's for a root action, the successor cost for the others. A cost
        // lowered together with the counter that paid for it balances the books and is
        // refused all the same (AG-040 R2, the controller's probe K1).
        (
            "a recorded cost lowered and the counter raised to match (the decision's)",
            Box::new(|world, first, _| {
                set_decision_cost(first, FIRST_ROOT_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "a recorded cost lowered and the counter raised to match (decision and selected action)",
            Box::new(|world, first, _| {
                set_decision_cost(first, FIRST_ROOT_COST - 5);
                set_selected_cost(first, FIRST_ROOT_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "the cost of a selected action alone",
            Box::new(|_, first, _| set_selected_cost(first, FIRST_ROOT_COST - 5)),
        ),
        (
            "the second fact's cost lowered and the counter raised to match",
            Box::new(|world, _, second| {
                set_decision_cost(second, SECOND_ROOT_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "a selected action with no cost",
            Box::new(|_, first, _| {
                first["payload"]["selected_action"]["estimated_cost_upper_micros"] = Value::Null;
            }),
        ),
        (
            "a root action whose action_seq no root opportunity has",
            Box::new(|_, _, second| {
                second["payload"]["action_seq"] = json!(777);
                second["payload"]["selected_action"]["action_seq"] = json!(777);
                second["payload"]["decision"]["action"]["action_seqs"] = json!([777]);
            }),
        ),
        (
            "a root action of another root slot",
            Box::new(|_, first, _| {
                first["payload"]["selected_action"]["kind"]["root_slot"] = json!(2);
            }),
        ),
        (
            "a Deepen that carries the action_seq of a root",
            Box::new(|_, first, _| {
                first["payload"]["selected_action"]["kind"] =
                    json!({"action": "deepen", "parent_node_seq": 1});
            }),
        ),
        (
            "nodes with no dispatch behind them, under the counters of an untouched world",
            Box::new(|world, _, _| {
                world["payload"]["dispatch_ids"] = json!([]);
                world["payload"]["remaining_root_micros"] = json!(REGISTERED_ROOT_MICROS);
                world["payload"]["remaining_recovery_dispatches"] =
                    json!(REGISTERED_RECOVERY_DISPATCHES);
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// 5. The registered budget is rebuilt from the dispatch facts, exactly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_uncertain_dispatch_spent_its_cost_and_a_claimed_one_has_spent_nothing() {
    let env = env().await;

    // Uncertain: the model failed after the dispatch was claimed. The cost is
    // incurred, so the registration is the stored counters plus that cost.
    let uncertain = world_for("world-uncertain");
    env.register(&uncertain).await.unwrap();
    let step = env
        .coordinator
        .run_next(
            Some(&FailingModel),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-uncertain", 1, "uncertain-1"),
        )
        .await
        .unwrap();
    assert!(step.outcome.contains("not found"), "{}", step.outcome);
    let stored = env.world("world-uncertain").await;
    assert_eq!(
        counters(&stored).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    let fact = raw_fact(&env.store, step.dispatch_id.as_deref().unwrap()).await;
    assert_eq!(fact["payload"]["state"], "uncertain");
    expect_already_registered(env.register(&uncertain).await, "an uncertain dispatch");

    // Claimed: one observed dispatch, then a second one that is claimed and never
    // observed (the future is dropped once the model was entered). The claimed
    // one has spent nothing, so only the observed cost is added back.
    let mixed = world_for("world-mixed");
    env.register(&mixed).await.unwrap();
    env.step("world-mixed", 1, "mixed-1").await;
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
            env.fixture.request("world-mixed", 2, "mixed-2"),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let stored = env.world("world-mixed").await;
    assert_eq!(stored["dispatch_ids"].as_array().unwrap().len(), 2);
    assert_eq!(stored["node_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        counters(&stored).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    let claimed = raw_fact(&env.store, stored["dispatch_ids"][1].as_str().unwrap()).await;
    assert_eq!(claimed["payload"]["state"], "claimed");
    expect_already_registered(env.register(&mixed).await, "a claimed dispatch");

    // A world whose only dispatch is claimed has not spent anything either.
    let claimed_only = world_for("world-claimed-only");
    env.register(&claimed_only).await.unwrap();
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
            env.fixture.request("world-claimed-only", 1, "claimed-only-1"),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let stored = env.world("world-claimed-only").await;
    assert_eq!(stored["dispatch_ids"].as_array().unwrap().len(), 1);
    assert_eq!(stored["node_ids"], json!([]));
    assert_eq!(counters(&stored).0, REGISTERED_ROOT_MICROS);
    expect_already_registered(env.register(&claimed_only).await, "a claimed-only world");
    let mut other = claimed_only.clone();
    other.remaining_root_micros = REGISTERED_ROOT_MICROS - 1;
    expect_conflict(
        env.register(&other).await,
        "a claimed-only world, other budget",
    );
}

#[tokio::test]
async fn a_dispatch_that_spent_a_recovery_dispatch_is_added_back() {
    // `run_next` spends one recovery dispatch, and the successor cost, when the
    // selected action is a `Recover`. A real recovery needs a repairable failure no
    // step produces yet (E09 PR-B), so the fact of one real dispatch is rewritten
    // into the fact of the recover dispatch `derive_legal_actions` would have derived
    // for the first node (id, numbering after the roots, successor cost) and the
    // world's counters into what that dispatch would have left.
    let env = env().await;
    let world = world_for("world-1");
    env.register(&world).await.unwrap();
    let step = env.step("world-1", 1, "recover-1").await;
    let dispatch_id = step.dispatch_id.clone().unwrap();
    let fact = raw_fact(&env.store, &dispatch_id).await;
    let world_record = raw_record(&env.store, WORLD_KIND, "world-1").await;
    expect_already_registered(env.register(&world).await, "the real dispatch");

    let mut recover_fact = fact.clone();
    let payload = &mut recover_fact["payload"];
    payload["action_id"] = json!("recover-1");
    payload["action_seq"] = json!(1_000_003);
    payload["selected_action"] = json!({
        "action_id": "recover-1",
        "action_seq": 1_000_003,
        "branch_seq": 1,
        "target_depth": 2,
        "kind": {"action": "recover", "failed_node_seq": 1, "episode_id": "episode-1"},
        "estimated_cost_upper_micros": SUCCESSOR_COST,
    });
    payload["decision"]["action"]["action_ids"] = json!(["recover-1"]);
    payload["decision"]["action"]["action_seqs"] = json!([1_000_003]);
    payload["decision"]["action"]["estimated_cost_upper_micros"] = json!(SUCCESSOR_COST);
    // The world as that dispatch would have left it: it cost the successor cost and
    // one recovery dispatch.
    let mut spent_world = world_record.clone();
    spent_world["payload"]["remaining_recovery_dispatches"] =
        json!(REGISTERED_RECOVERY_DISPATCHES - 1);
    spent_world["payload"]["remaining_root_micros"] =
        json!(REGISTERED_ROOT_MICROS - SUCCESSOR_COST);
    // The real dispatch's world with one recovery dispatch spent.
    let mut spent_recovery_only = world_record.clone();
    spent_recovery_only["payload"]["remaining_recovery_dispatches"] =
        json!(REGISTERED_RECOVERY_DISPATCHES - 1);

    // A recover dispatch and the counters that paid for it: registered.
    put_raw_fact(&env.store, &dispatch_id, &recover_fact).await;
    put_raw_record(&env.store, WORLD_KIND, "world-1", &spent_world).await;
    expect_already_registered(env.register(&world).await, "a spent recovery dispatch");
    // A recover dispatch the counters never paid for, and a recovery counter that
    // spent one with no recover dispatch behind it: neither is the registered world.
    put_raw_record(&env.store, WORLD_KIND, "world-1", &world_record).await;
    expect_conflict(
        env.register(&world).await,
        "a recover dispatch that spent nothing",
    );
    put_raw_fact(&env.store, &dispatch_id, &fact).await;
    put_raw_record(&env.store, WORLD_KIND, "world-1", &spent_recovery_only).await;
    expect_conflict(
        env.register(&world).await,
        "a spent recovery counter without a recover dispatch",
    );
    put_raw_record(&env.store, WORLD_KIND, "world-1", &world_record).await;
    expect_already_registered(env.register(&world).await, "restored");
}

#[tokio::test]
async fn a_world_whose_facts_do_not_account_for_its_counters_is_not_the_registered_one() {
    let env = env().await;
    let world = world_for("world-1");
    env.register(&world).await.unwrap();
    let steps = env.dispatches("world-1", 2).await;
    let first_id = steps[0].dispatch_id.clone().unwrap();
    let second_id = steps[1].dispatch_id.clone().unwrap();
    let world_record = raw_record(&env.store, WORLD_KIND, "world-1").await;
    let first_fact = raw_fact(&env.store, &first_id).await;
    let second_fact = raw_fact(&env.store, &second_id).await;
    let first_node = first_fact["payload"]["node_id"].clone();
    assert!(first_node.is_string());
    expect_already_registered(env.register(&world).await, "untouched");

    // Each case edits one stored record; the registration must be refused
    // (fail closed) and the untouched records must be accepted again.
    let records = DispatchedRecords {
        world_id: "world-1".into(),
        first_id: first_id.clone(),
        second_id: second_id.clone(),
        world: world_record.clone(),
        first: first_fact.clone(),
        second: second_fact.clone(),
    };
    for (label, tamper) in unaccounted_tampers(first_node) {
        records.put(&env, &records.tampered(&tamper)).await;
        expect_conflict(env.register(&world).await, label);
        records.restore(&env).await;
        expect_already_registered(
            env.register(&world).await,
            &format!("after restoring: {label}"),
        );
    }

    // A dispatch fact that is gone is not a registration either.
    let mut session = env.store.session().await.unwrap();
    session
        .delete(
            &worker(),
            "artifact",
            &storage_id(DISPATCH_KIND, &second_id),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    expect_conflict(env.register(&world).await, "a missing dispatch fact");
    put_raw_fact(&env.store, &second_id, &second_fact).await;
    expect_already_registered(env.register(&world).await, "restored");

    // A fact the revocation cleanup redacted is named as such, not reported as a
    // storage failure.
    put_raw_fact(
        &env.store,
        &second_id,
        &json!({"schema_version": "rsia.redacted.v1"}),
    )
    .await;
    match env.register(&world).await {
        Err(Error::Conflict(message)) => assert!(message.contains("redacted"), "{message}"),
        other => panic!("a redacted dispatch fact must be a named Conflict, got {other:?}"),
    }
    put_raw_fact(&env.store, &second_id, &second_fact).await;
    expect_already_registered(env.register(&world).await, "restored again");
}

// ---------------------------------------------------------------------------
// 6. AG-040 R2: every recorded cost is the world's immutable cost of its action
//
// The budget rebuilt from the dispatch facts takes what each completed fact
// spent. A fact records that cost itself, so a fact edited together with the
// counter that paid for it balanced the books (the controller's probe K1) and
// raised what the world may still spend. The cost of an action is fixed by the
// world's registration (the fingerprint covers it, and both registration and
// `status` compare the fingerprint first): the cost of its root opportunity for
// a root action, the successor cost for a Deepen or a Recover, as
// `derive_legal_actions` derives them. Every fact, a claimed one included, has to
// record that cost, in its decision and in its selected action.
// ---------------------------------------------------------------------------

/// Registration, `status` and a second job for the same world all refuse what
/// `tamper` made of the records, and accept the untouched ones again.
async fn expect_refused_everywhere(
    env: &Env,
    world: &ExplorationWorldV1,
    done: &ManagementJob,
    records: &DispatchedRecords,
    key: &str,
    label: &str,
    tamper: &FactTamper,
) {
    records.put(env, &records.tampered(tamper)).await;
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
    records.restore(env).await;
    expect_already_registered(env.register(world).await, &format!("restored: {label}"));
    expect_status_ok(&env.dispatcher, done, &format!("restored: {label}")).await;
}

#[tokio::test]
async fn a_recorded_cost_lowered_with_the_counter_raised_to_match_is_refused_everywhere() {
    // K1: the cost the first completed fact records goes from 10 to 5 and the
    // remaining root budget of the world from 965 to 970. Nothing else is touched.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("k1-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let steps = env.dispatches("world-1", 2).await;
    let records = DispatchedRecords::load(&env, "world-1", &steps).await;
    assert_eq!(
        counters(&env.world("world-1").await).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SECOND_ROOT_COST
    );
    expect_already_registered(env.register(&world).await, "untouched");
    expect_status_ok(&env.dispatcher, &done, "untouched").await;

    let tampers: Vec<(&str, FactTamper)> = vec![
        (
            "K1: the first fact's decision cost 10 -> 5, the world's budget 965 -> 970",
            Box::new(|world, first, _| {
                set_decision_cost(first, 5);
                world["payload"]["remaining_root_micros"] = json!(970);
            }),
        ),
        (
            "the same with the selected action's cost lowered too",
            Box::new(|world, first, _| {
                set_decision_cost(first, 5);
                set_selected_cost(first, 5);
                world["payload"]["remaining_root_micros"] = json!(970);
            }),
        ),
        (
            "the second fact lowered by 5 and the budget raised by 5",
            Box::new(|world, _, second| {
                set_decision_cost(second, SECOND_ROOT_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "a selected action's cost alone, no counter touched",
            Box::new(|_, first, _| set_selected_cost(first, 5)),
        ),
        (
            "a cost raised and the budget lowered to match",
            Box::new(|world, first, _| {
                set_decision_cost(first, FIRST_ROOT_COST + 5);
                let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
                world["payload"]["remaining_root_micros"] = json!(root - 5);
            }),
        ),
    ];
    for (index, (label, tamper)) in tampers.iter().enumerate() {
        expect_refused_everywhere(
            &env,
            &world,
            &done,
            &records,
            &format!("k1-job-{index}"),
            label,
            tamper,
        )
        .await;
    }
}

#[tokio::test]
async fn a_deepen_fact_is_bound_to_the_successor_cost_of_the_world() {
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("deepen-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let steps = env.widen_then_deepen("world-1").await;

    // The real Deepen spent the successor cost, not a root's, and the facts account
    // for it: the untouched world is accepted by registration and by `status`.
    let stored = env.world("world-1").await;
    assert_eq!(
        counters(&stored),
        (
            REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SUCCESSOR_COST,
            u64::from(REGISTERED_RECOVERY_DISPATCHES)
        )
    );
    let records = DispatchedRecords::load(&env, "world-1", &steps).await;
    assert_eq!(
        records.second["payload"]["selected_action"]["kind"]["action"],
        "deepen"
    );
    expect_already_registered(env.register(&world).await, "after a real Deepen");
    expect_status_ok(&env.dispatcher, &done, "after a real Deepen").await;

    let tampers: Vec<(&str, FactTamper)> = vec![
        (
            "the Deepen's decision cost lowered, the budget raised to match",
            Box::new(|world, _, second| {
                set_decision_cost(second, SUCCESSOR_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "the Deepen's decision and selected cost lowered, the budget raised to match",
            Box::new(|world, _, second| {
                set_decision_cost(second, SUCCESSOR_COST - 5);
                set_selected_cost(second, SUCCESSOR_COST - 5);
                raise_remaining_root(world, 5);
            }),
        ),
        (
            "the Deepen recorded at the cost of a root, the budget lowered to match",
            Box::new(|world, _, second| {
                set_decision_cost(second, SECOND_ROOT_COST);
                set_selected_cost(second, SECOND_ROOT_COST);
                let root = world["payload"]["remaining_root_micros"].as_u64().unwrap();
                world["payload"]["remaining_root_micros"] = json!(root - 10);
            }),
        ),
        (
            "the Deepen's selected cost alone",
            Box::new(|_, _, second| set_selected_cost(second, SUCCESSOR_COST - 5)),
        ),
        (
            "a Deepen relabelled as a root action (no root has its action_seq)",
            Box::new(|_, _, second| {
                second["payload"]["selected_action"]["kind"] =
                    json!({"action": "widen", "root_slot": 2});
            }),
        ),
        (
            "a Deepen that carries the action_seq of a root",
            Box::new(|_, _, second| {
                second["payload"]["action_seq"] = json!(2);
                second["payload"]["selected_action"]["action_seq"] = json!(2);
                second["payload"]["decision"]["action"]["action_seqs"] = json!([2]);
            }),
        ),
    ];
    for (index, (label, tamper)) in tampers.iter().enumerate() {
        expect_refused_everywhere(
            &env,
            &world,
            &done,
            &records,
            &format!("deepen-job-{index}"),
            label,
            tamper,
        )
        .await;
    }
}

#[tokio::test]
async fn a_claimed_dispatch_fact_is_bound_to_the_worlds_cost_but_spends_nothing() {
    // One observed dispatch and one that is claimed and never completed (the future
    // is dropped once the model was entered): the claimed one has spent nothing, but
    // the cost it records is held to the world's cost of its action all the same.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("claimed-cost-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    env.step("world-1", 1, "claimed-cost-1").await;
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
            env.fixture.request("world-1", 2, "claimed-cost-2"),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let stored = env.world("world-1").await;
    let claimed_id = stored["dispatch_ids"][1].as_str().unwrap().to_string();
    let claimed = raw_fact(&env.store, &claimed_id).await;
    assert_eq!(claimed["payload"]["state"], "claimed");
    assert_eq!(
        counters(&stored).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    expect_already_registered(env.register(&world).await, "untouched");
    expect_status_ok(&env.dispatcher, &done, "untouched").await;

    let tampers: Vec<(&str, ValueEdit)> = vec![
        ("a claimed fact's decision cost", |fact| {
            set_decision_cost(fact, SECOND_ROOT_COST - 5);
        }),
        ("a claimed fact's selected cost", |fact| {
            set_selected_cost(fact, SECOND_ROOT_COST - 5);
        }),
        ("a claimed fact's cost raised", |fact| {
            set_decision_cost(fact, SECOND_ROOT_COST + 5);
        }),
    ];
    for (index, (label, tamper)) in tampers.into_iter().enumerate() {
        let mut tampered = claimed.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, claimed, "{label}: the tamper changed nothing");
        put_raw_fact(&env.store, &claimed_id, &tampered).await;
        expect_conflict(env.register(&world).await, label);
        expect_status_conflict(&env.dispatcher, &done, label).await;
        let again = env
            .start_job(&format!("claimed-cost-job-{index}"), &world)
            .await;
        assert_eq!(
            again.state,
            ManagementJobState::Failed,
            "{label}: {again:?}"
        );
        put_raw_fact(&env.store, &claimed_id, &claimed).await;
        expect_already_registered(env.register(&world).await, &format!("restored: {label}"));
        expect_status_ok(&env.dispatcher, &done, &format!("restored: {label}")).await;
    }
}

#[tokio::test]
async fn a_claimed_dispatch_resumed_to_completion_keeps_status_and_registration() {
    // The cost a fact records has to stay the world's cost of its action while the
    // fact goes from claimed to completed: a dispatch claimed and left in flight,
    // resumed with the same request, then a second dispatch (the controller's probe
    // K2). At every stage `status` holds, a re-registration is `AlreadyRegistered`,
    // and another request key reports the first decision.
    let env = env().await;
    let world = world_for("world-1");
    let done = env.start_job("resume-start", &world).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
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
            env.fixture.request("world-1", 1, "resume-1"),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let claimed = env.world("world-1").await;
    assert_eq!(claimed["node_ids"], json!([]));
    assert_eq!(counters(&claimed).0, REGISTERED_ROOT_MICROS);
    expect_status_ok(&env.dispatcher, &done, "claimed").await;
    expect_already_registered(env.register(&world).await, "claimed");

    // The same request, with a model that answers: the claimed dispatch completes.
    env.coordinator
        .run_next(
            Some(&EditingFixtureModel),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-1", 1, "resume-1"),
        )
        .await
        .unwrap();
    let completed = env.world("world-1").await;
    assert_eq!(completed["node_ids"].as_array().unwrap().len(), 1);
    assert_eq!(completed["dispatch_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        counters(&completed).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    expect_status_ok(&env.dispatcher, &done, "after the claim completed").await;
    expect_already_registered(env.register(&world).await, "after the claim completed");

    env.step("world-1", 2, "resume-2").await;
    assert_eq!(
        counters(&env.world("world-1").await).0,
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST - SECOND_ROOT_COST
    );
    expect_status_ok(&env.dispatcher, &done, "after a second dispatch").await;
    expect_already_registered(env.register(&world).await, "after a second dispatch");
    let again = env.start_job("resume-other-key", &world).await;
    assert_eq!(again.state, ManagementJobState::Succeeded, "{again:?}");
    assert_eq!(started_result(&again), started_result(&done));
}
