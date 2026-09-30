//! E14 increment 1 (plan §9, §9.1; V036, V086.c/d, V092.c, V094.c, V015 at the
//! "appears in a dispatched decision summary" level): the coordinator records
//! the digests of the policy and caps it decides with, dispatch facts carry
//! them (schema v2), mechanism usage is derived only from real dispatches, and
//! inheritance is verified from that usage, never from a boolean or a pointer.
//!
//! Not covered (and not claimed): inherited benefit, the same-S0 old/new
//! successor comparison, an Improver approval registry or job type, and
//! `meta.start` (still blocked).
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::improver::{ImproverContentV2, ImproverMechanismV2};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::replay::*;
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus,
    SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, MechanismUsageRecordV1,
    MechanismUsageState, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::meta::{
    META_DEPTH_CAP, MetaCandidate, cannot_write_protected, verify_mechanism_inheritance,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationStepRequest,
    PairedTaskResult, StoreOptimizationJournal,
};
use evo_engine::replay::{LiveWorldAuthority, run_replay};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::Notify;

const DISPATCH_V2: &str = "rsia.exploration_dispatch.v2";
const DISPATCH_V1: &str = "rsia.exploration_dispatch.v1";

// ---------------------------------------------------------------------------
// Fixtures: the same trusted source closure and request shape as
// `exploration_v41`, with the world's policy as a parameter.
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
        dependencies: vec![
            ExplorationDependency {
                kind: "run".into(),
                id: "run-failure".into(),
            },
            ExplorationDependency {
                kind: "run".into(),
                id: "run-success".into(),
            },
        ],
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

/// Seeds the trusted source closure (two Host-issued trace authorities, a
/// source selection grant and one watermark bump, so `source_watermark == 1`).
async fn seeded_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("meta-inheritance.sqlite3"))
        .await
        .unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
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
    _dir: tempfile::TempDir,
    store: Store,
    coordinator: PersistentCoordinator,
    journal: StoreOptimizationJournal,
    fixture: Fixture,
}

async fn env() -> Env {
    let (dir, store) = seeded_store().await;
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    Env {
        coordinator: PersistentCoordinator::new(store.clone(), worker.clone(), "worker").unwrap(),
        journal: StoreOptimizationJournal::new(store.clone(), worker, "worker").unwrap(),
        fixture: Fixture::new(),
        store,
        _dir: dir,
    }
}

impl Env {
    async fn register(&self, id: &str, policy: ElasticPolicyV1) {
        self.coordinator
            .register_world(world_for(id, policy))
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
}

// ---------------------------------------------------------------------------
// Raw store access, to tamper with persisted facts the way a corrupt store or
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
// 3. Decision digests
// ---------------------------------------------------------------------------

#[test]
fn worlds_differing_only_in_policy_share_the_context_but_not_the_registration() {
    let default_world = world_for("world-x", ElasticPolicyV1::default());
    let focus_world = world_for("world-x", policy_focus_one());
    // V094.c: the policy is not part of the world's compatibility signature ...
    assert_eq!(
        default_world.context_signature,
        focus_world.context_signature
    );
    // ... but it is part of what was registered.
    assert_ne!(
        default_world.registration_fingerprint().unwrap(),
        focus_world.registration_fingerprint().unwrap()
    );
    assert_ne!(
        default_world.policy.digest().unwrap(),
        focus_world.policy.digest().unwrap()
    );
    assert_eq!(
        default_world.caps.digest().unwrap(),
        focus_world.caps.digest().unwrap()
    );
}

#[tokio::test]
async fn every_pure_decision_carries_the_digests_of_the_policy_and_caps_it_used() {
    let env = env().await;
    env.register("world-default", ElasticPolicyV1::default())
        .await;
    env.register("world-focus", policy_focus_one()).await;
    let mut small_caps = world_for("world-small-caps", ElasticPolicyV1::default());
    small_caps.caps.max_nodes = 6;
    env.coordinator.register_world(small_caps).await.unwrap();

    let default_world = world_for("world-default", ElasticPolicyV1::default());
    let focus_world = world_for("world-focus", policy_focus_one());
    let default_decision = env.coordinator.decide_next("world-default").await.unwrap();
    let focus_decision = env.coordinator.decide_next("world-focus").await.unwrap();
    assert_eq!(
        default_decision.policy_digest,
        default_world.policy.digest().unwrap()
    );
    assert_eq!(
        default_decision.caps_digest,
        default_world.caps.digest().unwrap()
    );
    assert_eq!(
        focus_decision.policy_digest,
        focus_world.policy.digest().unwrap()
    );
    assert_eq!(
        focus_decision.caps_digest,
        focus_world.caps.digest().unwrap()
    );
    // Two worlds that differ only in the policy: same caps digest, different
    // policy digest (the first decision itself is the same action).
    assert_ne!(default_decision.policy_digest, focus_decision.policy_digest);
    assert_eq!(default_decision.caps_digest, focus_decision.caps_digest);
    assert_eq!(
        serde_json::to_value(&default_decision.action).unwrap(),
        serde_json::to_value(&focus_decision.action).unwrap()
    );
    // Different caps give a different caps digest, taken from the world.
    let caps_decision = env
        .coordinator
        .decide_next("world-small-caps")
        .await
        .unwrap();
    assert_ne!(caps_decision.caps_digest, default_decision.caps_digest);
    let mut small = ExplorationCapsV1::online();
    small.max_nodes = 6;
    assert_eq!(caps_decision.caps_digest, small.digest().unwrap());
    assert_eq!(caps_decision.policy_digest, default_decision.policy_digest);

    // The read-side view carries the same digests.
    let view = env.coordinator.decision_view("world-focus").await.unwrap();
    assert_eq!(view.decision.policy_digest, focus_decision.policy_digest);
    assert_eq!(view.decision.caps_digest, focus_decision.caps_digest);
}

#[tokio::test]
async fn a_stop_decision_for_a_non_collecting_world_still_carries_the_digests() {
    let env = env().await;
    env.register("world-sealed", policy_focus_one()).await;
    let mut raw = raw_record(&env.store, "exploration_world_v1", "world-sealed").await;
    raw["payload"]["state"] = json!("sealed");
    put_raw_record(&env.store, "exploration_world_v1", "world-sealed", &raw).await;
    let decision = env.coordinator.decide_next("world-sealed").await.unwrap();
    assert!(matches!(
        decision.action,
        BatchActionV1::Stop { ref reason } if reason == "world_not_collecting"
    ));
    let world = world_for("world-sealed", policy_focus_one());
    assert_eq!(decision.policy_digest, world.policy.digest().unwrap());
    assert_eq!(decision.caps_digest, world.caps.digest().unwrap());
}

#[tokio::test]
async fn exploration_start_result_carries_both_digests_and_status_rejects_tampering() {
    let (_dir, store) = seeded_store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let world = world_for("world-start", policy_focus_one());
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            json!({
                "schema_version": "rsia.management.exploration_start.v1",
                "request_key": "meta-inheritance-start",
                "world": serde_json::to_value(&world).unwrap(),
            }),
        )
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let (policy_digest, caps_digest) = match &done.result {
        Some(ManagementResult::ExplorationStarted {
            policy_digest,
            caps_digest,
            ..
        }) => (policy_digest.clone(), caps_digest.clone()),
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(policy_digest, world.policy.digest().unwrap());
    assert_eq!(caps_digest, world.caps.digest().unwrap());
    assert_ne!(policy_digest, ElasticPolicyV1::default().digest().unwrap());
    // The wire shape names both.
    let wire = serde_json::to_value(&done.result).unwrap();
    assert_eq!(wire["policy_digest"], json!(policy_digest));
    assert_eq!(wire["caps_digest"], json!(caps_digest));
    // The read side re-verifies the stored result against the live world.
    dispatcher.status(&admin, &done.id).await.unwrap();

    // Tampering with either digest in the stored result is a Conflict: the
    // status check compares the whole decision, not only the action.
    let original: Value = {
        let mut session = store.session().await.unwrap();
        let value = session.need(&admin, "job", &done.id).await.unwrap();
        session.commit().await.unwrap();
        value
    };
    for field in ["policy_digest", "caps_digest"] {
        let mut tampered = original.clone();
        tampered["result"][field] = json!(hash(b"forged"));
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "job", &done.id, admin.actor(), &tampered)
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(
            matches!(
                dispatcher.status(&admin, &done.id).await,
                Err(Error::Conflict(_))
            ),
            "tampered {field} must be a Conflict"
        );
    }
    // A pre-E14 stored result (no digests at all) is not re-read as if it
    // carried them: it no longer deserializes, so it fails closed instead of
    // being verified against empty digests.
    let mut legacy = original.clone();
    let legacy_result = legacy["result"].as_object_mut().unwrap();
    legacy_result.remove("policy_digest");
    legacy_result.remove("caps_digest");
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &done.id, admin.actor(), &legacy)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(dispatcher.status(&admin, &done.id).await.is_err());
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &done.id, admin.actor(), &original)
        .await
        .unwrap();
    session.commit().await.unwrap();
    dispatcher.status(&admin, &done.id).await.unwrap();
}

async fn wait_terminal(
    dispatcher: &ManagementDispatcher,
    context: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..200 {
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
        tokio::task::yield_now().await;
    }
    panic!("management job did not finish");
}

// ---------------------------------------------------------------------------
// 4. Dispatch fact v2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_dispatch_fact_is_v2_with_the_digests_and_a_v1_fact_is_refused_by_name() {
    let env = env().await;
    env.register("world-fact", policy_focus_one()).await;
    let step = env.dispatch("world-fact", 1, "fact-1").await;
    let world = world_for("world-fact", policy_focus_one());
    // The coordinator's own step result carries the digests too.
    assert_eq!(step.decision.policy_digest, world.policy.digest().unwrap());
    assert_eq!(step.decision.caps_digest, world.caps.digest().unwrap());

    let dispatch_id = step.dispatch_id.clone().unwrap();
    let fact = raw_fact(&env.store, &dispatch_id).await;
    assert_eq!(fact["payload"]["schema_version"], DISPATCH_V2);
    assert_eq!(
        fact["payload"]["decision"]["policy_digest"],
        json!(world.policy.digest().unwrap())
    );
    assert_eq!(
        fact["payload"]["decision"]["caps_digest"],
        json!(world.caps.digest().unwrap())
    );
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-fact")
            .await
            .unwrap()
            .len(),
        1
    );

    // A pre-E14 fact (v1 schema, decision without digests) is refused when it
    // is read, by name; it is not reinterpreted as a v2 fact.
    let mut v1 = fact.clone();
    v1["payload"]["schema_version"] = json!(DISPATCH_V1);
    let decision = v1["payload"]["decision"].as_object_mut().unwrap();
    decision.remove("policy_digest");
    decision.remove("caps_digest");
    put_raw_fact(&env.store, &dispatch_id, &v1).await;
    match env.coordinator.verified_mechanism_usage("world-fact").await {
        Err(Error::Conflict(message)) => {
            assert_eq!(message, "pre-E14 dispatch fact without policy digest");
        }
        other => panic!("expected the pre-E14 refusal, got {other:?}"),
    }
    // The resume/reconnect path reads the same fact and refuses it too.
    match env
        .coordinator
        .run_next(
            None,
            None,
            None,
            env.fixture.request("world-fact", 1, "fact-1"),
        )
        .await
    {
        Err(Error::Conflict(message)) => {
            assert_eq!(message, "pre-E14 dispatch fact without policy digest");
        }
        other => panic!("expected the pre-E14 refusal on reconnect, got {other:?}"),
    }
    // Relabelling alone is not enough either: a v1 label over a fact that has
    // digests, and a v2 label over a fact that has none, are both refused.
    let mut relabelled = fact.clone();
    relabelled["payload"]["schema_version"] = json!(DISPATCH_V1);
    put_raw_fact(&env.store, &dispatch_id, &relabelled).await;
    assert!(matches!(
        env.coordinator.verified_mechanism_usage("world-fact").await,
        Err(Error::Conflict(_))
    ));
    let mut missing_digests = v1.clone();
    missing_digests["payload"]["schema_version"] = json!(DISPATCH_V2);
    put_raw_fact(&env.store, &dispatch_id, &missing_digests).await;
    assert!(
        env.coordinator
            .verified_mechanism_usage("world-fact")
            .await
            .is_err()
    );
    let mut unknown_schema = fact.clone();
    unknown_schema["payload"]["schema_version"] = json!("rsia.exploration_dispatch.v3");
    put_raw_fact(&env.store, &dispatch_id, &unknown_schema).await;
    assert!(matches!(
        env.coordinator.verified_mechanism_usage("world-fact").await,
        Err(Error::Conflict(_))
    ));
    // Nothing above touched the world record or the pure decision.
    env.coordinator.decide_next("world-fact").await.unwrap();
    // Restoring the original fact restores the view.
    put_raw_fact(&env.store, &dispatch_id, &fact).await;
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-fact")
            .await
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// 6. MechanismUsageRecordV1
// ---------------------------------------------------------------------------

/// Two real dispatches under `max_focus_actions = 1`: the first is the lowest
/// root, the second must switch to the other root because one focused action
/// was already spent. Returns the two step results.
async fn two_dispatches(
    env: &Env,
    world_id: &str,
) -> (CoordinatorStepResult, CoordinatorStepResult) {
    env.register(world_id, policy_focus_one()).await;
    let first = env.dispatch(world_id, 1, &format!("{world_id}-1")).await;
    let second = env.dispatch(world_id, 2, &format!("{world_id}-2")).await;
    (first, second)
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

#[tokio::test]
async fn each_real_dispatch_under_a_non_default_policy_yields_one_verified_record() {
    let env = env().await;
    // (b) A world that is only registered, and one that only took pure
    // decisions, have no dispatch and therefore no mechanism usage.
    env.register("world-idle", policy_focus_one()).await;
    assert!(
        env.coordinator
            .verified_mechanism_usage("world-idle")
            .await
            .unwrap()
            .is_empty()
    );
    env.coordinator.decide_next("world-idle").await.unwrap();
    env.coordinator.decision_view("world-idle").await.unwrap();
    assert!(
        env.coordinator
            .verified_mechanism_usage("world-idle")
            .await
            .unwrap()
            .is_empty()
    );

    // (a) Real dispatches under a non-default policy.
    let (first, second) = two_dispatches(&env, "world-focus").await;
    let world = world_for("world-focus", policy_focus_one());
    assert_eq!(dispatched_seq(&first.decision.action), 1);
    assert_eq!(
        dispatched_seq(&second.decision.action),
        2,
        "max_focus_actions = 1 switches to the other root after one focused action"
    );
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-focus")
        .await
        .unwrap();
    assert_eq!(usage.len(), 2, "one record per real dispatch");
    for (record, step) in usage.iter().zip([&first, &second]) {
        assert_eq!(record.world_id(), "world-focus");
        assert_eq!(record.dispatch_id(), step.dispatch_id.as_deref().unwrap());
        assert_eq!(record.context_signature(), world.context_signature);
        assert_eq!(
            record.approved_parent_digest(),
            world.approved_parent_digest
        );
        assert_eq!(record.policy_digest(), world.policy.digest().unwrap());
        assert_ne!(
            record.policy_digest(),
            ElasticPolicyV1::default().digest().unwrap()
        );
        assert_eq!(record.caps_digest(), world.caps.digest().unwrap());
        assert_eq!(record.prefix_digest(), step.decision.prefix_digest);
        assert_eq!(
            record.legal_actions_digest(),
            step.decision.legal_actions_digest
        );
        assert_eq!(
            record.action_digest(),
            fingerprint(&step.decision.action).unwrap()
        );
        assert_eq!(record.dispatch_state(), MechanismUsageState::Observed);
        assert_eq!(record.node_id(), step.node_id.as_deref());
    }
    assert_ne!(
        usage[0].action_digest(),
        usage[1].action_digest(),
        "the two dispatched actions differ"
    );
    // The record is a serializable view only; it carries no authority.
    let wire = serde_json::to_value(&usage[0]).unwrap();
    assert_eq!(wire["dispatch_state"], "observed");
    assert_eq!(wire["policy_digest"], json!(world.policy.digest().unwrap()));

    // A reconnect with the same request returns the stored dispatch and does
    // not count a second use.
    let again = env
        .coordinator
        .run_next(
            None,
            None,
            None,
            env.fixture.request("world-focus", 1, "world-focus-1"),
        )
        .await
        .unwrap();
    assert_eq!(again.dispatch_id, first.dispatch_id);
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-focus")
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn the_policy_really_changes_the_dispatched_action() {
    // V086.d at the coordinator level: the same revealed first dispatch, then
    // two worlds that differ only in `max_focus_actions` get different next
    // actions from the one shared `decide_elastic`, and the record names the
    // policy that was used.
    let env = env().await;
    env.register("world-default", ElasticPolicyV1::default())
        .await;
    env.register("world-focus", policy_focus_one()).await;
    let default_first = env.dispatch("world-default", 1, "default-1").await;
    let focus_first = env.dispatch("world-focus", 1, "focus-1").await;
    assert_eq!(dispatched_seq(&default_first.decision.action), 1);
    assert_eq!(dispatched_seq(&focus_first.decision.action), 1);
    let default_next = env.coordinator.decide_next("world-default").await.unwrap();
    let focus_next = env.coordinator.decide_next("world-focus").await.unwrap();
    // Default: focus the branch that just gained (deepen node 1). Restricted to
    // one focused action: switch to the other root.
    assert_eq!(dispatched_seq(&default_next.action), 1_000_002);
    assert_eq!(dispatched_seq(&focus_next.action), 2);
    assert_ne!(default_next.policy_digest, focus_next.policy_digest);
    assert_eq!(default_next.caps_digest, focus_next.caps_digest);
    // Their usage records carry their own policies and cannot be swapped.
    let default_usage = env
        .coordinator
        .verified_mechanism_usage("world-default")
        .await
        .unwrap();
    let focus_usage = env
        .coordinator
        .verified_mechanism_usage("world-focus")
        .await
        .unwrap();
    assert_eq!(default_usage.len(), 1);
    assert_eq!(focus_usage.len(), 1);
    assert_eq!(
        default_usage[0].policy_digest(),
        ElasticPolicyV1::default().digest().unwrap()
    );
    assert_eq!(
        focus_usage[0].policy_digest(),
        policy_focus_one().digest().unwrap()
    );
}

#[tokio::test]
async fn uncertain_dispatches_count_and_claimed_ones_do_not() {
    let env = env().await;

    // Uncertain: the dispatch was claimed and the model was called, but the
    // outcome is unknown. The cost is already incurred, so it counts.
    env.register("world-uncertain", policy_focus_one()).await;
    let uncertain = env
        .coordinator
        .run_next(
            Some(&FailingModel),
            Some(&ImprovingFixtureRunner),
            Some(&env.journal),
            env.fixture.request("world-uncertain", 1, "uncertain-1"),
        )
        .await
        .unwrap();
    assert!(
        uncertain.outcome.contains("not found"),
        "{}",
        uncertain.outcome
    );
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-uncertain")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Uncertain);
    assert_eq!(usage[0].node_id(), uncertain.node_id.as_deref());
    assert_eq!(
        serde_json::to_value(&usage[0]).unwrap()["dispatch_state"],
        "uncertain"
    );

    // Claimed: one observed dispatch, then a second one that is claimed and
    // never observed (the future is dropped once the model was entered).
    env.register("world-mixed", policy_focus_one()).await;
    let observed = env.dispatch("world-mixed", 1, "mixed-1").await;
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
    let world = raw_record(&env.store, "exploration_world_v1", "world-mixed").await;
    let dispatch_ids = world["payload"]["dispatch_ids"].as_array().unwrap().clone();
    assert_eq!(dispatch_ids.len(), 2, "the claimed dispatch is listed");
    let claimed = raw_fact(&env.store, dispatch_ids[1].as_str().unwrap()).await;
    assert_eq!(claimed["payload"]["state"], "claimed");
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-mixed")
        .await
        .unwrap();
    assert_eq!(usage.len(), 1, "only the observed dispatch counts");
    assert_eq!(
        usage[0].dispatch_id(),
        observed.dispatch_id.as_deref().unwrap()
    );
    assert_eq!(usage[0].dispatch_state(), MechanismUsageState::Observed);

    // A claimed fact is still cross-checked: one that does not carry the
    // world's policy fails the whole view instead of being skipped silently.
    let mut forged_claim = claimed.clone();
    forged_claim["payload"]["decision"]["policy_digest"] = json!(hash(b"another-policy"));
    put_raw_fact(&env.store, dispatch_ids[1].as_str().unwrap(), &forged_claim).await;
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-mixed")
            .await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn a_tampered_or_dangling_fact_fails_the_whole_view() {
    let env = env().await;
    let (first, second) = two_dispatches(&env, "world-tamper").await;
    let second_id = second.dispatch_id.clone().unwrap();
    let second_node = second.node_id.clone().unwrap();
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-tamper")
            .await
            .unwrap()
            .len(),
        2
    );
    let original = raw_fact(&env.store, &second_id).await;
    let original_world = raw_record(&env.store, "exploration_world_v1", "world-tamper").await;

    type Tamper = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Tamper)> = vec![
        (
            "decision.policy_digest",
            Box::new(|fact| fact["payload"]["decision"]["policy_digest"] = json!(hash(b"x"))),
        ),
        (
            "decision.caps_digest",
            Box::new(|fact| fact["payload"]["decision"]["caps_digest"] = json!(hash(b"x"))),
        ),
        (
            "fact world_id",
            Box::new(|fact| fact["payload"]["world_id"] = json!("world-other")),
        ),
        (
            "fact context_signature",
            Box::new(|fact| fact["payload"]["context_signature"] = json!(hash(b"x"))),
        ),
        (
            "decision world_id",
            Box::new(|fact| fact["payload"]["decision"]["world_id"] = json!("world-other")),
        ),
        (
            "fact id",
            Box::new(|fact| fact["payload"]["id"] = json!("dispatch-other")),
        ),
        (
            "action_id no longer matches the decision",
            Box::new(|fact| fact["payload"]["action_id"] = json!("root-9")),
        ),
        (
            "decision action replaced by a stop",
            Box::new(|fact| {
                fact["payload"]["decision"]["action"] =
                    json!({"decision": "stop", "reason": "forged"});
            }),
        ),
        (
            "observed fact without a node",
            Box::new(|fact| fact["payload"]["node_id"] = Value::Null),
        ),
        (
            "node not listed by the world",
            Box::new(|fact| fact["payload"]["node_id"] = json!("node-world-tamper-99")),
        ),
    ];
    for (label, tamper) in &cases {
        let mut tampered = original.clone();
        tamper(&mut tampered);
        put_raw_fact(&env.store, &second_id, &tampered).await;
        assert!(
            matches!(
                env.coordinator
                    .verified_mechanism_usage("world-tamper")
                    .await,
                Err(Error::Conflict(_))
            ),
            "{label}: the whole view must fail with a Conflict"
        );
        put_raw_fact(&env.store, &second_id, &original).await;
        assert_eq!(
            env.coordinator
                .verified_mechanism_usage("world-tamper")
                .await
                .unwrap()
                .len(),
            2,
            "{label}: restoring the fact restores the view"
        );
    }

    // (d) An observed fact whose node is listed by the world but has no
    // persisted node record.
    let mut dangling_fact = original.clone();
    dangling_fact["payload"]["node_id"] = json!("node-world-tamper-dangling");
    let mut dangling_world = original_world.clone();
    dangling_world["payload"]["node_ids"]
        .as_array_mut()
        .unwrap()
        .push(json!("node-world-tamper-dangling"));
    put_raw_fact(&env.store, &second_id, &dangling_fact).await;
    put_raw_record(
        &env.store,
        "exploration_world_v1",
        "world-tamper",
        &dangling_world,
    )
    .await;
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-tamper")
            .await,
        Err(Error::NotFound)
    ));
    put_raw_fact(&env.store, &second_id, &original).await;
    put_raw_record(
        &env.store,
        "exploration_world_v1",
        "world-tamper",
        &original_world,
    )
    .await;

    // A dispatch listed twice would count one real use twice.
    let mut doubled = original_world.clone();
    let first_id = first.dispatch_id.clone().unwrap();
    doubled["payload"]["dispatch_ids"]
        .as_array_mut()
        .unwrap()
        .push(json!(first_id));
    put_raw_record(&env.store, "exploration_world_v1", "world-tamper", &doubled).await;
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-tamper")
            .await,
        Err(Error::Conflict(_))
    ));
    put_raw_record(
        &env.store,
        "exploration_world_v1",
        "world-tamper",
        &original_world,
    )
    .await;

    // A node that belongs to another world.
    env.register("world-neighbour", policy_focus_one()).await;
    let neighbour = env.dispatch("world-neighbour", 1, "neighbour-1").await;
    let mut foreign = original.clone();
    foreign["payload"]["node_id"] = json!(neighbour.node_id.clone().unwrap());
    let mut foreign_world = original_world.clone();
    foreign_world["payload"]["node_ids"]
        .as_array_mut()
        .unwrap()
        .push(json!(neighbour.node_id.clone().unwrap()));
    put_raw_fact(&env.store, &second_id, &foreign).await;
    put_raw_record(
        &env.store,
        "exploration_world_v1",
        "world-tamper",
        &foreign_world,
    )
    .await;
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-tamper")
            .await,
        Err(Error::Conflict(_))
    ));
    put_raw_fact(&env.store, &second_id, &original).await;
    put_raw_record(
        &env.store,
        "exploration_world_v1",
        "world-tamper",
        &original_world,
    )
    .await;
    assert!(second_node.starts_with("node-world-tamper-"));
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-tamper")
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn revoked_sources_fail_the_usage_view_closed() {
    let env = env().await;
    env.register("world-revoked", policy_focus_one()).await;
    env.dispatch("world-revoked", 1, "revoked-1").await;
    assert_eq!(
        env.coordinator
            .verified_mechanism_usage("world-revoked")
            .await
            .unwrap()
            .len(),
        1
    );
    // A tombstoned trusted run of the source closure: Forbidden.
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let mut session = env.store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "run-failure",
            "admin",
            &json!({"id": "run-failure"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-revoked")
            .await,
        Err(Error::Forbidden)
    ));
    // A watermark bump (the closure changed): Conflict.
    let mut session = env.store.session().await.unwrap();
    session
        .bump_watermark(&admin, "meta-inheritance-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-revoked")
            .await,
        Err(Error::Conflict(_))
    ));
    // An unknown world is not found, not empty.
    assert!(matches!(
        env.coordinator
            .verified_mechanism_usage("world-missing")
            .await,
        Err(Error::NotFound)
    ));
}

// ---------------------------------------------------------------------------
// 7. meta.rs: candidate validation and inheritance verification
// ---------------------------------------------------------------------------

fn i0() -> ImproverContentV2 {
    ImproverContentV2::builtin_default()
}

fn i1() -> ImproverContentV2 {
    let mut content = ImproverContentV2::builtin_default();
    let ImproverMechanismV2::ExplorationPolicy { policy } = &mut content.mechanism;
    *policy = policy_focus_one();
    content
}

#[test]
fn candidate_validation_rejects_flags_depth_no_change_and_bad_parent_digests() {
    let parent = i0().content_digest().unwrap();
    let candidate = MetaCandidate {
        depth: 1,
        content: i1(),
        parent_improver_digest: parent.clone(),
    };
    candidate.validate().unwrap();
    assert_eq!(META_DEPTH_CAP, 1);

    // The boolean the plan refuses to accept as evidence is not a field.
    let mut with_flag = serde_json::to_value(&candidate).unwrap();
    with_flag["used_in_next_job"] = json!(true);
    assert!(serde_json::from_value::<MetaCandidate>(with_flag).is_err());
    let mut old_shape = serde_json::to_value(&candidate).unwrap();
    old_shape["mechanism"] = json!("exploration");
    assert!(serde_json::from_value::<MetaCandidate>(old_shape).is_err());
    // A candidate with another field nested in its content is not accepted.
    let mut nested = serde_json::to_value(&candidate).unwrap();
    nested["content"]["mechanism"]["policy"]["max_nodes"] = json!(99);
    assert!(serde_json::from_value::<MetaCandidate>(nested).is_err());
    // The round trip of a valid candidate is accepted.
    let round_trip: MetaCandidate =
        serde_json::from_value(serde_json::to_value(&candidate).unwrap()).unwrap();
    round_trip.validate().unwrap();

    // Recursion depth is capped at 1.
    let too_deep = MetaCandidate {
        depth: 2,
        content: i1(),
        parent_improver_digest: parent.clone(),
    };
    assert!(matches!(too_deep.validate(), Err(Error::Invalid(_))));

    // A candidate identical to its parent carries nothing to inherit.
    let unchanged = MetaCandidate {
        depth: 1,
        content: i0(),
        parent_improver_digest: parent.clone(),
    };
    assert!(matches!(unchanged.validate(), Err(Error::Invalid(_))));
    // A non-default parent is fine as long as the content differs from it.
    let other_parent = MetaCandidate {
        depth: 1,
        content: i0(),
        parent_improver_digest: i1().content_digest().unwrap(),
    };
    other_parent.validate().unwrap();

    // The parent must be a lowercase sha256 digest.
    for bad in [
        "".to_string(),
        "i0".to_string(),
        parent.to_uppercase(),
        parent[..63].to_string(),
        format!("{parent}0"),
        format!("g{}", &parent[1..]),
    ] {
        let candidate = MetaCandidate {
            depth: 1,
            content: i1(),
            parent_improver_digest: bad.clone(),
        };
        assert!(
            matches!(candidate.validate(), Err(Error::Invalid(_))),
            "{bad:?}"
        );
    }

    // The content is validated with the same bounds as a parsed one.
    let mut out_of_bound = i1();
    let ImproverMechanismV2::ExplorationPolicy { policy } = &mut out_of_bound.mechanism;
    policy.fairness_wait_rounds = 12;
    let candidate = MetaCandidate {
        depth: 1,
        content: out_of_bound,
        parent_improver_digest: parent,
    };
    assert!(matches!(candidate.validate(), Err(Error::Invalid(_))));
}

#[test]
fn protected_fields_stay_outside_what_an_improver_may_write() {
    cannot_write_protected("improver.exploration_policy.max_focus_actions").unwrap();
    cannot_write_protected("improver.instruction").unwrap();
    for protected in ["budget", "approval", "namespace"] {
        assert!(matches!(
            cannot_write_protected(protected),
            Err(Error::Forbidden)
        ));
    }
    for control_plane in [
        "improver.exploration_policy.max_nodes",
        "improver.exploration_policy.w_online",
        "improver.exploration_policy.simulation",
        "grader",
    ] {
        assert!(matches!(
            cannot_write_protected(control_plane),
            Err(Error::Invalid(_))
        ));
    }
}

#[tokio::test]
async fn inheritance_is_shown_by_real_usage_of_the_approved_mechanism_only() {
    let env = env().await;
    let (first, second) = two_dispatches(&env, "world-inherit").await;
    let usage = env
        .coordinator
        .verified_mechanism_usage("world-inherit")
        .await
        .unwrap();
    assert_eq!(usage.len(), 2);

    // I1 (the approved mechanism) is the one the job decided with: verified.
    let evidence = verify_mechanism_inheritance(&i1(), &usage).unwrap();
    assert_eq!(
        evidence.approved_content_digest,
        i1().content_digest().unwrap()
    );
    assert_eq!(evidence.policy_digest, policy_focus_one().digest().unwrap());
    assert_eq!(
        evidence.caps_digest,
        ExplorationCapsV1::online().digest().unwrap()
    );
    assert_eq!(evidence.world_id, "world-inherit");
    assert_eq!(evidence.decisions, 2);
    assert_eq!(evidence.distinct_action_digests, 2);
    assert_ne!(
        fingerprint(&first.decision.action).unwrap(),
        fingerprint(&second.decision.action).unwrap()
    );
    let wire = serde_json::to_value(&evidence).unwrap();
    assert_eq!(wire["decisions"], 2);
    assert_eq!(wire["world_id"], "world-inherit");
    assert_eq!(wire["policy_digest"], json!(evidence.policy_digest));

    // The built-in mechanism (I0) did not produce those decisions: Conflict.
    match verify_mechanism_inheritance(&i0(), &usage) {
        Err(Error::Conflict(message)) => {
            assert_eq!(
                message,
                "job used a different mechanism than the approved improver"
            );
        }
        other => panic!("expected a Conflict for I0, got {other:?}"),
    }

    // No real dispatched decision: nothing to verify.
    match verify_mechanism_inheritance(&i1(), &[]) {
        Err(Error::Invalid(message)) => {
            assert_eq!(
                message,
                "no real dispatched decision used the approved mechanism"
            );
        }
        other => panic!("expected Invalid for an empty usage, got {other:?}"),
    }
    // A world that was only registered and decided has no usage either.
    env.register("world-pure", policy_focus_one()).await;
    env.coordinator.decide_next("world-pure").await.unwrap();
    let pure = env
        .coordinator
        .verified_mechanism_usage("world-pure")
        .await
        .unwrap();
    assert!(matches!(
        verify_mechanism_inheritance(&i1(), &pure),
        Err(Error::Invalid(_))
    ));

    // Records from another world cannot be mixed in, even when that world used
    // the very same policy.
    env.register("world-neighbour", policy_focus_one()).await;
    env.dispatch("world-neighbour", 1, "neighbour-1").await;
    let neighbour = env
        .coordinator
        .verified_mechanism_usage("world-neighbour")
        .await
        .unwrap();
    assert_eq!(neighbour.len(), 1);
    let mixed: Vec<MechanismUsageRecordV1> = vec![usage[0].clone(), neighbour[0].clone()];
    assert!(matches!(
        verify_mechanism_inheritance(&i1(), &mixed),
        Err(Error::Invalid(_))
    ));

    // A world that ran the built-in policy verifies I0 and refuses I1.
    env.register("world-builtin", ElasticPolicyV1::default())
        .await;
    env.dispatch("world-builtin", 1, "builtin-1").await;
    let builtin = env
        .coordinator
        .verified_mechanism_usage("world-builtin")
        .await
        .unwrap();
    let evidence = verify_mechanism_inheritance(&i0(), &builtin).unwrap();
    assert_eq!(evidence.decisions, 1);
    assert_eq!(evidence.distinct_action_digests, 1);
    assert!(matches!(
        verify_mechanism_inheritance(&i1(), &builtin),
        Err(Error::Conflict(_))
    ));

    // The approved content is validated before it is compared.
    let mut invalid = i1();
    invalid.schema_version = "rsia.improver_content.v1".into();
    assert!(matches!(
        verify_mechanism_inheritance(&invalid, &usage),
        Err(Error::Invalid(_))
    ));
}

// ---------------------------------------------------------------------------
// 8. The replay report names the policy by the same digest
// ---------------------------------------------------------------------------

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

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

fn replay_action(seq: u32, parent: &str, branch: u32, kind: ActionKindV1) -> ReplayActionSpecV1 {
    ReplayActionSpecV1 {
        record_seq: seq,
        generation_signature: d("generation-v1"),
        parent_context_signature: parent.into(),
        branch_seq: branch,
        target_depth: if matches!(kind, ActionKindV1::Widen { .. }) {
            1
        } else {
            2
        },
        action_kind: kind,
        estimated_cost_upper_micros: Some(10),
        writes_shared_workspace: false,
    }
}

fn replay_transition(
    seq: u32,
    id: &str,
    parent: &str,
    next: &str,
    kind: ActionKindV1,
    quality: u32,
) -> ReplayTransitionV2 {
    ReplayTransitionV2 {
        record_id: id.into(),
        record_seq: seq,
        generation_signature: d("generation-v1"),
        parent_context_signature: parent.into(),
        action_kind: kind,
        next_context_signature: next.into(),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: quality,
            },
        },
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_micros: Some(1),
            latency_millis: Some(1),
        },
        source_ids: vec![format!("source-{seq}")],
        observation_source_id: format!("source-{seq}"),
    }
}

fn replay_world() -> ReplayWorldV2 {
    let actions = vec![
        replay_action(1, &d("baseline"), 1, ActionKindV1::Widen { root_slot: 1 }),
        replay_action(
            2,
            &d("ctx-1"),
            1,
            ActionKindV1::Deepen { parent_node_seq: 1 },
        ),
    ];
    let transitions = vec![
        replay_transition(
            1,
            "opaque-root",
            &d("baseline"),
            &d("ctx-1"),
            ActionKindV1::Widen { root_slot: 1 },
            400_000,
        ),
        replay_transition(
            2,
            "opaque-child",
            &d("ctx-1"),
            &d("ctx-2"),
            ActionKindV1::Deepen { parent_node_seq: 1 },
            800_000,
        ),
    ];
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
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-source-1".into(),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: "source-1".into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: "source-2".into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: "baseline-source-1".into(),
                    content_digest: d("pending-baseline"),
                },
            ],
            revoke_watermark: 7,
            prefix_coverage: vec![
                PrefixCoverageV1 {
                    context_signature: d("baseline"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: d("ctx-1"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: d("ctx-2"),
                    exhausted: true,
                },
            ],
            action_catalog: actions,
        },
        transitions,
        sealed_digest: None,
    };
    refresh_evidence_digests(&mut world);
    world.seal().unwrap();
    world
}

fn replay_profile() -> ReplaySimulationProfile {
    ReplaySimulationProfile {
        simulation_version: SIMULATION_VERSION.into(),
        objective: ReplayObjective::ParetoAttainmentV2,
        w_sim: 1,
        probe_budget: 2,
        horizon: 2,
        lambda_work_micros: DEFAULT_LAMBDA_MICROS,
        lambda_round_micros: DEFAULT_LAMBDA_MICROS,
        fixed_seed: 9,
        global_recovery_dispatch_limit: 1,
        pool_digest: d("pool"),
        purpose: Purpose::Development,
        target_runtime_profile: "simulation-only".into(),
    }
}

fn replay_authority() -> LiveWorldAuthority {
    LiveWorldAuthority {
        revoke_watermark: 7,
        revoked_source_ids: BTreeSet::new(),
    }
}

#[test]
fn the_replay_report_names_the_policy_by_the_same_digest_and_the_same_decide() {
    let caps = ExplorationCapsV1::online();
    for policy in [ElasticPolicyV1::default(), policy_focus_one()] {
        policy.validate().unwrap();
        let report = run_replay(
            &replay_world(),
            &policy,
            &replay_profile(),
            &caps,
            &replay_authority(),
        )
        .unwrap();
        assert_eq!(report.policy_digest, policy.digest().unwrap());
        assert_eq!(report.policy_digest, fingerprint(&policy).unwrap());
        // The online and replay paths use one digest convention.
        assert_eq!(report.batches[0].action_seqs, vec![1]);
    }
    let default_report = run_replay(
        &replay_world(),
        &ElasticPolicyV1::default(),
        &replay_profile(),
        &caps,
        &replay_authority(),
    )
    .unwrap();
    let focus_report = run_replay(
        &replay_world(),
        &policy_focus_one(),
        &replay_profile(),
        &caps,
        &replay_authority(),
    )
    .unwrap();
    assert_ne!(default_report.policy_digest, focus_report.policy_digest);

    // V086.c: only a future quality and an opaque id differ; the first choice
    // under a non-default policy does not move.
    let original = replay_world();
    let first = run_replay(
        &original,
        &policy_focus_one(),
        &replay_profile(),
        &caps,
        &replay_authority(),
    )
    .unwrap();
    let mut poisoned = original.clone();
    poisoned.sealed_digest = None;
    poisoned.transitions[1].record_id = "renamed-future".into();
    poisoned.transitions[1].outcome = ReplayTransitionOutcome::Observed {
        status: ObservedStatus::Valid { quality_micros: 1 },
    };
    poisoned.manifest.source_closure[1].content_digest =
        replay_observation_digest(&poisoned.manifest, &poisoned.transitions[1]).unwrap();
    poisoned.seal().unwrap();
    let second = run_replay(
        &poisoned,
        &policy_focus_one(),
        &replay_profile(),
        &caps,
        &replay_authority(),
    )
    .unwrap();
    assert_eq!(first.batches[0].action_seqs, second.batches[0].action_seqs);
    assert_eq!(first.batches[0].action_seqs, vec![1]);
    assert_eq!(first.policy_digest, second.policy_digest);
}
