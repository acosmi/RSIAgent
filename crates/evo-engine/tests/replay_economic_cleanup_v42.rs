//! AG-037 (E08/E10/E11; plan §11): a revocation cleanup that reaches a
//! replay-economic experiment completes and keeps the experiment's history, and
//! `start` verifies the experiment's sources again in the session that writes the
//! job. Real SQLite store, no model, no provider, zero monetary cost (the budget
//! calls are bookkeeping rows with fixture amounts).
//!
//! Cleanup side. `register` stores the experiment with an edge to each of its
//! sources and to the replay report it selected from, and `start` stores the job
//! with an edge to the experiment. Revoking a run therefore reaches, in this
//! order, the world that names the run, the pool and the report over that world,
//! the experiment over the report and the job over the experiment. The cleanup
//! classifies a node by `(kind, schema_version, record_kind)`, and the economic
//! envelope (`rsia.replay_economic_artifact_envelope.v1`) was in neither the
//! preserved nor the redacted list, so the job stopped at the experiment with
//! `blocked_unknown_scope:artifact:...` and never completed. The four record
//! kinds of that envelope (experiment, job, cost receipt, report) are historical
//! facts and amounts: they are kept as they were, the world, pool and report they
//! selected are redacted as before, and any other record kind or schema version
//! still fails closed.
//!
//! Start side. `start` verifies the sources in its first session, commits, then
//! reads the live selection and the paired E05 ticket without a session, and only
//! then opens the session that writes the job. A revocation that commits in
//! between moves the revoke watermark, and the write used to go through. The write
//! session now verifies the sources again.
//!
//! The fixtures below are copied from `tests/replay_economics.rs` and from
//! `tests/replay_redacted_reads_v42.rs` / `tests/cleanup_fixpoint_v42.rs` (test
//! crates cannot import each other); the originals are untouched.

use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::replay::*;
use evo_core::replay_economics::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::replay::{LiveWorldAuthority, run_replay};
use evo_engine::replay_experiment::{
    PersistentReplayEconomicCoordinator, ReplayEconomicJobState, ReplayEconomicJobV1,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use evo_storage::replay::{REPLAY_REPORT_RECORD_KIND, replay_pool_storage_id};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const ENVELOPE_SCHEMA: &str = "rsia.replay_economic_artifact_envelope.v1";
const REDACTED: &str = "rsia.redacted.v1";
const EXPERIMENT_KIND: &str = "replay_economic_experiment_v1";
const JOB_KIND: &str = "replay_economic_job_v1";
const COST_KIND: &str = "replay_economic_cost_receipt_v1";
const REPORT_KIND: &str = "replay_economic_report_v1";
/// The run of the Select world: revoking it reaches the whole chain.
const REVOKED_RUN: &str = "run-select";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

// ---------------------------------------------------------------------------
// Fixture: sealed worlds, pool, replay report and the registered experiment
// (copied from tests/replay_economics.rs)
// ---------------------------------------------------------------------------

fn economic_source_body() -> Value {
    json!({"schema_version":"fixture.source.v1","id":"source-1"})
}

fn budget_binding() -> EconomicBudgetBindingV1 {
    let bindings = [
        (
            CostComponentKind::HistoryCollection,
            "group-history",
            EconomicBudgetStage::HistoryCollection,
        ),
        (
            CostComponentKind::PolicyGeneration,
            "group-policy",
            EconomicBudgetStage::CandidateGeneration,
        ),
        (
            CostComponentKind::ReplayCpu,
            "group-replay-cpu",
            EconomicBudgetStage::StorageCpu,
        ),
        (
            CostComponentKind::ReplayStorage,
            "group-replay-storage",
            EconomicBudgetStage::StorageCpu,
        ),
        (
            CostComponentKind::OnlineDevelopmentFixed,
            "group-fixed",
            EconomicBudgetStage::DevelopmentExecution,
        ),
        (
            CostComponentKind::OnlineDevelopmentReplaySelected,
            "group-replay",
            EconomicBudgetStage::DevelopmentExecution,
        ),
        (
            CostComponentKind::IndependentAcceptance,
            "group-acceptance",
            EconomicBudgetStage::FormalEvaluation,
        ),
        (
            CostComponentKind::CanaryOperations,
            "group-canary",
            EconomicBudgetStage::GrayOperations,
        ),
    ]
    .into_iter()
    .map(|(component, group, stage)| ComponentBillingBindingV1 {
        component,
        source: ComponentBillingSourceV1::BudgetCall {
            dispatch_group_id: group.into(),
            stage,
        },
    });
    EconomicBudgetBindingV1 {
        billing_scope: "economic-scope".into(),
        root_budget_id: "root-1".into(),
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        components: bindings
            .chain(std::iter::once(ComponentBillingBindingV1 {
                component: CostComponentKind::HumanReview,
                source: ComponentBillingSourceV1::AdminMeasurement {
                    source_id: "admin-review-receipt".into(),
                },
            }))
            .collect(),
    }
}

fn world(id: &str, cluster: &str, source_id: &str, partition: WorldPartition) -> ReplayWorldV2 {
    let generation = d("generation");
    let baseline = d("baseline-context");
    let next = d(&format!("next-{id}"));
    let action = ReplayActionSpecV1 {
        record_seq: 1,
        generation_signature: generation.clone(),
        parent_context_signature: baseline.clone(),
        branch_seq: 1,
        target_depth: 1,
        action_kind: ActionKindV1::Widen { root_slot: 1 },
        estimated_cost_upper_micros: Some(1),
        writes_shared_workspace: false,
    };
    let transition = ReplayTransitionV2 {
        record_id: format!("transition-{id}"),
        record_seq: 1,
        generation_signature: generation.clone(),
        parent_context_signature: baseline.clone(),
        action_kind: action.action_kind.clone(),
        next_context_signature: next.clone(),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: 600_000,
            },
        },
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_micros: Some(1),
            latency_millis: Some(1),
        },
        source_ids: vec![source_id.into()],
        observation_source_id: source_id.into(),
    };
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: id.into(),
            cluster_id: cluster.into(),
            partition,
            purpose: Purpose::Development,
            generation_signature: generation,
            world_context_signature: d("world-context"),
            baseline_context_signature: baseline,
            approved_parent_digest: d("approved-parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 500_000,
            baseline_observation_source_id: format!("baseline-{source_id}"),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: source_id.into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: format!("baseline-{source_id}"),
                    content_digest: String::new(),
                },
            ],
            revoke_watermark: 1,
            prefix_coverage: vec![
                PrefixCoverageV1 {
                    context_signature: d("baseline-context"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: next,
                    exhausted: true,
                },
            ],
            action_catalog: vec![action],
        },
        transitions: vec![transition],
        sealed_digest: None,
    };
    world.manifest.source_closure[1].content_digest =
        replay_baseline_observation_digest(&world.manifest).unwrap();
    world.manifest.source_closure[0].content_digest =
        replay_observation_digest(&world.manifest, &world.transitions[0]).unwrap();
    world.seal().unwrap();
    world
}

fn experiment(report: &StoredReplayReportV1) -> ReplayEconomicExperimentV1 {
    ReplayEconomicExperimentV1 {
        schema_version: EXPERIMENT_SCHEMA.into(),
        id: "economic-exp-1".into(),
        profile_id: "profile-1".into(),
        evaluator_actor: "evaluator".into(),
        executor_actor: "executor".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        primary_claim: EconomicClaim::NoninferiorSavings,
        min_gain_micros: 20_000,
        noninferiority_margin_micros: 10_000,
        preregistration_digest: d("preregistered"),
        paired_ticket_id: "paired-ticket".into(),
        paired_ticket_digest: d("paired-ticket"),
        dataset_epoch: "epoch-1".into(),
        tasks: vec![PairedTaskRef {
            ordinal: 1,
            task_id: "task-1".into(),
            task_digest: d("task-1"),
            cluster_id: "cluster-1".into(),
            arm_order: ArmOrder::FixedThenReplaySelected,
        }],
        runtime: MatchedRuntimeContract {
            w_online: 1,
            fixed_policy_digest: d("fixed-policy"),
            replay_selected_policy_digest: report.policy_digest.clone(),
            generation_strategy_digest: d("generation"),
            candidate_bundle_digest: d("candidate-bundle"),
            baseline_bundle_digest: d("baseline-bundle"),
            environment_digest: d("environment"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            runner_digest: d("runner"),
            grader_digest: d("scorer"),
            guidance_digest: d("guidance"),
            rules_digest: d("rules"),
            context_signature: d("context"),
            target_runtime_profile: "online-w1".into(),
            per_arm_node_budget: 12,
            per_arm_root_budget_micros: 1_000,
        },
        replay_selection: ReplaySelectionRef {
            report_artifact_id: report.report_id.clone(),
            report_digest: fingerprint(report).unwrap(),
            semantic_digest: report.semantic_reports_digest.clone(),
            world_pool_digest: report.pool_digest.clone(),
            policy_digest: report.policy_digest.clone(),
            profile_digest: report.profile_digest.clone(),
            caps_digest: report.caps_digest.clone(),
            selection_rule_version: REPLAY_SELECTION_RULE_V1.into(),
            selected_with_w_sim: 1,
            target_w_online: 1,
            simulation_only: vec![SimulationOnlyDiagnosticRef {
                w_sim: 2,
                report_id: "simulation-w2".into(),
                report_digest: d("simulation-w2"),
            }],
        },
        cost_plan: FullCostPlanV1 {
            component_kinds: CostComponentKind::ALL.into(),
            currency: "usd".into(),
            pricing_version: "price-v1".into(),
            payment_subject: "payer-1".into(),
        },
        budget_binding: budget_binding(),
        sources: vec![EconomicSourceRefV1 {
            id: "source-1".into(),
            digest: fingerprint(&economic_source_body()).unwrap(),
        }],
        source_watermark: 1,
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    evaluator: Context,
    experiment: ReplayEconomicExperimentV1,
    stored_report: StoredReplayReportV1,
}

/// Two sealed worlds over their trusted runs, the pool over both, the Select
/// replay report and the experiment registered over that report, at revoke
/// watermark 1 (what `tests/replay_economics.rs::setup` builds).
async fn setup() -> Fx {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("economic.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let train = world(
        "world-train",
        "cluster-train",
        "run-train",
        WorldPartition::Train,
    );
    let select = world(
        "world-select",
        "cluster-select",
        REVOKED_RUN,
        WorldPartition::Select,
    );
    for (id, cluster, replay_world) in [
        ("run-train", "family-train", &train),
        (REVOKED_RUN, "family-select", &select),
    ] {
        let body =
            replay_observation_bytes(&replay_world.manifest, &replay_world.transitions[0]).unwrap();
        store_trace_authority(
            &store,
            &Context::new("n", "host", Role::Host).unwrap(),
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: body.clone(),
                    parent_family: cluster.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: OptimizationTrace {
                    run_id: id.into(),
                    parent_family: cluster.into(),
                    source_digest: hash(&body),
                    purpose: Purpose::Development,
                    outcome: TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: String::from_utf8(body.clone()).unwrap(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: body.len(),
            },
        )
        .await
        .unwrap();
        let baseline_id = replay_world.manifest.baseline_observation_source_id.clone();
        let baseline_body = replay_baseline_observation_bytes(&replay_world.manifest).unwrap();
        store_trace_authority(
            &store,
            &Context::new("n", "host", Role::Host).unwrap(),
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: baseline_id.clone(),
                    body: baseline_body.clone(),
                    parent_family: cluster.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: OptimizationTrace {
                    run_id: baseline_id,
                    parent_family: cluster.into(),
                    source_digest: hash(&baseline_body),
                    purpose: Purpose::Development,
                    outcome: TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: String::from_utf8(baseline_body.clone()).unwrap(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: baseline_body.len(),
            },
        )
        .await
        .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "source-1",
            admin.actor(),
            &economic_source_body(),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "economic-initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    for world in [&train, &select] {
        let mut draft = world.clone();
        draft.sealed_digest = None;
        evo_storage::replay::put_replay_world_draft(&admin, &store, &draft)
            .await
            .unwrap();
        evo_storage::replay::seal_replay_world(&admin, &store, world)
            .await
            .unwrap();
    }
    let pool = evo_storage::replay::register_replay_pool(
        &admin,
        &store,
        &[
            train.manifest.world_id.clone(),
            select.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap();
    let policy = ElasticPolicyV1::default();
    let caps = ExplorationCapsV1::online();
    let profile = ReplaySimulationProfile {
        simulation_version: SIMULATION_VERSION.into(),
        objective: ReplayObjective::ParetoAttainmentV2,
        w_sim: 1,
        probe_budget: 1,
        horizon: 1,
        lambda_work_micros: DEFAULT_LAMBDA_MICROS,
        lambda_round_micros: DEFAULT_LAMBDA_MICROS,
        fixed_seed: 7,
        global_recovery_dispatch_limit: 1,
        pool_digest: pool.pool_digest.clone(),
        purpose: Purpose::Development,
        target_runtime_profile: "online-w1".into(),
    };
    let replay = run_replay(
        &select,
        &policy,
        &profile,
        &caps,
        &LiveWorldAuthority {
            revoke_watermark: 1,
            revoked_source_ids: BTreeSet::new(),
        },
    )
    .unwrap();
    let stored_report = StoredReplayReportV1::build(
        "n",
        WorldPartition::Select,
        &pool,
        policy,
        profile,
        caps,
        vec![replay],
    )
    .unwrap();
    evo_storage::replay::put_replay_report(&admin, &store, &stored_report)
        .await
        .unwrap();
    let experiment = experiment(&stored_report);
    PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment.clone())
        .await
        .unwrap();
    Fx {
        _dir: directory,
        store,
        admin,
        evaluator,
        experiment,
        stored_report,
    }
}

fn authorization() -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: "root-1".into(),
        billing_scope: "economic-scope".into(),
        allowed_namespaces: vec!["n".into()],
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        authorization_receipt_digest: d("authorization"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn reservation(
    call_id: &str,
    dispatch_group_id: &str,
    stage: BudgetStage,
    now: i64,
) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "economic-scope".into(),
        call_id: call_id.into(),
        dispatch_group_id: dispatch_group_id.into(),
        stage,
        actual_input_digest: d(&format!("input-{call_id}")),
        request_artifact: None,
        max_cost_micros: 100,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + 100,
        now,
    }
}

fn fence(call: &evo_storage::budget::BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The id a record is stored under: `e11-` and the digest of its record kind and
/// id. The tests that read a record look it up with this, and assert it exists,
/// so a change of the formula fails loudly instead of passing vacuously.
fn economic_id(record_kind: &str, id: &str) -> String {
    format!("e11-{}", fingerprint(&(record_kind, id)).unwrap())
}

/// The stored body of one object, straight from the store, so the assertions do
/// not depend on any consumer's read path.
async fn raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// Every stored body of the economic envelope, by storage id.
async fn economic_records(store: &Store, ctx: &Context) -> BTreeMap<String, Value> {
    let mut session = store.session().await.unwrap();
    let bodies: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    bodies
        .into_iter()
        .filter(|body| body["schema_version"] == ENVELOPE_SCHEMA)
        .map(|body| (body["id"].as_str().unwrap().to_string(), body))
        .collect()
}

fn record_kinds(records: &BTreeMap<String, Value>) -> BTreeSet<&str> {
    records
        .values()
        .map(|body| body["record_kind"].as_str().unwrap())
        .collect()
}

async fn dependents(store: &Store, ctx: &Context, kind: &str, id: &str) -> Vec<(String, String)> {
    let mut session = store.session().await.unwrap();
    let rows = session.dependents(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    rows
}

async fn world_manifest(store: &Store, ctx: &Context, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let (_, manifest) = session.get_world(ctx, id).await.unwrap().unwrap();
    session.commit().await.unwrap();
    manifest
}

async fn begin(store: &Store, admin: &Context, kind: &str, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        admin,
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        "source revoked",
        500,
    )
    .await
    .unwrap()
}

/// One cleanup step of `limit` edges; `number` numbers the steps of a test so
/// every `now` is distinct.
async fn step(
    store: &Store,
    admin: &Context,
    status: &CleanupStatus,
    limit: usize,
    number: i64,
) -> CleanupStatus {
    LifecycleStore::cleanup_step(admin, store, &status.job_id, limit, 501 + number)
        .await
        .unwrap()
}

/// Steps until the job stops moving; returns it and the number of steps.
async fn drive(
    store: &Store,
    admin: &Context,
    mut status: CleanupStatus,
    limit: usize,
    mut steps: i64,
) -> (CleanupStatus, i64) {
    while !matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
        assert!(steps < 5_000, "the cleanup did not finish: {status:?}");
        status = step(store, admin, &status, limit, steps).await;
        steps += 1;
    }
    (status, steps)
}

/// The `Conflict` message, failing on any other outcome (above all on
/// `Internal`, which is what an expected state must never be reported as).
fn expect_conflict<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) -> String {
    match result {
        Err(Error::Conflict(message)) => message,
        other => panic!("{what}: expected a Conflict, got {other:?}"),
    }
}

/// The message a read of a redacted replay record answers.
fn redacted_message(record_kind: &str, id: &str) -> String {
    format!("replay {record_kind} {id} was redacted because its source was revoked")
}

/// The job `start` writes for `request_key` over `experiment`, as a literal: the
/// first session of `start` and the reads between its two sessions cannot be run
/// apart from the write session, so the cases that call the write session have to
/// bring the job themselves. The case that compares it with a real `start` keeps
/// this literal faithful.
fn started_job(
    experiment: &ReplayEconomicExperimentV1,
    request_key: &str,
    created_seq: u64,
) -> ReplayEconomicJobV1 {
    ReplayEconomicJobV1 {
        schema_version: "rsia.replay_economic_job.v1".into(),
        id: format!(
            "replay-economic-job-{}",
            &fingerprint(&(experiment.id.as_str(), request_key)).unwrap()[..24]
        ),
        experiment_id: experiment.id.clone(),
        request_key: request_key.into(),
        request_digest: fingerprint(&(
            experiment.digest().unwrap(),
            request_key,
            created_seq,
            experiment.paired_ticket_id.as_str(),
        ))
        .unwrap(),
        paired_ticket_id: experiment.paired_ticket_id.clone(),
        state: ReplayEconomicJobState::BlockedSupport,
        blocked_reasons: vec![
            "e05_paired_ticket_not_found".into(),
            "real_provider_and_independent_runner_unavailable".into(),
        ],
        cost_receipt_ids: vec![],
        report_id: None,
        cancel_reason: None,
        created_seq,
    }
}

/// A job record as `start` stores it, written without `start`: what a writer that
/// is not stopped by the source check leaves behind. Its edge to the experiment is
/// what carries a revoked run to it.
async fn write_raw_job(fx: &Fx, job: &ReplayEconomicJobV1) -> Value {
    let id = economic_id(JOB_KIND, &job.id);
    let body = json!({
        "schema_version": ENVELOPE_SCHEMA,
        "id": id,
        "record_kind": JOB_KIND,
        "payload": job,
    });
    let experiment_id = economic_id(EXPERIMENT_KIND, &fx.experiment.id);
    let mut session = fx.store.session().await.unwrap();
    session
        .put(&fx.admin, "artifact", &id, fx.evaluator.actor(), &body)
        .await
        .unwrap();
    session
        .put_edge(&fx.admin, "artifact", &id, "artifact", &experiment_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    body
}

/// What a refused job write must leave as it found it: the economic records, the
/// artifacts, the edges over the experiment and the audit chain.
async fn written(fx: &Fx) -> (BTreeMap<String, Value>, u64, Vec<(String, String)>, usize) {
    let mut session = fx.store.session().await.unwrap();
    let artifacts = session
        .namespace_object_count(&fx.admin, "artifact")
        .await
        .unwrap();
    session.commit().await.unwrap();
    (
        economic_records(&fx.store, &fx.admin).await,
        artifacts,
        dependents(
            &fx.store,
            &fx.admin,
            "artifact",
            &economic_id(EXPERIMENT_KIND, &fx.experiment.id),
        )
        .await,
        fx.store.verify_audit(&fx.admin).await.unwrap(),
    )
}

// ---------------------------------------------------------------------------
// Fixture: an experiment with a started job, a billed call and a report
// ---------------------------------------------------------------------------

struct History {
    fx: Fx,
    job: ReplayEconomicJobV1,
    receipt: CostComponentReceiptV1,
    /// The four economic records as they are before any revocation.
    before: BTreeMap<String, Value>,
}

/// The experiment of `setup`, a started job, one finalized budget call recorded as
/// a cost receipt (a real amount) and the blocked report built over them: one
/// record of each of the four kinds.
async fn history() -> History {
    let fx = setup().await;
    fx.store
        .authorize_root_budget(&fx.admin, &authorization())
        .await
        .unwrap();
    let job = PersistentReplayEconomicCoordinator::start(
        &fx.evaluator,
        &fx.store,
        &fx.experiment.id,
        "start-key-1",
        1,
    )
    .await
    .unwrap();
    let reserved = fx
        .store
        .reserve_budget_call(
            &fx.evaluator,
            &reservation(
                "fixed-call",
                "group-fixed",
                BudgetStage::DevelopmentExecution,
                6,
            ),
        )
        .await
        .unwrap();
    let dispatch = fx
        .store
        .begin_budget_dispatch(&fx.evaluator, &fence(&reserved, 7))
        .await
        .unwrap();
    let finalized = fx
        .store
        .finalize_budget_call(
            &fx.evaluator,
            &fence(&dispatch.call, 8),
            &UsageCharge {
                amount_micros: 20,
                currency: "usd".into(),
                pricing_version: "price-v1".into(),
                provider_request_id: "provider-fixed".into(),
                usage_record_id: "usage-fixed".into(),
                output_digest: d("output-fixed"),
            },
        )
        .await
        .unwrap();
    fx.store
        .close_budget_call_execution(
            &fx.evaluator,
            "economic-scope",
            "fixed-call",
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            9,
        )
        .await
        .unwrap();
    let receipt = PersistentReplayEconomicCoordinator::record_budget_cost(
        &fx.evaluator,
        &fx.store,
        &job.id,
        "economic-scope",
        "fixed-call",
        CostComponentKind::OnlineDevelopmentFixed,
        CostScope::PerTask,
        1,
    )
    .await
    .unwrap();
    assert_eq!(receipt.measurement_state, MeasurementState::KnownFinal);
    assert_eq!(receipt.amount_micros, Some(20));
    let report = PersistentReplayEconomicCoordinator::build_blocked_report(
        &fx.evaluator,
        &fx.store,
        &job.id,
        10,
    )
    .await
    .unwrap();
    assert_eq!(report.terminal, EconomicReportTerminal::BlockedSupport);
    let before = economic_records(&fx.store, &fx.admin).await;
    assert_eq!(
        record_kinds(&before),
        BTreeSet::from([EXPERIMENT_KIND, JOB_KIND, COST_KIND, REPORT_KIND]),
        "one record of each of the four kinds: {before:#?}"
    );
    assert_eq!(before.len(), 4);
    History {
        fx,
        job,
        receipt,
        before,
    }
}

/// The reachability the cleanup relies on: a revoked run reaches the world that
/// names it, the pool and the report over that world, the experiment over the
/// report and the job over the experiment. (The cost receipt and the economic
/// report have no edge, so the closure never visits them.)
async fn assert_the_chain(history: &History) {
    let (store, admin) = (&history.fx.store, &history.fx.admin);
    let artifact = |id: String| ("artifact".to_string(), id);
    let pool_row = artifact(replay_pool_storage_id(&history.fx.stored_report.pool_digest).unwrap());
    let report_row = artifact(history.fx.stored_report.report_id.clone());
    let experiment_row = artifact(economic_id(EXPERIMENT_KIND, &history.fx.experiment.id));
    let job_row = artifact(economic_id(JOB_KIND, &history.job.id));
    assert!(
        dependents(store, admin, "run", REVOKED_RUN)
            .await
            .contains(&("replay_world".to_string(), "world-select".to_string()))
    );
    let over_world = dependents(store, admin, "replay_world", "world-select").await;
    assert!(
        over_world.contains(&pool_row) && over_world.contains(&report_row),
        "{over_world:?}"
    );
    assert!(
        dependents(store, admin, "artifact", &report_row.1)
            .await
            .contains(&experiment_row)
    );
    assert_eq!(
        dependents(store, admin, "artifact", &experiment_row.1).await,
        vec![job_row]
    );
}

// ---------------------------------------------------------------------------
// 1. The cleanup of a chain through an economic experiment
// ---------------------------------------------------------------------------

/// Revoking the run of the selection world reaches run -> world -> pool and report
/// -> experiment -> job. The job used to fail at the experiment
/// (`blocked_unknown_scope:artifact:rsia.replay_economic_artifact_envelope.v1:
/// replay_economic_experiment_v1`). It completes, the four economic records are as
/// they were, and what the experiment selected is redacted as before.
#[tokio::test]
async fn revoking_a_run_cleans_through_the_economic_experiment_and_keeps_its_history() {
    // One edge per step and eight per step: the order the chain is visited in
    // differs, the outcome does not.
    for limit in [1usize, 8] {
        let history = history().await;
        assert_the_chain(&history).await;
        let fx = &history.fx;
        let report_row = fx.stored_report.report_id.clone();
        let pool_row = replay_pool_storage_id(&fx.stored_report.pool_digest).unwrap();
        for id in [&report_row, &pool_row] {
            let body = raw(&fx.store, &fx.admin, "artifact", id).await;
            assert_ne!(body["schema_version"], REDACTED, "limit {limit}: {body}");
        }

        let status = begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
        let (status, _) = drive(&fx.store, &fx.admin, status, limit, 0).await;
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "limit {limit}: {status:?}"
        );
        assert_eq!(status.last_error, None, "limit {limit}");
        assert_eq!(status.pending_nodes, 0, "limit {limit}");
        // The closure was walked through the experiment and the job: the budget scan,
        // the world scan, the run, the world, the pool, the report, the experiment and
        // the job. (The receipt and the economic report have no edge.)
        assert_eq!(status.processed_nodes, 8, "limit {limit}: {status:?}");

        // History and money: the four records are exactly as they were.
        assert_eq!(
            economic_records(&fx.store, &fx.admin).await,
            history.before,
            "limit {limit}"
        );

        // What the experiment selected is redacted by the existing cleanup.
        for id in [&report_row, &pool_row] {
            let body = raw(&fx.store, &fx.admin, "artifact", id).await;
            assert_eq!(body["schema_version"], REDACTED, "limit {limit}: {body}");
        }
        assert_eq!(
            world_manifest(&fx.store, &fx.admin, "world-select").await["schema_version"],
            REDACTED,
            "limit {limit}"
        );
        // The run is deleted; the world of other runs is not part of the closure.
        let mut session = fx.store.session().await.unwrap();
        assert!(
            session
                .get::<Value>(&fx.admin, "run", REVOKED_RUN)
                .await
                .unwrap()
                .is_none()
        );
        session.commit().await.unwrap();
        assert_eq!(
            world_manifest(&fx.store, &fx.admin, "world-train").await["schema_version"],
            "rsia.replay_world.v2",
            "limit {limit}"
        );
    }
}

/// The cost receipt and the economic report have no dependency edge today, so the
/// closure of a revoked run never visits them and the first test cannot tell
/// whether they are classified. Linking them to the experiment, as a change that
/// gives them an edge would, makes the cleanup reach every one of the four kinds:
/// each is kept as it was.
#[tokio::test]
async fn each_listed_record_kind_is_kept_when_the_cleanup_reaches_it() {
    let history = history().await;
    let fx = &history.fx;
    let experiment_row = economic_id(EXPERIMENT_KIND, &fx.experiment.id);
    let unlinked: Vec<&String> = history
        .before
        .iter()
        .filter(|(_, body)| matches!(body["record_kind"].as_str(), Some(COST_KIND | REPORT_KIND)))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(unlinked.len(), 2, "{:?}", history.before.keys());
    let mut session = fx.store.session().await.unwrap();
    for id in unlinked {
        session
            .put_edge(&fx.admin, "artifact", id, "artifact", &experiment_row)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();

    let status = begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
    let (status, _) = drive(&fx.store, &fx.admin, status, 1, 0).await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    // The eight nodes of the chain and the two records linked to the experiment.
    assert_eq!(status.processed_nodes, 10, "{status:?}");
    assert_eq!(economic_records(&fx.store, &fx.admin).await, history.before);
}

/// A record of the economic envelope that the cleanup does not list stays
/// fail-closed: another record kind, a listed record kind under another schema
/// version, or no record kind. The job fails on that record and nothing else, and
/// the record is left exactly as it was written (neither kept by classification nor
/// redacted).
///
/// With one edge per step the job stops at the first node it cannot classify, so
/// `last_error` names the node that stopped it: the unlisted record, not the
/// experiment it hangs off. (With eight edges per step the nodes after a failed one
/// are still visited in the same step and the last failure is the one reported.)
#[tokio::test]
async fn another_record_kind_or_schema_version_of_the_economic_envelope_still_fails_closed() {
    for (case, schema, record_kind) in [
        (
            "an unlisted record kind",
            ENVELOPE_SCHEMA,
            "replay_economic_unlisted_v1",
        ),
        (
            "a listed record kind under another schema version",
            "rsia.replay_economic_artifact_envelope.v2",
            JOB_KIND,
        ),
        ("no record kind", ENVELOPE_SCHEMA, ""),
    ] {
        for limit in [1usize, 8] {
            let case = format!("{case}, {limit} edge(s) per step");
            let fx = setup().await;
            let id = "unlisted-economic-record";
            let body = json!({
                "schema_version": schema,
                "id": id,
                "record_kind": record_kind,
                "payload": {"note": "plain text that must not be left behind at Complete"},
            });
            let experiment_row = economic_id(EXPERIMENT_KIND, &fx.experiment.id);
            let mut session = fx.store.session().await.unwrap();
            session
                .put(&fx.admin, "artifact", id, fx.admin.actor(), &body)
                .await
                .unwrap();
            // It hangs off the experiment, as a job does.
            session
                .put_edge(&fx.admin, "artifact", id, "artifact", &experiment_row)
                .await
                .unwrap();
            session.commit().await.unwrap();

            let status = begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
            let (status, _) = drive(&fx.store, &fx.admin, status, limit, 0).await;
            assert_eq!(status.state, CleanupState::Failed, "{case}: {status:?}");
            assert_eq!(
                status.last_error,
                Some(format!(
                    "blocked_unknown_scope:artifact:{schema}:{record_kind}"
                )),
                "{case}"
            );
            assert_eq!(
                raw(&fx.store, &fx.admin, "artifact", id).await,
                body,
                "{case}"
            );
            // The registered experiment was kept, not redacted and not what failed.
            let experiment_body = raw(&fx.store, &fx.admin, "artifact", &experiment_row).await;
            assert_eq!(experiment_body["schema_version"], ENVELOPE_SCHEMA, "{case}");
            assert_eq!(experiment_body["record_kind"], EXPERIMENT_KIND, "{case}");
        }
    }
}

// ---------------------------------------------------------------------------
// 2. After the cleanup
// ---------------------------------------------------------------------------

/// The history survives and the coordinator answers by name: `register` names the
/// redacted selection report, `start` the moved watermark and `build_blocked_report`
/// the redacted report (or the cancelled job); none of them is `Internal`. The
/// accounting paths still book: a billed amount is not reversed and a new call is
/// still recorded.
#[tokio::test]
async fn after_the_cleanup_the_coordinator_answers_by_name_and_accounting_still_books() {
    let history = history().await;
    let fx = &history.fx;
    let report_message = redacted_message(REPLAY_REPORT_RECORD_KIND, &fx.stored_report.report_id);

    let status = begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
    let (status, _) = drive(&fx.store, &fx.admin, status, 8, 0).await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);

    // The job, the receipt and the report are still there and still readable.
    let seen =
        PersistentReplayEconomicCoordinator::status(&fx.evaluator, &fx.store, &history.job.id)
            .await
            .unwrap();
    assert_eq!(seen.experiment_id, fx.experiment.id);
    assert_eq!(seen.cost_receipt_count, 1);
    assert!(seen.report_id.is_some());
    let kept = economic_records(&fx.store, &fx.admin).await;
    assert_eq!(kept, history.before);
    let receipt_row = economic_id(COST_KIND, &history.receipt.id);
    assert_eq!(
        kept[&receipt_row]["payload"]["amount_micros"],
        json!(20),
        "the billed amount is not reversed"
    );

    // register: the selection report it reads first was redacted.
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::register(
                &fx.evaluator,
                &fx.store,
                fx.experiment.clone()
            )
            .await,
            "register after the cleanup"
        ),
        report_message
    );
    // start, for a new key and for the key that already started: the sources moved.
    for key in ["start-key-after", "start-key-1"] {
        assert_eq!(
            expect_conflict(
                PersistentReplayEconomicCoordinator::start(
                    &fx.evaluator,
                    &fx.store,
                    &fx.experiment.id,
                    key,
                    1,
                )
                .await,
                &format!("start {key} after the cleanup")
            ),
            "economic source watermark changed"
        );
    }
    // build_blocked_report: the report of the selection is redacted; a cancelled job
    // is named as cancelled.
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::build_blocked_report(
                &fx.evaluator,
                &fx.store,
                &history.job.id,
                10,
            )
            .await,
            "build_blocked_report after the cleanup"
        ),
        report_message
    );
    let cancelled = PersistentReplayEconomicCoordinator::cancel(
        &fx.evaluator,
        &fx.store,
        &history.job.id,
        "external execution remains unavailable",
    )
    .await
    .unwrap();
    assert_eq!(cancelled.state, ReplayEconomicJobState::Cancelled);
    assert!(matches!(
        PersistentReplayEconomicCoordinator::build_blocked_report(
            &fx.evaluator,
            &fx.store,
            &history.job.id,
            10,
        )
        .await,
        Err(Error::Cancelled)
    ));

    // Accounting: a call reserved and released after the cleanup is still booked
    // against the job, and so is an Admin measurement.
    let call = fx
        .store
        .reserve_budget_call(
            &fx.evaluator,
            &reservation(
                "post-cleanup-call",
                "group-history",
                BudgetStage::HistoryCollection,
                20,
            ),
        )
        .await
        .unwrap();
    fx.store
        .release_undispatched_budget_call(&fx.evaluator, &fence(&call, 21), "never dispatched")
        .await
        .unwrap();
    let booked = PersistentReplayEconomicCoordinator::record_budget_cost(
        &fx.evaluator,
        &fx.store,
        &history.job.id,
        "economic-scope",
        "post-cleanup-call",
        CostComponentKind::HistoryCollection,
        CostScope::OneTime,
        2,
    )
    .await
    .unwrap();
    assert_eq!(booked.measurement_state, MeasurementState::NotIncurred);
    let admin_receipt = CostComponentReceiptV1 {
        schema_version: COST_RECEIPT_SCHEMA.into(),
        id: "economic-cost-admin-review".into(),
        experiment_id: fx.experiment.id.clone(),
        component: CostComponentKind::HumanReview,
        scope: CostScope::OneTime,
        source_kind: CostSourceKind::AdminMeasurement,
        source_id: "admin-review-receipt".into(),
        source_digest: d("admin-review-body"),
        billing_scope: None,
        budget_call_id: None,
        amount_micros: Some(5),
        currency: Some("usd".into()),
        pricing_version: Some("price-v1".into()),
        payment_subject: Some("payer-1".into()),
        tokens: None,
        latency_micros: None,
        storage_bytes: None,
        cpu_nanos: None,
        human_minutes: Some(1),
        measurement_state: MeasurementState::KnownFinal,
        proof_digest: Some(d("admin-review-authorization")),
        reason: None,
        created_seq: 3,
    };
    PersistentReplayEconomicCoordinator::register_admin_cost(
        &fx.admin,
        &fx.store,
        &history.job.id,
        admin_receipt,
    )
    .await
    .unwrap();
    let after =
        PersistentReplayEconomicCoordinator::status(&fx.evaluator, &fx.store, &history.job.id)
            .await
            .unwrap();
    assert_eq!(after.cost_receipt_count, 3);
    assert_eq!(after.state, ReplayEconomicJobState::Cancelled);
}

// ---------------------------------------------------------------------------
// 3. The job write of `start` verifies the sources again
// ---------------------------------------------------------------------------

/// `start` verifies in its first session, then reads the live selection and the
/// paired ticket without a session. The window cannot be opened from outside, so the
/// write session (`write_started_job`) is called directly after the revocation
/// committed: it must refuse, and write no job, no edge and no audit entry.
#[tokio::test]
async fn the_job_write_of_start_refuses_a_revocation_that_committed_after_the_first_session() {
    // Control: the job the cases below bring is the one a real `start` stores (over
    // the same experiment: the report digest of another fixture differs, it holds a
    // measured CPU time).
    let control = setup().await;
    let real = PersistentReplayEconomicCoordinator::start(
        &control.evaluator,
        &control.store,
        &control.experiment.id,
        "toctou-key",
        1,
    )
    .await
    .unwrap();
    let literal = started_job(&control.experiment, "toctou-key", 1);
    assert_eq!(
        fingerprint(&real).unwrap(),
        fingerprint(&literal).unwrap(),
        "the literal the cases below use is the job `start` writes: {real:?}"
    );

    // On an unchanged store the write session stores the job and its edge.
    let fx = setup().await;
    let job = started_job(&fx.experiment, "toctou-key", 1);
    let job_row = economic_id(JOB_KIND, &job.id);
    let experiment_row = economic_id(EXPERIMENT_KIND, &fx.experiment.id);
    let stored = PersistentReplayEconomicCoordinator::write_started_job(
        &fx.evaluator,
        &fx.store,
        &fx.experiment,
        job.clone(),
    )
    .await
    .unwrap();
    assert_eq!(fingerprint(&stored).unwrap(), fingerprint(&job).unwrap());
    assert_eq!(
        raw(&fx.store, &fx.admin, "artifact", &job_row).await["payload"]["id"],
        json!(job.id)
    );
    assert_eq!(
        dependents(&fx.store, &fx.admin, "artifact", &experiment_row).await,
        vec![("artifact".to_string(), job_row.clone())]
    );
    // An idempotent replay is answered with the stored job; a changed request is a
    // conflict. Neither of these changed.
    let replay = PersistentReplayEconomicCoordinator::write_started_job(
        &fx.evaluator,
        &fx.store,
        &fx.experiment,
        job.clone(),
    )
    .await
    .unwrap();
    assert_eq!(fingerprint(&replay).unwrap(), fingerprint(&job).unwrap());
    let mut changed = job.clone();
    changed.request_digest = d("another request");
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::write_started_job(
                &fx.evaluator,
                &fx.store,
                &fx.experiment,
                changed.clone(),
            )
            .await,
            "a changed request over a started key"
        ),
        "economic job concurrently changed"
    );

    // The revocation commits after the first session verified: the same replay is
    // now refused (the check comes before the idempotency lookup, as it does in the
    // first session), and nothing is written.
    begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
    let seen = written(&fx).await;
    for (what, job) in [
        ("the stored job", job.clone()),
        ("a changed request", changed),
    ] {
        assert_eq!(
            expect_conflict(
                PersistentReplayEconomicCoordinator::write_started_job(
                    &fx.evaluator,
                    &fx.store,
                    &fx.experiment,
                    job,
                )
                .await,
                what
            ),
            "economic source watermark changed",
            "{what}"
        );
        assert_eq!(written(&fx).await, seen, "{what}");
    }
}

#[tokio::test]
async fn a_new_job_is_not_written_over_sources_revoked_or_changed_after_the_first_session() {
    // Revoked: the watermark moved.
    let fx = setup().await;
    let job = started_job(&fx.experiment, "new-job", 1);
    begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
    let seen = written(&fx).await;
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::write_started_job(
                &fx.evaluator,
                &fx.store,
                &fx.experiment,
                job.clone(),
            )
            .await,
            "write after a revocation"
        ),
        "economic source watermark changed"
    );
    assert_eq!(written(&fx).await, seen);
    assert!(seen.2.is_empty(), "no edge over the experiment: {seen:?}");
    assert_eq!(
        seen.0.len(),
        1,
        "the experiment is the only economic record: {seen:?}"
    );
    // The public path stops at its first session, with the same answer.
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::start(
                &fx.evaluator,
                &fx.store,
                &fx.experiment.id,
                "new-job",
                1,
            )
            .await,
            "start after a revocation"
        ),
        "economic source watermark changed"
    );
    assert_eq!(written(&fx).await, seen);

    // Changed: a source artifact no longer matches its registered digest, the
    // watermark did not move.
    let fx = setup().await;
    let job = started_job(&fx.experiment, "new-job", 1);
    let mut session = fx.store.session().await.unwrap();
    session
        .put(
            &fx.admin,
            "artifact",
            "source-1",
            fx.admin.actor(),
            &json!({"schema_version":"fixture.source.v1","id":"source-1","changed":true}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let seen = written(&fx).await;
    assert_eq!(
        expect_conflict(
            PersistentReplayEconomicCoordinator::write_started_job(
                &fx.evaluator,
                &fx.store,
                &fx.experiment,
                job,
            )
            .await,
            "write over a changed source"
        ),
        "economic source artifact digest changed"
    );
    assert_eq!(written(&fx).await, seen);
}

#[tokio::test]
async fn the_job_write_requires_what_start_requires_of_its_caller() {
    let fx = setup().await;
    let job = started_job(&fx.experiment, "guarded", 1);
    let other = Context::new("n", "other-evaluator", Role::Evaluator).unwrap();
    let seen = written(&fx).await;
    for (who, ctx) in [
        ("an Evaluator that does not own it", &other),
        ("an Admin", &fx.admin),
    ] {
        assert!(
            matches!(
                PersistentReplayEconomicCoordinator::write_started_job(
                    ctx,
                    &fx.store,
                    &fx.experiment,
                    job.clone(),
                )
                .await,
                Err(Error::Forbidden)
            ),
            "{who}"
        );
    }
    let mut foreign = job.clone();
    foreign.experiment_id = "another-experiment".into();
    assert!(matches!(
        PersistentReplayEconomicCoordinator::write_started_job(
            &fx.evaluator,
            &fx.store,
            &fx.experiment,
            foreign,
        )
        .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(written(&fx).await, seen);
}

// ---------------------------------------------------------------------------
// 4. A job written while the cleanup runs
// ---------------------------------------------------------------------------

/// Preserved records that hang off the replay report next to the experiment and sort
/// after it, so that the cleanup still has nodes to visit when it has expanded the
/// experiment (and would otherwise complete in the very step that expands it).
async fn add_anchors(fx: &Fx) {
    let report_row = fx.stored_report.report_id.clone();
    let mut session = fx.store.session().await.unwrap();
    for anchor in ["zz-anchor-1", "zz-anchor-2"] {
        session
            .put(
                &fx.admin,
                "artifact",
                anchor,
                fx.admin.actor(),
                &json!({"schema_version": "rsia.practice_registration.v1", "id": anchor}),
            )
            .await
            .unwrap();
        session
            .put_edge(&fx.admin, "artifact", anchor, "artifact", &report_row)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

/// Jobs written, without `start`, while the cleanup runs and after it expanded the
/// experiment: the experiment's dependents were read once, so the edge of such a job
/// is seen by nothing but the fixpoint recheck of the cleanup (AG-035). The recheck
/// finds each job and queues it, the job completes only after every one was visited,
/// and the jobs are kept as written. Before the experiment was classified, the
/// experiment itself failed the job.
#[tokio::test]
async fn jobs_written_while_the_cleanup_runs_are_found_by_the_recheck_and_kept() {
    const LATE: i64 = 12;
    // The nodes of the chain without any late job.
    let control = setup().await;
    add_anchors(&control).await;
    let status = begin(&control.store, &control.admin, "run", REVOKED_RUN).await;
    let (control_status, _) = drive(&control.store, &control.admin, status, 1, 0).await;
    assert_eq!(
        control_status.state,
        CleanupState::Complete,
        "{control_status:?}"
    );

    let fx = setup().await;
    add_anchors(&fx).await;
    // `expand_frontier` redacts the idempotency rows of a node when it has expanded
    // the node: this row turns into a conflict once the experiment was expanded.
    let experiment_row = economic_id(EXPERIMENT_KIND, &fx.experiment.id);
    let mut session = fx.store.session().await.unwrap();
    session
        .cache(
            &fx.admin,
            "ag037.marker",
            "experiment-expanded",
            &json!({}),
            &experiment_row,
            &json!({"expanded": false}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    async fn expanded(fx: &Fx) -> bool {
        let mut session = fx.store.session().await.unwrap();
        let row = session
            .cached::<Value, _>(&fx.admin, "ag037.marker", "experiment-expanded", &json!({}))
            .await;
        session.commit().await.unwrap();
        match row {
            Ok(Some(_)) => false,
            Err(Error::Conflict(_)) => true,
            other => panic!("unexpected marker state: {other:?}"),
        }
    }

    // One edge per step, until the experiment has been expanded.
    let mut status = begin(&fx.store, &fx.admin, "run", REVOKED_RUN).await;
    let mut steps = 0;
    while !expanded(&fx).await {
        assert!(steps < 200, "the experiment was never expanded: {status:?}");
        status = step(&fx.store, &fx.admin, &status, 1, steps).await;
        steps += 1;
    }
    // The experiment is expanded and the cleanup is still running: it has the
    // anchors left to visit, so it cannot complete in this step.
    let mut late = Vec::new();
    for round in 0..LATE {
        assert!(
            matches!(status.state, CleanupState::Pending | CleanupState::Running),
            "round {round}: a job written while the cleanup runs cannot finish it: {status:?}"
        );
        let job = started_job(&fx.experiment, &format!("late-{round}"), 1);
        let body = write_raw_job(&fx, &job).await;
        late.push((job, body));
        status = step(&fx.store, &fx.admin, &status, 1, steps).await;
        steps += 1;
    }
    let (status, _) = drive(&fx.store, &fx.admin, status, 1, steps).await;

    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);
    // Every late job was visited exactly once, none was missed.
    assert_eq!(
        status.processed_nodes,
        control_status.processed_nodes + LATE as u64,
        "{status:?} against {control_status:?}"
    );
    for (job, body) in &late {
        assert_eq!(
            raw(
                &fx.store,
                &fx.admin,
                "artifact",
                &economic_id(JOB_KIND, &job.id)
            )
            .await,
            *body
        );
    }
    assert_eq!(
        raw(&fx.store, &fx.admin, "artifact", &experiment_row).await["record_kind"],
        EXPERIMENT_KIND
    );
}

// ---------------------------------------------------------------------------
// 5. What the preserved records hold
// ---------------------------------------------------------------------------

/// The classification keeps the four record kinds whole, so they must hold nothing
/// that came from a source. Every string in them is an identifier or a digest
/// (the alphabet of `evo_core::identifier`), except the reason strings, which are
/// either generated by the coordinator from a fixed vocabulary or typed by the
/// Evaluator or Admin (a cancel reason, an Admin measurement note). A record that
/// gains another free-text field, or a reason that is not one of these, fails here
/// and has to be classified again before it is kept.
#[tokio::test]
async fn the_preserved_records_hold_identifiers_digests_and_reason_strings_only() {
    let history = history().await;
    let fx = &history.fx;
    // The reasons the coordinator generates for a budget call that never ran, and
    // one typed by the Admin.
    let call = fx
        .store
        .reserve_budget_call(
            &fx.evaluator,
            &reservation(
                "never-run-call",
                "group-history",
                BudgetStage::HistoryCollection,
                30,
            ),
        )
        .await
        .unwrap();
    fx.store
        .release_undispatched_budget_call(&fx.evaluator, &fence(&call, 31), "never dispatched")
        .await
        .unwrap();
    PersistentReplayEconomicCoordinator::record_budget_cost(
        &fx.evaluator,
        &fx.store,
        &history.job.id,
        "economic-scope",
        "never-run-call",
        CostComponentKind::HistoryCollection,
        CostScope::OneTime,
        2,
    )
    .await
    .unwrap();
    let typed_note = "review still open";
    PersistentReplayEconomicCoordinator::register_admin_cost(
        &fx.admin,
        &fx.store,
        &history.job.id,
        CostComponentReceiptV1 {
            schema_version: COST_RECEIPT_SCHEMA.into(),
            id: "economic-cost-admin-review".into(),
            experiment_id: fx.experiment.id.clone(),
            component: CostComponentKind::HumanReview,
            scope: CostScope::OneTime,
            source_kind: CostSourceKind::AdminMeasurement,
            source_id: "admin-review-receipt".into(),
            source_digest: d("admin-review-body"),
            billing_scope: None,
            budget_call_id: None,
            amount_micros: None,
            currency: Some("usd".into()),
            pricing_version: Some("price-v1".into()),
            payment_subject: Some("payer-1".into()),
            tokens: None,
            latency_micros: None,
            storage_bytes: None,
            cpu_nanos: None,
            human_minutes: None,
            measurement_state: MeasurementState::UsageUncertain,
            proof_digest: Some(d("admin-review-authorization")),
            reason: Some(typed_note.into()),
            created_seq: 3,
        },
    )
    .await
    .unwrap();
    let typed_cancel = "external execution remains unavailable";
    PersistentReplayEconomicCoordinator::cancel(
        &fx.evaluator,
        &fx.store,
        &history.job.id,
        typed_cancel,
    )
    .await
    .unwrap();

    // Where a reason string may be, and what it may say.
    const REASON_PATHS: [&str; 5] = [
        "/payload/blocked_reasons/*",
        "/payload/cancel_reason",
        "/payload/reason",
        "/payload/reasons/*",
        "/payload/economics/reasons/*",
    ];
    const GENERATED: [&str; 4] = [
        "budget call was provably never dispatched",
        "dispatched usage is not finalized and execution-closed",
        "reservation is neither released nor final usage",
        "budget call state cannot prove final cost",
    ];
    // Generated from a receipt id, a component name or an error of the E05 read.
    const GENERATED_PREFIXES: [&str; 5] = [
        "e05_paired_ticket_unavailable: ",
        "missing_component: ",
        "incomparable_pricing: ",
        "nonmonetary_only: ",
        "unsupported_measurement: ",
    ];
    fn leaves(value: &Value, path: String, out: &mut Vec<(String, String)>) {
        match value {
            Value::String(text) => out.push((path, text.clone())),
            Value::Array(items) => items
                .iter()
                .for_each(|item| leaves(item, format!("{path}/*"), out)),
            Value::Object(fields) => fields
                .iter()
                .for_each(|(key, item)| leaves(item, format!("{path}/{key}"), out)),
            _ => {}
        }
    }
    fn identifier_like(text: &str) -> bool {
        !text.is_empty()
            && text.len() <= 128
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    }
    let records = economic_records(&fx.store, &fx.admin).await;
    assert_eq!(
        record_kinds(&records),
        BTreeSet::from([EXPERIMENT_KIND, JOB_KIND, COST_KIND, REPORT_KIND])
    );
    let mut free_text = BTreeSet::new();
    for body in records.values() {
        let mut strings = Vec::new();
        leaves(body, String::new(), &mut strings);
        for (path, text) in strings {
            if identifier_like(&text) {
                continue;
            }
            let allowed = REASON_PATHS.contains(&path.as_str())
                && (GENERATED.contains(&text.as_str())
                    || GENERATED_PREFIXES
                        .iter()
                        .any(|prefix| text.starts_with(prefix))
                    || text == typed_note
                    || text == typed_cancel);
            assert!(
                allowed,
                "{} {path}: {text:?} is free text outside the reason strings",
                body["record_kind"]
            );
            free_text.insert((body["record_kind"].as_str().unwrap().to_string(), path));
        }
    }
    // The reason strings are really there: the check above is not vacuous.
    let paths: BTreeSet<_> = free_text.iter().map(|(_, path)| path.as_str()).collect();
    assert!(paths.contains("/payload/cancel_reason"), "{free_text:?}");
    assert!(paths.contains("/payload/reason"), "{free_text:?}");
}
