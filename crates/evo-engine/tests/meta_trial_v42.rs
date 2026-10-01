//! AG-034 (E14.2a; plan §9, §5.7 MetaTrial row, E14; V033, V034, V035, V094.c):
//! one S0 template is forked by trusted code into two independent exploration
//! streams, the old improver I0 and a candidate, and the stored trial is
//! re-verified on every read.
//!
//! What this file shows: the two worlds differ only in their id and their
//! policy and nothing a caller says can make them differ otherwise; each
//! stream's stage facts, dispatch facts, nodes, history, model cache keys and
//! budget dispatch group are disjoint from the other's; a request id reused
//! across streams is refused by the budget layer; each stream's real spend is
//! read back as it is under the trial's billing scope and may differ, and a
//! stream billed under another scope makes the trial unverifiable (a
//! `Conflict`, never a silent zero); the fork replays to the same trial, also
//! after a crash in the middle of it and after a dispatch; a revoked source
//! blocks the trial and its cleanup redacts the record; the fork enqueues no
//! management job; and a trial without a verified usage record is only a
//! candidate change.
//!
//! Not covered (and not claimed): that a successor is better or that I1 beats
//! I0 (there is no FormalEvaluation or statistic), the same evidence step by
//! step, a hard budget per stream (the streams spend one shared root budget,
//! which `one_stream_can_exhaust_the_shared_root_and_starve_the_other` and
//! `an_uncertain_call_of_one_stream_holds_the_roots_concurrency_slot_against_the_other`
//! pin), an isolated workspace, an Improver approval registry, the next-job
//! binding and `meta.start`.
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::improver::{ImproverContentV2, ImproverMechanismV2};
use evo_core::optimization::{
    EditSuggestion, ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{
    ElasticPolicyV1, ExplorationCapsV1, HistoryOutcome, HistoryQuery, OptimizationHistoryEntry,
    SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, MechanismUsageState,
    PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::meta::{
    META_TRIAL_ID_MAX_BYTES, MetaStream, MetaTrialCoordinator, MetaTrialForkRequest,
    MetaTrialStatus, MetaTrialV1, MetaTrialView, stream_request_id, stream_world_id,
    verified_meta_trial,
};
use evo_engine::model::{ModelExecutionProvenance, ModelPort, ModelResponse};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationStepRequest,
    PairedTaskResult, StoreOptimizationJournal,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallRecord, BudgetCallReservation, BudgetCallState, BudgetStage, RootBudgetAuthorization,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

const TENANT: &str = "n";
const SCOPE: &str = "scope-1";
const ROOT: &str = "root-1";
const TRIAL: &str = "trial-1";

/// What one model call of a stream is charged, by stream. The two differ on
/// purpose: the streams are not made to spend the same.
const OLD_CALL_COST: i64 = 7;
const NEW_CALL_COST: i64 = 13;

const WORLD: &str = "exploration_world_v1";
const NODE: &str = "exploration_node_v1";
const DISPATCH: &str = "exploration_dispatch_v1";
const HISTORY: &str = "optimization_history_v1";
const TRIAL_KIND: &str = "meta_trial_v1";
const ENVELOPE: &str = "rsia.exploration_artifact_envelope.v1";
const STAGE_FACT_SCHEMA: &str = "rsia.optimization.stage_fact.v1";

/// The two trusted runs the S0 source closure consists of.
const RUNS: [&str; 2] = ["run-failure", "run-success"];
/// A trusted run that belongs to no trial.
const UNRELATED_RUN: &str = "run-unrelated";

// ---------------------------------------------------------------------------
// Fixtures: the trusted source closure and the request shape of
// `meta_inheritance_v42` and `exploration_cleanup_v42`, with a real
// `PersistentModelBroker` in front of a fixture transport so that every model
// call is a call row of the ledger.
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

fn policy_focus_one() -> ElasticPolicyV1 {
    ElasticPolicyV1 {
        max_focus_actions: 1,
        ..ElasticPolicyV1::default()
    }
}

/// The built-in improver I0.
fn i0() -> ImproverContentV2 {
    ImproverContentV2::builtin_default()
}

/// The candidate improver I1: one focused action per branch, so its second
/// dispatch switches to the other root while I0's would deepen the first node.
fn i1() -> ImproverContentV2 {
    let mut content = ImproverContentV2::builtin_default();
    let ImproverMechanismV2::ExplorationPolicy { policy } = &mut content.mechanism;
    *policy = policy_focus_one();
    content
}

/// An S0 template: the world both streams start from. Its policy is I0's.
fn template() -> ExplorationWorldV1 {
    world_for("s0-template", ElasticPolicyV1::default())
}

fn world_for(id: &str, policy: ElasticPolicyV1) -> ExplorationWorldV1 {
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
        policy,
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

fn fork_request(trial_id: &str) -> MetaTrialForkRequest {
    MetaTrialForkRequest {
        trial_id: trial_id.into(),
        s0_template: template(),
        new_content: i1(),
        billing_scope: SCOPE.into(),
    }
}

/// Seeds three Host-issued trace authorities (the two runs of the closure and
/// one unrelated run), a source selection grant for the closure and one
/// watermark bump, so every world freezes `source_watermark == 1`.
async fn seeded_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("meta-trial.sqlite3"))
        .await
        .unwrap();
    let host = Context::new(TENANT, "host", Role::Host).unwrap();
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

/// The suggestion the fixture model of the exploration tests returns for a
/// reflection request.
fn fixture_output(request: &ModelRequest) -> Result<String> {
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
    serde_json::to_string(&vec![suggestion]).map_err(|_| Error::Internal)
}

/// The fixture transport behind the real broker: it answers like the fixture
/// model of the exploration tests and charges `OLD_CALL_COST` to a call of the
/// old stream and `NEW_CALL_COST` to one of the new stream (the stream is the
/// request's episode: the world id).
struct StreamTransport {
    executions: Arc<AtomicUsize>,
    /// While set, a call of the old stream fails after it was dispatched, so the
    /// ledger records it as uncertain.
    fail_old_stream: Arc<AtomicBool>,
}

#[async_trait]
impl ModelTransport for StreamTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        if self.fail_old_stream.load(Ordering::SeqCst) && request.episode_id.ends_with("-old") {
            return Err(Error::Internal);
        }
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            output: fixture_output(request)?,
            actual_cost_micros: if request.episode_id.ends_with("-old") {
                OLD_CALL_COST
            } else {
                NEW_CALL_COST
            },
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
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
            TENANT,
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
    /// `Widen` action. `request_id` is the optimizer request id of the step.
    fn request(&self, world_id: &str, step: u32, request_id: &str) -> OptimizationStepRequest<'_> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.bindings,
            traces: vec![
                trace("run-failure", "family-a", TraceOutcome::TaskFailure),
                trace("run-success", "family-b", TraceOutcome::Success),
            ],
            model_context: ModelRequestContext {
                request_id: request_id.into(),
                namespace: TENANT.into(),
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
                namespace: TENANT.into(),
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
                request_id: format!("dev-{request_id}"),
                namespace: TENANT.into(),
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
                candidate_bundle_digest: hash(format!("candidate-{request_id}").as_bytes()),
                environment_digest: hash(b"environment"),
                grader_digest: hash(b"grader"),
                rules_digest: hash(b"rules"),
                tools_digest: hash(b"tools"),
                revoke_watermark: 1,
                idempotency_key: format!("dev-idempotency-{request_id}"),
            },
            allow_rank_call: false,
        }
    }
}

fn worker() -> Context {
    Context::new(TENANT, "worker", Role::Worker).unwrap()
}

fn admin() -> Context {
    Context::new(TENANT, "admin", Role::Admin).unwrap()
}

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    broker: PersistentModelBroker<StreamTransport>,
    /// How often the transport behind the broker really ran: a model call.
    executions: Arc<AtomicUsize>,
    fail_old_stream: Arc<AtomicBool>,
    fixture: Fixture,
}

async fn authorize_root(store: &Store, total_limit_micros: i64) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: ROOT.into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec![TENANT.into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"admin-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

async fn env() -> Env {
    env_with_root(10_000).await
}

/// A broker over the ledger of `store` whose calls are billed to `billing_scope`.
fn broker_for_scope(
    store: &Store,
    executions: &Arc<AtomicUsize>,
    fail_old_stream: &Arc<AtomicBool>,
    billing_scope: &str,
) -> PersistentModelBroker<StreamTransport> {
    PersistentModelBroker::with_clock(
        store.clone(),
        StreamTransport {
            executions: executions.clone(),
            fail_old_stream: fail_old_stream.clone(),
        },
        BrokerConfig {
            billing_scope: billing_scope.into(),
            actor: "trusted-broker".into(),
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(|| 10),
    )
    .unwrap()
}

async fn env_with_root(total_limit_micros: i64) -> Env {
    let (dir, store) = seeded_store().await;
    authorize_root(&store, total_limit_micros).await;
    let executions = Arc::new(AtomicUsize::new(0));
    let fail_old_stream = Arc::new(AtomicBool::new(false));
    let broker = broker_for_scope(&store, &executions, &fail_old_stream, SCOPE);
    let worker = worker();
    Env {
        coordinator: PersistentCoordinator::new(store.clone(), worker.clone(), "worker").unwrap(),
        journal: StoreOptimizationJournal::new(store.clone(), worker, "worker").unwrap(),
        broker,
        executions,
        fail_old_stream,
        fixture: Fixture::new(),
        store,
        _dir: dir,
    }
}

impl Env {
    async fn fork(&self, trial_id: &str) -> MetaTrialV1 {
        MetaTrialCoordinator::fork(&worker(), &self.store, fork_request(trial_id))
            .await
            .unwrap()
    }

    async fn view(&self, trial_id: &str) -> Result<MetaTrialView> {
        verified_meta_trial(&worker(), &self.store, trial_id).await
    }

    /// One real step of `stream`, with the request id the trial derives for it.
    async fn step(&self, trial_id: &str, stream: MetaStream, step: u32) -> CoordinatorStepResult {
        self.step_with(
            trial_id,
            stream,
            step,
            &stream_request_id(trial_id, stream, step),
        )
        .await
    }

    /// One step of `stream` under an explicit optimizer request id.
    async fn step_with(
        &self,
        trial_id: &str,
        stream: MetaStream,
        step: u32,
        request_id: &str,
    ) -> CoordinatorStepResult {
        self.try_step(trial_id, stream, step, request_id)
            .await
            .unwrap()
    }

    async fn try_step(
        &self,
        trial_id: &str,
        stream: MetaStream,
        step: u32,
        request_id: &str,
    ) -> Result<CoordinatorStepResult> {
        self.try_step_via(&self.broker, trial_id, stream, step, request_id)
            .await
    }

    /// A broker over the same ledger and the same transport behaviour that bills
    /// the calls it makes to another root budget.
    fn broker_for(&self, billing_scope: &str) -> PersistentModelBroker<StreamTransport> {
        broker_for_scope(
            &self.store,
            &self.executions,
            &self.fail_old_stream,
            billing_scope,
        )
    }

    /// One real step of `stream` through `port`, with the request id the trial
    /// derives for it.
    async fn step_via(
        &self,
        port: &dyn ModelPort,
        trial_id: &str,
        stream: MetaStream,
        step: u32,
    ) -> CoordinatorStepResult {
        self.try_step_via(
            port,
            trial_id,
            stream,
            step,
            &stream_request_id(trial_id, stream, step),
        )
        .await
        .unwrap()
    }

    async fn try_step_via(
        &self,
        port: &dyn ModelPort,
        trial_id: &str,
        stream: MetaStream,
        step: u32,
        request_id: &str,
    ) -> Result<CoordinatorStepResult> {
        let world_id = stream_world_id(trial_id, stream);
        self.coordinator
            .run_next(
                Some(port),
                Some(&ImprovingFixtureRunner),
                Some(&self.journal),
                self.fixture.request(&world_id, step, request_id),
            )
            .await
    }

    async fn calls(&self, world_id: &str) -> Vec<BudgetCallRecord> {
        self.calls_in(SCOPE, world_id).await
    }

    async fn calls_in(&self, billing_scope: &str, world_id: &str) -> Vec<BudgetCallRecord> {
        let mut session = self.store.session().await.unwrap();
        let calls = session
            .budget_calls_for_group(&worker(), billing_scope, world_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        calls
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
// Raw store access, to look at and to tamper with persisted facts the way a
// corrupt store or a hostile writer could.
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

async fn raw_exists(store: &Store, record_kind: &str, id: &str) -> bool {
    let mut session = store.session().await.unwrap();
    let found = session
        .get::<Value>(&worker(), "artifact", &storage_id(record_kind, id))
        .await
        .unwrap()
        .is_some();
    session.commit().await.unwrap();
    found
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

async fn delete_raw_record(store: &Store, record_kind: &str, id: &str) {
    let mut session = store.session().await.unwrap();
    session
        .delete(&worker(), "artifact", &storage_id(record_kind, id))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn world_of(store: &Store, world_id: &str) -> ExplorationWorldV1 {
    serde_json::from_value(raw_record(store, WORLD, world_id).await["payload"].clone()).unwrap()
}

/// Every object of kind `artifact` whose body has the given schema.
async fn artifacts_with_schema(store: &Store, schema: &str) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let all: Vec<Value> = session.list(&worker(), "artifact").await.unwrap();
    session.commit().await.unwrap();
    all.into_iter()
        .filter(|body| body["schema_version"] == schema)
        .collect()
}

async fn object_count(store: &Store, kind: &str) -> u64 {
    let mut session = store.session().await.unwrap();
    let count = session
        .namespace_object_count(&worker(), kind)
        .await
        .unwrap();
    session.commit().await.unwrap();
    count
}

async fn dependents(store: &Store, dst_kind: &str, dst_id: &str) -> BTreeSet<(String, String)> {
    let mut session = store.session().await.unwrap();
    let found = session
        .dependents(&worker(), dst_kind, dst_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    found.into_iter().collect()
}

fn without(value: &Value, fields: &[&str]) -> Value {
    let mut copy = value.clone();
    for field in fields {
        copy.as_object_mut().unwrap().remove(*field);
    }
    copy
}

fn id_list(world: &Value, field: &str) -> Vec<String> {
    world["payload"][field]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

fn digest_of_policy(policy: &ElasticPolicyV1) -> String {
    policy.digest().unwrap()
}

// ---------------------------------------------------------------------------
// 1. The fork: two worlds that differ only in id and policy (V033, V094.c)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fork_makes_two_worlds_that_differ_only_in_id_and_policy() {
    let env = env().await;
    let trial = env.fork(TRIAL).await;
    assert_eq!(trial.trial_id, TRIAL);
    assert_eq!(trial.old_world_id, "trial-1-old");
    assert_eq!(trial.new_world_id, "trial-1-new");
    assert_eq!(trial.old_world_id, stream_world_id(TRIAL, MetaStream::Old));
    assert_eq!(trial.new_world_id, stream_world_id(TRIAL, MetaStream::New));

    let old_raw = raw_record(&env.store, WORLD, &trial.old_world_id).await;
    let new_raw = raw_record(&env.store, WORLD, &trial.new_world_id).await;
    let old: ExplorationWorldV1 = serde_json::from_value(old_raw["payload"].clone()).unwrap();
    let new: ExplorationWorldV1 = serde_json::from_value(new_raw["payload"].clone()).unwrap();
    let template = template();

    // Only the id and the policy differ; every other field is the template's.
    assert_eq!(old.id, "trial-1-old");
    assert_eq!(new.id, "trial-1-new");
    assert_ne!(
        serde_json::to_value(&old.policy).unwrap(),
        serde_json::to_value(&new.policy).unwrap()
    );
    let expected = without(&serde_json::to_value(&template).unwrap(), &["id", "policy"]);
    assert_eq!(
        without(&old_raw["payload"], &["id", "policy"]),
        expected,
        "the old world is the S0 template but for id and policy"
    );
    assert_eq!(
        without(&new_raw["payload"], &["id", "policy"]),
        expected,
        "the new world is the S0 template but for id and policy"
    );

    // The fields the plan names, one by one (V033: same start, tools, evidence
    // opportunities and declared limits).
    assert_eq!(old.context_signature, new.context_signature);
    assert_eq!(old.context_signature, template.context_signature);
    assert_eq!(old.approved_parent_digest, new.approved_parent_digest);
    assert_eq!(old.tools_digest, new.tools_digest);
    assert_eq!(
        serde_json::to_value(&old.dependencies).unwrap(),
        serde_json::to_value(&new.dependencies).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&old.caps).unwrap(),
        serde_json::to_value(&new.caps).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&old.root_opportunities).unwrap(),
        serde_json::to_value(&new.root_opportunities).unwrap()
    );
    assert_eq!(
        serde_json::to_value(old.simulation).unwrap(),
        serde_json::to_value(new.simulation).unwrap()
    );
    assert_eq!(old.successor_cost_upper_micros, 10);
    assert_eq!(new.successor_cost_upper_micros, 10);
    assert_eq!(old.remaining_root_micros, new.remaining_root_micros);
    assert_eq!(old.remaining_recovery_dispatches, 2);
    assert_eq!(new.remaining_recovery_dispatches, 2);
    // Registered, but not the same registration: the policy is part of it.
    assert_ne!(
        old.registration_fingerprint().unwrap(),
        new.registration_fingerprint().unwrap()
    );
    // Both start fresh: nothing was dispatched at fork time.
    for world in [&old, &new] {
        assert_eq!(world.state, WorldState::Collecting);
        assert!(world.node_ids.is_empty() && world.dispatch_ids.is_empty());
        assert!(world.history_ids.is_empty() && world.decision_round == 0);
    }

    // The policies are the improvers': I0's for the old stream, I1's for the new.
    assert_eq!(digest_of_policy(&old.policy), trial.old_policy_digest);
    assert_eq!(digest_of_policy(&new.policy), trial.new_policy_digest);
    assert_eq!(
        trial.old_policy_digest,
        i0().exploration_policy().digest().unwrap()
    );
    assert_eq!(
        trial.new_policy_digest,
        i1().exploration_policy().digest().unwrap()
    );
    assert_eq!(trial.old_content_digest, i0().content_digest().unwrap());
    assert_eq!(trial.new_content_digest, i1().content_digest().unwrap());
    assert_ne!(trial.old_content_digest, trial.new_content_digest);

    // The trial record: its own envelope, the declared limits both streams
    // share, the S0 source closure and the watermark.
    let record = raw_record(&env.store, TRIAL_KIND, TRIAL).await;
    assert_eq!(record["schema_version"], ENVELOPE);
    assert_eq!(record["record_kind"], TRIAL_KIND);
    assert_eq!(record["payload"]["schema_version"], "rsia.meta_trial.v1");
    assert_eq!(
        record["payload"]["s0_template_digest"],
        json!(trial.s0_template_digest)
    );
    assert_eq!(trial.s0_template_digest.len(), 64);
    assert_eq!(trial.billing_scope, SCOPE);
    assert_eq!(trial.source_closure, RUNS.to_vec());
    assert_eq!(trial.revoke_watermark, 1);
    assert_eq!(trial.declared_stream_limits.remaining_root_micros, 1_000);
    assert_eq!(trial.declared_stream_limits.successor_cost_upper_micros, 10);
    assert_eq!(
        trial.declared_stream_limits.remaining_recovery_dispatches,
        2
    );
    assert_eq!(
        serde_json::to_value(&trial.declared_stream_limits.root_opportunities).unwrap(),
        serde_json::to_value(&template.root_opportunities).unwrap()
    );
    assert!(trial.created_at > 0);

    // The read side agrees, and shows the same limits for both streams.
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(view.trial_id(), TRIAL);
    assert_eq!(view.s0_template_digest(), trial.s0_template_digest);
    assert_eq!(view.old_stream().world_id(), "trial-1-old");
    assert_eq!(view.new_stream().world_id(), "trial-1-new");
    assert_eq!(view.old_stream().policy_digest(), trial.old_policy_digest);
    assert_eq!(view.new_stream().policy_digest(), trial.new_policy_digest);
    assert_eq!(view.old_stream().content_digest(), trial.old_content_digest);
    assert_eq!(view.new_stream().content_digest(), trial.new_content_digest);
    assert_eq!(view.source_closure(), RUNS);
    assert_eq!(view.revoke_watermark(), 1);
    assert_eq!(view.declared_stream_limits().remaining_root_micros, 1_000);
    assert_eq!(
        view.stream(MetaStream::Old).world_id(),
        view.old_stream().world_id()
    );

    // The trial's dependency edges: it hangs on every source run and on both
    // worlds, which is how a revocation reaches it. The worlds keep theirs.
    let trial_sid = storage_id(TRIAL_KIND, TRIAL);
    for run in RUNS {
        let from_run = dependents(&env.store, "run", run).await;
        assert!(from_run.contains(&("artifact".to_owned(), trial_sid.clone())));
        assert!(from_run.contains(&("artifact".to_owned(), storage_id(WORLD, "trial-1-old"))));
        assert!(from_run.contains(&("artifact".to_owned(), storage_id(WORLD, "trial-1-new"))));
    }
    for world in ["trial-1-old", "trial-1-new"] {
        assert!(
            dependents(&env.store, "artifact", &storage_id(WORLD, world))
                .await
                .contains(&("artifact".to_owned(), trial_sid.clone()))
        );
    }
}

// ---------------------------------------------------------------------------
// 2. The request cannot say anything per stream; the gates (V033, V094.c)
// ---------------------------------------------------------------------------

#[test]
fn the_request_has_no_field_that_could_differ_between_the_streams() {
    let request = serde_json::to_value(fork_request(TRIAL)).unwrap();
    // Exactly the four things the plan names.
    let mut keys: Vec<&str> = request
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["billing_scope", "new_content", "s0_template", "trial_id"]
    );
    let round_trip: MetaTrialForkRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(round_trip.trial_id, TRIAL);

    // Anything else, per stream or not, is not a field of the request: it is
    // refused when the request is read, before a fork could see it.
    for (key, value) in [
        ("old_policy", json!(ElasticPolicyV1::default())),
        ("new_policy", json!(policy_focus_one())),
        ("old_world", json!(template())),
        ("new_world", json!(template())),
        ("old_world_id", json!("trial-1-old")),
        ("worlds", json!([template(), template()])),
        ("caps", json!(ExplorationCapsV1::online())),
        ("simulation", json!({"online": {"fixed_seed": 1}})),
        ("root_opportunities", json!([])),
        ("remaining_root_micros", json!(5)),
        ("successor_cost_upper_micros", json!(5)),
        ("tools_digest", json!(hash(b"tools"))),
        ("old_content", json!(i0())),
        ("budget", json!({"limit": 1})),
    ] {
        let mut injected = request.clone();
        injected[key] = value;
        assert!(
            serde_json::from_value::<MetaTrialForkRequest>(injected).is_err(),
            "{key} must not be a field of the request"
        );
    }
    // Nor inside the template world or the candidate content.
    for key in ["old_policy", "caps_override", "max_nodes"] {
        let mut injected = request.clone();
        injected["s0_template"][key] = json!(1);
        assert!(
            serde_json::from_value::<MetaTrialForkRequest>(injected).is_err(),
            "{key} inside the template"
        );
    }
    for (key, value) in [
        ("caps", json!(ExplorationCapsV1::online())),
        ("simulation", json!({"online": {"fixed_seed": 1}})),
        ("max_nodes", json!(99)),
    ] {
        let mut injected = request.clone();
        injected["new_content"]["mechanism"][key] = value.clone();
        assert!(
            serde_json::from_value::<MetaTrialForkRequest>(injected).is_err(),
            "{key} inside the mechanism of the candidate"
        );
        let mut injected = request.clone();
        injected["new_content"]["mechanism"]["policy"][key] = value;
        assert!(
            serde_json::from_value::<MetaTrialForkRequest>(injected).is_err(),
            "{key} inside the policy of the candidate"
        );
    }
}

#[test]
fn candidate_bytes_with_caps_or_a_simulation_never_become_improver_content() {
    let policy = serde_json::to_value(policy_focus_one()).unwrap();
    let parse = |value: Value| ImproverContentV2::parse(value.to_string().as_bytes());

    // The content a fork accepts parses, and only that content.
    let good = json!({
        "schema_version": "rsia.improver_content.v2",
        "mechanism": {"class": "exploration_policy", "policy": policy},
    });
    parse(good.clone()).unwrap();

    // Caps and a simulation profile smuggled in beside the policy.
    for (key, value) in [
        ("caps", json!(ExplorationCapsV1::online())),
        ("simulation", json!({"online": {"fixed_seed": 1}})),
    ] {
        let mut smuggled = good.clone();
        smuggled["mechanism"][key] = value.clone();
        assert!(
            matches!(parse(smuggled), Err(Error::Invalid(_))),
            "{key} next to the policy"
        );
        let mut inside = good.clone();
        inside["mechanism"]["policy"][key] = value.clone();
        assert!(
            matches!(parse(inside), Err(Error::Invalid(_))),
            "{key} inside the policy"
        );
        let mut at_top = good.clone();
        at_top[key] = value;
        assert!(
            matches!(parse(at_top), Err(Error::Invalid(_))),
            "{key} at the top"
        );
    }
    // The control-plane classes are refused by name.
    for class in ["caps", "exploration_caps", "simulation_profile"] {
        let mut other = good.clone();
        other["mechanism"] = json!({"class": class, "policy": policy});
        assert!(
            matches!(parse(other), Err(Error::Forbidden)),
            "mechanism class {class}"
        );
    }
}

#[tokio::test]
async fn the_fork_refuses_what_is_not_a_built_in_s0_a_real_candidate_or_an_authorized_budget() {
    let env = env().await;
    let refused_without_trace = |trial_id: &'static str| {
        let store = env.store.clone();
        async move {
            assert!(!raw_exists(&store, TRIAL_KIND, trial_id).await);
            for stream in MetaStream::BOTH {
                assert!(!raw_exists(&store, WORLD, &stream_world_id(trial_id, stream)).await);
            }
            assert!(
                dependents(&store, "run", "run-failure")
                    .await
                    .iter()
                    .all(|(_, id)| id != &storage_id(TRIAL_KIND, trial_id))
            );
        }
    };

    // The S0 template must carry I0's policy.
    let mut request = fork_request("not-i0");
    request.s0_template.policy = policy_focus_one();
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Invalid(_))
    ));
    refused_without_trace("not-i0").await;

    // The candidate must differ from I0 ...
    let mut request = fork_request("same-as-i0");
    request.new_content = i0();
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Invalid(_))
    ));
    refused_without_trace("same-as-i0").await;
    // ... and must be valid: a typed value built in code is held to the same
    // bounds as parsed bytes.
    let mut request = fork_request("out-of-bounds");
    let ImproverMechanismV2::ExplorationPolicy { policy } = &mut request.new_content.mechanism;
    policy.fairness_wait_rounds = 12;
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Invalid(_))
    ));
    refused_without_trace("out-of-bounds").await;
    let mut request = fork_request("wrong-schema");
    request.new_content.schema_version = "rsia.improver_content.v1".into();
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Invalid(_))
    ));
    refused_without_trace("wrong-schema").await;

    // S0 is a fresh start.
    let mut request = fork_request("stale-s0");
    request.s0_template.decision_round = 1;
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Invalid(_))
    ));
    refused_without_trace("stale-s0").await;

    // The trial id roots two world ids and the node ids below them.
    for bad in ["", "has space", &"a".repeat(META_TRIAL_ID_MAX_BYTES + 1)] {
        let request = fork_request(bad);
        assert!(
            matches!(
                MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
                Err(Error::Invalid(_))
            ),
            "trial id {bad:?}"
        );
    }

    // The billing scope must name a root budget of this namespace.
    let mut request = fork_request("no-budget");
    request.billing_scope = "scope-unknown".into();
    assert!(matches!(
        MetaTrialCoordinator::fork(&worker(), &env.store, request).await,
        Err(Error::Budget)
    ));
    refused_without_trace("no-budget").await;
    let other_namespace = Context::new("other", "worker", Role::Worker).unwrap();
    assert!(
        MetaTrialCoordinator::fork(&other_namespace, &env.store, fork_request("elsewhere"))
            .await
            .is_err()
    );

    // Who may fork and who may read.
    for role in [Role::Agent, Role::Host, Role::Evaluator] {
        let ctx = Context::new(TENANT, "someone", role).unwrap();
        assert!(matches!(
            MetaTrialCoordinator::fork(&ctx, &env.store, fork_request("forbidden")).await,
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            verified_meta_trial(&ctx, &env.store, TRIAL).await,
            Err(Error::Forbidden)
        ));
    }
    refused_without_trace("forbidden").await;

    // Nothing above left anything behind, and the same calls work once the
    // request is a proper one: the refusals were the request's, not the fixture's.
    assert!(!raw_exists(&env.store, TRIAL_KIND, TRIAL).await);
    assert_eq!(env.fork(TRIAL).await.trial_id, TRIAL);
    // The template is only a template: its own id is never registered.
    assert!(!raw_exists(&env.store, WORLD, &template().id).await);
    // A trial of another namespace is not found, not readable.
    assert!(matches!(
        verified_meta_trial(&other_namespace, &env.store, TRIAL).await,
        Err(Error::NotFound)
    ));
    let admin_fork = MetaTrialCoordinator::fork(&admin(), &env.store, fork_request("by-admin"))
        .await
        .unwrap();
    assert_eq!(admin_fork.old_world_id, "by-admin-old");
    verified_meta_trial(&admin(), &env.store, "by-admin")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 3. Isolation: two real streams share no stored fact (V033)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_real_streams_share_no_journal_dispatch_node_history_or_cache_key() {
    let env = env().await;
    env.fork(TRIAL).await;
    let (old_id, new_id) = ("trial-1-old", "trial-1-new");
    let new_before_old_ran = raw_record(&env.store, WORLD, new_id).await;

    // The old stream (default policy) takes one step; the new stream (one
    // focused action per branch) takes two.
    let old_step = env.step(TRIAL, MetaStream::Old, 1).await;
    assert_eq!(
        raw_record(&env.store, WORLD, new_id).await,
        new_before_old_ran,
        "a step of the old stream does not touch the new stream's world"
    );
    let new_first = env.step(TRIAL, MetaStream::New, 1).await;
    let new_second = env.step(TRIAL, MetaStream::New, 2).await;
    env.coordinator
        .record_history(old_id, history("trial-1-old-history-1", 1))
        .await
        .unwrap();
    env.coordinator
        .record_history(new_id, history("trial-1-new-history-1", 1))
        .await
        .unwrap();

    let old_world = raw_record(&env.store, WORLD, old_id).await;
    let new_world = raw_record(&env.store, WORLD, new_id).await;
    let (old_nodes, new_nodes) = (
        id_list(&old_world, "node_ids"),
        id_list(&new_world, "node_ids"),
    );
    let (old_dispatches, new_dispatches) = (
        id_list(&old_world, "dispatch_ids"),
        id_list(&new_world, "dispatch_ids"),
    );
    let (old_history, new_history) = (
        id_list(&old_world, "history_ids"),
        id_list(&new_world, "history_ids"),
    );
    assert_eq!((old_nodes.len(), new_nodes.len()), (1, 2));
    assert_eq!((old_dispatches.len(), new_dispatches.len()), (1, 2));
    assert_eq!((old_history.len(), new_history.len()), (1, 1));
    let disjoint = |a: &[String], b: &[String]| {
        a.iter()
            .collect::<BTreeSet<_>>()
            .is_disjoint(&b.iter().collect())
    };
    assert!(disjoint(&old_nodes, &new_nodes), "node ids");
    assert!(disjoint(&old_dispatches, &new_dispatches), "dispatch ids");
    assert!(disjoint(&old_history, &new_history), "history ids");
    for (world_id, entries) in [(old_id, &old_history), (new_id, &new_history)] {
        for entry in entries {
            let record = raw_record(&env.store, HISTORY, entry).await;
            assert_eq!(record["payload"]["entry_id"], entry.as_str());
            // An entry hangs on the world it was recorded in, and on no other.
            for other in [old_id, new_id] {
                let hangs_on = dependents(&env.store, "artifact", &storage_id(WORLD, other))
                    .await
                    .contains(&("artifact".to_owned(), storage_id(HISTORY, entry)));
                assert_eq!(hangs_on, other == world_id, "{entry} on {other}");
            }
        }
    }

    // Every node and dispatch fact belongs to exactly its own stream's world.
    for (world_id, nodes, dispatches) in [
        (old_id, &old_nodes, &old_dispatches),
        (new_id, &new_nodes, &new_dispatches),
    ] {
        for node in nodes {
            let record = raw_record(&env.store, NODE, node).await;
            assert_eq!(record["payload"]["world_id"], world_id, "{node}");
            assert!(node.contains(world_id), "{node}");
        }
        for dispatch in dispatches {
            let record = raw_record(&env.store, DISPATCH, dispatch).await;
            assert_eq!(record["payload"]["world_id"], world_id, "{dispatch}");
            assert_eq!(
                record["payload"]["decision"]["world_id"], world_id,
                "{dispatch}"
            );
        }
    }

    // The journal: every stage fact is of one world, and no fact id or request
    // id appears in both.
    let facts = artifacts_with_schema(&env.store, STAGE_FACT_SCHEMA).await;
    let mut by_world: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for fact in &facts {
        let entry = by_world
            .entry(fact["episode_id"].as_str().unwrap().to_owned())
            .or_default();
        entry
            .0
            .insert(fact["artifact_id"].as_str().unwrap().to_owned());
        entry
            .1
            .insert(fact["request_id"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        by_world.keys().map(String::as_str).collect::<Vec<_>>(),
        [new_id, old_id]
    );
    let (new_facts, old_facts) = (&by_world[new_id], &by_world[old_id]);
    assert!(!old_facts.0.is_empty() && !new_facts.0.is_empty());
    assert!(old_facts.0.is_disjoint(&new_facts.0), "stage fact ids");
    assert!(old_facts.1.is_disjoint(&new_facts.1), "stage request ids");

    // The budget ledger: each stream's dispatch group holds only its own calls,
    // under call ids and effective-request digests (the cache keys) of its own.
    let (old_calls, new_calls) = (env.calls(old_id).await, env.calls(new_id).await);
    assert_eq!((old_calls.len(), new_calls.len()), (2, 4));
    for (world_id, calls) in [(old_id, &old_calls), (new_id, &new_calls)] {
        for call in calls {
            assert_eq!(call.dispatch_group_id, world_id);
            assert_eq!(call.state, BudgetCallState::Finalized);
            let request: ModelRequest =
                serde_json::from_str(&call.request_artifact.as_ref().unwrap().body).unwrap();
            assert_eq!(request.episode_id, world_id);
            assert_eq!(request.cache_key_digest, call.actual_input_digest);
        }
    }
    let ids = |calls: &[BudgetCallRecord]| -> BTreeSet<String> {
        calls.iter().map(|call| call.call_id.clone()).collect()
    };
    let keys = |calls: &[BudgetCallRecord]| -> BTreeSet<String> {
        calls
            .iter()
            .map(|call| call.actual_input_digest.clone())
            .collect()
    };
    assert!(ids(&old_calls).is_disjoint(&ids(&new_calls)), "call ids");
    assert!(
        keys(&old_calls).is_disjoint(&keys(&new_calls)),
        "cache keys"
    );
    assert_eq!(ids(&old_calls).len() + ids(&new_calls).len(), 6);

    // Development history is per world, and a stream does not see the other's.
    let parent = hash(b"approved-parent");
    let environment = hash(b"env");
    let query = HistoryQuery {
        parent_digest: &parent,
        environment_digest: &environment,
        task_family: "family",
        source_watermark: 1,
    };
    let seen_by_old = env
        .coordinator
        .matching_history(old_id, query.clone())
        .await
        .unwrap();
    let seen_by_new = env
        .coordinator
        .matching_history(new_id, query)
        .await
        .unwrap();
    assert_eq!(
        seen_by_old
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["trial-1-old-history-1"]
    );
    assert_eq!(
        seen_by_new
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["trial-1-new-history-1"]
    );

    // Each stream decided with its own policy, and the records cannot be swapped.
    let old_usage = env
        .coordinator
        .verified_mechanism_usage(old_id)
        .await
        .unwrap();
    let new_usage = env
        .coordinator
        .verified_mechanism_usage(new_id)
        .await
        .unwrap();
    assert_eq!((old_usage.len(), new_usage.len()), (1, 2));
    assert!(
        old_usage
            .iter()
            .all(|record| record.policy_digest() == digest_of_policy(&ElasticPolicyV1::default()))
    );
    assert!(
        new_usage
            .iter()
            .all(|record| record.policy_digest() == digest_of_policy(&policy_focus_one()))
    );
    assert_ne!(old_usage[0].dispatch_id(), new_usage[0].dispatch_id());
    assert_eq!(
        old_step.dispatch_id.as_deref(),
        Some(old_usage[0].dispatch_id())
    );
    assert_eq!(
        new_first.dispatch_id.as_deref(),
        Some(new_usage[0].dispatch_id())
    );
    assert_eq!(
        new_second.dispatch_id.as_deref(),
        Some(new_usage[1].dispatch_id())
    );
    // The policies really changed what was dispatched: I1's second step
    // switched to the other root, which I0's default would have deepened.
    assert_eq!(
        new_second.decision.policy_digest,
        digest_of_policy(&policy_focus_one())
    );
    assert_ne!(
        old_step.decision.policy_digest,
        new_first.decision.policy_digest
    );
    assert_ne!(
        env.coordinator
            .decide_next(old_id)
            .await
            .unwrap()
            .policy_digest,
        env.coordinator
            .decide_next(new_id)
            .await
            .unwrap()
            .policy_digest
    );

    // The trial still verifies, and reads the two streams apart.
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(view.old_stream().verified_usage(), 1);
    assert_eq!(view.new_stream().verified_usage(), 2);
}

// ---------------------------------------------------------------------------
// 4. Request ids: each stream derives its own; a reused one is refused (V033)
// ---------------------------------------------------------------------------

#[test]
fn a_stream_request_id_is_pure_and_never_the_other_streams() {
    for trial in [
        "trial-1",
        "trial-2",
        "a-much-longer-trial-identifier-for-the-test",
    ] {
        for step in [1u32, 2, 3, 12] {
            let old = stream_request_id(trial, MetaStream::Old, step);
            let new = stream_request_id(trial, MetaStream::New, step);
            assert_ne!(old, new, "{trial} step {step}");
            assert_eq!(old, stream_request_id(trial, MetaStream::Old, step));
            assert_eq!(new, stream_request_id(trial, MetaStream::New, step));
            assert!(old.len() <= 128 && new.len() <= 128);
        }
    }
}

#[tokio::test]
async fn a_request_id_reused_across_streams_is_refused_by_the_budget_layer() {
    let env = env().await;
    env.fork(TRIAL).await;
    let (old_id, new_id) = ("trial-1-old", "trial-1-new");

    // The old stream spends under its own derived request id.
    let old_request_id = stream_request_id(TRIAL, MetaStream::Old, 1);
    env.step_with(TRIAL, MetaStream::Old, 1, &old_request_id)
        .await;
    let old_calls = env.calls(old_id).await;
    assert_eq!(old_calls.len(), 2);
    let spent_before = env.executions.load(Ordering::SeqCst);
    assert_eq!(spent_before, 2);

    // The budget layer keys a call by its id and refuses (Conflict, fail
    // closed) the same id for another effective request: here the same call id
    // but the episode of the new stream. Built from the very request the old
    // stream's call stored, with only the stream's world changed.
    let stored: ModelRequest =
        serde_json::from_str(&old_calls[0].request_artifact.as_ref().unwrap().body).unwrap();
    assert_eq!(stored.request_id, old_calls[0].call_id);
    let reused = ModelRequest::build(
        ModelRequestContext {
            request_id: stored.request_id.clone(),
            namespace: stored.namespace.clone(),
            purpose: stored.purpose,
            stage: stored.stage,
            episode_id: new_id.into(),
            step: stored.step,
            attempt: stored.attempt,
            parent_skill_digest: stored.parent_skill_digest.clone(),
            bundle_digest: stored.bundle_digest.clone(),
            source_closure: stored.source_closure.clone(),
            model_digest: stored.model_digest.clone(),
            tools_digest: stored.tools_digest.clone(),
            rules_digest: stored.rules_digest.clone(),
            sampling_digest: stored.sampling_digest.clone(),
            revoke_watermark: stored.revoke_watermark,
            max_suggestions: stored.max_suggestions,
        },
        stored.input.clone(),
    )
    .unwrap();
    assert_ne!(reused.cache_key_digest, stored.cache_key_digest);
    match env.broker.dispatch(reused).await {
        Err(Error::Conflict(message)) => {
            assert_eq!(
                message,
                "call_id reused with a different effective model request"
            );
        }
        other => panic!("expected the budget layer's Conflict, got {other:?}"),
    }
    // The same at the reservation itself, which is where the ledger keys a call
    // by its id: the old stream's call id asked for in the new stream's
    // dispatch group is refused (Conflict, fail closed), and so is a call id
    // that is asked for again with another effective request digest.
    let first = &old_calls[0];
    let reservation =
        |dispatch_group_id: &str, actual_input_digest: String| BudgetCallReservation {
            billing_scope: SCOPE.into(),
            call_id: first.call_id.clone(),
            dispatch_group_id: dispatch_group_id.into(),
            stage: first.stage,
            actual_input_digest,
            request_artifact: first.request_artifact.clone(),
            max_cost_micros: first.reserved_micros,
            lease_token: first.lease_token.clone(),
            lease_until: first.lease_until,
            now: 10,
        };
    for (label, asked) in [
        (
            "the new stream's group",
            reservation(new_id, first.actual_input_digest.clone()),
        ),
        (
            "another effective request",
            reservation(old_id, hash(b"another effective request")),
        ),
    ] {
        match env.store.reserve_budget_call(&worker(), &asked).await {
            Err(Error::Conflict(message)) => {
                assert_eq!(message, "call_id_reused_with_different_content", "{label}");
            }
            other => panic!("{label}: expected the ledger's Conflict, got {other:?}"),
        }
    }
    // The same call, asked for again exactly as it was reserved, is the same
    // call: that is what a stream's own retry gets.
    let again = env
        .store
        .reserve_budget_call(
            &worker(),
            &reservation(old_id, first.actual_input_digest.clone()),
        )
        .await
        .unwrap();
    assert_eq!(&again, first);
    // The refusals made no call and charged nothing to either stream.
    assert_eq!(env.executions.load(Ordering::SeqCst), spent_before);
    assert!(env.calls(new_id).await.is_empty());
    assert_eq!(env.calls(old_id).await, old_calls);

    // Through the coordinator the same mistake is not a silent shared call
    // either: the new stream's step under the old stream's request id ends as
    // an uncertain dispatch that names the conflict, and bills nothing.
    let misused = env
        .step_with(TRIAL, MetaStream::New, 1, &old_request_id)
        .await;
    assert!(
        misused
            .outcome
            .contains("call_id reused with a different effective model request"),
        "{}",
        misused.outcome
    );
    let usage = env
        .coordinator
        .verified_mechanism_usage(new_id)
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Uncertain);
    assert_eq!(env.executions.load(Ordering::SeqCst), spent_before);
    assert!(env.calls(new_id).await.is_empty());
    assert_eq!(
        env.calls(old_id).await,
        old_calls,
        "the old stream's spend is untouched"
    );

    // Each stream deriving its own id is what keeps them apart: with ids from
    // `stream_request_id` both streams of another trial run and are billed
    // independently.
    env.fork("trial-2").await;
    env.step("trial-2", MetaStream::Old, 1).await;
    env.step("trial-2", MetaStream::New, 1).await;
    assert_eq!(env.calls("trial-2-old").await.len(), 2);
    assert_eq!(env.calls("trial-2-new").await.len(), 2);
    for call in env
        .calls("trial-2-old")
        .await
        .iter()
        .chain(env.calls("trial-2-new").await.iter())
    {
        assert_eq!(call.state, BudgetCallState::Finalized);
    }
}

// ---------------------------------------------------------------------------
// 5. Actual spend is recorded per stream and may differ (V035)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_stream_reads_back_what_it_really_spent_and_stopping_one_does_not_stop_the_other() {
    let env = env().await;
    env.fork(TRIAL).await;
    let (old_id, new_id) = ("trial-1-old", "trial-1-new");

    // Nothing is spent at fork time, and the declared limits are the same.
    let fresh = env.view(TRIAL).await.unwrap();
    for stream in MetaStream::BOTH {
        let spend = fresh.stream(stream).spend();
        assert_eq!(
            (
                spend.calls,
                spend.finalized_calls,
                spend.uncertain_calls,
                spend.actual_cost_micros,
                spend.unsettled_reserved_micros
            ),
            (0, 0, 0, 0, 0)
        );
        assert!(!fresh.stream(stream).dispatch_group_stopped());
    }

    // The new stream takes a step; then its dispatch group is stopped. The old
    // stream, not stopped, takes its step afterwards and is billed normally.
    env.step(TRIAL, MetaStream::New, 1).await;
    let stopped = env
        .store
        .stop_dispatch_group(&admin(), SCOPE, new_id, "futility", 20)
        .await
        .unwrap();
    assert!(stopped.stopped);
    // The stopped stream's next step is refused by the budget layer before any
    // call row exists: no reservation and no model call.
    let refused = env.step(TRIAL, MetaStream::New, 2).await;
    assert!(
        refused.outcome.contains("cancelled"),
        "the stopped stream's step ends on the stopped group: {}",
        refused.outcome
    );
    assert_eq!(env.executions.load(Ordering::SeqCst), 2);
    env.step(TRIAL, MetaStream::Old, 1).await;

    let view = env.view(TRIAL).await.unwrap();
    let (old, new) = (view.old_stream(), view.new_stream());
    assert!(
        !old.dispatch_group_stopped(),
        "stopping the new stream's group did not stop the old one's"
    );
    assert!(new.dispatch_group_stopped());

    // Each stream's own calls and cost, as the ledger holds them: the old
    // stream made two calls at 7 and the new stream two at 13; the new stream's
    // refused step left no call row at all.
    let old_spend = old.spend();
    assert_eq!(old_spend.calls, 2);
    assert_eq!(old_spend.finalized_calls, 2);
    assert_eq!(old_spend.uncertain_calls, 0);
    assert_eq!(old_spend.actual_cost_micros, 2 * OLD_CALL_COST);
    assert_eq!(old_spend.unsettled_reserved_micros, 0);
    let new_spend = new.spend();
    assert_eq!(new_spend.calls, 2);
    assert_eq!(new_spend.finalized_calls, 2);
    assert_eq!(new_spend.uncertain_calls, 0);
    assert_eq!(new_spend.actual_cost_micros, 2 * NEW_CALL_COST);
    assert_eq!(new_spend.unsettled_reserved_micros, 0);
    assert_ne!(
        old_spend.actual_cost_micros, new_spend.actual_cost_micros,
        "the streams are not forced to spend the same"
    );
    assert!(
        env.calls(new_id)
            .await
            .iter()
            .all(|call| call.state == BudgetCallState::Finalized)
    );

    // Both streams declared the same limits; what each spent differs, and the
    // view does not pad either stream to match the other.
    assert_eq!(view.declared_stream_limits().remaining_root_micros, 1_000);
    let old_world = world_of(&env.store, old_id).await;
    let new_world = world_of(&env.store, new_id).await;
    assert_eq!(old_world.remaining_root_micros, 1_000 - 10);
    assert_eq!(new_world.remaining_root_micros, 1_000 - 2 * 10);
    assert_eq!(old.verified_usage(), 1);
    assert_eq!(new.verified_usage(), 2);
    assert_eq!(view.status(), MetaTrialStatus::UsageObserved);

    // Stopping is per stream: nothing ran for the stopped stream's step 2, the
    // other stream's group and the root budget itself are untouched.
    assert_eq!(
        env.executions.load(Ordering::SeqCst),
        4,
        "no call was made for the stopped stream's step 2"
    );
    let root = env
        .store
        .root_budget(&worker(), SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert!(!root.stopped, "the root budget itself was not stopped");
    assert_eq!(root.spent_micros, 2 * OLD_CALL_COST + 2 * NEW_CALL_COST);
    assert_eq!(root.reserved_micros, 0);
}

#[tokio::test]
async fn one_stream_can_exhaust_the_shared_root_and_starve_the_other() {
    // The boundary the trial does not hide: the declared limits are the same
    // for both streams, but the root budget is one, shared, and not split.
    // A root that holds 60: a call reserves up to 20, so after the new stream
    // spent 52 there is no room left for even one call of the old stream.
    let env = env_with_root(60).await;
    env.fork(TRIAL).await;
    let (old_id, new_id) = ("trial-1-old", "trial-1-new");
    env.step(TRIAL, MetaStream::New, 1).await;
    env.step(TRIAL, MetaStream::New, 2).await;
    assert_eq!(env.executions.load(Ordering::SeqCst), 4);
    let root = env
        .store
        .root_budget(&worker(), SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 4 * NEW_CALL_COST);

    // The old stream's step finds no budget: its call is refused, nothing runs.
    let starved = env.step(TRIAL, MetaStream::Old, 1).await;
    assert_eq!(env.executions.load(Ordering::SeqCst), 4);
    assert!(
        starved.outcome.contains("budget"),
        "the old stream's step ends on the exhausted root: {}",
        starved.outcome
    );

    let view = env.view(TRIAL).await.unwrap();
    // Declared the same, spent very differently, and the old stream never got
    // to spend its declared share: the trial reports it, it does not prevent it.
    assert_eq!(view.declared_stream_limits().remaining_root_micros, 1_000);
    assert_eq!(
        view.new_stream().spend().actual_cost_micros,
        4 * NEW_CALL_COST
    );
    assert_eq!(view.old_stream().spend().actual_cost_micros, 0);
    assert_eq!(view.old_stream().spend().finalized_calls, 0);
    assert!(
        env.calls(old_id)
            .await
            .iter()
            .all(|call| call.state != BudgetCallState::Finalized)
    );
    assert!(!view.old_stream().dispatch_group_stopped());
    assert!(!view.new_stream().dispatch_group_stopped());
    assert_eq!(env.calls(new_id).await.len(), 4);
}

#[tokio::test]
async fn an_uncertain_call_of_one_stream_holds_the_roots_concurrency_slot_against_the_other() {
    // The other place the streams are coupled through the one shared root: the
    // ledger allows one unsettled dispatch per root, so a call of the old
    // stream whose outcome is unknown keeps the new stream from dispatching.
    let env = env().await;
    env.fork(TRIAL).await;
    env.fail_old_stream.store(true, Ordering::SeqCst);
    let failed = env.step(TRIAL, MetaStream::Old, 1).await;
    env.fail_old_stream.store(false, Ordering::SeqCst);
    assert!(
        failed.outcome.contains("usage remains unknown"),
        "{}",
        failed.outcome
    );
    assert_eq!(env.executions.load(Ordering::SeqCst), 1);

    let blocked = env.step(TRIAL, MetaStream::New, 1).await;
    assert!(
        blocked.outcome.contains("root_budget_concurrency_limit"),
        "{}",
        blocked.outcome
    );
    assert_eq!(
        env.executions.load(Ordering::SeqCst),
        1,
        "the new stream made no model call"
    );

    // The view reports both streams as they are: the old stream's call is
    // uncertain (its cost unknown, its reservation held), the new stream's
    // call was reserved and never dispatched, and nothing was settled at all.
    let view = env.view(TRIAL).await.unwrap();
    let (old, new) = (view.old_stream().spend(), view.new_stream().spend());
    assert_eq!(
        (
            old.calls,
            old.finalized_calls,
            old.uncertain_calls,
            old.actual_cost_micros,
            old.unsettled_reserved_micros
        ),
        (1, 0, 1, 0, 20)
    );
    assert_eq!(
        (
            new.calls,
            new.finalized_calls,
            new.uncertain_calls,
            new.actual_cost_micros,
            new.unsettled_reserved_micros
        ),
        (1, 0, 0, 0, 20)
    );
    // Both dispatches are real dispatches that happened under their own
    // policies, uncertain ones included (they were already paid for).
    assert_eq!(view.old_stream().verified_usage(), 1);
    assert_eq!(view.new_stream().verified_usage(), 1);
    assert_eq!(view.status(), MetaTrialStatus::UsageObserved);
    let wire = serde_json::to_value(&view).unwrap();
    assert_eq!(wire["old"]["spend"]["uncertain_calls"], 1);
    assert_eq!(wire["new"]["spend"]["unsettled_reserved_micros"], 20);
}

#[tokio::test]
async fn the_longest_trial_id_still_yields_valid_world_node_and_request_ids() {
    // Two world ids hang off a trial id, and node ids off those; the id bound is
    // what keeps all of them identifiers. At the bound a stream still runs.
    let env = env().await;
    let long = "t".repeat(META_TRIAL_ID_MAX_BYTES);
    let trial = env.fork(&long).await;
    assert_eq!(trial.old_world_id.len(), META_TRIAL_ID_MAX_BYTES + 4);
    env.step(&long, MetaStream::Old, 1).await;
    env.step(&long, MetaStream::New, 1).await;
    env.step(&long, MetaStream::New, 2).await;
    let view = env.view(&long).await.unwrap();
    assert_eq!(view.old_stream().verified_usage(), 1);
    assert_eq!(view.new_stream().verified_usage(), 2);
    assert_eq!(view.new_stream().spend().calls, 4);
}

// ---------------------------------------------------------------------------
// 5b. The trial's billing scope binds the streams' spend (V033, V035)
// ---------------------------------------------------------------------------

/// The message a trial that is not verifiable on account of its billing scope
/// returns: fixed, and naming only the stream.
fn outside_scope_message(stream: &str) -> String {
    format!(
        "meta trial {stream} stream: its dispatch group was billed outside the trial's billing scope"
    )
}

fn assert_outside_scope(result: Result<MetaTrialView>, stream: &str) {
    match result {
        Err(Error::Conflict(message)) => assert_eq!(message, outside_scope_message(stream)),
        other => panic!(
            "expected the Conflict that names the {stream} stream, got {:?}",
            other.map(|view| view.status())
        ),
    }
}

/// A plain reflection request of `episode`, as the transport answers it.
fn plain_request(request_id: &str, episode: &str) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: request_id.into(),
            namespace: TENANT.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: episode.into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent-v1"),
            bundle_digest: hash(b"bundle-v1"),
            source_closure: vec![EvidenceRef {
                id: "run-failure".into(),
                digest: hash(b"run-failure"),
            }],
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "parent-skill".into(),
            content: serde_json::to_string(&parent_skill()).unwrap(),
        }],
    )
    .unwrap()
}

#[tokio::test]
async fn a_stream_billed_outside_the_trials_billing_scope_makes_the_trial_unverifiable() {
    // The trial declares one billing scope for both streams: they are compared
    // on one ceiling, one root budget. A stream that spends from another root
    // has no such ceiling, and a reader that totals its dispatch group under the
    // declared scope would report that spend as zero. The trial is then not
    // verifiable, and the view does not come out with a false account.
    let env = env().await;
    authorize_second_scope(&env.store, 10_000).await;
    let elsewhere = env.broker_for("scope-2");

    // The new stream's step is made by a broker bound to the other root; the old
    // stream's by the declared one.
    env.fork(TRIAL).await;
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step_via(&elsewhere, TRIAL, MetaStream::New, 1).await;
    // The premise: the new stream really spent (two calls, 26 micros), under
    // scope-2, and its dispatch is a real, verified one; the declared scope holds
    // nothing for its group.
    let spent = env.calls_in("scope-2", "trial-1-new").await;
    assert_eq!(spent.len(), 2);
    assert!(
        spent
            .iter()
            .all(|call| call.state == BudgetCallState::Finalized)
    );
    assert_eq!(
        spent
            .iter()
            .filter_map(|call| call.actual_cost_micros)
            .sum::<i64>(),
        2 * NEW_CALL_COST
    );
    assert!(env.calls("trial-1-new").await.is_empty());
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("trial-1-new")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(env.calls("trial-1-old").await.len(), 2);
    assert_outside_scope(env.view(TRIAL).await, "new");

    // The refusal is the same whichever stream it is, and names that stream.
    env.fork("trial-2").await;
    env.step_via(&elsewhere, "trial-2", MetaStream::Old, 1)
        .await;
    env.step("trial-2", MetaStream::New, 1).await;
    assert_outside_scope(env.view("trial-2").await, "old");

    // A stream with calls under the declared scope as well as under another one
    // is not verifiable either: any call outside the scope is enough.
    env.fork("trial-3").await;
    env.step("trial-3", MetaStream::New, 1).await;
    env.step_via(&elsewhere, "trial-3", MetaStream::New, 2)
        .await;
    assert_eq!(env.calls("trial-3-new").await.len(), 2);
    assert_eq!(env.calls_in("scope-2", "trial-3-new").await.len(), 2);
    assert_outside_scope(env.view("trial-3").await, "new");

    // With both streams outside, the old stream is the one named (it is read first).
    env.fork("trial-4").await;
    env.step_via(&elsewhere, "trial-4", MetaStream::Old, 1)
        .await;
    env.step_via(&elsewhere, "trial-4", MetaStream::New, 1)
        .await;
    assert_outside_scope(env.view("trial-4").await, "old");

    // A trial whose streams both spent under the declared scope is read as
    // usual, in the same store and next to the ones above.
    env.fork("trial-5").await;
    env.step("trial-5", MetaStream::Old, 1).await;
    env.step("trial-5", MetaStream::New, 1).await;
    let view = env.view("trial-5").await.unwrap();
    assert_eq!(view.status(), MetaTrialStatus::UsageObserved);
    assert_eq!(
        view.old_stream().spend().actual_cost_micros,
        2 * OLD_CALL_COST
    );
    assert_eq!(
        view.new_stream().spend().actual_cost_micros,
        2 * NEW_CALL_COST
    );

    // Reading changed nothing: the rows are as the steps left them.
    assert_eq!(env.calls_in("scope-2", "trial-1-new").await, spent);
}

#[tokio::test]
async fn calls_of_unrelated_dispatch_groups_under_another_scope_do_not_touch_the_trial() {
    let env = env().await;
    authorize_second_scope(&env.store, 10_000).await;
    env.fork(TRIAL).await;
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step(TRIAL, MetaStream::New, 1).await;
    let before = serde_json::to_value(env.view(TRIAL).await.unwrap()).unwrap();
    assert_eq!(
        before["new"]["spend"]["actual_cost_micros"],
        2 * NEW_CALL_COST
    );

    // Calls of other dispatch groups under the other root: an unrelated episode
    // and groups whose names only look like a stream's (a prefix, a suffix, the
    // trial id itself). A group is matched by its exact name.
    let elsewhere = env.broker_for("scope-2");
    let groups = [
        "unrelated-episode",
        "trial-1-new-extra",
        "trial-1-old-x",
        "x-trial-1-new",
        "trial-1",
    ];
    for (index, group) in groups.iter().enumerate() {
        let request = plain_request(&format!("unrelated-call-{index}"), group);
        assert!(matches!(
            elsewhere.dispatch(request).await.unwrap(),
            ModelResponse::Completed { .. }
        ));
    }
    // An unrelated group under the declared scope itself is no different.
    let declared = plain_request("unrelated-call-declared", "unrelated-episode-2");
    assert!(matches!(
        env.broker.dispatch(declared).await.unwrap(),
        ModelResponse::Completed { .. }
    ));
    // They are real calls under scope-2, and of no stream's group.
    for group in groups {
        assert_eq!(env.calls_in("scope-2", group).await.len(), 1, "{group}");
    }
    for stream in MetaStream::BOTH {
        let world = stream_world_id(TRIAL, stream);
        assert!(env.calls_in("scope-2", &world).await.is_empty());
    }

    // The trial reads exactly as before.
    let after = serde_json::to_value(env.view(TRIAL).await.unwrap()).unwrap();
    assert_eq!(after, before);
}

#[tokio::test]
async fn before_any_call_the_billing_scope_is_only_the_trials_own_declaration() {
    // A boundary, pinned. No world carries the billing scope, so until a stream
    // has a call row there is nothing to compare the stored declaration with:
    // changing it to another authorized scope goes unnoticed. The first call of a
    // stream, which sits under the real scope, exposes it.
    let env = env().await;
    authorize_second_scope(&env.store, 10_000).await;
    env.fork(TRIAL).await;
    let original = raw_record(&env.store, TRIAL_KIND, TRIAL).await;
    let mut moved = original.clone();
    moved["payload"]["billing_scope"] = json!("scope-2");
    put_raw_record(&env.store, TRIAL_KIND, TRIAL, &moved).await;
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(view.billing_scope(), "scope-2");
    assert_eq!(view.old_stream().spend().calls, 0);
    assert_eq!(view.new_stream().spend().calls, 0);

    env.step(TRIAL, MetaStream::New, 1).await;
    assert_outside_scope(env.view(TRIAL).await, "new");
    put_raw_record(&env.store, TRIAL_KIND, TRIAL, &original).await;
    assert_eq!(
        env.view(TRIAL).await.unwrap().billing_scope(),
        SCOPE,
        "with the declaration restored the trial reads again"
    );
}

/// A root budget `scope` that `authorizing` (a namespace's admin) opens to `namespaces`.
async fn authorize_scope(store: &Store, authorizing: &Context, scope: &str, namespaces: &[&str]) {
    store
        .authorize_root_budget(
            authorizing,
            &RootBudgetAuthorization {
                root_budget_id: format!("root-of-{scope}"),
                billing_scope: scope.into(),
                allowed_namespaces: namespaces.iter().map(|name| (*name).into()).collect(),
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(scope.as_bytes()),
                per_call_cap_micros: 20,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

async fn reserve_call(store: &Store, ctx: &Context, scope: &str, call_id: &str, group: &str) {
    store
        .reserve_budget_call(
            ctx,
            &BudgetCallReservation {
                billing_scope: scope.into(),
                call_id: call_id.into(),
                dispatch_group_id: group.into(),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(call_id.as_bytes()),
                request_artifact: None,
                max_cost_micros: 10,
                lease_token: format!("lease-{call_id}"),
                lease_until: 11,
                now: 10,
            },
        )
        .await
        .unwrap();
}

async fn scopes_of(store: &Store, ctx: &Context, group: &str) -> Result<Vec<String>> {
    let mut session = store.session().await.unwrap();
    let scopes = session.budget_call_scopes_for_group(ctx, group).await;
    session.commit().await.unwrap();
    scopes
}

#[tokio::test]
async fn the_scopes_of_a_dispatch_group_are_distinct_sorted_and_stay_inside_the_namespace() {
    // The ledger query the trial's scope check stands on, against a ledger that
    // holds the same group name in several scopes and in two namespaces.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("scopes.sqlite3"))
        .await
        .unwrap();
    let admin_n = admin();
    let admin_m = Context::new("m", "admin", Role::Admin).unwrap();
    let worker_n = worker();
    let worker_m = Context::new("m", "worker", Role::Worker).unwrap();
    authorize_scope(&store, &admin_n, "scope-b", &["n"]).await;
    authorize_scope(&store, &admin_n, "scope-a", &["n"]).await;
    authorize_scope(&store, &admin_n, "scope-x", &["n", "m"]).await;
    authorize_scope(&store, &admin_m, "scope-m", &["m"]).await;

    // Namespace n: group g under scope-b first (twice), then under scope-a; group
    // h under the shared scope-x; and group g2, whose name starts with g.
    reserve_call(&store, &worker_n, "scope-b", "b-1", "g").await;
    reserve_call(&store, &worker_n, "scope-b", "b-2", "g").await;
    reserve_call(&store, &worker_n, "scope-a", "a-1", "g").await;
    reserve_call(&store, &worker_n, "scope-x", "x-n-1", "h").await;
    reserve_call(&store, &worker_n, "scope-b", "b-3", "g2").await;
    // Namespace m: the same group name g, under its own scope and under the
    // shared one.
    reserve_call(&store, &worker_m, "scope-m", "m-1", "g").await;
    reserve_call(&store, &worker_m, "scope-x", "x-m-1", "g").await;
    let rows_before = {
        let mut session = store.session().await.unwrap();
        let rows = session
            .budget_calls_for_group(&worker_n, "scope-b", "g")
            .await
            .unwrap();
        session.commit().await.unwrap();
        rows
    };
    assert_eq!(rows_before.len(), 2);

    // Distinct (scope-b holds two calls of g) and in byte order (scope-b was
    // used first), and nothing of namespace m's g.
    assert_eq!(
        scopes_of(&store, &worker_n, "g").await.unwrap(),
        ["scope-a", "scope-b"]
    );
    // A group is matched by its exact name, and only for the caller's namespace:
    // h sits under the scope both namespaces share, g of namespace m does too.
    assert_eq!(
        scopes_of(&store, &worker_n, "h").await.unwrap(),
        ["scope-x"]
    );
    assert_eq!(
        scopes_of(&store, &worker_n, "g2").await.unwrap(),
        ["scope-b"]
    );
    assert_eq!(
        scopes_of(&store, &worker_m, "g").await.unwrap(),
        ["scope-m", "scope-x"]
    );
    assert!(scopes_of(&store, &worker_m, "h").await.unwrap().is_empty());
    assert!(scopes_of(&store, &worker_m, "g2").await.unwrap().is_empty());
    // No call, no scope.
    assert!(
        scopes_of(&store, &worker_n, "no-such-group")
            .await
            .unwrap()
            .is_empty()
    );
    // A namespace without any call of its own sees nothing, whatever the others did.
    let worker_other = Context::new("other", "worker", Role::Worker).unwrap();
    assert!(
        scopes_of(&store, &worker_other, "g")
            .await
            .unwrap()
            .is_empty()
    );

    // Every reader role of the ledger may ask, the same answer; an agent may not.
    for role in [Role::Admin, Role::Host, Role::Evaluator, Role::Worker] {
        let ctx = Context::new("n", "reader", role).unwrap();
        assert_eq!(
            scopes_of(&store, &ctx, "g").await.unwrap(),
            ["scope-a", "scope-b"],
            "{role:?}"
        );
    }
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    assert!(matches!(
        scopes_of(&store, &agent, "g").await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        scopes_of(&store, &worker_n, "not an identifier").await,
        Err(Error::Invalid(_))
    ));

    // Reading is read-only: the rows are as they were.
    let rows_after = {
        let mut session = store.session().await.unwrap();
        let rows = session
            .budget_calls_for_group(&worker_n, "scope-b", "g")
            .await
            .unwrap();
        session.commit().await.unwrap();
        rows
    };
    assert_eq!(rows_after, rows_before);
}

// ---------------------------------------------------------------------------
// 6. Idempotency (V033)
// ---------------------------------------------------------------------------

async fn snapshot(env: &Env, trial_id: &str) -> Vec<Value> {
    let mut records = vec![raw_record(&env.store, TRIAL_KIND, trial_id).await];
    for stream in MetaStream::BOTH {
        records.push(raw_record(&env.store, WORLD, &stream_world_id(trial_id, stream)).await);
    }
    records
}

#[tokio::test]
async fn replaying_a_fork_converges_on_the_same_trial_and_writes_nothing() {
    let env = env().await;
    let first = env.fork(TRIAL).await;
    let before = snapshot(&env, TRIAL).await;
    let edges_before = dependents(&env.store, "run", "run-failure").await;

    for _ in 0..2 {
        let again = env.fork(TRIAL).await;
        // The first writer's trial, `created_at` included, not a new one.
        assert_eq!(
            serde_json::to_value(&again).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
    }
    assert_eq!(snapshot(&env, TRIAL).await, before);
    assert_eq!(
        dependents(&env.store, "run", "run-failure").await,
        edges_before
    );
    env.view(TRIAL).await.unwrap();
}

#[tokio::test]
async fn the_same_trial_id_with_different_content_is_a_conflict_and_changes_nothing() {
    let env = env().await;
    env.fork(TRIAL).await;
    let before = snapshot(&env, TRIAL).await;

    let mut other_candidate = fork_request(TRIAL);
    let ImproverMechanismV2::ExplorationPolicy { policy } =
        &mut other_candidate.new_content.mechanism;
    policy.max_focus_actions = 3;
    let mut other_limits = fork_request(TRIAL);
    other_limits.s0_template.remaining_root_micros = 999;
    let mut other_cost = fork_request(TRIAL);
    other_cost.s0_template.successor_cost_upper_micros = 11;
    let mut other_caps = fork_request(TRIAL);
    other_caps.s0_template.caps.max_nodes = 6;
    let mut other_scope = fork_request(TRIAL);
    other_scope.billing_scope = "scope-2".into();
    authorize_second_scope(&env.store, 100).await;
    for (label, request) in [
        ("another candidate", other_candidate),
        ("another declared quota", other_limits),
        ("another successor cost", other_cost),
        ("other caps", other_caps),
        ("another billing scope", other_scope),
    ] {
        let result = MetaTrialCoordinator::fork(&worker(), &env.store, request).await;
        assert!(
            matches!(result, Err(Error::Conflict(_))),
            "{label}: {result:?}"
        );
    }
    assert_eq!(snapshot(&env, TRIAL).await, before);
    env.view(TRIAL).await.unwrap();
}

/// A second root budget, `scope-2`, authorized for the same namespace.
async fn authorize_second_scope(store: &Store, total_limit_micros: i64) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-2".into(),
                billing_scope: "scope-2".into(),
                allowed_namespaces: vec![TENANT.into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-2".into(),
                authorization_receipt_digest: hash(b"admin-authorization-2"),
                per_call_cap_micros: 20,
                total_limit_micros,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_fork_interrupted_before_the_worlds_were_registered_converges_when_replayed() {
    let env = env().await;
    let first = env.fork(TRIAL).await;
    let complete = snapshot(&env, TRIAL).await;

    // Crash after the trial record: neither world exists yet. The trial alone
    // is not a verified trial.
    delete_raw_record(&env.store, WORLD, "trial-1-old").await;
    delete_raw_record(&env.store, WORLD, "trial-1-new").await;
    assert!(raw_exists(&env.store, TRIAL_KIND, TRIAL).await);
    assert!(
        env.view(TRIAL).await.is_err(),
        "a trial without its worlds is never reported live"
    );
    let replayed = env.fork(TRIAL).await;
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(snapshot(&env, TRIAL).await, complete);
    env.view(TRIAL).await.unwrap();

    // Crash after the old stream's world: the new stream's world is missing.
    delete_raw_record(&env.store, WORLD, "trial-1-new").await;
    assert!(env.view(TRIAL).await.is_err());
    let replayed = env.fork(TRIAL).await;
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(snapshot(&env, TRIAL).await, complete);
    env.view(TRIAL).await.unwrap();

    // The world records came back without a second trial and without touching
    // what was already there.
    assert_eq!(object_count_of_kind(&env.store, TRIAL_KIND).await, 1);

    // Crash before the trial record: the worlds exist, the record does not.
    // The same request adopts the worlds that are exactly what it would
    // register, and writes the record.
    delete_raw_record(&env.store, TRIAL_KIND, TRIAL).await;
    assert!(env.view(TRIAL).await.is_err());
    let replayed = env.fork(TRIAL).await;
    assert_eq!(replayed.new_world_id, first.new_world_id);
    assert_eq!(
        raw_record(&env.store, WORLD, "trial-1-old").await,
        complete[1]
    );
    env.view(TRIAL).await.unwrap();
}

async fn object_count_of_kind(store: &Store, record_kind: &str) -> usize {
    let mut session = store.session().await.unwrap();
    let all: Vec<Value> = session.list(&worker(), "artifact").await.unwrap();
    session.commit().await.unwrap();
    all.iter()
        .filter(|body| body["schema_version"] == ENVELOPE && body["record_kind"] == record_kind)
        .count()
}

#[tokio::test]
async fn a_world_id_the_trial_derives_may_only_be_taken_by_exactly_that_stream_world() {
    let env = env().await;
    // Someone registered a world under the id the new stream would get, with
    // another policy than the candidate's. The fork stops before it records a
    // trial over it.
    let mut squatter = world_for("trial-1-new", ElasticPolicyV1::default());
    squatter.root_opportunities[0].estimated_cost_upper_micros = 11;
    env.coordinator.register_world(squatter).await.unwrap();
    let result = MetaTrialCoordinator::fork(&worker(), &env.store, fork_request(TRIAL)).await;
    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert!(!raw_exists(&env.store, TRIAL_KIND, TRIAL).await);
    assert!(!raw_exists(&env.store, WORLD, "trial-1-old").await);

    // Nor one that is the right world but already holds less than the trial
    // declares for both streams: it would not have the same ceiling.
    let mut smaller = world_for("trial-4-new", policy_focus_one());
    smaller.remaining_root_micros = 500;
    env.coordinator.register_world(smaller).await.unwrap();
    let result = MetaTrialCoordinator::fork(&worker(), &env.store, fork_request("trial-4")).await;
    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert!(!raw_exists(&env.store, TRIAL_KIND, "trial-4").await);

    // A world with the right registration but the wrong policy is no better.
    let wrong_policy = world_for("trial-2-new", ElasticPolicyV1::default());
    env.coordinator.register_world(wrong_policy).await.unwrap();
    let result = MetaTrialCoordinator::fork(&worker(), &env.store, fork_request("trial-2")).await;
    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert!(!raw_exists(&env.store, TRIAL_KIND, "trial-2").await);

    // A world that is exactly the stream's world, registered ahead of the fork,
    // is adopted, and the fork writes the rest.
    env.coordinator
        .register_world(world_for("trial-3-new", policy_focus_one()))
        .await
        .unwrap();
    let adopted = env.fork("trial-3").await;
    assert_eq!(adopted.new_world_id, "trial-3-new");
    env.view("trial-3").await.unwrap();
}

#[tokio::test]
async fn after_a_dispatch_replaying_the_fork_and_reading_the_trial_still_agree() {
    let env = env().await;
    let first = env.fork(TRIAL).await;
    let declared = first.declared_stream_limits.remaining_root_micros;
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step(TRIAL, MetaStream::New, 1).await;
    env.step(TRIAL, MetaStream::New, 2).await;

    // The dispatches spent the worlds' own counters: they no longer hold what
    // was registered, which is what a re-registration of a stored world would
    // trip over. The trial keeps its own declaration.
    let old = world_of(&env.store, "trial-1-old").await;
    let new = world_of(&env.store, "trial-1-new").await;
    assert!(old.remaining_root_micros < declared);
    assert!(new.remaining_root_micros < old.remaining_root_micros);
    assert_eq!(old.dispatch_ids.len(), 1);
    assert_eq!(new.dispatch_ids.len(), 2);

    // The read side does not mistake that for tampering ...
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(
        view.declared_stream_limits().remaining_root_micros,
        declared
    );
    assert_eq!(view.old_stream().verified_usage(), 1);
    assert_eq!(view.new_stream().verified_usage(), 2);
    // ... and replaying the fork converges on the same trial, no world rewritten.
    let before = snapshot(&env, TRIAL).await;
    let replayed = env.fork(TRIAL).await;
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(snapshot(&env, TRIAL).await, before);
    env.view(TRIAL).await.unwrap();
}

// ---------------------------------------------------------------------------
// 7. Revocation: a revoked source blocks the trial and its cleanup reaches it
// ---------------------------------------------------------------------------

async fn revoke(store: &Store, run: &str) -> CleanupStatus {
    LifecycleCoordinator::revoke_source(&admin(), store, run, "privacy", 200)
        .await
        .unwrap()
}

/// Pages the cleanup to its end. A `Failed` job (an unknown scope) fails the
/// test.
async fn finish_cleanup(store: &Store, mut status: CleanupStatus, page: usize) -> CleanupStatus {
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
        "cleanup did not complete: {:?}",
        status.last_error
    );
    assert_eq!(status.pending_nodes, 0);
    assert!(status.last_error.is_none(), "{:?}", status.last_error);
    status
}

fn blocked(result: Result<impl std::fmt::Debug>) -> Error {
    match result {
        Err(error) => error,
        Ok(value) => panic!("succeeded on a revoked source: {value:?}"),
    }
}

#[tokio::test]
async fn a_revoked_source_blocks_the_trial_makes_no_model_call_and_its_cleanup_redacts_the_record()
{
    let env = env().await;
    let trial = env.fork(TRIAL).await;
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step(TRIAL, MetaStream::New, 1).await;
    // Positive controls: everything below works before the revocation.
    env.view(TRIAL).await.unwrap();
    let executions = env.executions.load(Ordering::SeqCst);
    let calls_before = (
        env.calls("trial-1-old").await,
        env.calls("trial-1-new").await,
    );

    let originals = snapshot(&env, TRIAL).await;
    let record_text = originals[0].to_string();
    // What only the original record carries and a tombstone must not.
    let content = [
        trial.s0_template_digest.clone(),
        trial.old_content_digest.clone(),
        trial.new_content_digest.clone(),
        trial.old_policy_digest.clone(),
        trial.new_policy_digest.clone(),
    ];
    for needle in &content {
        assert!(record_text.contains(needle.as_str()));
    }

    // The logical block comes first: the moment the source is revoked, before
    // a single cleanup step ran, the trial is neither readable nor forkable and
    // no stream reaches a model.
    let started = revoke(&env.store, "run-failure").await;
    assert!(
        matches!(
            blocked(env.view(TRIAL).await),
            Error::Conflict(_) | Error::Forbidden
        ),
        "the trial must not verify over a revoked source"
    );
    assert!(matches!(
        blocked(MetaTrialCoordinator::fork(&worker(), &env.store, fork_request(TRIAL)).await),
        Error::Conflict(_) | Error::Forbidden
    ));
    assert!(matches!(
        blocked(MetaTrialCoordinator::fork(&worker(), &env.store, fork_request("trial-9")).await),
        Error::Conflict(_) | Error::Forbidden
    ));
    for (stream, step) in [(MetaStream::Old, 2), (MetaStream::New, 3)] {
        let result = env
            .try_step(TRIAL, stream, step, &stream_request_id(TRIAL, stream, step))
            .await;
        assert!(
            matches!(blocked(result), Error::Conflict(_) | Error::Forbidden),
            "{stream:?}"
        );
    }
    assert_eq!(
        env.executions.load(Ordering::SeqCst),
        executions,
        "no model call after the revocation"
    );
    assert_eq!(
        (
            env.calls("trial-1-old").await,
            env.calls("trial-1-new").await
        ),
        calls_before,
        "reading or refusing the trial reserved nothing"
    );
    // The records are still there to be cleaned: the block does not wait for it.
    assert_eq!(snapshot(&env, TRIAL).await, originals);
    assert!(!raw_exists(&env.store, TRIAL_KIND, "trial-9").await);

    // The cleanup reaches the trial through its edges and completes: it is not
    // an unknown scope. The record becomes a tombstone without its content.
    finish_cleanup(&env.store, started, 2).await;
    let tombstone = raw_record(&env.store, TRIAL_KIND, TRIAL).await;
    assert_eq!(tombstone["schema_version"], "rsia.redacted.v1");
    assert_eq!(tombstone["state"], "source_revoked");
    assert_eq!(tombstone["original_kind"], "artifact");
    assert_eq!(tombstone["original_schema"], ENVELOPE);
    assert_eq!(tombstone["metadata"]["record_kind"], TRIAL_KIND);
    assert!(tombstone.get("payload").is_none());
    let tombstone_text = tombstone.to_string();
    for needle in content.iter().chain([&SCOPE.to_owned()]) {
        assert!(
            !tombstone_text.contains(needle.as_str()),
            "the tombstone still carries {needle}"
        );
    }
    for stream in MetaStream::BOTH {
        let world = raw_record(&env.store, WORLD, &stream_world_id(TRIAL, stream)).await;
        assert_eq!(world["schema_version"], "rsia.redacted.v1", "{stream:?}");
    }
    // Afterwards the redaction is named, not reported as a storage failure.
    match blocked(env.view(TRIAL).await) {
        Error::Conflict(message) => {
            assert!(message.contains("redacted"), "{message}");
            assert!(message.contains(TRIAL_KIND), "{message}");
        }
        other => panic!("expected a Conflict naming the redaction, got {other:?}"),
    }
    assert!(matches!(
        blocked(MetaTrialCoordinator::fork(&worker(), &env.store, fork_request(TRIAL)).await),
        Error::Conflict(_) | Error::Forbidden
    ));
    assert_eq!(env.executions.load(Ordering::SeqCst), executions);
}

#[tokio::test]
async fn revoking_the_other_source_of_the_closure_redacts_the_trial_too() {
    // Any source of the closure reaches the record, not only the first one.
    let env = env().await;
    env.fork(TRIAL).await;
    let started = revoke(&env.store, "run-success").await;
    finish_cleanup(&env.store, started, 3).await;
    for (kind, id) in [
        (TRIAL_KIND, TRIAL.to_owned()),
        (WORLD, "trial-1-old".to_owned()),
        (WORLD, "trial-1-new".to_owned()),
    ] {
        assert_eq!(
            raw_record(&env.store, kind, &id).await["schema_version"],
            "rsia.redacted.v1",
            "{kind} {id}"
        );
    }
}

#[tokio::test]
async fn revoking_an_unrelated_run_leaves_the_trial_record_as_it_was() {
    let env = env().await;
    env.fork(TRIAL).await;
    let before = snapshot(&env, TRIAL).await;
    let edges = (
        dependents(&env.store, "run", "run-failure").await,
        dependents(&env.store, "run", "run-success").await,
    );
    let started = revoke(&env.store, UNRELATED_RUN).await;
    finish_cleanup(&env.store, started, 2).await;
    // Byte for byte what it was. (Any revocation moves the namespace watermark,
    // so the trial can no longer be read as live: that is the logical block,
    // not a change to its records.)
    assert_eq!(snapshot(&env, TRIAL).await, before);
    assert_eq!(
        (
            dependents(&env.store, "run", "run-failure").await,
            dependents(&env.store, "run", "run-success").await
        ),
        edges
    );
    assert!(matches!(
        blocked(env.view(TRIAL).await),
        Error::Conflict(_) | Error::Forbidden
    ));
}

// ---------------------------------------------------------------------------
// 8. The fork enqueues no management job (V034)
// ---------------------------------------------------------------------------

fn exploration_start_payload(request_key: &str, world: &ExplorationWorldV1) -> Value {
    json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": request_key,
        "world": serde_json::to_value(world).unwrap(),
    })
}

async fn wait_terminal(dispatcher: &ManagementDispatcher, job_id: &str) -> ManagementJob {
    for _ in 0..400 {
        let job = dispatcher.status(&admin(), job_id).await.unwrap();
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

#[tokio::test]
async fn a_fork_enqueues_no_management_job_and_makes_no_paid_call() {
    let env = env().await;
    let private_input = "rsia.management_private_input.v1";
    assert_eq!(object_count(&env.store, "job").await, 0);
    assert!(
        artifacts_with_schema(&env.store, private_input)
            .await
            .is_empty()
    );

    env.fork(TRIAL).await;
    // Replays, reads and a refused request do not enqueue either.
    env.fork(TRIAL).await;
    env.view(TRIAL).await.unwrap();
    let _ = MetaTrialCoordinator::fork(&worker(), &env.store, {
        let mut request = fork_request("refused");
        request.new_content = i0();
        request
    })
    .await;

    assert_eq!(
        object_count(&env.store, "job").await,
        0,
        "no management job"
    );
    assert!(
        artifacts_with_schema(&env.store, private_input)
            .await
            .is_empty()
    );
    let dispatcher = ManagementDispatcher::new(env.store.clone(), vec![admin()]).unwrap();
    assert_eq!(
        dispatcher.recover_pending().await.unwrap(),
        0,
        "nothing is waiting to be dispatched"
    );
    // No paid call and no reservation: the fork touches no ledger row.
    assert_eq!(env.executions.load(Ordering::SeqCst), 0);
    for stream in MetaStream::BOTH {
        assert!(env.calls(&stream_world_id(TRIAL, stream)).await.is_empty());
    }
    let root = env
        .store
        .root_budget(&worker(), SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.spent_micros, root.reserved_micros), (0, 0));

    // Control: a job that is queued through the dispatcher is counted by the
    // same measurements, so the zeros above mean something.
    let queued = dispatcher
        .submit(
            &admin(),
            "exploration.start",
            exploration_start_payload(
                "control",
                &world_for("world-control", ElasticPolicyV1::default()),
            ),
        )
        .await
        .unwrap();
    wait_terminal(&dispatcher, &queued.id).await;
    assert_eq!(object_count(&env.store, "job").await, 1);
    assert_eq!(
        artifacts_with_schema(&env.store, private_input).await.len(),
        1
    );
}

// ---------------------------------------------------------------------------
// 9. Tampering with the stored record or a world is a Conflict
// ---------------------------------------------------------------------------

type Tamper = Box<dyn Fn(&mut Value)>;

async fn assert_tamper_is_conflict(
    env: &Env,
    record_kind: &str,
    id: &str,
    cases: Vec<(&str, Tamper)>,
) {
    let original = raw_record(&env.store, record_kind, id).await;
    for (label, tamper) in &cases {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        assert_ne!(tampered, original, "{label}: the tamper changed nothing");
        put_raw_record(&env.store, record_kind, id, &tampered).await;
        let result = env.view(TRIAL).await;
        assert!(
            matches!(result, Err(Error::Conflict(_))),
            "{record_kind} {label}: expected a Conflict, got {:?}",
            result.map(|_| ())
        );
        put_raw_record(&env.store, record_kind, id, &original).await;
        env.view(TRIAL)
            .await
            .unwrap_or_else(|error| panic!("{record_kind} {label}: restoring failed: {error:?}"));
    }
}

#[tokio::test]
async fn tampering_with_the_trial_record_is_a_conflict() {
    let env = env().await;
    authorize_second_scope(&env.store, 100).await;
    env.fork(TRIAL).await;
    // A trial that already ran, so that the records have something to agree on.
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step(TRIAL, MetaStream::New, 1).await;

    let forged_content = {
        let mut content = i1();
        let ImproverMechanismV2::ExplorationPolicy { policy } = &mut content.mechanism;
        policy.max_focus_actions = 3;
        content
    };
    let cases: Vec<(&str, Tamper)> = vec![
        (
            "s0_template_digest",
            Box::new(|r| r["payload"]["s0_template_digest"] = json!(hash(b"forged"))),
        ),
        (
            "old_content_digest",
            Box::new(|r| r["payload"]["old_content_digest"] = json!(hash(b"forged"))),
        ),
        (
            "new_content_digest",
            Box::new(|r| r["payload"]["new_content_digest"] = json!(hash(b"forged"))),
        ),
        (
            "old_policy_digest",
            Box::new(|r| r["payload"]["old_policy_digest"] = json!(hash(b"forged"))),
        ),
        (
            "new_policy_digest swapped for the old one",
            Box::new(|r| {
                let old = r["payload"]["old_policy_digest"].clone();
                r["payload"]["new_policy_digest"] = old;
            }),
        ),
        (
            "new_content replaced (digests kept)",
            Box::new(move |r| {
                r["payload"]["new_content"] = serde_json::to_value(&forged_content).unwrap();
            }),
        ),
        (
            "old_content is not I0",
            Box::new(|r| r["payload"]["old_content"] = serde_json::to_value(i1()).unwrap()),
        ),
        (
            "old_world_id",
            Box::new(|r| r["payload"]["old_world_id"] = json!("trial-1-new")),
        ),
        (
            "new_world_id",
            Box::new(|r| r["payload"]["new_world_id"] = json!("other-new")),
        ),
        (
            "trial_id",
            Box::new(|r| r["payload"]["trial_id"] = json!("trial-2")),
        ),
        (
            "schema_version",
            Box::new(|r| r["payload"]["schema_version"] = json!("rsia.meta_trial.v0")),
        ),
        (
            "declared remaining_root_micros raised",
            Box::new(|r| {
                r["payload"]["declared_stream_limits"]["remaining_root_micros"] = json!(2_000)
            }),
        ),
        (
            "declared remaining_root_micros lowered",
            Box::new(|r| {
                r["payload"]["declared_stream_limits"]["remaining_root_micros"] = json!(5)
            }),
        ),
        (
            "declared remaining_recovery_dispatches",
            Box::new(|r| {
                r["payload"]["declared_stream_limits"]["remaining_recovery_dispatches"] = json!(1)
            }),
        ),
        (
            "declared successor cost",
            Box::new(|r| {
                r["payload"]["declared_stream_limits"]["successor_cost_upper_micros"] = json!(11)
            }),
        ),
        (
            "declared root opportunity cost",
            Box::new(|r| {
                r["payload"]["declared_stream_limits"]["root_opportunities"][0]["estimated_cost_upper_micros"] =
                    json!(11);
            }),
        ),
        (
            "source closure",
            Box::new(|r| r["payload"]["source_closure"] = json!(["run-failure", UNRELATED_RUN])),
        ),
        (
            "source closure emptied",
            Box::new(|r| r["payload"]["source_closure"] = json!([])),
        ),
        (
            "revoke_watermark",
            Box::new(|r| r["payload"]["revoke_watermark"] = json!(2)),
        ),
        // The streams' calls sit under the real scope, so a record that names
        // another one no longer matches the ledger (before any call it would).
        (
            "billing_scope moved to another authorized scope",
            Box::new(|r| r["payload"]["billing_scope"] = json!("scope-2")),
        ),
        (
            "envelope record kind",
            Box::new(|r| r["record_kind"] = json!(WORLD)),
        ),
    ];
    // (An envelope whose id differs from the storage id it is written under is
    // refused by the store itself: its objects table checks `$.id`.)
    assert_tamper_is_conflict(&env, TRIAL_KIND, TRIAL, cases).await;
}

fn world_tampers() -> Vec<(&'static str, Tamper)> {
    vec![
        (
            "approved_parent_digest",
            Box::new(|r| r["payload"]["approved_parent_digest"] = json!(hash(b"forged"))),
        ),
        (
            "parent_skill_digest (and so the signature no longer matches)",
            Box::new(|r| r["payload"]["parent_skill_digest"] = json!(hash(b"forged"))),
        ),
        (
            "environment_digest",
            Box::new(|r| r["payload"]["environment_digest"] = json!(hash(b"forged"))),
        ),
        (
            "tools_digest",
            Box::new(|r| r["payload"]["tools_digest"] = json!(hash(b"forged"))),
        ),
        (
            "grader_digest",
            Box::new(|r| r["payload"]["grader_digest"] = json!(hash(b"forged"))),
        ),
        (
            "caps",
            Box::new(|r| r["payload"]["caps"]["max_nodes"] = json!(6)),
        ),
        (
            "simulation",
            Box::new(|r| r["payload"]["simulation"] = json!({"online": {"fixed_seed": 8}})),
        ),
        (
            "root opportunity cost",
            Box::new(|r| {
                r["payload"]["root_opportunities"][0]["estimated_cost_upper_micros"] = json!(11)
            }),
        ),
        (
            "root opportunity branch",
            Box::new(|r| r["payload"]["root_opportunities"][1]["branch_seq"] = json!(5)),
        ),
        (
            "successor_cost_upper_micros",
            Box::new(|r| r["payload"]["successor_cost_upper_micros"] = json!(11)),
        ),
        (
            "initial_baseline_quality_micros",
            Box::new(|r| r["payload"]["initial_baseline_quality_micros"] = json!(400_000)),
        ),
        (
            "dependencies (another existing trusted run)",
            Box::new(|r| r["payload"]["dependencies"][1]["id"] = json!(UNRELATED_RUN)),
        ),
        (
            "remaining_root_micros above the declaration",
            Box::new(|r| r["payload"]["remaining_root_micros"] = json!(1_001)),
        ),
        (
            "remaining_recovery_dispatches above the declaration",
            Box::new(|r| r["payload"]["remaining_recovery_dispatches"] = json!(3)),
        ),
        (
            "world id",
            Box::new(|r| r["payload"]["id"] = json!("trial-1-other")),
        ),
    ]
}

#[tokio::test]
async fn tampering_with_a_world_outside_its_policy_is_a_conflict() {
    let env = env().await;
    env.fork(TRIAL).await;
    env.step(TRIAL, MetaStream::Old, 1).await;
    env.step(TRIAL, MetaStream::New, 1).await;
    for world in ["trial-1-old", "trial-1-new"] {
        assert_tamper_is_conflict(&env, WORLD, world, world_tampers()).await;
    }
}

#[tokio::test]
async fn a_world_no_dispatch_started_in_must_still_carry_the_declared_counters() {
    // Before any dispatch started a world is exactly what was registered, so a
    // lowered budget counter or a started schedule cannot be a spend. (Once a
    // dispatch started the check can only bound what the world holds from
    // above, which `tampering_with_a_world_outside_its_policy_is_a_conflict`
    // covers.)
    let env = env().await;
    env.fork(TRIAL).await;
    for world in ["trial-1-old", "trial-1-new"] {
        let cases: Vec<(&str, Tamper)> = vec![
            (
                "remaining_root_micros lowered",
                Box::new(|r| r["payload"]["remaining_root_micros"] = json!(999)),
            ),
            (
                "remaining_recovery_dispatches lowered",
                Box::new(|r| r["payload"]["remaining_recovery_dispatches"] = json!(1)),
            ),
            (
                "decision_round",
                Box::new(|r| r["payload"]["decision_round"] = json!(1)),
            ),
            (
                "current_branch_seq",
                Box::new(|r| r["payload"]["current_branch_seq"] = json!(1)),
            ),
            (
                "current_branch_focus_actions",
                Box::new(|r| r["payload"]["current_branch_focus_actions"] = json!(1)),
            ),
            (
                "waits",
                Box::new(|r| {
                    r["payload"]["waits"] = json!([{"action_seq": 1, "waited_rounds": 1}]);
                }),
            ),
        ];
        assert_tamper_is_conflict(&env, WORLD, world, cases).await;
    }
}

#[tokio::test]
async fn a_world_that_carries_the_other_streams_policy_or_another_one_is_a_conflict() {
    let env = env().await;
    env.fork(TRIAL).await;
    let old_policy = serde_json::to_value(ElasticPolicyV1::default()).unwrap();
    let new_policy = serde_json::to_value(policy_focus_one()).unwrap();
    let mut third = policy_focus_one();
    third.max_focus_actions = 3;
    let third = serde_json::to_value(third).unwrap();
    for (world, other_streams_policy) in [
        ("trial-1-new", old_policy.clone()),
        ("trial-1-old", new_policy.clone()),
    ] {
        let cases: Vec<(&str, Tamper)> = vec![
            (
                "the other stream's policy",
                Box::new(move |r| r["payload"]["policy"] = other_streams_policy.clone()),
            ),
            (
                "a third policy",
                Box::new({
                    let third = third.clone();
                    move |r| r["payload"]["policy"] = third.clone()
                }),
            ),
        ];
        assert_tamper_is_conflict(&env, WORLD, world, cases).await;
    }
    // A forged scheduling state is not a registration fact and is not checked
    // here: the trial only reports what the stored worlds hold.
    let original = raw_record(&env.store, WORLD, "trial-1-new").await;
    let mut moved = original.clone();
    moved["payload"]["decision_round"] = json!(0);
    moved["payload"]["state"] = json!("sealed");
    put_raw_record(&env.store, WORLD, "trial-1-new", &moved).await;
    env.view(TRIAL).await.unwrap();
    put_raw_record(&env.store, WORLD, "trial-1-new", &original).await;

    // Replacing a world by the other stream's world wholesale is detected too.
    let old_world = raw_record(&env.store, WORLD, "trial-1-old").await;
    let mut as_new = old_world.clone();
    as_new["id"] = json!(storage_id(WORLD, "trial-1-new"));
    as_new["payload"]["id"] = json!("trial-1-new");
    put_raw_record(&env.store, WORLD, "trial-1-new", &as_new).await;
    assert!(matches!(env.view(TRIAL).await, Err(Error::Conflict(_))));
    put_raw_record(&env.store, WORLD, "trial-1-new", &original).await;
    env.view(TRIAL).await.unwrap();

    // A world or the record that is missing is not found, not a verified trial.
    delete_raw_record(&env.store, WORLD, "trial-1-new").await;
    assert!(matches!(env.view(TRIAL).await, Err(Error::NotFound)));
    put_raw_record(&env.store, WORLD, "trial-1-new", &original).await;
    assert!(matches!(
        env.view("trial-unknown").await,
        Err(Error::NotFound)
    ));
}

// ---------------------------------------------------------------------------
// 10. The derived status (plan §5.7: no successor call is only a candidate change)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_trial_without_a_verified_usage_record_is_only_a_candidate_change() {
    let env = env().await;
    env.fork(TRIAL).await;
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(view.status(), MetaTrialStatus::CandidateOnly);
    assert_eq!(
        serde_json::to_value(&view).unwrap()["status"],
        "candidate_only"
    );
    assert_eq!(view.old_stream().verified_usage(), 0);
    assert_eq!(view.new_stream().verified_usage(), 0);

    // Pure decisions, status reads and a registered world are not usage.
    env.coordinator.decide_next("trial-1-old").await.unwrap();
    env.coordinator.decide_next("trial-1-new").await.unwrap();
    env.coordinator.decision_view("trial-1-new").await.unwrap();
    assert_eq!(
        env.view(TRIAL).await.unwrap().status(),
        MetaTrialStatus::CandidateOnly
    );

    // A dispatch that was claimed and never observed is not usage either: it
    // is not proven that a call was made. (The status counts only what
    // `verified_mechanism_usage` yields.)
    env.fork("trial-claimed").await;
    let claimed_world = stream_world_id("trial-claimed", MetaStream::New);
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
            env.fixture.request(
                &claimed_world,
                1,
                &stream_request_id("trial-claimed", MetaStream::New, 1),
            ),
        ) => panic!("the hanging model must not answer: {result:?}"),
        () = entered.notified() => {}
    }
    let world = raw_record(&env.store, WORLD, &claimed_world).await;
    let dispatches = id_list(&world, "dispatch_ids");
    assert_eq!(dispatches.len(), 1, "the claimed dispatch is listed");
    assert_eq!(
        raw_record(&env.store, DISPATCH, &dispatches[0]).await["payload"]["state"],
        "claimed"
    );
    assert_eq!(
        env.view("trial-claimed").await.unwrap().status(),
        MetaTrialStatus::CandidateOnly
    );

    // A real dispatch of the new stream makes it a trial with observed usage.
    env.step(TRIAL, MetaStream::New, 1).await;
    let view = env.view(TRIAL).await.unwrap();
    assert_eq!(view.status(), MetaTrialStatus::UsageObserved);
    assert_eq!(
        serde_json::to_value(&view).unwrap()["status"],
        "usage_observed"
    );
    assert_eq!(view.new_stream().verified_usage(), 1);
    assert_eq!(view.old_stream().verified_usage(), 0);

    // The status is derived on every read and cannot be set: the view cannot be
    // built from JSON and there is no stored status to rewrite.
    let record = raw_record(&env.store, TRIAL_KIND, TRIAL).await;
    assert!(record["payload"].get("status").is_none());
    assert!(!record.to_string().contains("usage_observed"));
    assert!(!record.to_string().contains("candidate_only"));
    // Another trial is judged by its own streams only.
    assert_eq!(
        env.view("trial-claimed").await.unwrap().status(),
        MetaTrialStatus::CandidateOnly
    );
}
