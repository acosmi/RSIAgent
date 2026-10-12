//! AG-103 (E09/E10; plan §7.1.1): online exploration moves its fairness waits only over
//! the legal opportunities of the decision round a dispatch completes: the structural
//! actions `derive_legal_actions` offers that evo-core's shared filter
//! (`legal_candidates`) leaves legal on the prefix, budget and caps of that decision,
//! the filter the replay moves its waits over. A root whose cost is over the remaining
//! root budget is offered but is no opportunity: it neither waits nor stays among the
//! waits. A world without such a root keeps exactly the waits it had. The decisions, the
//! actions offered and the stop are unchanged.
//!
//! Not declared: a boundary, recorded as online exploration behaves, not claimed as a
//! contract. An action that becomes ineligible only after the last completed round (a
//! root the last dispatch left over the remaining root budget, for instance) keeps the
//! wait entry it had, as before this change: no later round judges it. The replay's
//! other boundary, an action entering the waits at zero before any round judged it, has
//! no online counterpart, since online waits list only actions offered in a completed
//! round. That the chosen action keeps the entry it had until the next completed round
//! is how the coordinator always kept its waits; this change leaves it as it was.
//!
//! Real SQLite store, a fixture model that suggests no edit (so every step ends with no
//! change, in a terminal node that cannot be deepened) and a runner that is never
//! reached; no provider, zero monetary cost. The harness is copied from
//! `exploration_postpaid_v42.rs` and `exploration_v41.rs`, which are unchanged.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace, SkillFailureDiagnosis,
    SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, LegalActionV1, LegalActionsV1,
    SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    CoordinatorStepResult, ExplorationDependency, ExplorationWorldV1, PersistentCoordinator,
    RootOpportunity, WorldState,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentManifest, DevelopmentRunReport,
    DevelopmentRunRequest, DevelopmentTask, OptimizationStepRequest,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use trusted_compiler_inputs::StoreOptimizationJournal;

/// The root budget every world here is registered with.
const REGISTERED_ROOT_MICROS: u64 = 1_000;
const FIRST_ROOT_COST: u64 = 10;
const STOP_REASON: &str = "no_legal_action_with_authorized_cost";
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

// The registered world freezes the same explicit Admin empty root as its
// optimization request. Search nodes still carry their own complete Skill.
fn actual_fixture_parent_digest() -> String {
    trusted_compiler_inputs::inputs(
        "n",
        "profile",
        parent_skill(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &evo_core::hash(b"environment"),
    )
    .profile
    .parent_digest
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

/// One Host-issued trusted run.
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
    let store = Store::open(&dir.path().join("exploration-fairness-waits.sqlite3"))
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

/// A model port that answers both reflection stages with no edit suggestion, and
/// counts its calls (a call is the step being paid for).
#[derive(Default)]
struct NoChangeModel(AtomicUsize);

#[async_trait]
impl ModelPort for NoChangeModel {
    // This is an explicit deterministic fixture adapter. Its receipts retain
    // Fixture provenance; the production broker verifies the full use itself.
    async fn dispatch_bound(
        &self,
        request: evo_engine::model::BoundModelRequest,
    ) -> evo_core::Result<evo_engine::model::ModelResponse> {
        request.validate()?;
        self.dispatch(request.request).await
    }

    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
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

struct UnusedRunner;

#[async_trait]
impl DevRunner for UnusedRunner {
    async fn run(&self, _: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Err(Error::Invalid(
            "no-change fixture must not run development".into(),
        ))
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
            actual_fixture_parent_digest(),
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
                parent_digest: actual_fixture_parent_digest(),
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

    /// A request against the world's own parent skill/bundle, i.e. for a `Widen`
    /// action. `tag` keeps the request/idempotency ids distinct.
    fn request(&self, world_id: &str, step: u32, tag: &str) -> OptimizationStepRequest<'_> {
        trusted_compiler_inputs::request(OptimizationStepRequest {
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
                approved_parent_digest: actual_fixture_parent_digest(),
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
        })
    }
}

/// A world registered with a root budget of 1000 and two roots: root 1 costs 10 and
/// root 2 (branch 2) costs `second_root_cost`. A no-change step ends in a node that
/// cannot be deepened, so the roots are all the world ever offers.
fn world(id: &str, second_root_cost: u64) -> ExplorationWorldV1 {
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
        approved_parent_digest: actual_fixture_parent_digest(),
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
                estimated_cost_upper_micros: second_root_cost,
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
        successor_cost_upper_micros: 15,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: REGISTERED_ROOT_MICROS,
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

/// The digest a decision records for the legal actions offered when only root 2 is
/// left, the root as `derive_legal_actions` offers it.
fn only_second_root_offered(cost: u64) -> String {
    fingerprint(&LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: vec![LegalActionV1 {
            action_id: "root-2".into(),
            action_seq: 2,
            branch_seq: 2,
            target_depth: 1,
            kind: ActionKindV1::Widen { root_slot: 2 },
            estimated_cost_upper_micros: Some(cost),
        }],
    })
    .unwrap()
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

fn worker() -> Context {
    Context::new("n", "worker", Role::Worker).unwrap()
}

async fn raw_record(store: &Store, record_kind: &str, id: &str) -> Value {
    let storage_id = format!("e09-{}", fingerprint(&(record_kind, id)).unwrap());
    let mut session = store.session().await.unwrap();
    let value: Value = session
        .need(&worker(), "artifact", &storage_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
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
    Env {
        coordinator: PersistentCoordinator::new(store.clone(), worker(), "worker").unwrap(),
        journal: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
        fixture: Fixture::new(),
        store,
        _dir: dir,
    }
}

impl Env {
    /// One real dispatch through `run_next`: the step is paid for (the model is asked)
    /// and ends with no change, and the dispatch is settled.
    async fn dispatch(&self, world_id: &str, step: u32, tag: &str) -> CoordinatorStepResult {
        let model = NoChangeModel::default();
        let result = self
            .coordinator
            .run_next(
                Some(&model),
                Some(&UnusedRunner),
                Some(&self.journal),
                self.fixture.request(world_id, step, tag),
            )
            .await
            .unwrap();
        assert!(
            model.0.load(Ordering::SeqCst) > 0,
            "the step was not paid for"
        );
        assert!(result.node_id.is_some(), "the dispatch was not settled");
        result
    }

    /// The stored world, as the store holds it.
    async fn world(&self, world_id: &str) -> Value {
        raw_record(&self.store, WORLD_KIND, world_id).await["payload"].clone()
    }

    async fn assert_stopped(&self, world_id: &str, offered: String) {
        let decision = self.coordinator.decide_next(world_id).await.unwrap();
        assert_eq!(decision.legal_actions_digest, offered, "{world_id}");
        match decision.action {
            BatchActionV1::Stop { reason } => assert_eq!(reason, STOP_REASON, "{world_id}"),
            dispatch => panic!("{world_id}: expected the stop, got {dispatch:?}"),
        }
    }
}

#[tokio::test]
async fn a_root_over_the_remaining_budget_neither_waits_nor_stays_among_the_waits() {
    let env = env().await;
    env.coordinator
        .register_world(world("world-over-budget", 2_000))
        .await
        .unwrap();
    // Root 2 costs more than the world may spend: the first legal root is root 1.
    let first = env
        .coordinator
        .decide_next("world-over-budget")
        .await
        .unwrap();
    assert_eq!(dispatched_seq(&first.action), 1);
    let step = env.dispatch("world-over-budget", 1, "over-1").await;
    assert_eq!(dispatched_seq(&step.decision.action), 1);

    let stored = env.world("world-over-budget").await;
    assert_eq!(stored["decision_round"], 1);
    assert_eq!(
        stored["remaining_root_micros"],
        REGISTERED_ROOT_MICROS - FIRST_ROOT_COST
    );
    // Root 2 was offered in round 1 but was no legal opportunity of it: it did not wait
    // the round, and it is not among the waits (it used to wait one round).
    assert_eq!(stored["waits"], json!([]));
    // Root 2 is still offered as the world derives it, and nothing legal is left.
    env.assert_stopped("world-over-budget", only_second_root_offered(2_000))
        .await;
}

#[tokio::test]
async fn without_a_root_over_the_budget_the_waits_are_what_they_were() {
    let env = env().await;
    env.coordinator
        .register_world(world("world-within-budget", 25))
        .await
        .unwrap();
    let first = env.dispatch("world-within-budget", 1, "within-1").await;
    assert_eq!(dispatched_seq(&first.decision.action), 1);
    // Root 2 was a legal opportunity of round 1 that was not chosen: it waited one round.
    let stored = env.world("world-within-budget").await;
    assert_eq!(
        stored["waits"],
        json!([{"action_seq": 2, "waited_rounds": 1}])
    );

    let second = env.dispatch("world-within-budget", 2, "within-2").await;
    assert_eq!(dispatched_seq(&second.decision.action), 2);
    // The round that chose root 2 leaves its entry as it was, as it always did.
    let stored = env.world("world-within-budget").await;
    assert_eq!(stored["decision_round"], 2);
    assert_eq!(
        stored["waits"],
        json!([{"action_seq": 2, "waited_rounds": 1}])
    );
    env.assert_stopped(
        "world-within-budget",
        fingerprint(&LegalActionsV1 {
            schema_version: "rsia.legal_actions.v1".into(),
            actions: vec![],
        })
        .unwrap(),
    )
    .await;
}
