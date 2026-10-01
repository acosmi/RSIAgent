//! AG-048 (E09 PR-B step B4c; plan §5.3.1 "no_change is an explicit end point",
//! §5.8, §6.7.3 "DevelopmentSelection, unmatched edits, no change, rejected
//! candidates and failure modes enter OptimizationHistory", §7.2 "a legal direction
//! that failed, a compile/shape/implementation error, an external failure, a lack
//! of resources and a safety rejection are recorded apart, an unknown one as
//! unknown", §11 "a retained record keeps only redacted facts", V088.b, V089.c,
//! V090.a): the terminal of an optimization step is a closed typed class with a
//! fixed code, and no free-form text of an error, a model or a source reaches a
//! record.
//!
//! * every exit of a step (23 of them, one per class and per way of ending in an
//!   error) ends in the class and the fixed code the journal keeps: the `StepCompleted`
//!   fact, the terminal stage fact where the exit writes one, and the outcome the step
//!   returns. Text planted where an error, a model or a provider can put it (the words
//!   of a rejection, the message of a port, a runner or a journal that failed) and the
//!   sentences the engine used to write are searched for in the raw bytes of the
//!   database and its write-ahead log, and are not there. A model's own answer is the
//!   one thing the journal keeps whole (it is within the revocation closure, and
//!   recovering a paid step needs it), so the exit that echoes such an answer in an
//!   error is checked for the echo, and the answer is held only by the fact of its
//!   call;
//! * a rejection keeps its typed kind and nothing of the provider's words, in the
//!   `ResponseObserved` fact, in the journaled answer (`DispatchObserved`) and in the
//!   class; `request_suggestions` reports the kind, and a refused answer without
//!   echoing it;
//! * the exploration node and its dispatch fact record the class as the same typed
//!   value, told apart for a no change, an incumbent kept for either of its two
//!   reasons, a compile failure, a model rejection and two uncertain outcomes; the
//!   dispatch's reason and the step's outcome are its fixed code. The decision is what
//!   it was before the classes existed (these assertions also run on the baseline):
//!   every such node is a terminal failure or an uncertain usage that offers no
//!   successor, is no final candidate, spends the same budget and recovers nothing;
//! * a consolidation keeps only the fixed code in its run record, which survives the
//!   revocation of its sources, and none of the words planted in its port or runner,
//!   before the revocation and after the cleanup completed;
//! * records written before the classes (a node and a dispatch fact with a free-text
//!   reason and no class, a `StepCompleted` fact with only a reason) are read,
//!   registered and served as they were, and their text is not carried forward;
//! * a skill group reports the class of its step in memory, and no text.
//!
//! Not covered (and not claimed): the typed source of a repairable failure and the
//! `Recover` that follows it (AG-047), the optimization history in the request and
//! its signature (PR-C), a kept incumbent that can be deepened (undecided), and any
//! quality gain. Real SQLite store, fixture model and runner, no provider, zero
//! monetary cost. Fixtures are copied from `exploration_postpaid_v42.rs`,
//! `optimization.rs` and `consolidation_terminal_category_v42.rs`; those files are not
//! used here.
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    EditSuggestion, ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, ReflectionBatch, ReflectionBatchKind, SkillFailureDiagnosis,
    SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, SimulationContext};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationDispatchFact, ExplorationWorldV1,
    MechanismUsageState, PersistentCoordinator, PersistentSearchNode, RegisterWorldOutcome,
    RootOpportunity, WorldState,
};
use evo_engine::groups::{
    GroupOutcomeKind, SkillGroupJobRequest, SkillGroupSpec, run_skill_group_job,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepOutcome,
    OptimizationStepRequest, PairedTaskResult, StageFact, StageFactKind, StepErrorKind,
    StepTerminalClass, StoreOptimizationJournal, SuggestionOutcome, request_suggestions,
    run_optimization_step,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A string no code of the engine can produce. Wherever it is planted (the words of
/// a provider's rejection, the message of a port, a runner or a journal that
/// failed) it is text that came from outside the step, and the stores must not
/// keep it.
const MARK: &str = "ZQ48-marker-7c1e9a52";
const NAMESPACE: &str = "n";
const NODE_KIND: &str = "exploration_node_v1";
const WORLD_KIND: &str = "exploration_world_v1";
const DISPATCH_KIND: &str = "exploration_dispatch_v1";
const RUNS: [&str; 2] = ["run-failure", "run-success"];
const REGISTERED_ROOT_MICROS: u64 = 1_000;
const REGISTERED_RECOVERY_DISPATCHES: u8 = 2;
const FIRST_ROOT_COST: u64 = 10;
const SECOND_ROOT_COST: u64 = 25;
const SUCCESSOR_COST: u64 = 15;

// ---------------------------------------------------------------------------
// The trusted source closure, the world and the optimization step request
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

fn admin() -> Context {
    Context::new(NAMESPACE, "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NAMESPACE, "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new(NAMESPACE, "host", Role::Host).unwrap()
}

/// The two Host-issued trusted runs, their selection grant and one watermark bump.
/// `outcomes` are the outcomes the stored traces have: the step's own traces must
/// equal them.
async fn seeded_store(outcomes: [TraceOutcome; 2]) -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("step-terminal-classes.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let host = host();
    for ((id, family), outcome) in [(RUNS[0], "family-a"), (RUNS[1], "family-b")]
        .into_iter()
        .zip(outcomes)
    {
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
    (dir, path, store)
}

/// Everything an `OptimizationStepRequest` borrows, built once.
struct Fixture {
    outcomes: [TraceOutcome; 2],
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
    allow_rank_call: bool,
}

impl Fixture {
    fn new() -> Self {
        Self::with(
            [TraceOutcome::TaskFailure, TraceOutcome::Success],
            parent_skill(),
        )
    }

    fn with(outcomes: [TraceOutcome; 2], parent: SkillSnapshot) -> Self {
        let evidence = EvidenceSet::build(
            "evidence",
            RUNS.iter()
                .map(|run| EvidenceMember {
                    source_id: (*run).into(),
                    content_digest: hash(run.as_bytes()),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                })
                .collect(),
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
        let allowed: Vec<EvidenceRef> = RUNS
            .iter()
            .map(|run| EvidenceRef {
                id: (*run).into(),
                digest: hash(run.as_bytes()),
            })
            .collect();
        let edit_context = TrustedEditContext::new(
            NAMESPACE,
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
            outcomes,
            evidence,
            source_selection: SourceSelection {
                roots: vec![],
                run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            },
            bindings: [(RUNS[0], "family-a"), (RUNS[1], "family-b")]
                .into_iter()
                .map(|(run, family)| TrustedSourceBinding {
                    source_id: run.into(),
                    source_digest: hash(run.as_bytes()),
                    parent_family: family.into(),
                })
                .collect(),
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
            allow_rank_call: false,
        }
    }

    /// A request against the world's own parent skill and bundle (a `Widen`). `tag`
    /// keeps the request and idempotency ids apart.
    fn request(&self, world_id: &str, step: u32, tag: &str) -> OptimizationStepRequest<'_> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.bindings,
            traces: [(RUNS[0], "family-a"), (RUNS[1], "family-b")]
                .into_iter()
                .zip(self.outcomes)
                .map(|((run, family), outcome)| trace(run, family, outcome))
                .collect(),
            model_context: ModelRequestContext {
                request_id: format!("optimizer-{tag}"),
                namespace: NAMESPACE.into(),
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
                namespace: NAMESPACE.into(),
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
                namespace: NAMESPACE.into(),
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
            allow_rank_call: self.allow_rank_call,
        }
    }
}

/// The world the exploration tests register, with the root costs this file measures.
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

// ---------------------------------------------------------------------------
// What a port, a runner or a journal can be made to do
// ---------------------------------------------------------------------------

/// What the scripted model port answers.
#[derive(Clone)]
enum ModelScript {
    /// One bounded insertion per reflection batch.
    Edits,
    /// One insertion per batch whose preimage digest is not the text's: it cannot
    /// compile.
    BadPreimage,
    /// One replacement per batch that puts the text it replaces back: it compiles
    /// to no change.
    Identity,
    /// `n` suggestions per batch.
    Many(usize),
    /// A completed answer with exactly this output.
    Raw(String),
    /// A completed answer with no suggestion.
    Empty,
    /// The provider rejected the request, in these words.
    Reject {
        kind: ModelRejectionKind,
        reason: String,
    },
    /// The usage of the call is unknown.
    Uncertain,
    /// The port itself fails, with this message.
    Fail(String),
}

struct ScriptedModel {
    script: ModelScript,
    /// What the ranking call answers; `None` makes a ranking call fail.
    rank: Option<String>,
    calls: AtomicUsize,
}

impl ScriptedModel {
    fn new(script: ModelScript) -> Self {
        Self {
            script,
            rank: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn receipt(tag: &str) -> ModelExecutionReceipt {
    ModelExecutionReceipt {
        call_id: format!("fixture-call-{tag}"),
        dispatch_id: format!("fixture-dispatch-{tag}"),
        root_budget_id: "fixture-budget".into(),
        provider_request_id: format!("fixture-provider-{tag}"),
        usage_record_id: format!("fixture-usage-{tag}"),
        provenance: ModelExecutionProvenance::Fixture,
    }
}

fn completed(request: ModelRequest, output: String, tag: &str) -> ModelResponse {
    ModelResponse::Completed {
        request_id: request.request_id,
        response_id: format!("fixture-response-{tag}"),
        actual_model_digest: request.model_digest,
        input_digest: request.input_digest,
        output_digest: hash(output.as_bytes()),
        output,
        execution_receipt: receipt(tag),
    }
}

/// The suggestions of one reflection batch.
fn suggestions_for(request: &ModelRequest, script: &ModelScript) -> Result<Vec<EditSuggestion>> {
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
    let (id, batch_id, field, text) = match request.stage {
        ModelStage::ReflectFailure => (
            "repair-content",
            "reflection-failure",
            SkillTextField::Content,
            parent.content.clone(),
        ),
        ModelStage::ReflectSuccess => (
            "preserve-applicability",
            "reflection-success",
            SkillTextField::Applicability,
            parent.applicability.clone(),
        ),
        _ => return Err(Error::Invalid("unexpected fixture stage".into())),
    };
    let copies = match script {
        ModelScript::Many(count) => *count,
        _ => 1,
    };
    Ok((0..copies)
        .map(|copy| {
            let edit = match script {
                ModelScript::Identity => SkillTextEdit {
                    field,
                    start: 0,
                    end: text.len(),
                    expected_text_digest: hash(text.as_bytes()),
                    exact_anchor: None,
                    operation: TextEditOperation::Replace { text: text.clone() },
                },
                other => SkillTextEdit {
                    field,
                    start: text.len(),
                    end: text.len(),
                    expected_text_digest: if matches!(other, ModelScript::BadPreimage) {
                        hash(b"not the preimage")
                    } else {
                        hash(b"")
                    },
                    exact_anchor: None,
                    operation: TextEditOperation::Insert {
                        text: " [repair]".into(),
                    },
                },
            };
            EditSuggestion {
                id: if copies == 1 {
                    id.to_string()
                } else {
                    format!("{id}-{copy}")
                },
                hypothesis: "bounded deterministic fixture repair".into(),
                batch_ids: vec![batch_id.into()],
                support: vec![source.clone()],
                counterexamples: (request.stage == ModelStage::ReflectSuccess)
                    .then_some(source.clone())
                    .into_iter()
                    .collect(),
                dependencies: vec![source.clone()],
                edit,
            }
        })
        .collect())
}

#[async_trait]
impl ModelPort for ScriptedModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let tag = match request.stage {
            ModelStage::ReflectFailure => "failure",
            ModelStage::ReflectSuccess => "success",
            ModelStage::Rank => "rank",
            _ => "other",
        };
        if request.stage == ModelStage::Rank {
            let output = self
                .rank
                .clone()
                .ok_or_else(|| Error::Invalid("fixture has no ranking answer".into()))?;
            return Ok(completed(request, output, tag));
        }
        match &self.script {
            ModelScript::Edits
            | ModelScript::BadPreimage
            | ModelScript::Identity
            | ModelScript::Many(_) => {
                let items = suggestions_for(&request, &self.script)?;
                let output = serde_json::to_string(&items).map_err(|_| Error::Internal)?;
                Ok(completed(request, output, tag))
            }
            ModelScript::Raw(output) => Ok(completed(request, output.clone(), tag)),
            ModelScript::Empty => Ok(completed(request, "[]".into(), tag)),
            ModelScript::Reject { kind, reason } => {
                let dispatch = match kind {
                    ModelRejectionKind::Unauthorized
                    | ModelRejectionKind::BudgetUnavailable
                    | ModelRejectionKind::InvalidRequest
                    | ModelRejectionKind::CancelledBeforeDispatch => {
                        RejectedDispatch::NotDispatched
                    }
                    ModelRejectionKind::ProviderRejected
                    | ModelRejectionKind::CancelledAfterDispatch => RejectedDispatch::Dispatched {
                        receipt: receipt(tag),
                    },
                };
                Ok(ModelResponse::Rejected {
                    request_id: request.request_id,
                    kind: *kind,
                    reason: reason.clone(),
                    dispatch,
                })
            }
            ModelScript::Uncertain => Ok(ModelResponse::Uncertain {
                request_id: request.request_id,
                dispatch_id: format!("fixture-dispatch-{tag}"),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: None,
                usage_record_id: None,
            }),
            ModelScript::Fail(message) => Err(Error::Conflict(message.clone())),
        }
    }
}

/// What the scripted development runner reports for the one task of the manifest.
#[derive(Clone)]
enum RunnerScript {
    /// `(parent score, candidate score, parent passed, candidate passed)`.
    Scores(u32, u32, bool, bool),
    /// A candidate score outside `0..=1_000_000`: the report is refused by the
    /// development selection.
    ScoreOutOfRange,
    /// The runner itself fails, with this message.
    Fail(String),
}

struct ScriptedRunner {
    script: RunnerScript,
    calls: AtomicUsize,
}

impl ScriptedRunner {
    fn new(script: RunnerScript) -> Self {
        Self {
            script,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DevRunner for ScriptedRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (parent_score, candidate_score, parent_passed, candidate_passed) = match &self.script {
            RunnerScript::Scores(parent, candidate, parent_passed, candidate_passed) => {
                (*parent, *candidate, *parent_passed, *candidate_passed)
            }
            RunnerScript::ScoreOutOfRange => (500_000, 2_000_000, true, true),
            RunnerScript::Fail(message) => return Err(Error::Conflict(message.clone())),
        };
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results: vec![PairedTaskResult {
                task_id: "task".into(),
                parent_score_micros: parent_score,
                candidate_score_micros: candidate_score,
                parent_passed,
                candidate_passed,
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

/// How the journal the step writes to misbehaves.
#[derive(Clone)]
enum JournalFault {
    /// None: the store's own journal.
    None,
    /// The first commit of a fact of this kind and stage fails, with this message,
    /// and everything after it succeeds.
    FailOnce {
        kind: StageFactKind,
        stage: Option<OptimizationJournalStage>,
        message: String,
    },
    /// The first commit of a fact of this kind and stage fails, and so does every
    /// commit after it: a crash. The step is then run again on the store's own
    /// journal.
    Crash {
        kind: StageFactKind,
        stage: Option<OptimizationJournalStage>,
        message: String,
    },
    /// Somebody else already claimed every dispatch of this stage.
    ClaimedElsewhere { stage: OptimizationJournalStage },
}

struct FaultyJournal {
    inner: StoreOptimizationJournal,
    fault: JournalFault,
    tripped: AtomicBool,
}

impl FaultyJournal {
    fn new(inner: StoreOptimizationJournal, fault: JournalFault) -> Self {
        Self {
            inner,
            fault,
            tripped: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl OptimizationJournal for FaultyJournal {
    async fn commit(&self, fact: StageFact) -> Result<()> {
        match &self.fault {
            JournalFault::FailOnce {
                kind,
                stage,
                message,
            } if fact.kind == *kind
                && stage.is_none_or(|stage| stage == fact.stage)
                && !self.tripped.swap(true, Ordering::SeqCst) =>
            {
                return Err(Error::Conflict(message.clone()));
            }
            JournalFault::Crash {
                kind,
                stage,
                message,
            } if (fact.kind == *kind && stage.is_none_or(|stage| stage == fact.stage))
                || self.tripped.load(Ordering::SeqCst) =>
            {
                self.tripped.store(true, Ordering::SeqCst);
                return Err(Error::Conflict(message.clone()));
            }
            _ => {}
        }
        self.inner.commit(fact).await
    }

    async fn claim(&self, fact: StageFact) -> Result<bool> {
        if let JournalFault::ClaimedElsewhere { stage } = &self.fault
            && fact.kind == StageFactKind::DispatchPrepared
            && fact.stage == *stage
        {
            return Ok(false);
        }
        self.inner.claim(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(artifact_id).await
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
// Reading what the stores hold
// ---------------------------------------------------------------------------

/// The files of a SQLite database: the database and its write-ahead log.
fn database_files(path: &Path) -> [PathBuf; 2] {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
    ]
}

/// The database files that hold `needle` anywhere in their bytes: every table row, a
/// freed page and the log alike.
fn files_holding(path: &Path, needle: &str) -> Vec<String> {
    database_files(path)
        .into_iter()
        .filter(|file| {
            std::fs::read(file)
                .unwrap_or_default()
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        })
        .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// The `kind` (a stage fact's) or the schema (any other artifact's) of every stored
/// artifact whose body holds `needle`.
async fn artifacts_holding(store: &Store, needle: &str) -> BTreeSet<String> {
    let mut session = store.session().await.unwrap();
    let values = session.list::<Value>(&worker(), "artifact").await.unwrap();
    session.commit().await.unwrap();
    values
        .iter()
        .filter(|value| value.to_string().contains(needle))
        .map(|value| {
            value["kind"]
                .as_str()
                .or_else(|| value["schema_version"].as_str())
                .unwrap_or("unnamed")
                .to_string()
        })
        .collect()
}

/// Every stage fact of the optimization journal, in the order the store lists them.
async fn stage_facts(store: &Store) -> Vec<StageFact> {
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
        .collect()
}

fn fact_of(facts: &[StageFact], kind: StageFactKind) -> Vec<&StageFact> {
    facts.iter().filter(|fact| fact.kind == kind).collect()
}

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

async fn payload(store: &Store, record_kind: &str, id: &str) -> Value {
    raw_record(store, record_kind, id).await["payload"].clone()
}

// ---------------------------------------------------------------------------
// The environment: a store, its journal, the coordinator and the management surface
// ---------------------------------------------------------------------------

struct Env {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    dispatcher: ManagementDispatcher,
    fixture: Fixture,
}

impl Env {
    async fn new() -> Self {
        Self::with(Fixture::new()).await
    }

    async fn with(fixture: Fixture) -> Self {
        let (dir, path, store) = seeded_store(fixture.outcomes).await;
        Self {
            coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
            journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            dispatcher: ManagementDispatcher::new(store.clone(), vec![admin()]).unwrap(),
            fixture,
            path,
            store,
            _dir: dir,
        }
    }

    async fn register(&self, world: &ExplorationWorldV1) -> Result<RegisterWorldOutcome> {
        self.coordinator
            .register_world_idempotent(world.clone())
            .await
    }

    /// One real dispatch through `run_next`, the result as it is.
    async fn run(
        &self,
        model: &dyn ModelPort,
        runner: &dyn DevRunner,
        journal: &dyn OptimizationJournal,
        world_id: &str,
        step: u32,
        tag: &str,
    ) -> Result<CoordinatorStepResult> {
        self.coordinator
            .run_next(
                Some(model),
                Some(runner),
                Some(journal),
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

    async fn start_job(&self, request_key: &str, world: &ExplorationWorldV1) -> ManagementJob {
        let queued = self
            .dispatcher
            .submit(
                &admin(),
                "exploration.start",
                json!({
                    "schema_version": "rsia.management.exploration_start.v1",
                    "request_key": request_key,
                    "world": serde_json::to_value(world).unwrap(),
                }),
            )
            .await
            .unwrap();
        for _ in 0..400 {
            let job = self.dispatcher.status(&admin(), &queued.id).await.unwrap();
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
        panic!("the management job never ended");
    }

    async fn world(&self, world_id: &str) -> Value {
        payload(&self.store, WORLD_KIND, world_id).await
    }
}

fn dispatched_seqs(action: &BatchActionV1) -> Vec<u32> {
    match action {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs.clone(),
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

/// `status` on the job holds and serves the stored result unchanged.
async fn expect_status_ok(env: &Env, job: &ManagementJob, when: &str) {
    match env.dispatcher.status(&admin(), &job.id).await {
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

fn expect_already_registered(result: Result<RegisterWorldOutcome>, what: &str) {
    match result {
        Ok(RegisterWorldOutcome::AlreadyRegistered) => {}
        other => panic!("{what}: expected AlreadyRegistered, got {other:?}"),
    }
}

/// The findings of a table-driven test: every case is run and every deviation is
/// reported, not only the first.
#[derive(Default)]
struct Findings(Vec<String>);

impl Findings {
    fn check(&mut self, ok: bool, message: impl FnOnce() -> String) {
        if !ok {
            self.0.push(message());
        }
    }

    fn eq(&mut self, name: &str, what: &str, got: &Value, want: &Value) {
        if got != want {
            self.0
                .push(format!("{name}: {what}: got {got}, expected {want}"));
        }
    }

    fn done(self) {
        assert!(
            self.0.is_empty(),
            "{} deviation(s):\n{}",
            self.0.len(),
            self.0.join("\n")
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Every exit of a step ends in a fixed code and a typed class
// ---------------------------------------------------------------------------

/// The kind of terminal outcome of a step, as its stored status names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    NoChange,
    Rejected,
    Uncertain,
}

impl Kind {
    fn status(self) -> &'static str {
        match self {
            Kind::NoChange => "no_change",
            Kind::Rejected => "rejected",
            Kind::Uncertain => "uncertain",
        }
    }
}

/// The class an outcome that is no candidate carries.
fn class_of(outcome: &OptimizationStepOutcome) -> Option<StepTerminalClass> {
    match outcome {
        OptimizationStepOutcome::NoChange { class }
        | OptimizationStepOutcome::Rejected { class }
        | OptimizationStepOutcome::Uncertain { class } => Some(*class),
        OptimizationStepOutcome::Candidate { .. } => None,
    }
}

fn kind_of(outcome: &OptimizationStepOutcome) -> Option<Kind> {
    match outcome {
        OptimizationStepOutcome::NoChange { .. } => Some(Kind::NoChange),
        OptimizationStepOutcome::Rejected { .. } => Some(Kind::Rejected),
        OptimizationStepOutcome::Uncertain { .. } => Some(Kind::Uncertain),
        OptimizationStepOutcome::Candidate { .. } => None,
    }
}

/// How a step is driven to an exit.
struct Setup {
    outcomes: [TraceOutcome; 2],
    parent: SkillSnapshot,
    allow_excerpts: bool,
    allow_rank_call: bool,
    model: ModelScript,
    /// What the ranking call answers (the step must have been allowed one).
    rank: Option<String>,
    runner: RunnerScript,
    fault: JournalFault,
    /// Changes the request after it is built: what a request that is not as the
    /// journal's authority has it looks like.
    mutate: Option<fn(&mut OptimizationStepRequest<'_>)>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            outcomes: [TraceOutcome::TaskFailure, TraceOutcome::Success],
            parent: parent_skill(),
            allow_excerpts: true,
            allow_rank_call: false,
            model: ModelScript::Edits,
            rank: None,
            runner: RunnerScript::Scores(500_000, 900_000, true, true),
            fault: JournalFault::None,
            mutate: None,
        }
    }
}

/// What a driven step left behind.
struct Driven {
    env: Env,
    outcome: Result<OptimizationStepOutcome>,
    model_calls: usize,
    runner_calls: usize,
}

/// The episode a driven step belongs to.
const EPISODE: &str = "episode-a";

impl Setup {
    /// Runs the step once (twice after a crash) and leaves the stores for the
    /// caller to read.
    async fn drive(self) -> Driven {
        let mut fixture = Fixture::with(self.outcomes, self.parent);
        fixture.allow_rank_call = self.allow_rank_call;
        fixture.source_selection.allow_model_excerpts = self.allow_excerpts;
        let env = Env::with(fixture).await;
        let mut model = ScriptedModel::new(self.model);
        model.rank = self.rank;
        let runner = ScriptedRunner::new(self.runner);
        let faulty = FaultyJournal::new(env.journal.clone(), self.fault.clone());
        let request = || {
            let mut request = env.fixture.request(EPISODE, 1, "step");
            if let Some(mutate) = self.mutate {
                mutate(&mut request);
            }
            request
        };
        let first =
            run_optimization_step(Some(&model), Some(&runner), Some(&faulty), request()).await;
        let outcome = if matches!(self.fault, JournalFault::Crash { .. }) {
            assert!(first.is_err(), "the crashed journal fails the step");
            run_optimization_step(Some(&model), Some(&runner), Some(&env.journal), request()).await
        } else {
            first
        };
        Driven {
            outcome,
            model_calls: model.calls(),
            runner_calls: runner.calls(),
            env,
        }
    }
}

/// What a step that ended without a candidate must have left.
struct Expect {
    kind: Kind,
    /// The fixed code every record keeps of the outcome.
    code: String,
    /// The typed class stored next to it, as it is serialized.
    class: Value,
    /// The class the outcome of the step carries, as the type.
    typed: Option<StepTerminalClass>,
    /// The terminal stage fact the exit writes, if it writes one.
    terminal: Option<StageFactKind>,
    /// Text no stored byte may contain.
    forbidden: Vec<String>,
    /// Text the journal keeps by design, as the model's own answer or the step's own
    /// input, and the kinds of the facts that may hold it. No other artifact may.
    journaled: Vec<(String, Vec<&'static str>)>,
}

impl Expect {
    fn new(kind: Kind, code: &str, class: Value) -> Self {
        Self {
            kind,
            code: code.into(),
            class,
            typed: None,
            terminal: None,
            forbidden: vec![],
            journaled: vec![],
        }
    }

    fn journaled_only_in(mut self, text: &str, kinds: &[&'static str]) -> Self {
        self.journaled.push((text.into(), kinds.to_vec()));
        self
    }

    fn is(mut self, class: StepTerminalClass) -> Self {
        self.typed = Some(class);
        self
    }

    fn terminal(mut self, kind: StageFactKind) -> Self {
        self.terminal = Some(kind);
        self
    }

    fn forbidding(mut self, texts: &[&str]) -> Self {
        self.forbidden
            .extend(texts.iter().map(|text| (*text).into()));
        self
    }
}

/// Checks everything a driven step has to have left for its exit.
async fn check_exit(name: &str, driven: &Driven, expect: &Expect, findings: &mut Findings) {
    let outcome = match &driven.outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            findings
                .0
                .push(format!("{name}: the step ended in {error:?}"));
            return;
        }
    };
    findings.check(kind_of(outcome) == Some(expect.kind), || {
        format!("{name}: outcome {outcome:?}, expected {:?}", expect.kind)
    });
    if let Some(typed) = expect.typed {
        findings.check(class_of(outcome) == Some(typed), || {
            format!("{name}: class {:?}, expected {typed:?}", class_of(outcome))
        });
        // The serialized form is the typed class as it stands, and its code is the one
        // every record keeps.
        findings.eq(
            name,
            "serialized class",
            &serde_json::to_value(typed).unwrap(),
            &expect.class,
        );
        findings.check(typed.code() == expect.code, || {
            format!("{name}: code {}, expected {}", typed.code(), expect.code)
        });
    }
    let facts = stage_facts(&driven.env.store).await;
    let completed = fact_of(&facts, StageFactKind::StepCompleted);
    if completed.len() != 1 {
        findings
            .0
            .push(format!("{name}: {} StepCompleted facts", completed.len()));
        return;
    }
    let stored = &completed[0].payload;
    findings.eq(
        name,
        "status",
        &stored["status"],
        &json!(expect.kind.status()),
    );
    findings.eq(name, "reason", &stored["reason"], &json!(expect.code));
    findings.eq(name, "class", &stored["class"], &expect.class);
    if let Some(kind) = expect.terminal {
        let terminals = fact_of(&facts, kind);
        findings.check(terminals.len() == 1, || {
            format!("{name}: {} {kind:?} facts", terminals.len())
        });
        if let Some(fact) = terminals.first() {
            findings.eq(
                name,
                "terminal fact payload",
                &fact.payload,
                &json!({"reason": expect.code, "class": expect.class}),
            );
        }
    }
    for needle in &expect.forbidden {
        let holding = files_holding(&driven.env.path, needle);
        findings.check(holding.is_empty(), || {
            format!("{name}: the database still holds {needle:?} ({holding:?})")
        });
    }
    for (needle, kinds) in &expect.journaled {
        let holders = artifacts_holding(&driven.env.store, needle).await;
        let allowed: BTreeSet<String> = kinds.iter().map(|kind| (*kind).to_string()).collect();
        findings.check(holders == allowed, || {
            format!("{name}: {needle:?} is held by {holders:?}, expected only {allowed:?}")
        });
    }
}

fn class(tag: &str) -> Value {
    json!({ "class": tag })
}

/// Drives `setup` to its exit and pairs the result with what it must have left.
async fn run(name: &'static str, setup: Setup, expect: Expect) -> (&'static str, Driven, Expect) {
    (name, setup.drive().await, expect)
}

#[tokio::test]
async fn every_exit_of_a_step_ends_in_a_fixed_code_and_a_typed_class() {
    let mut findings = Findings::default();
    let mut cases = Vec::new();

    // --- no change ---------------------------------------------------------
    cases.push(
        run(
            "no eligible reflection batch",
            Setup {
                outcomes: [
                    TraceOutcome::EnvironmentFailure,
                    TraceOutcome::EnvironmentFailure,
                ],
                ..Setup::default()
            },
            Expect::new(
                Kind::NoChange,
                "no_eligible_reflection_batch",
                class("no_eligible_reflection_batch"),
            )
            .is(StepTerminalClass::NoEligibleReflectionBatch)
            .terminal(StageFactKind::TerminalNoChange)
            .forbidding(&["no eligible development reflection batch"]),
        )
        .await,
    );
    cases.push(
        run(
            "the model proposes nothing",
            Setup {
                model: ModelScript::Empty,
                ..Setup::default()
            },
            Expect::new(
                Kind::NoChange,
                "no_edit_suggestions",
                class("no_edit_suggestions"),
            )
            .is(StepTerminalClass::NoEditSuggestions)
            .terminal(StageFactKind::TerminalNoChange)
            .forbidding(&["model returned no edit suggestions"]),
        )
        .await,
    );
    cases.push(
        run(
            "the edit changes nothing",
            Setup {
                model: ModelScript::Identity,
                ..Setup::default()
            },
            Expect::new(
                Kind::NoChange,
                "edit_produced_no_change",
                class("edit_produced_no_change"),
            )
            .is(StepTerminalClass::EditProducedNoChange)
            .terminal(StageFactKind::TerminalNoChange)
            .forbidding(&["atomic skill edit produced no change"]),
        )
        .await,
    );

    // --- the incumbent is kept ----------------------------------------------
    cases.push(
        run(
            "the candidate does not improve",
            Setup {
                runner: RunnerScript::Scores(500_000, 500_000, true, true),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "keep_incumbent_not_improved",
                json!({
                    "class": "keep_incumbent_not_improved",
                    "parent_total_micros": 500_000,
                    "candidate_total_micros": 500_000,
                }),
            )
            .is(StepTerminalClass::KeepIncumbentNotImproved {
                parent_total_micros: 500_000,
                candidate_total_micros: 500_000,
            })
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["candidate did not strictly improve"]),
        )
        .await,
    );
    cases.push(
        run(
            "the candidate scores higher and breaks a passing task",
            Setup {
                runner: RunnerScript::Scores(400_000, 900_000, true, false),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "keep_incumbent_retention_broken",
                json!({
                    "class": "keep_incumbent_retention_broken",
                    "parent_total_micros": 400_000,
                    "candidate_total_micros": 900_000,
                }),
            )
            .is(StepTerminalClass::KeepIncumbentRetentionBroken {
                parent_total_micros: 400_000,
                candidate_total_micros: 900_000,
            })
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["candidate did not strictly improve"]),
        )
        .await,
    );

    // --- rejected -----------------------------------------------------------
    cases.push(
        run(
            "the source grant allows no model excerpt",
            Setup {
                allow_excerpts: false,
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "grant_unavailable",
                class("grant_unavailable"),
            )
            .is(StepTerminalClass::GrantUnavailable)
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["model excerpt grant is absent or incompatible"]),
        )
        .await,
    );
    cases.push(
        run(
            "the provider rejects the request",
            Setup {
                model: ModelScript::Reject {
                    kind: ModelRejectionKind::ProviderRejected,
                    reason: format!("provider words {MARK}"),
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "model_rejected_provider_rejected",
                json!({"class": "model_rejected", "kind": "provider_rejected"}),
            )
            .is(StepTerminalClass::ModelRejected {
                kind: ModelRejectionKind::ProviderRejected,
            })
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&[MARK]),
        )
        .await,
    );
    cases.push(
        run(
            "the model answers with something that is no suggestion list",
            Setup {
                model: ModelScript::Raw(format!(r#"[{{"unknown-{MARK}":1}}]"#)),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "suggestion_shape_invalid",
                class("suggestion_shape_invalid"),
            )
            .is(StepTerminalClass::SuggestionShapeInvalid)
            .forbidding(&["invalid optimizer suggestions", "unknown field"])
            // What the model answered is its own work product and the journal keeps it
            // whole, in the fact of the call it was the answer to (it is within the
            // revocation closure); the error that echoed it is kept nowhere.
            .journaled_only_in(MARK, &["dispatch_observed"]),
        )
        .await,
    );
    cases.push(
        run(
            "the pool is over four and ranking was not preregistered",
            Setup {
                model: ModelScript::Many(3),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "suggestion_pool_unranked",
                class("suggestion_pool_unranked"),
            )
            .is(StepTerminalClass::SuggestionPoolUnranked)
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["suggestion pool exceeds four"]),
        )
        .await,
    );
    cases.push(
        run(
            "the atomic edit does not compile",
            Setup {
                model: ModelScript::BadPreimage,
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "edit_compile_failed",
                class("edit_compile_failed"),
            )
            .is(StepTerminalClass::EditCompileFailed)
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["atomic skill edit rejected", "preimage_digest_mismatch"]),
        )
        .await,
    );
    cases.push(
        run(
            "the complete bundle does not compile",
            Setup {
                parent: SkillSnapshot {
                    required_capabilities: vec!["capability-a".into()],
                    ..parent_skill()
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "bundle_compile_failed",
                class("bundle_compile_failed"),
            )
            .is(StepTerminalClass::BundleCompileFailed)
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["complete bundle compilation failed"]),
        )
        .await,
    );
    cases.push(
        run(
            "the development report is refused",
            Setup {
                runner: RunnerScript::ScoreOutOfRange,
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "development_report_rejected",
                class("development_report_rejected"),
            )
            .is(StepTerminalClass::DevelopmentReportRejected)
            .terminal(StageFactKind::TerminalRejected)
            .forbidding(&["development report rejected", "development score micros"]),
        )
        .await,
    );
    cases.push(
        run(
            "the journal refuses a fact",
            Setup {
                fault: JournalFault::FailOnce {
                    kind: StageFactKind::EditCompiled,
                    stage: None,
                    message: format!("journal words {MARK}"),
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "step_error_conflict",
                json!({"class": "step_error", "error": "conflict"}),
            )
            .is(StepTerminalClass::StepError {
                error: StepErrorKind::Conflict,
            })
            .forbidding(&[MARK]),
        )
        .await,
    );

    cases.push(
        run(
            "the ranking selects ids that are not in the pool",
            Setup {
                model: ModelScript::Many(3),
                allow_rank_call: true,
                rank: Some(r#"["invented-id"]"#.into()),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "step_error_invalid",
                json!({"class": "step_error", "error": "invalid"}),
            )
            .is(StepTerminalClass::StepError {
                error: StepErrorKind::Invalid,
            })
            .forbidding(&["ranking selected invalid suggestion ids"]),
        )
        .await,
    );
    cases.push(
        run(
            "the ranking answers with something that is no id list",
            Setup {
                model: ModelScript::Many(3),
                allow_rank_call: true,
                rank: Some(format!("not a list {MARK}")),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "step_error_invalid",
                json!({"class": "step_error", "error": "invalid"}),
            )
            .is(StepTerminalClass::StepError {
                error: StepErrorKind::Invalid,
            })
            .forbidding(&["invalid ranking response", "expected value at line"]),
        )
        .await,
    );
    cases.push(
        run(
            "a trace is given twice",
            Setup {
                mutate: Some(|request| request.traces.push(request.traces[0].clone())),
                ..Setup::default()
            },
            Expect::new(
                Kind::Rejected,
                "step_error_conflict",
                json!({"class": "step_error", "error": "conflict"}),
            )
            .is(StepTerminalClass::StepError {
                error: StepErrorKind::Conflict,
            })
            .forbidding(&["duplicate optimization trace run"]),
        )
        .await,
    );

    // --- uncertain ----------------------------------------------------------
    cases.push(
        run(
            "the usage of the model call is unknown",
            Setup {
                model: ModelScript::Uncertain,
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "model_usage_unknown",
                class("model_usage_unknown"),
            )
            .is(StepTerminalClass::ModelUsageUnknown)
            .terminal(StageFactKind::TerminalUncertain)
            .forbidding(&[
                "model usage remains unknown",
                "model dispatch or usage remains uncertain",
            ]),
        )
        .await,
    );
    cases.push(
        run(
            "the model transport fails",
            Setup {
                model: ModelScript::Fail(format!("transport words {MARK}")),
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "model_transport_outcome_unknown",
                class("model_transport_outcome_unknown"),
            )
            .is(StepTerminalClass::ModelTransportOutcomeUnknown)
            .forbidding(&[MARK, "model transport outcome unknown"]),
        )
        .await,
    );
    cases.push(
        run(
            "the development runner fails",
            Setup {
                runner: RunnerScript::Fail(format!("runner words {MARK}")),
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "development_execution_outcome_unknown",
                class("development_execution_outcome_unknown"),
            )
            .is(StepTerminalClass::DevelopmentExecutionOutcomeUnknown)
            .forbidding(&[MARK, "development execution outcome unknown"]),
        )
        .await,
    );
    cases.push(
        run(
            "a prepared model dispatch has no durable response",
            Setup {
                fault: JournalFault::Crash {
                    kind: StageFactKind::DispatchObserved,
                    stage: Some(OptimizationJournalStage::ReflectFailure),
                    message: format!("journal words {MARK}"),
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "model_dispatch_unrecorded",
                class("model_dispatch_unrecorded"),
            )
            .is(StepTerminalClass::ModelDispatchUnrecorded)
            .forbidding(&[MARK, "prepared model dispatch has no durable response"]),
        )
        .await,
    );
    cases.push(
        run(
            "another worker holds the model dispatch",
            Setup {
                fault: JournalFault::ClaimedElsewhere {
                    stage: OptimizationJournalStage::ReflectFailure,
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "model_dispatch_concurrent",
                class("model_dispatch_concurrent"),
            )
            .is(StepTerminalClass::ModelDispatchConcurrent)
            .forbidding(&["concurrent model dispatch remains uncertain"]),
        )
        .await,
    );
    cases.push(
        run(
            "a prepared development execution has no durable result",
            Setup {
                fault: JournalFault::Crash {
                    kind: StageFactKind::DispatchObserved,
                    stage: Some(OptimizationJournalStage::Development),
                    message: format!("journal words {MARK}"),
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "development_execution_unrecorded",
                class("development_execution_unrecorded"),
            )
            .is(StepTerminalClass::DevelopmentExecutionUnrecorded)
            .forbidding(&[MARK, "prepared development execution has no durable result"]),
        )
        .await,
    );
    cases.push(
        run(
            "another worker holds the development execution",
            Setup {
                fault: JournalFault::ClaimedElsewhere {
                    stage: OptimizationJournalStage::Development,
                },
                ..Setup::default()
            },
            Expect::new(
                Kind::Uncertain,
                "development_execution_concurrent",
                class("development_execution_concurrent"),
            )
            .is(StepTerminalClass::DevelopmentExecutionConcurrent)
            .forbidding(&["concurrent development execution remains uncertain"]),
        )
        .await,
    );

    for (name, driven, expect) in &cases {
        check_exit(name, driven, expect, &mut findings).await;
    }
    findings.done();
}

#[tokio::test]
async fn a_rejected_model_response_keeps_its_typed_kind_and_never_its_words() {
    let mut findings = Findings::default();
    for (kind, tag) in [
        (ModelRejectionKind::Unauthorized, "unauthorized"),
        (ModelRejectionKind::BudgetUnavailable, "budget_unavailable"),
        (ModelRejectionKind::InvalidRequest, "invalid_request"),
        (ModelRejectionKind::ProviderRejected, "provider_rejected"),
        (
            ModelRejectionKind::CancelledBeforeDispatch,
            "cancelled_before_dispatch",
        ),
        (
            ModelRejectionKind::CancelledAfterDispatch,
            "cancelled_after_dispatch",
        ),
    ] {
        let name = format!("rejected as {tag}");
        let driven = Setup {
            model: ModelScript::Reject {
                kind,
                reason: format!("words of the provider {MARK}-{tag}"),
            },
            ..Setup::default()
        }
        .drive()
        .await;
        let expect = Expect::new(
            Kind::Rejected,
            &format!("model_rejected_{tag}"),
            json!({"class": "model_rejected", "kind": tag}),
        )
        .is(StepTerminalClass::ModelRejected { kind })
        .terminal(StageFactKind::TerminalRejected)
        .forbidding(&[MARK]);
        check_exit(&name, &driven, &expect, &mut findings).await;
        // Both observations of the answer keep its kind, as the typed field it is.
        let facts = stage_facts(&driven.env.store).await;
        let observed = fact_of(&facts, StageFactKind::ResponseObserved);
        findings.check(observed.len() == 1, || {
            format!("{name}: {} ResponseObserved facts", observed.len())
        });
        if let Some(fact) = observed.first() {
            findings.eq(
                &name,
                "ResponseObserved kind",
                &fact.payload["Rejected"]["kind"],
                &json!(tag),
            );
        }
        let journaled = fact_of(&facts, StageFactKind::DispatchObserved);
        findings.check(journaled.len() == 1, || {
            format!("{name}: {} DispatchObserved facts", journaled.len())
        });
        if let Some(fact) = journaled.first() {
            findings.eq(&name, "journaled kind", &fact.payload["kind"], &json!(tag));
            findings.eq(
                &name,
                "journaled words",
                &fact.payload["reason"],
                &json!(tag),
            );
        }
    }
    findings.done();
}

#[tokio::test]
async fn a_finished_step_answers_again_from_its_journal_without_reaching_a_port() {
    for (name, setup) in [
        (
            "no change",
            Setup {
                model: ModelScript::Empty,
                ..Setup::default()
            },
        ),
        (
            "kept incumbent",
            Setup {
                runner: RunnerScript::Scores(500_000, 500_000, true, true),
                ..Setup::default()
            },
        ),
        (
            "rejected by the provider",
            Setup {
                model: ModelScript::Reject {
                    kind: ModelRejectionKind::ProviderRejected,
                    reason: format!("words {MARK}"),
                },
                ..Setup::default()
            },
        ),
        (
            "uncertain transport",
            Setup {
                model: ModelScript::Fail(format!("words {MARK}")),
                ..Setup::default()
            },
        ),
    ] {
        let driven = setup.drive().await;
        let first = kind_of(driven.outcome.as_ref().unwrap());
        let first_class = class_of(driven.outcome.as_ref().unwrap());
        assert!(first.is_some() && first_class.is_some(), "{name}");
        let (model_calls, runner_calls) = (driven.model_calls, driven.runner_calls);
        let model = ScriptedModel::new(ModelScript::Edits);
        let runner = ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true));
        let again = run_optimization_step(
            Some(&model),
            Some(&runner),
            Some(&driven.env.journal),
            driven.env.fixture.request(EPISODE, 1, "step"),
        )
        .await
        .unwrap();
        assert_eq!(kind_of(&again), first, "{name}");
        // The class is the one the journal stored, with its typed fields.
        assert_eq!(class_of(&again), first_class, "{name}");
        assert_eq!((model.calls(), runner.calls()), (0, 0), "{name}");
        assert!(
            model_calls + runner_calls > 0 || name == "no change",
            "{name}"
        );
    }
}

#[tokio::test]
async fn an_accepted_candidate_is_recorded_as_it_always_was() {
    let driven = Setup::default().drive().await;
    let outcome = driven.outcome.as_ref().unwrap();
    assert!(
        matches!(outcome, OptimizationStepOutcome::Candidate { .. }),
        "{outcome:?}"
    );
    let facts = stage_facts(&driven.env.store).await;
    let mut kinds: Vec<String> = facts
        .iter()
        .map(|fact| {
            serde_json::to_value(fact.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [
            "development_observed",
            "development_request_prepared",
            "dispatch_observed",
            "dispatch_observed",
            "dispatch_observed",
            "dispatch_prepared",
            "dispatch_prepared",
            "dispatch_prepared",
            "edit_compiled",
            "request_prepared",
            "request_prepared",
            "response_observed",
            "response_observed",
            "step_completed",
            "step_prepared",
            "terminal_candidate",
        ]
    );
    let completed = fact_of(&facts, StageFactKind::StepCompleted);
    let stored = completed[0].payload.as_object().unwrap();
    let mut keys: Vec<&str> = stored.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["bundle", "output", "patch", "report", "selection", "status"]
    );
    assert_eq!(stored["status"], "candidate");
    // A candidate has no terminal class, and its selection keeps the words it always
    // had (a fixed sentence of the engine, not text of an error or a model).
    assert_eq!(
        stored["selection"]["reason"],
        "strictly improved the same full development manifest and preserved passes"
    );
    assert_eq!(
        (
            stored["selection"]["parent_total_micros"].as_u64(),
            stored["selection"]["candidate_total_micros"].as_u64()
        ),
        (Some(500_000), Some(900_000))
    );
}

#[tokio::test]
async fn the_database_scan_finds_what_a_store_holds() {
    // The control of every "no stored byte contains" check above: the scan does see
    // a string that was stored, in the database or in its log.
    let env = Env::new().await;
    let mut session = env.store.session().await.unwrap();
    session
        .put(
            &worker(),
            "artifact",
            "planted",
            "worker",
            &json!({"words": MARK}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(!files_holding(&env.path, MARK).is_empty());
    assert!(files_holding(&env.path, "a string nobody stored 4d1f").is_empty());
}

// ---------------------------------------------------------------------------
// 2. The exploration node and its dispatch fact record the class apart
// ---------------------------------------------------------------------------

/// A model port that moves the source watermark while its call is in flight: what a
/// revocation or a new source does to the closure of a world that is being worked.
struct WatermarkBumpingModel {
    store: Store,
}

#[async_trait]
impl ModelPort for WatermarkBumpingModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut session = self.store.session().await?;
        session.bump_watermark(&host(), "e09-bumped").await?;
        session.commit().await?;
        ScriptedModel::new(ModelScript::Edits)
            .dispatch(request)
            .await
    }
}

/// One world, one step and what its node and dispatch fact must say of it.
struct Scenario {
    world: &'static str,
    model: ModelScript,
    runner: RunnerScript,
    fault: JournalFault,
    /// The fixed code of the outcome, which is the reason the fact stores.
    code: &'static str,
    /// The typed class stored on the node and on the dispatch fact, as it is serialized.
    class: Value,
    /// The same class as the type.
    typed: StepTerminalClass,
    /// Whether the dispatch is terminal as `uncertain` (and its node `usage_uncertain`)
    /// rather than as `observed` (and `hard_failure`).
    uncertain: bool,
    forbidden: Vec<String>,
}

fn scenarios() -> Vec<Scenario> {
    let plain = |world, model, runner, code, class: Value, typed| Scenario {
        world,
        model,
        runner,
        fault: JournalFault::None,
        code,
        class,
        typed,
        uncertain: false,
        forbidden: vec![],
    };
    let improving = RunnerScript::Scores(500_000, 900_000, true, true);
    vec![
        plain(
            "world-no-change",
            ModelScript::Empty,
            improving.clone(),
            "no_edit_suggestions",
            class("no_edit_suggestions"),
            StepTerminalClass::NoEditSuggestions,
        ),
        plain(
            "world-not-improved",
            ModelScript::Edits,
            RunnerScript::Scores(500_000, 500_000, true, true),
            "keep_incumbent_not_improved",
            json!({
                "class": "keep_incumbent_not_improved",
                "parent_total_micros": 500_000,
                "candidate_total_micros": 500_000,
            }),
            StepTerminalClass::KeepIncumbentNotImproved {
                parent_total_micros: 500_000,
                candidate_total_micros: 500_000,
            },
        ),
        plain(
            "world-retention-broken",
            ModelScript::Edits,
            RunnerScript::Scores(400_000, 900_000, true, false),
            "keep_incumbent_retention_broken",
            json!({
                "class": "keep_incumbent_retention_broken",
                "parent_total_micros": 400_000,
                "candidate_total_micros": 900_000,
            }),
            StepTerminalClass::KeepIncumbentRetentionBroken {
                parent_total_micros: 400_000,
                candidate_total_micros: 900_000,
            },
        ),
        plain(
            "world-compile-failed",
            ModelScript::BadPreimage,
            improving.clone(),
            "edit_compile_failed",
            class("edit_compile_failed"),
            StepTerminalClass::EditCompileFailed,
        ),
        Scenario {
            forbidden: vec![MARK.into()],
            ..plain(
                "world-model-rejected",
                ModelScript::Reject {
                    kind: ModelRejectionKind::ProviderRejected,
                    reason: format!("words of the provider {MARK}"),
                },
                improving.clone(),
                "model_rejected_provider_rejected",
                json!({"class": "model_rejected", "kind": "provider_rejected"}),
                StepTerminalClass::ModelRejected {
                    kind: ModelRejectionKind::ProviderRejected,
                },
            )
        },
        Scenario {
            uncertain: true,
            forbidden: vec![MARK.into()],
            ..plain(
                "world-transport-failed",
                ModelScript::Fail(format!("words of the transport {MARK}")),
                improving.clone(),
                "model_transport_outcome_unknown",
                class("model_transport_outcome_unknown"),
                StepTerminalClass::ModelTransportOutcomeUnknown,
            )
        },
        Scenario {
            uncertain: true,
            forbidden: vec![MARK.into()],
            fault: JournalFault::Crash {
                kind: StageFactKind::StepCompleted,
                stage: None,
                message: format!("words of the journal {MARK}"),
            },
            ..plain(
                "world-journal-crashed",
                ModelScript::Edits,
                improving,
                "optimization_dispatch_outcome_uncertain",
                class("optimization_dispatch_outcome_uncertain"),
                StepTerminalClass::OptimizationDispatchOutcomeUncertain,
            )
        },
    ]
}

/// What the decision of a world that took one non-candidate step has to be: nothing
/// of the step gives its node a successor, so only the other root is left, and once
/// that one is spent too the world stops.
async fn assert_decision_semantics(env: &Env, scenario: &Scenario, step: &CoordinatorStepResult) {
    let world = scenario.world;
    let name = world;
    let node_id = step.node_id.clone().unwrap();
    // Exactly the other root is legal, never a Deepen of the node.
    let next = env.coordinator.decide_next(world).await.unwrap();
    assert_eq!(dispatched_seqs(&next.action), vec![2], "{name}");
    // The node is no valid observed candidate, so it is no final candidate.
    let proposed = env
        .coordinator
        .final_candidate_request(world, &node_id)
        .await;
    assert!(
        matches!(proposed, Err(Error::Conflict(_))),
        "{name}: {proposed:?}"
    );
    // The budget is what the dispatch spent, and nothing is recovered.
    let stored = env.world(world).await;
    assert_eq!(
        (
            stored["remaining_root_micros"].as_u64().unwrap(),
            stored["remaining_recovery_dispatches"].as_u64().unwrap(),
            stored["decision_round"].as_u64().unwrap(),
        ),
        (REGISTERED_ROOT_MICROS - FIRST_ROOT_COST, 2, 1),
        "{name}"
    );
    let node = payload(&env.store, NODE_KIND, &node_id).await;
    assert_eq!(node["node"]["repair_failures_dispatched"], 0, "{name}");
    assert_eq!(node["node"]["depth"], 1, "{name}");
    assert_eq!(node["node"]["search_parent_seq"], Value::Null, "{name}");
    let usage = env
        .coordinator
        .verified_mechanism_usage(world)
        .await
        .unwrap();
    assert_eq!(usage.len(), 1, "{name}");
    assert_eq!(
        usage[0].dispatch_state(),
        if scenario.uncertain {
            MechanismUsageState::Uncertain
        } else {
            MechanismUsageState::Observed
        },
        "{name}"
    );
    // The second root is spent by a step that changes nothing, and then the world has
    // nothing left to offer: a kept incumbent, a refused edit or an uncertain node
    // never opened a successor.
    let second = env
        .run(
            &ScriptedModel::new(ModelScript::Empty),
            &ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true)),
            &env.journal,
            world,
            2,
            &format!("{world}-2"),
        )
        .await
        .unwrap();
    assert!(second.node_id.is_some(), "{name}: {second:?}");
    let after = env.coordinator.decide_next(world).await.unwrap();
    assert!(
        matches!(after.action, BatchActionV1::Stop { .. }),
        "{name}: {:?}",
        after.action
    );
}

#[tokio::test]
async fn the_node_and_the_dispatch_fact_of_each_terminal_class_are_recorded_apart() {
    let env = Env::new().await;
    let mut findings = Findings::default();
    let mut classes = Vec::new();
    let mut steps = Vec::new();
    for scenario in scenarios() {
        let world = world_for(scenario.world);
        env.register(&world).await.unwrap();
        let model = ScriptedModel::new(scenario.model.clone());
        let runner = ScriptedRunner::new(scenario.runner.clone());
        let journal = FaultyJournal::new(env.journal.clone(), scenario.fault.clone());
        let step = env
            .run(
                &model,
                &runner,
                &journal,
                scenario.world,
                1,
                &format!("{}-1", scenario.world),
            )
            .await
            .expect("a paid step never ends in an error: its claim is settled");
        let name = scenario.world;
        let node_id = step
            .node_id
            .clone()
            .expect("a settled dispatch names its node");
        let node = payload(&env.store, NODE_KIND, &node_id).await;
        let fact = payload(
            &env.store,
            DISPATCH_KIND,
            step.dispatch_id.as_deref().unwrap(),
        )
        .await;
        // What the step answered and what the fact stores is the one fixed code.
        findings.eq(
            name,
            "step outcome",
            &json!(step.outcome),
            &json!(scenario.code),
        );
        findings.eq(
            name,
            "fact reason",
            &fact["outcome_reason"],
            &json!(scenario.code),
        );
        // The class itself is on the node and on the fact, as the same typed value.
        findings.eq(name, "node class", &node["terminal_class"], &scenario.class);
        findings.eq(name, "fact class", &fact["terminal_class"], &scenario.class);
        // And both records read back as the typed class, whose code is the reason.
        let read_node: PersistentSearchNode = serde_json::from_value(node.clone()).unwrap();
        let read_fact: ExplorationDispatchFact = serde_json::from_value(fact.clone()).unwrap();
        findings.check(read_node.terminal_class == Some(scenario.typed), || {
            format!("{name}: node reads as {:?}", read_node.terminal_class)
        });
        findings.check(read_fact.terminal_class == Some(scenario.typed), || {
            format!("{name}: fact reads as {:?}", read_fact.terminal_class)
        });
        findings.check(scenario.typed.code() == scenario.code, || {
            format!(
                "{name}: the code of {:?} is {}",
                scenario.typed,
                scenario.typed.code()
            )
        });
        classes.push(node["terminal_class"].clone());
        // The node is what the decision has always taken it for.
        findings.eq(
            name,
            "node status",
            &node["node"]["status"],
            &json!({"status": if scenario.uncertain { "usage_uncertain" } else { "hard_failure" }}),
        );
        findings.eq(
            name,
            "fact state",
            &fact["state"],
            &json!(if scenario.uncertain {
                "uncertain"
            } else {
                "observed"
            }),
        );
        findings.eq(
            name,
            "node evidence",
            &node["evidence"],
            &json!("not_observed"),
        );
        findings.eq(
            name,
            "fact evidence",
            &fact["evidence"],
            &json!("not_observed"),
        );
        findings.check(
            node["candidate_bundle_digest"].is_null()
                && node["candidate_skill_digest"].is_null()
                && node["development_selection_digest"].is_null(),
            || format!("{name}: a node without a candidate names one: {node}"),
        );
        steps.push((scenario, step));
    }
    // The classes are told apart: no two of the seven records say the same.
    let distinct: BTreeSet<String> = classes.iter().map(Value::to_string).collect();
    findings.check(distinct.len() == classes.len(), || {
        format!("the node classes are not all different: {classes:?}")
    });
    // What no stored byte may contain.
    for (scenario, _) in &steps {
        for needle in &scenario.forbidden {
            let holding = files_holding(&env.path, needle);
            findings.check(holding.is_empty(), || {
                format!(
                    "{}: the database still holds {needle:?} ({holding:?})",
                    scenario.world
                )
            });
        }
    }
    // The decision of every world is what it was before the classes existed.
    for (scenario, step) in &steps {
        assert_decision_semantics(&env, scenario, step).await;
    }
    // A client that reconnects to a settled dispatch is answered with the same code.
    for (scenario, step) in &steps {
        let again = env
            .reconnect(scenario.world, 1, &format!("{}-1", scenario.world))
            .await
            .unwrap();
        assert_eq!(again.node_id, step.node_id, "{}", scenario.world);
        findings.eq(
            scenario.world,
            "reconnect outcome",
            &json!(again.outcome),
            &json!(scenario.code),
        );
    }
    findings.done();
}

#[tokio::test]
async fn a_world_whose_closure_moved_while_the_step_ran_ends_uncertain_with_a_fixed_code() {
    let env = Env::new().await;
    env.register(&world_for("world-closure")).await.unwrap();
    let model = WatermarkBumpingModel {
        store: env.store.clone(),
    };
    let runner = ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true));
    let step = env
        .run(
            &model,
            &runner,
            &env.journal,
            "world-closure",
            1,
            "closure-1",
        )
        .await
        .expect("a paid step never ends in an error: its claim is settled");
    let code = "exploration_source_closure_changed_after_dispatch";
    assert_eq!(step.outcome, code);
    let node = payload(&env.store, NODE_KIND, step.node_id.as_deref().unwrap()).await;
    let fact = payload(
        &env.store,
        DISPATCH_KIND,
        step.dispatch_id.as_deref().unwrap(),
    )
    .await;
    assert_eq!(fact["outcome_reason"], code);
    assert_eq!(fact["state"], "uncertain");
    assert_eq!(node["node"]["status"], json!({"status": "usage_uncertain"}));
    assert_eq!(node["terminal_class"], class(code));
    assert_eq!(fact["terminal_class"], class(code));
}

// ---------------------------------------------------------------------------
// 3. Records written before the classes existed read as they did
// ---------------------------------------------------------------------------

/// Rewrites a stored node and dispatch fact into the shape they had before this
/// change: no class field, and a free-text reason.
async fn into_legacy_shape(env: &Env, step: &CoordinatorStepResult, reason: &str) {
    let node_id = step.node_id.as_deref().unwrap();
    let mut node = raw_record(&env.store, NODE_KIND, node_id).await;
    node["payload"]
        .as_object_mut()
        .unwrap()
        .remove("terminal_class");
    put_raw_record(&env.store, NODE_KIND, node_id, &node).await;
    let dispatch_id = step.dispatch_id.as_deref().unwrap();
    let mut fact = raw_record(&env.store, DISPATCH_KIND, dispatch_id).await;
    let fields = fact["payload"].as_object_mut().unwrap();
    fields.remove("terminal_class");
    fields.insert("outcome_reason".into(), json!(reason));
    put_raw_record(&env.store, DISPATCH_KIND, dispatch_id, &fact).await;
}

#[tokio::test]
async fn nodes_and_dispatch_facts_written_before_the_classes_are_read_registered_and_served_as_before()
 {
    let env = Env::new().await;
    let world = world_for("world-legacy");
    let job = env.start_job("legacy-start", &world).await;
    assert_eq!(job.state, ManagementJobState::Succeeded, "{job:?}");
    assert!(matches!(
        job.result,
        Some(ManagementResult::ExplorationStarted { .. })
    ));
    // The two roots are spent by a step that changed nothing and by a transport that
    // failed, and both records are then rewritten as the previous version wrote them:
    // a free-text reason (the old fixed sentence, and an error echoed into a reason)
    // and no class.
    let first = env
        .run(
            &ScriptedModel::new(ModelScript::Empty),
            &ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true)),
            &env.journal,
            "world-legacy",
            1,
            "legacy-1",
        )
        .await
        .unwrap();
    let second = env
        .run(
            &ScriptedModel::new(ModelScript::Fail("transport words".into())),
            &ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true)),
            &env.journal,
            "world-legacy",
            2,
            "legacy-2",
        )
        .await
        .unwrap();
    into_legacy_shape(&env, &first, "model returned no edit suggestions").await;
    into_legacy_shape(
        &env,
        &second,
        "model transport outcome unknown: state conflict: transport words",
    )
    .await;

    // They are read: a client reconnecting to either dispatch is answered with the
    // reason that was stored, the node it named and nothing is dispatched.
    let again = env.reconnect("world-legacy", 1, "legacy-1").await.unwrap();
    assert_eq!(again.node_id, first.node_id);
    assert_eq!(again.outcome, "model returned no edit suggestions");
    let again = env.reconnect("world-legacy", 2, "legacy-2").await.unwrap();
    assert_eq!(again.node_id, second.node_id);
    assert_eq!(
        again.outcome,
        "model transport outcome unknown: state conflict: transport words"
    );
    // The usage view counts both dispatches as it did, and the decision is the same:
    // no node of either shape offers a successor.
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-legacy")
        .await
        .unwrap();
    assert_eq!(
        usage
            .iter()
            .map(|record| record.dispatch_state())
            .collect::<Vec<_>>(),
        [
            MechanismUsageState::Observed,
            MechanismUsageState::Uncertain
        ]
    );
    let after = env.coordinator.decide_next("world-legacy").await.unwrap();
    assert!(matches!(after.action, BatchActionV1::Stop { .. }));
    // The world is registered and its management status holds with records of the
    // old shape, and `register_world_idempotent` still recognises it.
    expect_already_registered(env.register(&world).await, "after the legacy dispatches");
    expect_status_ok(&env, &job, "with legacy dispatch facts").await;
    let view = env.coordinator.decision_view("world-legacy").await.unwrap();
    assert!(matches!(view.decision.action, BatchActionV1::Stop { .. }));
}

/// The payload a `StepCompleted` fact had before the classes: the outcome status and
/// its free-text reason.
fn legacy_step_payload(status: &str, reason: &str) -> Value {
    json!({"status": status, "reason": reason})
}

async fn rewrite_stage_fact(store: &Store, kind: StageFactKind, payload: Value) {
    let facts = stage_facts(store).await;
    let mut fact = facts
        .into_iter()
        .find(|fact| fact.kind == kind)
        .expect("the fact to rewrite exists");
    fact.payload = payload;
    let fact = fact.seal().unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&worker(), "artifact", &fact.artifact_id, "worker", &fact)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn a_step_whose_terminal_fact_was_written_before_the_classes_still_answers_from_it() {
    for (name, setup, status, reason) in [
        (
            "no change",
            Setup {
                model: ModelScript::Empty,
                ..Setup::default()
            },
            "no_change",
            "model returned no edit suggestions",
        ),
        (
            "rejected",
            Setup {
                runner: RunnerScript::Scores(500_000, 500_000, true, true),
                ..Setup::default()
            },
            "rejected",
            "candidate did not strictly improve while preserving all incumbent passes",
        ),
        (
            "uncertain",
            Setup {
                model: ModelScript::Uncertain,
                ..Setup::default()
            },
            "uncertain",
            "model usage remains unknown",
        ),
    ] {
        let driven = setup.drive().await;
        let first = kind_of(driven.outcome.as_ref().unwrap()).unwrap();
        assert_eq!(first.status(), status, "{name}");
        rewrite_stage_fact(
            &driven.env.store,
            StageFactKind::StepCompleted,
            legacy_step_payload(status, reason),
        )
        .await;
        let model = ScriptedModel::new(ModelScript::Edits);
        let runner = ScriptedRunner::new(RunnerScript::Scores(500_000, 900_000, true, true));
        let again = run_optimization_step(
            Some(&model),
            Some(&runner),
            Some(&driven.env.journal),
            driven.env.fixture.request(EPISODE, 1, "step"),
        )
        .await
        .unwrap();
        assert_eq!(kind_of(&again), Some(first), "{name}");
        // The free text of the old terminal is not carried forward: the class says only
        // that the terminal was written before the classes existed.
        assert_eq!(
            class_of(&again),
            Some(StepTerminalClass::LegacyUnclassified),
            "{name}"
        );
        assert_eq!((model.calls(), runner.calls()), (0, 0), "{name}");
    }
}

// ---------------------------------------------------------------------------
// 3b. The answer of the model, as `request_suggestions` reports it
// ---------------------------------------------------------------------------

fn reflection_request() -> (ModelRequest, ReflectionBatch) {
    let source = EvidenceRef {
        id: "run-a".into(),
        digest: hash(b"run-a"),
    };
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: "request-a".into(),
            namespace: NAMESPACE.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-a".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent"),
            bundle_digest: hash(b"bundle"),
            source_closure: vec![source.clone()],
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::Evidence,
            label: "failure".into(),
            content: "complete fixture payload".into(),
        }],
    )
    .unwrap();
    let batch = ReflectionBatch {
        id: "reflection-failure".into(),
        kind: ReflectionBatchKind::Failure,
        traces: vec![],
        independent_parent_families: BTreeSet::new(),
        source_closure: vec![source],
    };
    (request, batch)
}

fn pool_of(count: usize) -> String {
    let source = EvidenceRef {
        id: "run-a".into(),
        digest: hash(b"run-a"),
    };
    let items: Vec<EditSuggestion> = (0..count)
        .map(|index| EditSuggestion {
            id: format!("suggestion-{index}"),
            hypothesis: "bounded fixture hypothesis".into(),
            batch_ids: vec!["reflection-failure".into()],
            support: vec![source.clone()],
            counterexamples: vec![],
            dependencies: vec![source.clone()],
            edit: SkillTextEdit {
                field: SkillTextField::Content,
                start: 0,
                end: 0,
                expected_text_digest: hash(b""),
                exact_anchor: None,
                operation: TextEditOperation::Insert {
                    text: format!("[{index}]"),
                },
            },
        })
        .collect();
    serde_json::to_string(&items).unwrap()
}

#[tokio::test]
async fn an_answer_that_is_no_suggestion_list_is_refused_without_echoing_it() {
    let (request, batch) = reflection_request();
    let fixed = "optimizer suggestions are not a valid suggestion list";
    for (name, answer) in [
        ("an unknown field", format!(r#"[{{"unknown-{MARK}":1}}]"#)),
        ("not json at all", format!("words of the model {MARK}")),
        ("a pool over the request's maximum", pool_of(5)),
    ] {
        let model = ScriptedModel::new(ModelScript::Raw(answer));
        match request_suggestions(Some(&model), request.clone(), std::slice::from_ref(&batch)).await
        {
            Err(Error::Invalid(message)) => {
                assert_eq!(message, fixed, "{name}");
                assert!(!message.contains(MARK), "{name}");
            }
            other => panic!("{name}: expected the fixed refusal, got {other:?}"),
        }
    }
    // A pool within the maximum is a pool, and an empty one is the model's no change.
    let model = ScriptedModel::new(ModelScript::Raw(pool_of(4)));
    let outcome = request_suggestions(Some(&model), request.clone(), std::slice::from_ref(&batch))
        .await
        .unwrap();
    assert!(
        matches!(&outcome, SuggestionOutcome::Suggestions { items, .. } if items.len() == 4),
        "{outcome:?}"
    );
    let model = ScriptedModel::new(ModelScript::Empty);
    let outcome = request_suggestions(Some(&model), request, std::slice::from_ref(&batch))
        .await
        .unwrap();
    assert!(
        matches!(outcome, SuggestionOutcome::NoChange { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_rejection_or_an_unknown_usage_is_reported_with_its_typed_kind_and_no_text() {
    let (request, batch) = reflection_request();
    let model = ScriptedModel::new(ModelScript::Reject {
        kind: ModelRejectionKind::BudgetUnavailable,
        reason: format!("words of the provider {MARK}"),
    });
    let outcome = request_suggestions(Some(&model), request.clone(), std::slice::from_ref(&batch))
        .await
        .unwrap();
    assert!(
        matches!(
            &outcome,
            SuggestionOutcome::Rejected {
                kind: ModelRejectionKind::BudgetUnavailable,
                dependency_ids
            } if dependency_ids.is_empty()
        ),
        "{outcome:?}"
    );
    let written = serde_json::to_value(&outcome).unwrap();
    assert_eq!(
        written,
        json!({"Rejected": {"kind": "budget_unavailable", "dependency_ids": []}})
    );
    assert!(!written.to_string().contains(MARK));

    let model = ScriptedModel::new(ModelScript::Uncertain);
    let outcome = request_suggestions(Some(&model), request, std::slice::from_ref(&batch))
        .await
        .unwrap();
    assert!(
        matches!(&outcome, SuggestionOutcome::Uncertain { dependency_ids } if !dependency_ids.is_empty()),
        "{outcome:?}"
    );
    // What is written of an unknown usage holds the ids it names and nothing else.
    let written = serde_json::to_value(&outcome).unwrap();
    assert_eq!(
        written.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["Uncertain"]
    );
    assert_eq!(
        written["Uncertain"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["dependency_ids"]
    );
}

// ---------------------------------------------------------------------------
// 3c. A skill group reports the class of its step, in memory only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_skill_group_reports_the_class_of_its_step_and_no_text_of_an_error_or_a_model() {
    let cases: Vec<(&str, Setup, GroupOutcomeKind, Option<StepTerminalClass>)> = vec![
        (
            "a candidate",
            Setup::default(),
            GroupOutcomeKind::Candidate,
            None,
        ),
        (
            "no change",
            Setup {
                model: ModelScript::Empty,
                ..Setup::default()
            },
            GroupOutcomeKind::NoChange,
            Some(StepTerminalClass::NoEditSuggestions),
        ),
        (
            "a kept incumbent",
            Setup {
                runner: RunnerScript::Scores(400_000, 900_000, true, false),
                ..Setup::default()
            },
            GroupOutcomeKind::Rejected,
            Some(StepTerminalClass::KeepIncumbentRetentionBroken {
                parent_total_micros: 400_000,
                candidate_total_micros: 900_000,
            }),
        ),
        (
            "the provider rejects",
            Setup {
                model: ModelScript::Reject {
                    kind: ModelRejectionKind::ProviderRejected,
                    reason: format!("words of the provider {MARK}"),
                },
                ..Setup::default()
            },
            GroupOutcomeKind::Rejected,
            Some(StepTerminalClass::ModelRejected {
                kind: ModelRejectionKind::ProviderRejected,
            }),
        ),
        (
            "the transport fails",
            Setup {
                model: ModelScript::Fail(format!("words of the transport {MARK}")),
                ..Setup::default()
            },
            GroupOutcomeKind::Uncertain,
            Some(StepTerminalClass::ModelTransportOutcomeUnknown),
        ),
        (
            "a trace the stored source does not back",
            Setup {
                mutate: Some(|request| request.traces[0].excerpt = "unbacked bytes".into()),
                ..Setup::default()
            },
            GroupOutcomeKind::Failed,
            Some(StepTerminalClass::StepError {
                error: StepErrorKind::Forbidden,
            }),
        ),
    ];
    for (name, setup, kind, terminal) in cases {
        let fixture = Fixture::with(setup.outcomes, setup.parent.clone());
        let env = Env::with(fixture).await;
        let model = ScriptedModel::new(setup.model.clone());
        let runner = ScriptedRunner::new(setup.runner.clone());
        let mut request = env.fixture.request("episode-group", 1, "group");
        if let Some(mutate) = setup.mutate {
            mutate(&mut request);
        }
        let outcome = run_skill_group_job(
            Some(&model),
            Some(&runner),
            Some(&env.journal),
            SkillGroupJobRequest {
                job_id: "job-1".into(),
                groups: vec![SkillGroupSpec {
                    group_id: "group-a".into(),
                    request,
                }],
            },
        )
        .await
        .unwrap();
        let group = &outcome.groups[0];
        assert_eq!(group.kind, kind, "{name}");
        assert_eq!(group.terminal, terminal, "{name}");
        assert_eq!(
            group.candidate_bundle_digest.is_some(),
            kind == GroupOutcomeKind::Candidate,
            "{name}"
        );
        // The outcome as it would be serialized holds the class and no text of what the
        // port or the runner failed with.
        let written = serde_json::to_string(&outcome).unwrap();
        assert!(!written.contains(MARK), "{name}: {written}");
        assert_eq!(
            serde_json::to_value(group).unwrap()["terminal"],
            serde_json::to_value(terminal).unwrap(),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. A consolidation keeps only the fixed code of its terminal, revocation or not
// ---------------------------------------------------------------------------

/// The consolidation fixtures are copied from `tests/consolidation_terminal_category_v42.rs`
/// (test crates cannot import one another): the monitoring environment, two closed
/// development cycles, the claim of the first generation and the step material.
mod consolidation {
    use super::{MARK, files_holding};
    use async_trait::async_trait;
    use evo_core::contract::{
        CapabilityLevel, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile,
        SkillSnapshot, SurfaceCoverage, SurfaceItem, SystemSnapshot,
    };
    use evo_core::evidence::{
        EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin,
    };
    use evo_core::optimization::{
        EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
        SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
    };
    use evo_core::skill_edit::{
        EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA,
        SkillEditBatch, SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext,
        skill_snapshot_digest,
    };
    use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
    use evo_engine::broker::BudgetPortBinding;
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
        store_trace_authority,
    };
    use evo_engine::model::{
        ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind,
        ModelResponse, RejectedDispatch,
    };
    use evo_engine::monitoring::{
        ClaimOutcome, CompleteDevelopmentCycleRequest, ConsolidationClaim, ConsolidationDevRunner,
        ConsolidationModelPort, ConsolidationRunOutcome, ConsolidationRunRecord,
        DevelopmentCycleRecord, EnvironmentEvidenceScope, MonitoringCoordinator,
        RecordEnvironmentRequest, RecordedEnvironment,
    };
    use evo_engine::optimization::{
        BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
        DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask,
        OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournal, OptimizationJournalStage,
        OptimizationStepRequest, PairedTaskResult, StageDependency, StageFact, StageFactKind,
        StoreOptimizationJournal,
    };
    use evo_engine::release_store::{
        PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence,
    };
    use evo_storage::Store;
    use evo_storage::budget::{RootBudgetAuthorization, RootBudgetRecord};
    use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TENANT: &str = "tenant";
    const SCOPE: &str = "billing-scope";
    const ROOT: &str = "root-budget";

    fn d(value: &str) -> String {
        hash(value.as_bytes())
    }

    fn context(actor: &str, role: Role) -> Context {
        Context::new(TENANT, actor, role).unwrap()
    }

    async fn authorize_root(store: &Store, admin: &Context) -> RootBudgetRecord {
        store
            .authorize_root_budget(
                admin,
                &RootBudgetAuthorization {
                    root_budget_id: ROOT.into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![TENANT.into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "payer".into(),
                    authorization_receipt_digest: d("authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap()
    }

    fn surface() -> HostSurfaceManifest {
        HostSurfaceManifest {
            schema_version: "rsia.host_surface.v1".into(),
            host: "monitor-host".into(),
            host_version: "1.0.0".into(),
            adapter_version: "adapter-v1".into(),
            source_digest: d("surface-source"),
            items: vec![SurfaceItem {
                name: "model".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("host.model".into()),
                consumer: Some("runner".into()),
                reason: "fixture projection".into(),
            }],
        }
    }

    fn system(model: &str) -> SystemSnapshot {
        SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "profile".into(),
            host_id: "monitor-host".into(),
            host_version: "1.0.0".into(),
            model_id: model.into(),
            tools: vec!["tool-a".into()],
            mandatory_context_digest: d("mandatory"),
        }
    }

    /// A trusted Host development source. `source-1` is a diagnosed failure and
    /// `source-2` a success, so the optimizer has a failure and a success batch.
    fn authority(id: &str, body: &str) -> StoredTraceAuthority {
        let failing = id == "source-1";
        StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: body.as_bytes().to_vec(),
                parent_family: format!("family-{id}"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: id.into(),
                parent_family: format!("family-{id}"),
                source_digest: hash(body.as_bytes()),
                purpose: Purpose::Development,
                outcome: if failing {
                    TraceOutcome::TaskFailure
                } else {
                    TraceOutcome::Success
                },
                diagnosis: failing.then(|| SkillFailureDiagnosis {
                    kind: SkillFailureKind::SkillDefect,
                    skill_id: "skill-a".into(),
                    bundle_digest: d("parent-bundle"),
                    request_digest: d("task-request"),
                    rule_id: Some("rule-a".into()),
                    support: vec![EvidenceRef {
                        id: id.into(),
                        digest: hash(body.as_bytes()),
                    }],
                    counterexamples: vec![],
                    reason: "fixture defect".into(),
                }),
                excerpt: body.into(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: body.len(),
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        store: Store,
        admin: Context,
        host: Context,
        worker: Context,
        environment: RecordedEnvironment,
        authorities: Vec<StoredTraceAuthority>,
        manifest: DevelopmentManifest,
        parent_skill: SkillSnapshot,
        manifest_digest: String,
        grader_digest: String,
        strategy_digest: String,
        parent_skill_digest: String,
        parent_bundle_digest: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("consolidation-stage.sqlite3");
            let store = Store::open(&path).await.unwrap();
            let admin = context("admin", Role::Admin);
            let host = context("host", Role::Host);
            let worker = context("worker", Role::Worker);
            ReleaseStore::register_host_surface(
                &admin,
                &store,
                "surface",
                surface(),
                vec!["model".into()],
            )
            .await
            .unwrap();
            prepare_host_run(&host, &store, "host-run-1", "model-v1", "task-input").await;
            let authorities = vec![
                authority("source-1", "first development source"),
                authority("source-2", "second development source"),
            ];
            for source in &authorities {
                store_trace_authority(&store, &host, source).await.unwrap();
            }
            let mut session = store.session().await.unwrap();
            assert_eq!(
                session
                    .bump_watermark(&host, &d("initial-watermark"))
                    .await
                    .unwrap(),
                1
            );
            session.commit().await.unwrap();
            let manifest = DevelopmentManifest::build(
                "development-manifest",
                vec![DevelopmentTask {
                    id: "task-a".into(),
                    parent_family: "development-family".into(),
                    input_digest: d("development-task-input"),
                }],
            )
            .unwrap();
            let manifest_digest = manifest.digest.clone();
            let grader_digest = d("development-grader");
            let strategy_digest = fingerprint(&Strategy::default()).unwrap();
            let parent_skill = SkillSnapshot {
                content: "parent rule".into(),
                applicability: "development only".into(),
                counterexample: "counterexample".into(),
                required_capabilities: vec![],
                dependencies: vec![],
            };
            let environment = MonitoringCoordinator::record_environment_from_run(
                &host,
                &store,
                RecordEnvironmentRequest {
                    run_id: "host-run-1".into(),
                    development_manifest_digest: manifest_digest.clone(),
                    grader_digest: grader_digest.clone(),
                    generation_strategy_digest: strategy_digest.clone(),
                    revoke_watermark: 1,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                environment.environment.evidence_scope,
                EnvironmentEvidenceScope::ProgramFixture
            );
            authorize_root(&store, &admin).await;
            Self {
                _dir: dir,
                path,
                store,
                admin,
                host,
                worker,
                environment,
                authorities,
                manifest,
                parent_skill: parent_skill.clone(),
                manifest_digest,
                grader_digest,
                strategy_digest,
                parent_skill_digest: skill_snapshot_digest(&parent_skill).unwrap(),
                parent_bundle_digest: d("parent-bundle"),
            }
        }

        fn cycle_request(&self, fact_id: String) -> CompleteDevelopmentCycleRequest {
            CompleteDevelopmentCycleRequest {
                skill_id: "skill-a".into(),
                profile_id: "profile".into(),
                environment_id: self.environment.environment.id.clone(),
                development_manifest_digest: self.manifest_digest.clone(),
                grader_digest: self.grader_digest.clone(),
                generation_strategy_digest: self.strategy_digest.clone(),
                parent_skill_digest: self.parent_skill_digest.clone(),
                parent_bundle_digest: self.parent_bundle_digest.clone(),
                billing_scope: SCOPE.into(),
                root_budget_id: ROOT.into(),
                report_fact_id: fact_id,
                source_ids: vec!["source-2".into(), "source-1".into()],
                revoke_watermark: 1,
            }
        }

        /// Stages a development report for `cycle` and closes it as a cycle.
        async fn close_cycle(&self, cycle: u32, contrast: bool) -> Result<DevelopmentCycleRecord> {
            let fact = self.stage_report(cycle, contrast).await;
            MonitoringCoordinator::close_development_cycle(
                &self.worker,
                &self.store,
                self.cycle_request(fact),
            )
            .await
        }

        /// Closes two cycles, with a before/after contrast or without one, and claims
        /// the first generation.
        async fn claimed(&self, contrast: bool) -> ConsolidationClaim {
            for cycle in 1..=2 {
                self.close_cycle(cycle, contrast).await.unwrap();
            }
            let scope_id = self.scope_id().await;
            let ClaimOutcome::Claimed(claim) =
                MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, &scope_id)
                    .await
                    .unwrap()
            else {
                panic!("the next consolidation generation was not claimed")
            };
            claim
        }

        async fn scope_id(&self) -> String {
            let mut session = self.store.session().await.unwrap();
            let scopes: Vec<Value> = session.list(&self.worker, "artifact").await.unwrap();
            session.commit().await.unwrap();
            let ids: Vec<_> = scopes
                .iter()
                .filter(|value| value["schema_version"] == "rsia.monitoring.consolidation_scope.v1")
                .map(|value| value["id"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(ids.len(), 1, "exactly one consolidation scope");
            ids[0].clone()
        }

        /// Stages a DevelopmentObserved fact whose report pairs one task for `cycle`
        /// (both pass; the score goes 500k -> 600k with a contrast, and stays at 500k
        /// without one), with the fixture receipts the stage fact depends on.
        async fn stage_report(&self, cycle: u32, contrast: bool) -> String {
            let runner_execution = format!("runner-execution-{cycle}");
            let parent_execution = format!("parent-execution-{cycle}-0");
            let candidate_execution = format!("candidate-execution-{cycle}-0");
            let grader_receipt = d(&format!("grader-receipt-{cycle}-0"));
            let receipts = [
                ("execution", runner_execution.clone()),
                ("execution", parent_execution.clone()),
                ("execution", candidate_execution.clone()),
                ("grader", grader_receipt.clone()),
            ];
            let mut dependencies = vec![
                StageDependency {
                    kind: "execution".into(),
                    id: parent_execution.clone(),
                },
                StageDependency {
                    kind: "execution".into(),
                    id: candidate_execution.clone(),
                },
                StageDependency {
                    kind: "grader".into(),
                    id: grader_receipt.clone(),
                },
            ];
            let report = DevelopmentRunReport {
                request_id: format!("development-request-{cycle}"),
                manifest_digest: self.manifest_digest.clone(),
                parent_bundle_digest: self.parent_bundle_digest.clone(),
                candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
                environment_digest: self.environment.environment.environment_digest.clone(),
                grader_digest: self.grader_digest.clone(),
                results: vec![PairedTaskResult {
                    task_id: "task-a".into(),
                    parent_score_micros: 500_000,
                    candidate_score_micros: if contrast { 600_000 } else { 500_000 },
                    parent_passed: true,
                    candidate_passed: true,
                    parent_execution_id: parent_execution,
                    candidate_execution_id: candidate_execution,
                    grader_receipt_digest: grader_receipt,
                }],
                execution_receipt_id: runner_execution,
                usage_record_ids: vec![],
                provenance: DevelopmentExecutionProvenance::Fixture,
            };
            let mut session = self.store.session().await.unwrap();
            for (logical_kind, id) in &receipts {
                session
                .put(
                    &self.worker,
                    "artifact",
                    id,
                    self.worker.actor(),
                    &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":cycle,"fixture":true}),
                )
                .await
                .unwrap();
            }
            session.commit().await.unwrap();
            for source in ["source-1", "source-2"] {
                dependencies.push(StageDependency {
                    kind: "run".into(),
                    id: source.into(),
                });
            }
            dependencies.push(StageDependency {
                kind: "revoke_watermark".into(),
                id: "1".into(),
            });
            let fact = StageFact {
                schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
                artifact_id: "placeholder".into(),
                namespace: TENANT.into(),
                episode_id: format!("development-episode-{cycle}"),
                step: cycle,
                attempt: 1,
                stage: OptimizationJournalStage::Development,
                kind: StageFactKind::DevelopmentObserved,
                request_id: report.request_id.clone(),
                input_digest: d(&format!("development-input-{cycle}")),
                output_digest: None,
                dependencies,
                payload: serde_json::to_value(&report).unwrap(),
            }
            .seal()
            .unwrap();
            let id = fact.artifact_id.clone();
            self.journal().commit(fact).await.unwrap();
            id
        }

        fn journal(&self) -> StoreOptimizationJournal {
            StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker")
                .unwrap()
        }
    }

    async fn prepare_host_run(
        host: &Context,
        store: &Store,
        run_id: &str,
        model: &str,
        task: &str,
    ) {
        let snapshot = ReleaseStore::prepare_run(
            host,
            store,
            PrepareRunRequest {
                run_id: run_id.into(),
                profile_id: "profile".into(),
                system_snapshot: system(model),
                host_surface_id: "surface".into(),
                host_capabilities: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                task_input_digest: d(task),
                evolution_enabled: true,
                capability_level: CapabilityLevel::Attached,
            },
        )
        .await
        .unwrap();
        ReleaseStore::record_applied_request(
            host,
            store,
            run_id,
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
    }

    /// Everything an `OptimizationStepRequest` borrows, frozen once per fixture.
    struct StepMaterial {
        selection: SourceSelection,
        evidence: EvidenceSet,
        bindings: Vec<TrustedSourceBinding>,
        allowed: Vec<EvidenceRef>,
        edit_context: TrustedEditContext,
        edit_template: SkillEditBatch,
        profile: Profile,
        baseline: SkillSnapshot,
        strategy: Strategy,
        patch: ImproverPatch,
        caps: HostCapabilities,
        revoked: BTreeSet<String>,
    }

    impl StepMaterial {
        async fn new(fixture: &Fixture) -> Self {
            let selection = SourceSelection {
                roots: vec![],
                run_ids: vec!["source-1".into(), "source-2".into()],
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            };
            store_source_selection(&fixture.store, &fixture.host, &selection)
                .await
                .unwrap();
            let records = fixture
                .authorities
                .iter()
                .map(|authority| authority.record.clone())
                .collect::<Vec<_>>();
            let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
            let bindings = fixture
                .authorities
                .iter()
                .map(|authority| TrustedSourceBinding {
                    source_id: authority.record.id.clone(),
                    source_digest: authority.trace.source_digest.clone(),
                    parent_family: authority.record.parent_family.clone(),
                })
                .collect::<Vec<_>>();
            let allowed = fixture
                .authorities
                .iter()
                .map(|authority| EvidenceRef {
                    id: authority.record.id.clone(),
                    digest: authority.trace.source_digest.clone(),
                })
                .collect::<Vec<_>>();
            let approved_parent = d("approved-parent");
            let safe_baseline = d("safe-baseline");
            let edit_context = TrustedEditContext::new(
                TENANT,
                "profile",
                "skill-a",
                "v1",
                approved_parent.clone(),
                safe_baseline.clone(),
                &fixture.parent_skill,
                allowed.clone(),
            )
            .unwrap();
            let edit_template = SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: TENANT.into(),
                profile_id: "profile".into(),
                skill_id: "skill-a".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(&fixture.parent_skill).unwrap(),
                approved_parent_digest: approved_parent.clone(),
                safe_baseline_digest: safe_baseline.clone(),
                evidence: EvidenceClosure {
                    support: vec![allowed[0].clone()],
                    counterexamples: vec![allowed[1].clone()],
                    dependencies: allowed.clone(),
                },
                edits: vec![],
            };
            Self {
                selection,
                evidence,
                bindings,
                allowed,
                edit_context,
                edit_template,
                profile: Profile {
                    id: "profile".into(),
                    evolution_enabled: true,
                    parent_digest: approved_parent,
                    baseline_digest: safe_baseline,
                },
                baseline: fixture.parent_skill.clone(),
                strategy: Strategy::default(),
                patch: ImproverPatch::default(),
                caps: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                revoked: BTreeSet::new(),
            }
        }

        /// The optimization request of `episode` (a claim id for a consolidation).
        fn request<'a>(
            &'a self,
            fixture: &'a Fixture,
            episode: &str,
        ) -> OptimizationStepRequest<'a> {
            OptimizationStepRequest {
                evidence: &self.evidence,
                source_selection: &self.selection,
                source_bindings: &self.bindings,
                traces: fixture
                    .authorities
                    .iter()
                    .map(|authority| authority.trace.clone())
                    .collect(),
                model_context: ModelRequestContext {
                    request_id: "consolidation-request-1".into(),
                    namespace: TENANT.into(),
                    purpose: Purpose::Development,
                    stage: ModelStage::Consolidate,
                    episode_id: episode.into(),
                    step: 1,
                    attempt: 1,
                    parent_skill_digest: fixture.parent_skill_digest.clone(),
                    bundle_digest: fixture.parent_bundle_digest.clone(),
                    source_closure: self.allowed.clone(),
                    model_digest: d("fixture-model"),
                    tools_digest: d("tools"),
                    rules_digest: d("rules"),
                    sampling_digest: d("sampling"),
                    revoke_watermark: 1,
                    max_suggestions: 4,
                },
                parent_skill: &fixture.parent_skill,
                edit_context: &self.edit_context,
                edit_batch_template: self.edit_template.clone(),
                protected_ranges: &[],
                bundle_context: BundleCompileContext {
                    profile: &self.profile,
                    baseline: &self.baseline,
                    parent_strategy: &self.strategy,
                    baseline_strategy: &self.strategy,
                    improver_patch: &self.patch,
                    caps: &self.caps,
                    revoked: &self.revoked,
                },
                development_request: DevelopmentRunRequest {
                    request_id: "consolidation-development-1".into(),
                    namespace: TENANT.into(),
                    purpose: Purpose::Development,
                    episode_id: episode.into(),
                    step: 1,
                    attempt: 1,
                    manifest: fixture.manifest.clone(),
                    parent_bundle_digest: fixture.parent_bundle_digest.clone(),
                    candidate_bundle_digest: d("candidate-1"),
                    environment_digest: fixture.environment.environment.environment_digest.clone(),
                    grader_digest: fixture.grader_digest.clone(),
                    rules_digest: d("rules"),
                    tools_digest: d("tools"),
                    revoke_watermark: 1,
                    idempotency_key: "consolidation-idempotency-1".into(),
                },
                allow_rank_call: false,
            }
        }
    }

    // -----------------------------------------------------------------------
    // The scripted model and development runner of a consolidation
    // -----------------------------------------------------------------------

    /// What the scripted consolidation model answers.
    enum Script {
        /// One bounded edit suggestion per reflection batch.
        Edits,
        /// The provider rejected the request, in these words.
        Rejects {
            kind: ModelRejectionKind,
            reason: String,
        },
    }

    struct ScriptedModel {
        script: Script,
    }

    impl ScriptedModel {
        /// One bounded insertion per reflection batch, read from the labelled inputs
        /// of the request (as `tests/monitoring_consolidation_v42.rs` does).
        fn suggestions(request: &ModelRequest) -> Result<String> {
            let input = |label: &str| -> Result<&str> {
                request
                    .input
                    .iter()
                    .find(|part| part.label == label)
                    .map(|part| part.content.as_str())
                    .ok_or_else(|| Error::Invalid(format!("fixture input {label} missing")))
            };
            let parent: SkillSnapshot = serde_json::from_str(input("parent-skill")?)
                .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
            let strategy: Strategy = serde_json::from_str(input("generation-strategy")?)
                .map_err(|_| Error::Invalid("fixture strategy input invalid".into()))?;
            let success = request
                .input
                .iter()
                .any(|part| part.label == "reflection-success");
            let (id, batch_id, field, start) = if success {
                (
                    "preserve-rule",
                    "reflection-success",
                    SkillTextField::Applicability,
                    parent.applicability.len(),
                )
            } else {
                (
                    "fix-rule",
                    "reflection-failure",
                    SkillTextField::Content,
                    parent.content.len(),
                )
            };
            let source = request.source_closure[0].clone();
            let suggestion = EditSuggestion {
                id: id.into(),
                hypothesis: "bounded fixture hypothesis".into(),
                batch_ids: vec![batch_id.into()],
                support: vec![source.clone()],
                counterexamples: if success {
                    vec![source.clone()]
                } else {
                    vec![]
                },
                dependencies: vec![source],
                edit: SkillTextEdit {
                    field,
                    start,
                    end: start,
                    expected_text_digest: hash(b""),
                    exact_anchor: None,
                    operation: TextEditOperation::Insert {
                        text: format!(" [{} terminal-class]", strategy.instruction),
                    },
                },
            };
            serde_json::to_string(&vec![suggestion]).map_err(|_| Error::Internal)
        }
    }

    fn receipt(request_id: &str) -> ModelExecutionReceipt {
        ModelExecutionReceipt {
            call_id: format!("call-{request_id}"),
            dispatch_id: format!("dispatch-{request_id}"),
            root_budget_id: ROOT.into(),
            provider_request_id: format!("provider-{request_id}"),
            usage_record_id: format!("usage-{request_id}"),
            provenance: ModelExecutionProvenance::Fixture,
        }
    }

    #[async_trait]
    impl ModelPort for ScriptedModel {
        async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
            match &self.script {
                Script::Edits => {
                    let output = Self::suggestions(&request)?;
                    Ok(ModelResponse::Completed {
                        response_id: format!("response-{}", request.request_id),
                        actual_model_digest: request.model_digest.clone(),
                        input_digest: request.input_digest.clone(),
                        output_digest: hash(output.as_bytes()),
                        output,
                        execution_receipt: receipt(&request.request_id),
                        request_id: request.request_id,
                    })
                }
                Script::Rejects { kind, reason } => {
                    let dispatch = match kind {
                        ModelRejectionKind::Unauthorized
                        | ModelRejectionKind::BudgetUnavailable
                        | ModelRejectionKind::InvalidRequest
                        | ModelRejectionKind::CancelledBeforeDispatch => {
                            RejectedDispatch::NotDispatched
                        }
                        ModelRejectionKind::ProviderRejected
                        | ModelRejectionKind::CancelledAfterDispatch => {
                            RejectedDispatch::Dispatched {
                                receipt: receipt(&request.request_id),
                            }
                        }
                    };
                    Ok(ModelResponse::Rejected {
                        request_id: request.request_id,
                        kind: *kind,
                        reason: reason.clone(),
                        dispatch,
                    })
                }
            }
        }
    }

    #[async_trait]
    impl ConsolidationModelPort for ScriptedModel {
        async fn trusted_budget_binding(&self, _namespace: &str) -> Result<BudgetPortBinding> {
            Ok(BudgetPortBinding {
                billing_scope: SCOPE.into(),
                root_budget_id: ROOT.into(),
            })
        }
    }

    /// A development runner that always fails with a fixed message and counts its calls.
    struct FailingDevRunner {
        message: String,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl DevRunner for FailingDevRunner {
        async fn run(&self, _request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(Error::Conflict(self.message.clone()))
        }
    }

    #[async_trait]
    impl ConsolidationDevRunner for FailingDevRunner {
        async fn trusted_budget_binding(&self, _namespace: &str) -> Result<BudgetPortBinding> {
            Ok(BudgetPortBinding {
                billing_scope: SCOPE.into(),
                root_budget_id: ROOT.into(),
            })
        }
    }

    /// Revokes `source` and drives the cleanup job until it stops moving.
    async fn revoke_and_clean(fixture: &Fixture, source: &str) -> CleanupStatus {
        let mut status = LifecycleStore::begin_revoke(
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
        .unwrap();
        for now in 501..800 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(
                &fixture.admin,
                &fixture.store,
                &status.job_id,
                8,
                now,
            )
            .await
            .unwrap();
        }
        status
    }

    async fn consolidate(
        fixture: &Fixture,
        material: &StepMaterial,
        claim: &ConsolidationClaim,
        model: &ScriptedModel,
        runner: &FailingDevRunner,
    ) -> Result<ConsolidationRunRecord> {
        MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &claim.id,
            Some(model),
            Some(runner),
            Some(&fixture.journal()),
            material.request(fixture, &claim.id),
        )
        .await
    }

    /// The terminal run record of a claim, as the store holds it now.
    async fn stored_run_record(
        fixture: &Fixture,
        claim: &ConsolidationClaim,
    ) -> ConsolidationRunRecord {
        let mut session = fixture.store.session().await.unwrap();
        let record = session
            .need(
                &fixture.worker,
                "artifact",
                &format!("consolidation-run-{}", claim.id),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        record
    }

    /// Runs a consolidation that ends in a terminal of `outcome` and checks what it
    /// keeps: the fixed code in its run record and in its step fact, nothing of the
    /// words planted in the port or the runner, as the run ended and after every
    /// source of the claim was revoked and the cleanup completed.
    async fn assert_fixed_code_survives(
        name: &str,
        model: ScriptedModel,
        runner: FailingDevRunner,
        outcome: ConsolidationRunOutcome,
        code: &str,
    ) {
        let fixture = Fixture::new().await;
        let material = StepMaterial::new(&fixture).await;
        let claim = fixture.claimed(true).await;
        let record = consolidate(&fixture, &material, &claim, &model, &runner)
            .await
            .unwrap();
        let mut findings = super::Findings::default();
        assert_eq!(record.outcome, outcome, "{name}: {record:?}");
        findings.eq(
            name,
            "run record reason",
            &json!(record.reason),
            &json!(code),
        );
        findings.check(files_holding(&fixture.path, MARK).is_empty(), || {
            format!(
                "{name}: the database holds the planted words ({:?})",
                files_holding(&fixture.path, MARK)
            )
        });
        // The step fact of the journal says the same code, as a status and a class.
        let mut session = fixture.store.session().await.unwrap();
        let all: Vec<Value> = session.list(&fixture.worker, "artifact").await.unwrap();
        session.commit().await.unwrap();
        let steps: Vec<_> = all
            .iter()
            .filter(|fact| {
                fact["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA
                    && fact["episode_id"] == json!(claim.id)
                    && fact["kind"] == "step_completed"
            })
            .collect();
        findings.check(steps.len() == 1, || {
            format!("{name}: {} StepCompleted facts", steps.len())
        });
        if let Some(step) = steps.first() {
            findings.eq(
                name,
                "step fact reason",
                &step["payload"]["reason"],
                &json!(code),
            );
        }
        // After a revocation of its sources the cleanup redacts what is in their
        // closure and keeps the terminal record of the run, which is where the old
        // category kept a prefix of the words.
        let status = revoke_and_clean(&fixture, "source-1").await;
        assert_eq!(status.state, CleanupState::Complete, "{name}: {status:?}");
        let kept = stored_run_record(&fixture, &claim).await;
        findings.eq(
            name,
            "run record reason after the revocation",
            &json!(kept.reason),
            &json!(code),
        );
        findings.check(files_holding(&fixture.path, MARK).is_empty(), || {
            format!(
                "{name}: after the revocation the database holds the planted words ({:?})",
                files_holding(&fixture.path, MARK)
            )
        });
        findings.done();
    }

    #[tokio::test]
    async fn a_rejected_consolidation_keeps_only_a_fixed_code_before_and_after_its_sources_are_revoked()
     {
        assert_fixed_code_survives(
            "rejected by the provider",
            ScriptedModel {
                script: Script::Rejects {
                    kind: ModelRejectionKind::Unauthorized,
                    reason: format!("words of the provider {MARK}"),
                },
            },
            FailingDevRunner {
                message: "never reached".into(),
                calls: AtomicUsize::new(0),
            },
            ConsolidationRunOutcome::Rejected,
            "model_rejected_unauthorized",
        )
        .await;
    }

    #[tokio::test]
    async fn an_uncertain_consolidation_keeps_only_a_fixed_code_before_and_after_its_sources_are_revoked()
     {
        assert_fixed_code_survives(
            "runner failed",
            ScriptedModel {
                script: Script::Edits,
            },
            FailingDevRunner {
                message: format!("words of the runner {MARK}"),
                calls: AtomicUsize::new(0),
            },
            ConsolidationRunOutcome::Uncertain,
            "development_execution_outcome_unknown",
        )
        .await;
    }
}
