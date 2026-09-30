//! AG-024 (E09/E08; plan §7.2/§7.2.1, §11 and §11.5, V017): the nodes, dispatch
//! facts and optimization history entries of an exploration world are on the
//! source-revocation cleanup closure.
//!
//! A world has always depended on its source runs (world -> run). Its node,
//! dispatch-fact and history records carried no edge, so revoking a run redacted
//! the world and left their content in the store. Each of them now depends on
//! its world (record -> world); the cleanup follows `dependents`, so it reaches
//! them as run -> world -> record.
//!
//! Not covered (and not claimed): records written before the edges existed have
//! none and nothing back-fills them (`a_record_without_the_edge_...` pins that
//! boundary), and the cleanup's redaction rules themselves, which are unchanged.
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evaluation::DataUse;
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
    ElasticPolicyV1, ExplorationCapsV1, HistoryOutcome, OptimizationHistoryEntry, SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, PersistentCoordinator,
    RootOpportunity, WorldState, verified_world_decision_view,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationStepRequest,
    PairedTaskResult, StoreOptimizationJournal,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, CleanupStatus};
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

const WORLD: &str = "exploration_world_v1";
const NODE: &str = "exploration_node_v1";
const DISPATCH: &str = "exploration_dispatch_v1";
const HISTORY: &str = "optimization_history_v1";
const ENVELOPE: &str = "rsia.exploration_artifact_envelope.v1";

/// The two trusted runs a world's source closure consists of.
const RUNS: [&str; 2] = ["run-failure", "run-success"];
/// A trusted run that belongs to no world of the main closure.
const UNRELATED_RUN: &str = "run-unrelated";

// ---------------------------------------------------------------------------
// Fixtures: the trusted source closure and request shape of `exploration_v41`
// and `meta_inheritance_v42`, with the world's id, policy and runs as
// parameters.
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

/// One focused action per branch: the first dispatch widens root 1 and the
/// second must widen root 2, so both can be requested against the world's own
/// parent skill and bundle.
fn policy_focus_one() -> ElasticPolicyV1 {
    ElasticPolicyV1 {
        max_focus_actions: 1,
        ..ElasticPolicyV1::default()
    }
}

fn world_for(id: &str, runs: &[&str]) -> ExplorationWorldV1 {
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
        policy: policy_focus_one(),
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
        dependencies: runs
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

/// Seeds three Host-issued trace authorities (the two runs of the main closure
/// and one unrelated run), a source selection grant for the main closure and one
/// watermark bump, so every world freezes `source_watermark == 1`.
async fn seeded_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("exploration-cleanup.sqlite3"))
        .await
        .unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
        (UNRELATED_RUN, "family-c", TraceOutcome::Success),
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

/// Counts every model dispatch: a revoked world must never reach one.
#[derive(Default)]
struct CountingModel(AtomicUsize);

#[async_trait]
impl ModelPort for CountingModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        EditingFixtureModel.dispatch(request).await
    }
}

/// A model port that never answers. Dropping the `run_next` future once it was
/// entered simulates a crash after the dispatch fact was claimed and before it
/// was observed: the fact stays `claimed`.
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

/// A model port that revokes `run-failure` while its dispatch is in flight and
/// then answers as the fixture model would: a result that arrives after the
/// logical block.
struct RevokingModel {
    store: Store,
    revoked: AtomicBool,
}

#[async_trait]
impl ModelPort for RevokingModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        if !self.revoked.swap(true, Ordering::SeqCst) {
            revoke(&self.store, "run-failure").await;
        }
        EditingFixtureModel.dispatch(request).await
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
                run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
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

    /// A request against the world's own parent skill/bundle, i.e. for a
    /// `Widen` action. `tag` keeps the request/idempotency ids distinct.
    fn request(&self, world_id: &str, step: u32, tag: &str) -> OptimizationStepRequest<'_> {
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
                parent_skill_digest: skill_snapshot_digest(&self.parent).unwrap(),
                bundle_digest: self.parent_bundle.clone(),
                source_closure: self.allowed.clone(),
                model_digest: hash(b"model"),
                tools_digest: hash(b"tools"),
                rules_digest: hash(b"rules"),
                sampling_digest: hash(b"sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &self.parent,
            edit_context: &self.edit_context,
            edit_batch_template: SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: "n".into(),
                profile_id: "profile".into(),
                skill_id: "skill".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(&self.parent).unwrap(),
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
                parent_bundle_digest: self.parent_bundle.clone(),
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

struct Env {
    dir: tempfile::TempDir,
    store: Store,
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    fixture: Fixture,
}

async fn env() -> Env {
    let (dir, store) = seeded_store().await;
    let worker = worker();
    Env {
        coordinator: PersistentCoordinator::new(store.clone(), worker.clone(), "worker").unwrap(),
        journal: StoreOptimizationJournal::new(store.clone(), worker, "worker").unwrap(),
        fixture: Fixture::new(),
        store,
        dir,
    }
}

fn worker() -> Context {
    Context::new("n", "worker", Role::Worker).unwrap()
}

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}

impl Env {
    async fn register(&self, id: &str, runs: &[&str]) {
        self.coordinator
            .register_world(world_for(id, runs))
            .await
            .unwrap();
    }

    /// One real dispatch through `run_next`, observed with a fixture runner.
    async fn dispatch(&self, world_id: &str, step: u32, tag: &str) -> CoordinatorStepResult {
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

    /// Enters a dispatch whose model never answers and drops it: the dispatch
    /// fact is committed as `claimed`, no node exists yet.
    async fn claim_and_abandon(&self, world_id: &str, step: u32, tag: &str) {
        let entered = Arc::new(Notify::new());
        let hanging = HangingModel {
            entered: entered.clone(),
        };
        tokio::select! {
            biased;
            result = self.coordinator.run_next(
                Some(&hanging),
                Some(&ImprovingFixtureRunner),
                Some(&self.journal),
                self.fixture.request(world_id, step, tag),
            ) => panic!("the hanging model must not answer: {result:?}"),
            () = entered.notified() => {}
        }
    }
}

fn history(entry_id: &str, sequence: u64) -> OptimizationHistoryEntry {
    OptimizationHistoryEntry {
        entry_id: entry_id.into(),
        sequence,
        parent_digest: hash(b"approved-parent"),
        environment_digest: hash(b"env"),
        task_family: "family".into(),
        source_watermark: 1,
        input_digest: hash(b"input"),
        patch_digest: hash(b"patch"),
        evidence_digest: hash(b"evidence"),
        outcome: HistoryOutcome::NoChange,
        deterministic_error: true,
        data_use: DataUse::Development,
        summary: format!("development summary of {entry_id}"),
    }
}

// ---------------------------------------------------------------------------
// Store access and the world under test
// ---------------------------------------------------------------------------

fn storage_id(record_kind: &str, id: &str) -> String {
    format!("e09-{}", fingerprint(&(record_kind, id)).unwrap())
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

async fn run_object(store: &Store, run: &str) -> Option<Value> {
    let mut session = store.session().await.unwrap();
    let value = session.get::<Value>(&worker(), "run", run).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// A world holding real records of every kind: two observed dispatches (so two
/// dispatch facts and two nodes) and two history entries.
struct Chain {
    world_id: String,
    first: CoordinatorStepResult,
    second: CoordinatorStepResult,
    nodes: Vec<String>,
    dispatches: Vec<String>,
    history: Vec<String>,
}

impl Chain {
    /// `(record_kind, id)` of every record that depends on the world.
    fn dependents(&self) -> Vec<(&'static str, String)> {
        let mut records = Vec::new();
        records.extend(self.nodes.iter().map(|id| (NODE, id.clone())));
        records.extend(self.dispatches.iter().map(|id| (DISPATCH, id.clone())));
        records.extend(self.history.iter().map(|id| (HISTORY, id.clone())));
        records
    }

    /// The world followed by everything that depends on it.
    fn records(&self) -> Vec<(&'static str, String)> {
        let mut records = vec![(WORLD, self.world_id.clone())];
        records.extend(self.dependents());
        records
    }

    fn world_storage_id(&self) -> String {
        storage_id(WORLD, &self.world_id)
    }
}

fn id_list(world: &Value, field: &str) -> Vec<String> {
    world["payload"][field]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

async fn chain(env: &Env, world_id: &str) -> Chain {
    env.register(world_id, &RUNS).await;
    let first = env.dispatch(world_id, 1, &format!("{world_id}-1")).await;
    let second = env.dispatch(world_id, 2, &format!("{world_id}-2")).await;
    for sequence in 1..=2u64 {
        env.coordinator
            .record_history(
                world_id,
                history(&format!("{world_id}-history-{sequence}"), sequence),
            )
            .await
            .unwrap();
    }
    let world = raw_record(&env.store, WORLD, world_id).await;
    let chain = Chain {
        world_id: world_id.into(),
        first,
        second,
        nodes: id_list(&world, "node_ids"),
        dispatches: id_list(&world, "dispatch_ids"),
        history: id_list(&world, "history_ids"),
    };
    assert_eq!(chain.nodes.len(), 2, "two nodes");
    assert_eq!(chain.dispatches.len(), 2, "two dispatch facts");
    assert_eq!(chain.history.len(), 2, "two history entries");
    assert_eq!(
        [&chain.first.node_id, &chain.second.node_id].map(|id| id.as_deref().unwrap()),
        [chain.nodes[0].as_str(), chain.nodes[1].as_str()]
    );
    chain
}

// ---------------------------------------------------------------------------
// The dependency graph, seen only through `Session::dependents`
// ---------------------------------------------------------------------------

type Edge = (String, String, String, String);

/// Every stored edge row whose destination is one of `dsts`, as
/// `(src_kind, src_id, dst_kind, dst_id)`. A row listed twice fails the test, so
/// the size of the set is the number of rows.
async fn edges_into(store: &Store, dsts: &[(&str, String)]) -> BTreeSet<Edge> {
    let mut session = store.session().await.unwrap();
    let mut edges = BTreeSet::new();
    for (dst_kind, dst_id) in dsts {
        for (src_kind, src_id) in session
            .dependents(&worker(), dst_kind, dst_id)
            .await
            .unwrap()
        {
            let edge = (src_kind, src_id, (*dst_kind).to_owned(), dst_id.clone());
            assert!(edges.insert(edge.clone()), "duplicate edge row {edge:?}");
        }
    }
    session.commit().await.unwrap();
    edges
}

/// The objects an exploration edge can point at: the runs, the world and every
/// one of its records.
fn edge_universe(chain: &Chain) -> Vec<(&'static str, String)> {
    let mut dsts: Vec<(&'static str, String)> = RUNS
        .iter()
        .chain([&UNRELATED_RUN])
        .map(|run| ("run", (*run).to_owned()))
        .collect();
    dsts.extend(
        chain
            .records()
            .into_iter()
            .map(|(kind, id)| ("artifact", storage_id(kind, &id))),
    );
    dsts
}

/// Every object reachable from `start` by the cleanup's own step: the
/// dependents of a node are its children.
async fn closure_from(store: &Store, start: (&str, &str)) -> BTreeSet<(String, String)> {
    let mut session = store.session().await.unwrap();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([(start.0.to_owned(), start.1.to_owned())]);
    while let Some((kind, id)) = queue.pop_front() {
        for child in session.dependents(&worker(), &kind, &id).await.unwrap() {
            if seen.insert(child.clone()) {
                queue.push_back(child);
            }
        }
    }
    session.commit().await.unwrap();
    seen
}

// ---------------------------------------------------------------------------
// Revocation and cleanup
// ---------------------------------------------------------------------------

async fn revoke(store: &Store, run: &str) -> CleanupStatus {
    LifecycleCoordinator::revoke_source(&admin(), store, run, "privacy", 200)
        .await
        .unwrap()
}

/// Pages the cleanup to its end with a small edge page so the walk needs many
/// steps. A `Failed` job (an unknown scope) fails the test. Returns the final
/// status and the number of steps taken.
async fn finish_cleanup(
    store: &Store,
    mut status: CleanupStatus,
    page: usize,
) -> (CleanupStatus, u32) {
    let admin = admin();
    let mut steps = 0;
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
        steps += 1;
    }
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "cleanup did not complete: {:?}",
        status.last_error
    );
    assert_eq!(status.pending_nodes, 0);
    assert!(status.last_error.is_none(), "{:?}", status.last_error);
    (status, steps)
}

async fn revoke_and_clean(store: &Store, run: &str, page: usize) -> (CleanupStatus, u32) {
    let started = revoke(store, run).await;
    finish_cleanup(store, started, page).await
}

/// The stored object is the redacted tombstone the cleanup leaves behind, and
/// it keeps none of `content`.
async fn assert_redacted(store: &Store, record_kind: &str, id: &str, content: &[String]) {
    let value = raw_record(store, record_kind, id).await;
    assert_eq!(
        value["schema_version"], "rsia.redacted.v1",
        "{record_kind} {id} was not redacted: {value}"
    );
    assert_eq!(value["state"], "source_revoked", "{record_kind} {id}");
    assert_eq!(value["original_kind"], "artifact", "{record_kind} {id}");
    assert_eq!(value["original_schema"], ENVELOPE, "{record_kind} {id}");
    assert_eq!(
        value["metadata"]["record_kind"], record_kind,
        "{record_kind} {id}"
    );
    assert!(value.get("payload").is_none(), "{record_kind} {id}");
    let text = value.to_string();
    for needle in content {
        assert!(
            !text.contains(needle.as_str()),
            "{record_kind} {id} still carries {needle}"
        );
    }
}

/// The record's original envelope is stored as it was.
async fn assert_intact(store: &Store, record_kind: &str, id: &str, before: &Value) {
    let value = raw_record(store, record_kind, id).await;
    assert_eq!(value["schema_version"], ENVELOPE, "{record_kind} {id}");
    assert_eq!(&value, before, "{record_kind} {id} changed");
}

/// Content that only the original payload carries and the redacted tombstone
/// must not.
fn content_of(record_kind: &str, envelope: &Value) -> Vec<String> {
    let payload = &envelope["payload"];
    let fields: &[&str] = match record_kind {
        WORLD => &["approved_parent_digest", "parent_bundle_digest"],
        NODE => &[
            "candidate_bundle_digest",
            "candidate_skill_digest",
            "development_selection_digest",
        ],
        DISPATCH => &["idempotency_key", "expected_parent_bundle_digest"],
        HISTORY => &["summary", "patch_digest"],
        other => panic!("unexpected record kind {other}"),
    };
    let content: Vec<String> = fields
        .iter()
        .map(|field| {
            payload[field]
                .as_str()
                .unwrap_or_else(|| panic!("{record_kind}.{field} is not a string: {payload}"))
                .to_owned()
        })
        .collect();
    let text = envelope.to_string();
    for needle in &content {
        assert!(text.contains(needle.as_str()));
    }
    content
}

/// An uncertain node carries no candidate digest; what it does carry that the
/// tombstone must not is the approved parent digest of its prefix.
fn content_of_uncertain(envelope: &Value) -> Vec<String> {
    let digest = envelope["payload"]["node"]["approved_parent_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(envelope.to_string().contains(&digest));
    vec![digest]
}

/// What the coordinator's entry points say about `world_id` right now.
struct Probe {
    results: Vec<(&'static str, Result<String>)>,
}

impl Probe {
    fn errors(&self) -> impl Iterator<Item = (&'static str, &Error)> {
        self.results.iter().map(|(name, result)| match result {
            Ok(value) => panic!("{name} succeeded on a revoked world: {value}"),
            Err(error) => (*name, error),
        })
    }
}

/// Calls `decide_next`, a fresh `run_next`, a reconnecting `run_next` (when the
/// world already has a dispatch), `verified_world_decision_view` and
/// `verified_mechanism_usage`, none of which may reach the model port.
async fn probe(
    env: &Env,
    world_id: &str,
    fresh: (u32, &str),
    reconnect: Option<(u32, &str)>,
) -> Probe {
    let counting = CountingModel::default();
    let decide_next = env
        .coordinator
        .decide_next(world_id)
        .await
        .map(|value| format!("{value:?}"));
    let fresh_run_next = env
        .coordinator
        .run_next(
            Some(&counting),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request(world_id, fresh.0, fresh.1),
        )
        .await
        .map(|value| format!("{value:?}"));
    let reconnect_run_next = match reconnect {
        Some((step, tag)) => Some(
            env.coordinator
                .run_next(None, None, None, env.fixture.request(world_id, step, tag))
                .await
                .map(|value| format!("{value:?}")),
        ),
        None => None,
    };
    let decision_view = verified_world_decision_view(&admin(), &env.store, world_id)
        .await
        .map(|value| format!("{value:?}"));
    let mechanism_usage = env
        .coordinator
        .verified_mechanism_usage(world_id)
        .await
        .map(|value| format!("{value:?}"));
    assert_eq!(
        counting.0.load(Ordering::SeqCst),
        0,
        "no model call may follow a revocation"
    );
    let mut results = vec![
        ("decide_next", decide_next),
        ("run_next (fresh request)", fresh_run_next),
    ];
    results.extend(reconnect_run_next.map(|result| ("run_next (reconnect)", result)));
    results.push(("verified_world_decision_view", decision_view));
    results.push(("verified_mechanism_usage", mechanism_usage));
    Probe { results }
}

// ---------------------------------------------------------------------------
// 1. The full chain: a revoked run redacts everything the world owns
// ---------------------------------------------------------------------------

async fn revoked_run_redacts_the_whole_world(revoked_run: &str) {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;

    // Before the revocation every record is a live, typed envelope that carries
    // content of its own; remember what the cleanup must remove.
    let mut originals = Vec::new();
    for (kind, id) in chain.records() {
        let envelope = raw_record(&env.store, kind, &id).await;
        assert_eq!(envelope["schema_version"], ENVELOPE, "{kind} {id}");
        assert_eq!(envelope["record_kind"], kind, "{kind} {id}");
        originals.push((kind, id.clone(), content_of(kind, &envelope)));
    }

    let (status, steps) = revoke_and_clean(&env.store, revoked_run, 2).await;
    assert_eq!(status.state, CleanupState::Complete);
    // The walk took several steps because the edge page is small.
    assert!(steps > 1, "steps: {steps}");

    // The world and every node, dispatch fact and history entry is redacted,
    // one object at a time, and keeps none of its original content.
    assert_eq!(originals.len(), 1 + 2 + 2 + 2);
    for (kind, id, content) in &originals {
        assert_redacted(&env.store, kind, id, content).await;
    }

    // The revoked run's own content is gone; the other source was not revoked.
    assert!(run_object(&env.store, revoked_run).await.is_none());
    let other = RUNS.iter().find(|run| **run != revoked_run).unwrap();
    assert!(run_object(&env.store, other).await.is_some());
}

#[tokio::test]
async fn revoking_the_failure_run_redacts_the_world_and_every_record_it_owns() {
    revoked_run_redacts_the_whole_world("run-failure").await;
}

#[tokio::test]
async fn revoking_the_success_run_redacts_the_world_and_every_record_it_owns() {
    // Any source of the closure blocks all dependents, not only the first one.
    revoked_run_redacts_the_whole_world("run-success").await;
}

// ---------------------------------------------------------------------------
// 2. Isolation: a run that belongs to no world leaves the world's records alone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_an_unrelated_run_leaves_every_exploration_record_as_it_was() {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;

    // A positive control that shares nothing with the chain but the store: a
    // world whose only source is the unrelated run, with one history entry.
    env.register("world-other", &[UNRELATED_RUN]).await;
    env.coordinator
        .record_history("world-other", history("world-other-history-1", 1))
        .await
        .unwrap();
    let other_world = raw_record(&env.store, WORLD, "world-other").await;
    let other_history = raw_record(&env.store, HISTORY, "world-other-history-1").await;
    let other_content = [
        content_of(WORLD, &other_world),
        content_of(HISTORY, &other_history),
    ];

    let mut before = Vec::new();
    for (kind, id) in chain.records() {
        before.push((kind, id.clone(), raw_record(&env.store, kind, &id).await));
    }
    let edges_before = edges_into(&env.store, &edge_universe(&chain)).await;

    let (status, _) = revoke_and_clean(&env.store, UNRELATED_RUN, 2).await;
    assert_eq!(status.state, CleanupState::Complete);

    // The cleanup ran and did reach what depends on the unrelated run ...
    assert!(run_object(&env.store, UNRELATED_RUN).await.is_none());
    assert_redacted(&env.store, WORLD, "world-other", &other_content[0]).await;
    assert_redacted(
        &env.store,
        HISTORY,
        "world-other-history-1",
        &other_content[1],
    )
    .await;

    // ... and every record of the other world is byte for byte what it was.
    // (Any revocation moves the namespace watermark, so the world can no longer
    // be used; that is the logical block, not a change to its records.)
    for (kind, id, envelope) in &before {
        assert_intact(&env.store, kind, id, envelope).await;
    }
    assert_eq!(
        edges_into(&env.store, &edge_universe(&chain)).await,
        edges_before
    );
    assert!(run_object(&env.store, "run-failure").await.is_some());
    assert!(run_object(&env.store, "run-success").await.is_some());
}

// ---------------------------------------------------------------------------
// 3. Fail closed after the revocation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_revoked_world_fails_closed_the_moment_its_source_is_revoked() {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    // Sanity: the same calls succeed before the revocation, so the errors below
    // are the revocation's and not the fixture's.
    assert!(env.coordinator.decide_next("world-chain").await.is_ok());
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-chain")
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        verified_world_decision_view(&admin(), &env.store, "world-chain")
            .await
            .is_ok()
    );

    // The logical block comes first (plan §11.5): the moment the source is
    // revoked, before one cleanup step ran, nothing reads, derives or
    // dispatches, and no model is reached.
    revoke(&env.store, "run-failure").await;
    let blocked = probe(
        &env,
        "world-chain",
        (3, "after-revoke"),
        Some((1, "world-chain-1")),
    )
    .await;
    assert_eq!(blocked.results.len(), 5);
    for (name, error) in blocked.errors() {
        assert!(
            matches!(
                error,
                Error::NotFound | Error::Forbidden | Error::Conflict(_)
            ),
            "{name}: {error:?}"
        );
    }
    // The records are still there to be cleaned: the block does not depend on
    // the cleanup having run.
    for (kind, id) in chain.records() {
        let value = raw_record(&env.store, kind, &id).await;
        assert_eq!(value["schema_version"], ENVELOPE, "{kind} {id}");
    }
}

#[tokio::test]
async fn a_redacted_world_stays_closed_after_the_cleanup() {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    revoke_and_clean(&env.store, "run-failure", 4).await;
    for (kind, id) in chain.records() {
        let value = raw_record(&env.store, kind, &id).await;
        assert_eq!(value["schema_version"], "rsia.redacted.v1", "{kind} {id}");
    }

    let cleaned = probe(
        &env,
        "world-chain",
        (3, "after-cleanup"),
        Some((2, "world-chain-2")),
    )
    .await;
    assert_eq!(cleaned.results.len(), 5);
    for (name, error) in cleaned.errors() {
        // Known gap, unchanged here: a redacted envelope does not decode, so
        // the coordinator's record reads report it as `Internal` rather than
        // naming the revocation (E03's development artifacts say `Conflict`).
        // Every entry point is closed either way, and none reached a model.
        assert!(
            matches!(
                error,
                Error::NotFound | Error::Forbidden | Error::Conflict(_) | Error::Internal
            ),
            "{name}: {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Idempotency: a reconnect adds no edge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconnecting_a_dispatch_adds_no_edge_and_no_duplicate() {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    let universe = edge_universe(&chain);
    let before = edges_into(&env.store, &universe).await;
    // One edge per node, dispatch fact and history entry points at the world.
    let to_world = before
        .iter()
        .filter(|edge| edge.3 == chain.world_storage_id())
        .count();
    assert_eq!(to_world, 6);
    let world_before = raw_record(&env.store, WORLD, "world-chain").await;

    // The same request again, without any port: the stored terminal dispatch
    // answers it and nothing is written.
    for _ in 0..2 {
        let again = env
            .coordinator
            .run_next(
                None,
                None,
                None,
                env.fixture.request("world-chain", 1, "world-chain-1"),
            )
            .await
            .unwrap();
        assert_eq!(again.dispatch_id, chain.first.dispatch_id);
        assert_eq!(again.node_id, chain.first.node_id);
    }
    // With every port present, the terminal dispatch still answers it and no
    // model is called.
    let counting = CountingModel::default();
    let again = env
        .coordinator
        .run_next(
            Some(&counting),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-chain", 2, "world-chain-2"),
        )
        .await
        .unwrap();
    assert_eq!(again.dispatch_id, chain.second.dispatch_id);
    assert_eq!(again.node_id, chain.second.node_id);
    assert_eq!(counting.0.load(Ordering::SeqCst), 0);

    let after = edges_into(&env.store, &universe).await;
    assert_eq!(after.len(), before.len(), "the number of edges changed");
    assert_eq!(after, before);
    assert_eq!(
        raw_record(&env.store, WORLD, "world-chain").await,
        world_before
    );
}

#[tokio::test]
async fn resuming_a_claimed_dispatch_adds_exactly_its_node_edge() {
    let env = env().await;
    env.register("world-resume", &RUNS).await;
    env.claim_and_abandon("world-resume", 1, "resume-1").await;

    // The claim is one dispatch fact with its edge and no node yet.
    let world = raw_record(&env.store, WORLD, "world-resume").await;
    let dispatches = id_list(&world, "dispatch_ids");
    assert_eq!(dispatches.len(), 1);
    assert!(id_list(&world, "node_ids").is_empty());
    let world_sid = storage_id(WORLD, "world-resume");
    let dispatch_edge: Edge = (
        "artifact".into(),
        storage_id(DISPATCH, &dispatches[0]),
        "artifact".into(),
        world_sid.clone(),
    );
    let claimed = edges_into(&env.store, &[("artifact", world_sid.clone())]).await;
    assert_eq!(claimed, BTreeSet::from([dispatch_edge.clone()]));

    // Resuming the same request finds the abandoned model call without a
    // durable response, records the dispatch as uncertain and derives the node:
    // the node's edge is the only addition, and the claim's edge is not written
    // again.
    let resumed = env
        .coordinator
        .run_next(
            Some(&EditingFixtureModel),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-resume", 1, "resume-1"),
        )
        .await
        .unwrap();
    let node_id = resumed
        .node_id
        .clone()
        .expect("the resumed dispatch derives a node");
    assert_eq!(resumed.dispatch_id.as_deref(), Some(dispatches[0].as_str()));
    let node_edge: Edge = (
        "artifact".into(),
        storage_id(NODE, &node_id),
        "artifact".into(),
        world_sid.clone(),
    );
    let observed = edges_into(&env.store, &[("artifact", world_sid.clone())]).await;
    assert_eq!(observed, BTreeSet::from([dispatch_edge, node_edge]));

    // Reconnecting after that changes nothing.
    env.coordinator
        .run_next(
            None,
            None,
            None,
            env.fixture.request("world-resume", 1, "resume-1"),
        )
        .await
        .unwrap();
    assert_eq!(
        edges_into(&env.store, &[("artifact", world_sid)]).await,
        observed
    );
}

// ---------------------------------------------------------------------------
// 5. The shape of the edges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_record_has_exactly_one_edge_and_it_points_at_its_world() {
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    let world_sid = chain.world_storage_id();

    // The record -> world edges, one per node, dispatch fact and history entry.
    let into_world = edges_into(&env.store, &[("artifact", world_sid.clone())]).await;
    let expected: BTreeSet<Edge> = chain
        .dependents()
        .into_iter()
        .map(|(kind, id)| {
            (
                "artifact".to_owned(),
                storage_id(kind, &id),
                "artifact".to_owned(),
                world_sid.clone(),
            )
        })
        .collect();
    assert_eq!(expected.len(), 6);
    assert_eq!(into_world, expected);

    // Nothing else leaves a record among the objects an edge can name: no
    // record -> run, no record -> record, no edge into a record.
    let record_ids: BTreeSet<String> = chain
        .dependents()
        .into_iter()
        .map(|(kind, id)| storage_id(kind, &id))
        .collect();
    let leaving_records: BTreeSet<Edge> = edges_into(&env.store, &edge_universe(&chain))
        .await
        .into_iter()
        .filter(|edge| record_ids.contains(&edge.1) || record_ids.contains(&edge.3))
        .collect();
    assert_eq!(leaving_records, expected);

    // The world keeps its own edges to exactly the runs of its closure.
    let runs: BTreeSet<Edge> = edges_into(
        &env.store,
        &RUNS
            .iter()
            .map(|run| ("run", (*run).to_owned()))
            .collect::<Vec<_>>(),
    )
    .await
    .into_iter()
    .filter(|edge| edge.1 == world_sid)
    .collect();
    assert_eq!(
        runs,
        RUNS.iter()
            .map(|run| (
                "artifact".to_owned(),
                world_sid.clone(),
                "run".to_owned(),
                (*run).to_owned()
            ))
            .collect::<BTreeSet<_>>()
    );

    // The direction is the cleanup's: its step from a node lists the objects
    // that depend on it, so a revoked run reaches the world and, through it,
    // every record. (Tests 1 to 3 show the same walk doing the actual cleanup.)
    for run in RUNS {
        let reached = closure_from(&env.store, ("run", run)).await;
        for (kind, id) in chain.records() {
            assert!(
                reached.contains(&("artifact".to_owned(), storage_id(kind, &id))),
                "{run} does not reach {kind} {id}"
            );
        }
    }
    // A record reaches nothing: it is a leaf of the closure.
    for (kind, id) in chain.dependents() {
        assert!(
            closure_from(&env.store, ("artifact", &storage_id(kind, &id)))
                .await
                .is_empty()
        );
    }
    // The unrelated run reaches none of them.
    let unrelated = closure_from(&env.store, ("run", UNRELATED_RUN)).await;
    for (kind, id) in chain.records() {
        assert!(!unrelated.contains(&("artifact".to_owned(), storage_id(kind, &id))));
    }
}

#[tokio::test]
async fn a_history_entry_belongs_to_the_world_it_was_recorded_into() {
    // The entry carries no run id and no world id: its only owner is the world
    // that lists it, so that is the edge it gets, per world.
    let env = env().await;
    env.register("world-a", &RUNS).await;
    env.register("world-b", &[UNRELATED_RUN]).await;
    env.coordinator
        .record_history("world-a", history("entry-a", 1))
        .await
        .unwrap();
    env.coordinator
        .record_history("world-b", history("entry-b", 1))
        .await
        .unwrap();
    // The entry id is unique in the namespace: it cannot be recorded twice, so
    // it cannot end up owned by two worlds.
    assert!(matches!(
        env.coordinator
            .record_history("world-b", history("entry-a", 2))
            .await,
        Err(Error::Conflict(_))
    ));
    let a = storage_id(WORLD, "world-a");
    let b = storage_id(WORLD, "world-b");
    let entry_a = storage_id(HISTORY, "entry-a");
    let entry_b = storage_id(HISTORY, "entry-b");
    let into_a = edges_into(&env.store, &[("artifact", a.clone())]).await;
    let into_b = edges_into(&env.store, &[("artifact", b.clone())]).await;
    assert_eq!(
        into_a,
        BTreeSet::from([("artifact".into(), entry_a, "artifact".into(), a)])
    );
    assert_eq!(
        into_b,
        BTreeSet::from([("artifact".into(), entry_b, "artifact".into(), b)])
    );
}

// ---------------------------------------------------------------------------
// Further boundaries of the closure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_claimed_dispatch_fact_is_redacted_because_its_edge_came_with_its_first_write() {
    let env = env().await;
    env.register("world-claimed", &RUNS).await;
    env.claim_and_abandon("world-claimed", 1, "claimed-1").await;
    let world = raw_record(&env.store, WORLD, "world-claimed").await;
    let dispatches = id_list(&world, "dispatch_ids");
    assert_eq!(dispatches.len(), 1);
    let claimed = raw_record(&env.store, DISPATCH, &dispatches[0]).await;
    assert_eq!(claimed["payload"]["state"], "claimed");
    assert!(claimed["payload"]["node_id"].is_null());
    let content = content_of(DISPATCH, &claimed);

    // The fact is still only claimed (it was never rewritten), yet the cleanup
    // reaches it.
    revoke_and_clean(&env.store, "run-success", 2).await;
    assert_redacted(&env.store, DISPATCH, &dispatches[0], &content).await;
    assert_redacted(
        &env.store,
        WORLD,
        "world-claimed",
        &content_of(WORLD, &world),
    )
    .await;

    // Resuming the abandoned dispatch is refused: the claim does not outlive
    // the revocation.
    let counting = CountingModel::default();
    let resumed = env
        .coordinator
        .run_next(
            Some(&counting),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-claimed", 1, "claimed-1"),
        )
        .await;
    assert!(resumed.is_err());
    assert_eq!(counting.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_interrupted_cleanup_resumes_through_the_record_edges_after_a_restart() {
    // E08: the cleanup survives a crash. Stop it as soon as the world itself is
    // redacted, while its records are still queued behind the walk, then reopen
    // the store and let a fresh process finish the job.
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    let mut content = Vec::new();
    for (kind, id) in chain.records() {
        let envelope = raw_record(&env.store, kind, &id).await;
        content.push((kind, id, content_of(kind, &envelope)));
    }

    let mut status = revoke(&env.store, "run-failure").await;
    let admin = admin();
    let mut now = 201;
    loop {
        let world = raw_record(&env.store, WORLD, "world-chain").await;
        if world["schema_version"] == "rsia.redacted.v1" {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(&admin, &env.store, &status.job_id, 1, now)
            .await
            .unwrap();
        now += 1;
        assert_eq!(status.state, CleanupState::Running, "{status:?}");
    }
    let mut still_intact = 0;
    for (kind, id) in chain.dependents() {
        let value = raw_record(&env.store, kind, &id).await;
        if value["schema_version"] == ENVELOPE {
            still_intact += 1;
        }
    }
    assert!(
        still_intact > 0,
        "the walk was expected to be interrupted before the records were reached"
    );

    // The crash: the process is gone, only the file is left.
    env.store.close().await;
    let reopened = Store::open(&env.dir.path().join("exploration-cleanup.sqlite3"))
        .await
        .unwrap();
    let (status, _) = finish_cleanup(&reopened, status, 1).await;
    assert_eq!(status.state, CleanupState::Complete);
    for (kind, id, content) in &content {
        assert_redacted(&reopened, kind, id, content).await;
    }
}

#[tokio::test]
async fn a_result_that_arrives_after_the_revoke_is_uncertain_and_is_still_cleaned() {
    // Plan §7.2.1/§11.5: a late result must not revive revoked content, and its
    // cost is still reconciled. The source is revoked while the model call is in
    // flight; the dispatch was claimed before, so the coordinator records it
    // (uncertain, without a candidate) instead of losing it. That node is first
    // written under the logical block, and it still gets its edge.
    let env = env().await;
    env.register("world-late", &RUNS).await;
    let model = RevokingModel {
        store: env.store.clone(),
        revoked: AtomicBool::new(false),
    };
    let late = env
        .coordinator
        .run_next(
            Some(&model),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-late", 1, "late-1"),
        )
        .await
        .unwrap();
    assert!(model.revoked.load(Ordering::SeqCst));
    let node_id = late.node_id.clone().expect("the late result is recorded");
    let dispatch_id = late.dispatch_id.clone().expect("the claimed dispatch");

    let node = raw_record(&env.store, NODE, &node_id).await;
    assert_eq!(
        node["payload"]["node"]["status"]["status"],
        "usage_uncertain"
    );
    for field in [
        "candidate_bundle_digest",
        "candidate_skill_digest",
        "development_selection_digest",
    ] {
        assert!(node["payload"][field].is_null(), "{field}: {node}");
    }
    let dispatch = raw_record(&env.store, DISPATCH, &dispatch_id).await;
    assert_eq!(dispatch["payload"]["state"], "uncertain");

    let world_sid = storage_id(WORLD, "world-late");
    assert_eq!(
        edges_into(&env.store, &[("artifact", world_sid.clone())]).await,
        BTreeSet::from([
            (
                "artifact".to_owned(),
                storage_id(DISPATCH, &dispatch_id),
                "artifact".to_owned(),
                world_sid.clone()
            ),
            (
                "artifact".to_owned(),
                storage_id(NODE, &node_id),
                "artifact".to_owned(),
                world_sid
            ),
        ])
    );

    // Revoking again returns the job the model port started; the cleanup
    // reaches the world and both records.
    let world = raw_record(&env.store, WORLD, "world-late").await;
    let started = revoke(&env.store, "run-failure").await;
    finish_cleanup(&env.store, started, 2).await;
    assert_redacted(&env.store, NODE, &node_id, &content_of_uncertain(&node)).await;
    assert_redacted(
        &env.store,
        DISPATCH,
        &dispatch_id,
        &content_of(DISPATCH, &dispatch),
    )
    .await;
    assert_redacted(&env.store, WORLD, "world-late", &content_of(WORLD, &world)).await;
}

#[tokio::test]
async fn a_record_without_the_edge_is_not_reached_and_nothing_back_fills_it() {
    // Boundary, not a goal: a node stored the way the code wrote it before the
    // edges existed has no edge, so the cleanup cannot reach it. No migration
    // or sweep adds the edge later; exploration `run_next` and `record_history`
    // have no production caller yet, so there is nothing to back-fill.
    let env = env().await;
    let chain = chain(&env, "world-chain").await;
    let mut legacy = raw_record(&env.store, NODE, &chain.nodes[0]).await;
    legacy["id"] = Value::String(storage_id(NODE, "node-legacy"));
    put_raw_record(&env.store, NODE, "node-legacy", &legacy).await;
    assert!(
        edges_into(&env.store, &[("artifact", chain.world_storage_id())])
            .await
            .iter()
            .all(|edge| edge.1 != storage_id(NODE, "node-legacy"))
    );

    revoke_and_clean(&env.store, "run-failure", 2).await;

    // Everything with an edge is redacted; the edgeless copy is exactly what it was.
    for (kind, id) in chain.records() {
        let value = raw_record(&env.store, kind, &id).await;
        assert_eq!(value["schema_version"], "rsia.redacted.v1", "{kind} {id}");
    }
    assert_intact(&env.store, NODE, "node-legacy", &legacy).await;
}
