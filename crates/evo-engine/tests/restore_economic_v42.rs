//! AG-051 (E16.5, E08, E11; plan §11.3, §11.5): the durable records of a
//! replay-economic experiment are protected recovery facts.
//!
//! `register`, `start`, `record_budget_cost` / `register_admin_cost` and
//! `build_blocked_report` persist four kinds of record in one envelope
//! (`rsia.replay_economic_artifact_envelope.v1`): the preregistered experiment, the
//! job started over it (its idempotency key and state), the cost receipts (with the
//! amounts they book) and the blocked report. The revocation cleanup keeps all four
//! as they were (AG-037: an amount or a history that happened is not reversed by a
//! deletion), but the envelope was in neither protected list of
//! `restore_backup.py::protected_facts` and `evo-storage`'s `lifecycle.rs`. A backup
//! that was older than the trusted anchor on any of them was therefore restored
//! without complaint and silently dropped the experiments, jobs, cost receipts and
//! reports the anchor had already recorded, and a restored directory the anchor had
//! moved past was admitted. The decision rule is unchanged (the anchor and the
//! backup agree on every protected fact, compared on the parsed body); only the set
//! of facts grew, by one schema that covers all four record kinds.
//!
//! Every case that involves a restore starts from a real `Store::backup` and a real
//! run of `scripts/restore_backup.py`; the gate cases run the real `StartupGate`
//! against the directory the script produced. The records are written by the
//! engine's own coordinator, not by fixture bodies: `register` over a real replay
//! report, `start`, `record_budget_cost` over a real finalized budget call,
//! `build_blocked_report`, and `cancel` for a job that changes in place. A receipt
//! booked by `register_admin_cost` is the same record kind and is selected the same
//! way (by the envelope's schema); no case here writes one.
//!
//! The money is the one thing that must not move between the backup and the anchor in
//! these cases, because the root budget tables are a protected fact of their own
//! (compared since AG-018). The budget call that the cost receipt books is therefore
//! reserved, dispatched and finalized before the backup is taken; only the economic
//! records are written after it. Each case asserts that, so a restore is isolated
//! because of the economic records and not because of the budget tables.
//!
//! The script's isolation message is fixed text and names no fact. What names the
//! facts is the script's own selection (`protected_facts`, compared here against the
//! anchor and the backup) and the gate, whose diagnostic names every key that
//! differs.
//!
//! The fixtures are copied from `tests/replay_economic_cleanup_v42.rs` (the economic
//! experiment, its worlds, report and budget call) and from
//! `tests/restore_coverage_v42.rs` (the backup, restore and gate harness); test
//! crates cannot import each other and the originals are untouched.

use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::replay::*;
use evo_core::replay_economics::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Role, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::replay::{LiveWorldAuthority, run_replay};
use evo_engine::replay_experiment::PersistentReplayEconomicCoordinator;
use evo_engine::startup_gate::{
    RESTORE_ADMISSION_FILE, RecoveryPosture, StartupGate, StartupGateError,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{
    BackupManifest, BackupWatermarkEntry, CleanupState, ControlPlaneFacts, LifecycleStore,
    PROTECTED_ACTION_SCHEMA_VERSIONS, PROTECTED_SCHEMA_VERSIONS, RevokeTombstone, TypedObjectRef,
    read_control_plane_facts,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const RESTORE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/restore_backup.py"
);
const NS: &str = "n";
/// The run of the Select world: revoking it reaches the whole chain (AG-037).
const REVOKED_RUN: &str = "run-select";
/// The script's verdict when the backup and the anchor disagree on a protected fact.
const FACTS_CHANGED: &str =
    "consumed query/alpha/dispatch/accounting facts changed; make a fresh backup";
/// `replay_experiment.rs` keeps the envelope constants private; these literals are the
/// persisted values. A change on either side makes `economic_objects` come back empty
/// and the cases below fail loudly instead of passing vacuously.
const ENVELOPE_SCHEMA: &str = "rsia.replay_economic_artifact_envelope.v1";
const EXPERIMENT_KIND: &str = "replay_economic_experiment_v1";
const JOB_KIND: &str = "replay_economic_job_v1";
const COST_KIND: &str = "replay_economic_cost_receipt_v1";
const REPORT_KIND: &str = "replay_economic_report_v1";
/// The schema of the job payload; `replay_experiment.rs` writes it as a literal.
const JOB_SCHEMA: &str = "rsia.replay_economic_job.v1";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn admin() -> Context {
    Context::new(NS, "admin", Role::Admin).unwrap()
}

fn evaluator() -> Context {
    Context::new(NS, "evaluator", Role::Evaluator).unwrap()
}

// ---------------------------------------------------------------------------
// fixtures: sealed worlds, pool, replay report, experiment and a finalized budget
// call (copied from tests/replay_economic_cleanup_v42.rs)
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

/// Everything `tests/replay_economic_cleanup_v42.rs::setup` builds up to, and not
/// including, the registration of the experiment: two sealed worlds over their
/// trusted runs, the pool over both, the Select replay report and the revoke
/// watermark 1 of namespace `n`. Returns the experiment to register over that report.
///
/// The watermark digest is a 64-digit hex string, which the restore script requires
/// of a backup's watermark; the original fixture's is not.
async fn prerequisites(store: &Store) -> ReplayEconomicExperimentV1 {
    let admin = admin();
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
            store,
            &Context::new(NS, "host", Role::Host).unwrap(),
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
            store,
            &Context::new(NS, "host", Role::Host).unwrap(),
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
        .bump_watermark(&admin, &d("initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    for world in [&train, &select] {
        let mut draft = world.clone();
        draft.sealed_digest = None;
        evo_storage::replay::put_replay_world_draft(&admin, store, &draft)
            .await
            .unwrap();
        evo_storage::replay::seal_replay_world(&admin, store, world)
            .await
            .unwrap();
    }
    let pool = evo_storage::replay::register_replay_pool(
        &admin,
        store,
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
        NS,
        WorldPartition::Select,
        &pool,
        policy,
        profile,
        caps,
        vec![replay],
    )
    .unwrap();
    evo_storage::replay::put_replay_report(&admin, store, &stored_report)
        .await
        .unwrap();
    experiment(&stored_report)
}

fn authorization() -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: "root-1".into(),
        billing_scope: "economic-scope".into(),
        allowed_namespaces: vec![NS.into()],
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

/// The root budget and one finalized, execution-closed call of 20 micros: the amount
/// the cost receipt books. Every table this writes is part of the base, so the root
/// budget tables of the backup and of the anchor are the same.
async fn spend_before_the_backup(store: &Store) {
    let (admin, evaluator) = (admin(), evaluator());
    store
        .authorize_root_budget(&admin, &authorization())
        .await
        .unwrap();
    let reserved = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "fixed-call",
                "group-fixed",
                BudgetStage::DevelopmentExecution,
                6,
            ),
        )
        .await
        .unwrap();
    let dispatch = store
        .begin_budget_dispatch(&evaluator, &fence(&reserved, 7))
        .await
        .unwrap();
    let finalized = store
        .finalize_budget_call(
            &evaluator,
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
    store
        .close_budget_call_execution(
            &evaluator,
            "economic-scope",
            "fixed-call",
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            9,
        )
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// harness: a live database (the anchor), a real backup, the real script
// (copied from tests/restore_coverage_v42.rs)
// ---------------------------------------------------------------------------

/// The id an economic record is stored under: `e11-` and the digest of its record
/// kind and id (`replay_experiment.rs::storage_id`).
fn economic_id(record_kind: &str, id: &str) -> String {
    format!("e11-{}", fingerprint(&(record_kind, id)).unwrap())
}

/// The key the gate names a record by.
fn key(record_kind: &str, id: &str) -> String {
    format!("{NS}/artifact/{}", economic_id(record_kind, id))
}

/// Relative path -> content hash (files), target (symlinks) or "dir".
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                out.insert(
                    relative,
                    format!("symlink:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if meta.is_dir() {
                out.insert(relative, "dir".into());
                walk(root, &path, out);
            } else {
                out.insert(relative, hash(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Run `program` (Python, with the database and the envelope schema as arguments)
/// over `database`, read-only and `immutable`: nothing is created beside a backup or
/// a restored directory. `immutable` ignores a write-ahead log, so a log that holds
/// anything (a store still open on the file) is refused instead of read stale; the
/// empty log a read-only connection leaves behind is not.
fn python_read(database: &Path, program: &str) -> Value {
    let wal = PathBuf::from(format!("{}-wal", database.display()));
    let wal_bytes = std::fs::metadata(&wal).map_or(0, |meta| meta.len());
    assert_eq!(
        wal_bytes,
        0,
        "{}: a write-ahead log with content: a store is still open on it",
        database.display()
    );
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(program)
        .arg(database)
        .arg(ENVELOPE_SCHEMA)
        .arg(RESTORE_SCRIPT)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "reading {} failed: {}",
        database.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Every object of the economic envelope in `database`, by `ns/kind/id`, read
/// straight from the file so the result does not depend on any protected list.
fn economic_objects(database: &Path) -> BTreeMap<String, Value> {
    let program = r#"
import json, sqlite3, sys
con = sqlite3.connect("file:" + sys.argv[1] + "?mode=ro&immutable=1", uri=True)
found = {}
for namespace, kind, id, body in con.execute("SELECT namespace,kind,id,body FROM objects ORDER BY namespace,kind,id"):
    value = json.loads(body)
    if isinstance(value, dict) and value.get("schema_version") == sys.argv[2]:
        found["/".join((namespace, kind, id))] = value
print(json.dumps(found))
"#;
    serde_json::from_value(python_read(database, program)).unwrap()
}

/// `protected_facts` of the real script over `database`, as `ns/kind/id`.
fn script_selection(database: &Path) -> BTreeSet<String> {
    let program = r#"
import importlib.util, json, sqlite3, sys
spec = importlib.util.spec_from_file_location("restore_backup", sys.argv[3])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
con = sqlite3.connect("file:" + sys.argv[1] + "?mode=ro&immutable=1", uri=True)
keys = sorted("/".join(fact[:3]) for fact in module.protected_facts(con, {"n"}) if len(fact) == 4)
print(json.dumps(keys))
"#;
    serde_json::from_value(python_read(database, program)).unwrap()
}

fn record_kinds(records: &BTreeMap<String, Value>) -> BTreeSet<&str> {
    records
        .values()
        .map(|body| body["record_kind"].as_str().unwrap())
        .collect()
}

/// One step in the life of an experiment, written by the engine's coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// `register`: the experiment.
    Register,
    /// `start`: the job.
    Start,
    /// `record_budget_cost`: a cost receipt of 20 micros; the job indexes it.
    Cost,
    /// `build_blocked_report`: the report; the job names it.
    Report,
    /// `cancel`: the job changes in place and nothing is added.
    Cancel,
}

impl Step {
    /// The order the steps can be taken in; a cancelled job takes no report.
    const ALL: [Step; 5] = [
        Step::Register,
        Step::Start,
        Step::Cost,
        Step::Report,
        Step::Cancel,
    ];
    /// What an experiment that runs to its report leaves behind: one record of each
    /// of the four kinds.
    const HISTORY: [Step; 4] = [Step::Register, Step::Start, Step::Cost, Step::Report];
}

/// What one step wrote, as the gate names objects (`ns/kind/id`): the objects that
/// are new on the anchor and the ones that changed in place.
struct Stepped {
    new: Vec<String>,
    changed: Vec<String>,
}

impl Stepped {
    /// The gate's reason when exactly this is what the anchor holds beyond the
    /// restored directory. Only keys are named, never values.
    fn reason(&self) -> String {
        let mut parts = Vec::new();
        if !self.new.is_empty() {
            parts.push(format!(
                "only in the trusted anchor: {}",
                self.new.join(", ")
            ));
        }
        if !self.changed.is_empty() {
            parts.push(format!("changed: {}", self.changed.join(", ")));
        }
        format!(
            "consumed accounting facts differ between the data directory and the trusted anchor ({}); re-run restore_backup.py from a fresh backup",
            parts.join("; ")
        )
    }
}

/// Objects that look like economic records and are not protected: the envelope of
/// another version, the schema of a record inside the envelope at the top level, and an
/// unrelated artifact. The comparison is by the exact schema of the envelope.
fn look_alikes() -> [(&'static str, Value); 3] {
    [
        (
            "look-alike-v2",
            json!({"schema_version":"rsia.replay_economic_artifact_envelope.v2","id":"look-alike-v2","record_kind":JOB_KIND,"payload":{}}),
        ),
        (
            "look-alike-payload",
            json!({"schema_version":JOB_SCHEMA,"id":"look-alike-payload"}),
        ),
        (
            "plain-1",
            json!({"schema_version":"some.other.v1","id":"plain-1"}),
        ),
    ]
}

/// The script's outcome.
struct Restored {
    dest: PathBuf,
    root: PathBuf,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Restored {
    fn output(&self) -> String {
        format!("stdout={:?} stderr={:?}", self.stdout, self.stderr)
    }

    fn assert_isolated(&self, context: &str) {
        assert_eq!(
            self.code,
            Some(1),
            "{context}: the restore was not isolated: {}",
            self.output()
        );
        assert!(
            self.stderr.starts_with("ISOLATE ")
                && self.stderr.contains(FACTS_CHANGED)
                && self.stderr.trim_end().ends_with("; not mounting"),
            "{context}: {}",
            self.output()
        );
        assert!(!self.stdout.contains("RESTORE_OK"), "{context}");
        assert!(
            !self.dest.exists(),
            "{context}: an isolated restore produced a destination"
        );
        let dest_name = self
            .dest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for entry in std::fs::read_dir(&self.root).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(&format!(".{dest_name}.restore-")),
                "{context}: a staging directory was left behind: {name}"
            );
        }
    }

    fn assert_restored(&self, context: &str, events: usize) {
        assert_eq!(
            self.code,
            Some(0),
            "{context}: the restore was refused: {}",
            self.output()
        );
        assert!(
            self.stdout.contains(&format!("RESTORE_OK events={events}")),
            "{context}: {}",
            self.output()
        );
        assert!(self.dest.join("restore.json").is_file(), "{context}");
        assert!(self.dest.join("rsia.sqlite3").is_file(), "{context}");
    }

    fn data(&self) -> PathBuf {
        self.dest.join("rsia.sqlite3")
    }
}

/// A revocation of `source` on the anchor, as the delta must report it.
struct Revocation {
    source: String,
    seq: i64,
    digest: String,
    tombstone_digest: String,
    reason: String,
    created_at: i64,
}

struct Scenario {
    _guard: tempfile::TempDir,
    root: PathBuf,
    /// The live database: the trusted anchor.
    live: PathBuf,
    backup: PathBuf,
    base: Option<BackupWatermarkEntry>,
    manifest_bytes: Vec<u8>,
    experiment: ReplayEconomicExperimentV1,
    job_id: Option<String>,
}

impl Scenario {
    /// A live database with namespace `n` at revoke watermark 1, the replay world,
    /// pool and report an experiment selects from, the root budget and one finalized
    /// call. No economic record and no backup yet.
    async fn begin() -> Self {
        let guard = tempfile::tempdir().unwrap();
        // the script refuses symlinks in any supplied path (`/var` on macOS)
        let root = guard.path().canonicalize().unwrap();
        let live = root.join("rsia.sqlite3");
        let store = Store::open(&live).await.unwrap();
        let experiment = prerequisites(&store).await;
        spend_before_the_backup(&store).await;
        store.close().await;
        Self {
            _guard: guard,
            backup: root.join("backup"),
            root,
            live,
            base: None,
            manifest_bytes: Vec::new(),
            experiment,
            job_id: None,
        }
    }

    /// Take the real backup of the live database as it is now.
    async fn take_backup(&mut self) {
        let store = Store::open(&self.live).await.unwrap();
        store.backup(&self.backup).await.unwrap();
        store.close().await;
        self.manifest_bytes = std::fs::read(self.backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&self.manifest_bytes).unwrap();
        assert_eq!(manifest.watermarks.len(), 1);
        self.base = Some(manifest.watermarks[0].clone());
    }

    fn backup_database(&self) -> PathBuf {
        self.backup.join("rsia.sqlite3")
    }

    /// The anchor stores the [`look_alikes`].
    async fn put_look_alikes(&self) {
        let store = Store::open(&self.live).await.unwrap();
        let admin = admin();
        let mut session = store.session().await.unwrap();
        for (id, body) in look_alikes() {
            session
                .put(&admin, "artifact", id, admin.actor(), &body)
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        store.close().await;
    }

    /// Take one step on the anchor, with the engine's own coordinator.
    async fn step(&mut self, step: Step) -> Stepped {
        let store = Store::open(&self.live).await.unwrap();
        let evaluator = evaluator();
        let stepped = match step {
            Step::Register => {
                PersistentReplayEconomicCoordinator::register(
                    &evaluator,
                    &store,
                    self.experiment.clone(),
                )
                .await
                .unwrap();
                Stepped {
                    new: vec![key(EXPERIMENT_KIND, &self.experiment.id)],
                    changed: vec![],
                }
            }
            Step::Start => {
                let job = PersistentReplayEconomicCoordinator::start(
                    &evaluator,
                    &store,
                    &self.experiment.id,
                    "start-key-1",
                    1,
                )
                .await
                .unwrap();
                let stepped = Stepped {
                    new: vec![key(JOB_KIND, &job.id)],
                    changed: vec![],
                };
                self.job_id = Some(job.id);
                stepped
            }
            Step::Cost => {
                let job_id = self.job_id.clone().expect("the job was started");
                let receipt = PersistentReplayEconomicCoordinator::record_budget_cost(
                    &evaluator,
                    &store,
                    &job_id,
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
                Stepped {
                    new: vec![key(COST_KIND, &receipt.id)],
                    changed: vec![key(JOB_KIND, &job_id)],
                }
            }
            Step::Report => {
                let job_id = self.job_id.clone().expect("the job was started");
                let report = PersistentReplayEconomicCoordinator::build_blocked_report(
                    &evaluator, &store, &job_id, 10,
                )
                .await
                .unwrap();
                assert_eq!(report.terminal, EconomicReportTerminal::BlockedSupport);
                Stepped {
                    new: vec![key(REPORT_KIND, &report.id)],
                    changed: vec![key(JOB_KIND, &job_id)],
                }
            }
            Step::Cancel => {
                let job_id = self.job_id.clone().expect("the job was started");
                PersistentReplayEconomicCoordinator::cancel(
                    &evaluator,
                    &store,
                    &job_id,
                    "external execution remains unavailable",
                )
                .await
                .unwrap();
                Stepped {
                    new: vec![],
                    changed: vec![key(JOB_KIND, &job_id)],
                }
            }
        };
        store.close().await;
        stepped
    }

    /// The whole history an experiment leaves: the four steps of [`Step::HISTORY`].
    async fn run_history(&mut self) {
        for step in Step::HISTORY {
            self.step(step).await;
        }
    }

    /// What a restore of the backup may differ on from the anchor, besides the
    /// economic records: nothing in the money, and no protected object of any other
    /// schema. These hold before this change and after it, so a restore that is
    /// isolated is isolated because of the economic records.
    async fn assert_only_economic_records_differ(&self, context: &str) {
        let anchor = read_control_plane_facts(&self.live).await.unwrap();
        let backup = read_control_plane_facts(&self.backup_database())
            .await
            .unwrap();
        assert_eq!(
            anchor.root_budget_tables, backup.root_budget_tables,
            "{context}: the root budget tables moved"
        );
        let outside_the_envelope = |facts: &ControlPlaneFacts| {
            facts
                .protected_objects
                .iter()
                .filter(|(_, body)| body["schema_version"] != ENVELOPE_SCHEMA)
                .map(|(key, body)| (key.clone(), body.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            outside_the_envelope(&anchor),
            outside_the_envelope(&backup),
            "{context}: a protected object of another schema moved"
        );
    }

    /// The same, and the revocation state is the backup's too.
    async fn assert_only_economic_records_differ_and_nothing_was_revoked(&self, context: &str) {
        self.assert_only_economic_records_differ(context).await;
        let anchor = read_control_plane_facts(&self.live).await.unwrap();
        let backup = read_control_plane_facts(&self.backup_database())
            .await
            .unwrap();
        assert_eq!(anchor.watermarks, backup.watermarks, "{context}");
        assert_eq!(anchor.tombstones, backup.tombstones, "{context}");
    }

    /// Revoke `source` on the anchor and run its cleanup closure to the end.
    async fn revoke_and_clean(&self, source: &str) -> Revocation {
        let store = Store::open(&self.live).await.unwrap();
        let admin = admin();
        let mut status = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            "revoked after the backup",
            9,
        )
        .await
        .unwrap();
        for _ in 0..64 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 100, 10)
                .await
                .unwrap();
        }
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "the cleanup closure did not finish: {status:?}"
        );
        let mut session = store.session().await.unwrap();
        let tombstone: RevokeTombstone = session.need(&admin, "tombstone", source).await.unwrap();
        session.commit().await.unwrap();
        store.close().await;
        Revocation {
            source: source.into(),
            seq: i64::try_from(tombstone.watermark_seq).unwrap(),
            digest: tombstone.watermark_digest.clone(),
            tombstone_digest: hash(&serde_json::to_vec(&tombstone).unwrap()),
            reason: tombstone.reason.clone(),
            created_at: tombstone.created_at,
        }
    }

    /// The revoke delta that takes the backup's watermark to the anchor's.
    fn delta(&self, name: &str, revocations: &[Revocation]) -> PathBuf {
        let base = self.base.as_ref().expect("a backup was taken");
        assert!(revocations.len() <= 1, "one revocation per scenario");
        let events: Vec<Value> = revocations
            .iter()
            .map(|revocation| {
                json!({
                    "seq": revocation.seq,
                    "previous_digest": base.digest,
                    "digest": revocation.digest,
                    "source_kind": "run",
                    "source_id": revocation.source,
                    "tombstone_digest": revocation.tombstone_digest,
                    "reason": revocation.reason,
                    "created_at": revocation.created_at,
                })
            })
            .collect();
        let (latest_seq, latest_digest) = match revocations.first() {
            Some(revocation) => (revocation.seq, revocation.digest.clone()),
            None => (base.seq, base.digest.clone()),
        };
        let path = self.root.join(name);
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schema_version": "rsia.revoke_delta.v1",
                "base_manifest_sha256": hash(&self.manifest_bytes),
                "namespaces": [{
                    "namespace": base.namespace,
                    "base_seq": base.seq,
                    "base_digest": base.digest,
                    "events": events,
                    "latest_seq": latest_seq,
                    "latest_digest": latest_digest,
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    /// Run the real script against the backup, with the live database as anchor.
    fn restore(&self, name: &str, delta: &Path) -> Restored {
        let dest = self.root.join(name);
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--backup")
            .arg(&self.backup)
            .arg("--dest")
            .arg(&dest)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(&self.live)
            .output()
            .unwrap();
        Restored {
            dest,
            root: self.root.clone(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `restore` with a delta that reports no revocation.
    fn restore_without_events(&self, name: &str) -> Restored {
        let delta = self.delta(&format!("{name}-delta.json"), &[]);
        self.restore(name, &delta)
    }
}

/// The gate must refuse the directory with `recovery_quarantine` and write nothing.
async fn quarantine_reason(step: Step, data: &Path, anchor: &Path, directory: &Path) -> String {
    let before = snapshot(directory);
    let error = match StartupGate::new(data, Some(anchor)).evaluate().await {
        Ok(decision) => panic!(
            "{step:?}: the gate admitted a directory the anchor has moved past (posture {:?})",
            decision.posture()
        ),
        Err(error) => error,
    };
    let StartupGateError::Quarantine(reason) = &error else {
        panic!("{step:?}: expected a quarantine, got {error}");
    };
    let text = error.to_string();
    assert!(
        text.starts_with("recovery_quarantine: ") && text.ends_with("; not mounting"),
        "{step:?}: {text}"
    );
    assert_eq!(
        snapshot(directory),
        before,
        "{step:?}: a refused startup changed the data directory ({text})"
    );
    reason.clone()
}

// ---------------------------------------------------------------------------
// 1. the two lists
// ---------------------------------------------------------------------------

fn python_tuple(source: &str, marker: &str) -> Vec<String> {
    assert_eq!(
        source.matches(marker).count(),
        1,
        "{marker:?} must appear exactly once in protected_facts"
    );
    let rest = &source[source.find(marker).unwrap() + marker.len()..];
    let body = &rest[..rest.find(')').expect("closing parenthesis")];
    let mut out = Vec::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            out.push(chars.by_ref().take_while(|c| *c != '"').collect());
        }
    }
    out
}

/// The envelope is one entry of the action group (an action that already happened, an
/// idempotency binding, a spend), in Rust and in the script, and nothing next to it is.
#[test]
fn the_economic_envelope_is_listed_once_in_rust_and_once_in_the_restore_script() {
    let in_action_group = PROTECTED_ACTION_SCHEMA_VERSIONS
        .iter()
        .filter(|schema| **schema == ENVELOPE_SCHEMA)
        .count();
    assert_eq!(in_action_group, 1, "PROTECTED_ACTION_SCHEMA_VERSIONS");
    // not the accounting group, whose size tests/control_plane_facts.rs pins
    assert!(
        !PROTECTED_SCHEMA_VERSIONS.contains(&ENVELOPE_SCHEMA),
        "PROTECTED_SCHEMA_VERSIONS"
    );

    let script = std::fs::read_to_string(RESTORE_SCRIPT).unwrap();
    let protected = &script[script.find("def protected_facts").unwrap()..];
    let protected = &protected[..protected.find("def verify_delta").unwrap()];
    let in_script = python_tuple(protected, "schema in (");
    assert_eq!(
        in_script
            .iter()
            .filter(|schema| *schema == ENVELOPE_SCHEMA)
            .count(),
        1,
        "restore_backup.py"
    );

    // the two implementations hold the same set (the drift guard of
    // startup_gate_v42.rs checks the same; this is the economic entry's own check)
    let rust: BTreeSet<&str> = PROTECTED_SCHEMA_VERSIONS
        .iter()
        .chain(PROTECTED_ACTION_SCHEMA_VERSIONS.iter())
        .copied()
        .collect();
    let script_set: BTreeSet<&str> = in_script.iter().map(String::as_str).collect();
    assert_eq!(rust, script_set, "the script and Rust lists drifted");

    // exact schema, exact comparison: a later version of the envelope or the schema
    // of a record inside it is a different schema and is not listed
    for schema in [
        "rsia.replay_economic_artifact_envelope.v2",
        JOB_SCHEMA,
        EXPERIMENT_SCHEMA,
        COST_RECEIPT_SCHEMA,
        REPORT_SCHEMA,
    ] {
        assert!(!rust.contains(schema), "{schema} is listed");
        assert!(!script_set.contains(schema), "{schema} is in the script");
    }
}

/// Both implementations select a record by the envelope's `schema_version` alone, so
/// the four record kinds are all in the selection, and a database holding them yields
/// the same selection from the script and from the Rust reader. Records of other
/// schemas that look like them are in neither.
#[tokio::test]
async fn the_script_and_the_reader_select_every_record_kind_of_the_economic_envelope() {
    let mut scenario = Scenario::begin().await;
    scenario.run_history().await;

    // not protected: see `look_alikes`
    scenario.put_look_alikes().await;

    let records = economic_objects(&scenario.live);
    assert_eq!(
        record_kinds(&records),
        BTreeSet::from([EXPERIMENT_KIND, JOB_KIND, COST_KIND, REPORT_KIND]),
        "one record of each of the four kinds: {records:#?}"
    );
    assert_eq!(records.len(), 4);
    let economic: BTreeSet<String> = records.keys().cloned().collect();
    let look_alike_keys: BTreeSet<String> = look_alikes()
        .iter()
        .map(|(id, _)| format!("{NS}/artifact/{id}"))
        .collect();

    let from_script = script_selection(&scenario.live);
    let from_rust: BTreeSet<String> = read_control_plane_facts(&scenario.live)
        .await
        .unwrap()
        .protected_objects
        .keys()
        .map(|(namespace, kind, id)| format!("{namespace}/{kind}/{id}"))
        .collect();
    assert_eq!(from_rust, from_script, "the two implementations disagree");
    assert!(
        economic.is_subset(&from_script),
        "the script does not select the economic records: selected {from_script:?}, records {economic:?}"
    );
    assert!(
        look_alike_keys.is_disjoint(&from_script),
        "a look-alike is selected: {from_script:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. the script isolates a backup that is behind the anchor
// ---------------------------------------------------------------------------

/// The case the work item names: back up, then register and start an experiment on
/// the anchor, book a cost and build the report; restoring the old backup would
/// forget all of it, including the amount.
#[tokio::test]
async fn a_backup_older_than_the_anchor_on_the_economic_history_is_isolated() {
    let mut scenario = Scenario::begin().await;
    scenario.take_backup().await;
    // control: while the anchor equals the backup the restore goes through
    scenario
        .restore_without_events("unchanged")
        .assert_restored("before the experiment", 0);

    scenario.run_history().await;

    // What holds before this change and after it: the anchor recorded the whole
    // history, amount included; the backup holds none of it; nothing else moved.
    let records = economic_objects(&scenario.live);
    assert_eq!(
        record_kinds(&records),
        BTreeSet::from([EXPERIMENT_KIND, JOB_KIND, COST_KIND, REPORT_KIND]),
        "one record of each of the four kinds: {records:#?}"
    );
    assert_eq!(records.len(), 4);
    let receipts: Vec<&Value> = records
        .values()
        .filter(|body| body["record_kind"] == COST_KIND)
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0]["payload"]["amount_micros"],
        json!(20),
        "the anchor books the amount"
    );
    assert!(
        economic_objects(&scenario.backup_database()).is_empty(),
        "the backup holds no economic record"
    );
    scenario
        .assert_only_economic_records_differ_and_nothing_was_revoked(
            "experiment, job, receipt, report",
        )
        .await;

    let restored = scenario.restore_without_events("restored");
    restored.assert_isolated(
        "an experiment, its job, its cost receipt and its report only on the anchor",
    );

    // What the script compared: it selects all four records on the anchor and none
    // in the backup, so they are what the two sides disagree on.
    let economic: BTreeSet<String> = records.keys().cloned().collect();
    let on_anchor = script_selection(&scenario.live);
    let in_backup = script_selection(&scenario.backup_database());
    assert!(
        economic.is_subset(&on_anchor),
        "the script does not select the anchor's economic records: {on_anchor:?}"
    );
    assert!(
        economic.is_disjoint(&in_backup),
        "the backup holds an economic record: {in_backup:?}"
    );
}

/// Every step of an experiment's life is a fact the backup must not be behind on:
/// the experiment, the job, a cost receipt, the report, and a job that changed in
/// place. For each, the backup is taken just before the step and the anchor takes it.
#[tokio::test]
async fn each_step_the_anchor_took_after_the_backup_isolates_the_restore() {
    for (index, step) in Step::ALL.into_iter().enumerate() {
        let mut scenario = Scenario::begin().await;
        for earlier in &Step::ALL[..index] {
            scenario.step(*earlier).await;
        }
        scenario.take_backup().await;
        // control: while the anchor equals the backup the restore goes through
        scenario
            .restore_without_events("unchanged")
            .assert_restored(&format!("{step:?}: before the step"), 0);

        let stepped = scenario.step(step).await;
        let before = economic_objects(&scenario.backup_database());
        let after = economic_objects(&scenario.live);
        for new in &stepped.new {
            assert!(
                after.contains_key(new) && !before.contains_key(new),
                "{step:?}: {new} is new on the anchor"
            );
        }
        for changed in &stepped.changed {
            assert!(
                before.contains_key(changed) && after[changed] != before[changed],
                "{step:?}: {changed} changed in place on the anchor"
            );
        }
        scenario
            .assert_only_economic_records_differ_and_nothing_was_revoked(&format!("{step:?}"))
            .await;

        scenario
            .restore_without_events("moved")
            .assert_isolated(&format!("{step:?}: the anchor took it after the backup"));
    }
}

// ---------------------------------------------------------------------------
// 3. the gate quarantines a restored directory the anchor has moved past
// ---------------------------------------------------------------------------

/// After the restore and before the first start, the anchor takes one more step. The
/// gate refuses the directory and names exactly what the anchor holds beyond it: the
/// new record, and for a step that changes the job, the job.
#[tokio::test]
async fn an_economic_record_the_anchor_took_after_the_restore_quarantines_the_directory() {
    for (index, step) in Step::ALL.into_iter().enumerate() {
        let mut scenario = Scenario::begin().await;
        for earlier in &Step::ALL[..index] {
            scenario.step(*earlier).await;
        }
        scenario.take_backup().await;
        let restored = scenario.restore_without_events("restored");
        restored.assert_restored(&format!("{step:?}: before the step"), 0);

        // While the anchor still equals the restore the directory verifies ...
        let verified = StartupGate::new(&restored.data(), Some(&scenario.live))
            .evaluate()
            .await
            .unwrap_or_else(|error| panic!("{step:?}: {error}"));
        assert_eq!(verified.posture(), RecoveryPosture::RestoredVerified);

        // ... and stops verifying the moment the anchor takes the step.
        let stepped = scenario.step(step).await;
        let reason =
            quarantine_reason(step, &restored.data(), &scenario.live, &restored.dest).await;
        assert_eq!(reason, stepped.reason(), "{step:?}");
    }
}

// ---------------------------------------------------------------------------
// 4. positive control: equal records restore and the gate admits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_equal_economic_history_restores_and_the_gate_admits_the_directory() {
    let mut scenario = Scenario::begin().await;
    scenario.run_history().await;
    scenario.take_backup().await;
    let restored = scenario.restore_without_events("restored");
    restored.assert_restored("the economic history is the same on both sides", 0);

    // first start: verified against the anchor, then admitted and recorded
    let decision = StartupGate::new(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    Store::open(&restored.data()).await.unwrap().close().await;
    assert!(!restored.dest.join(RESTORE_ADMISSION_FILE).exists());
    decision.admit().unwrap();
    assert!(restored.dest.join(RESTORE_ADMISSION_FILE).is_file());
    // the next start needs no anchor
    let again = StartupGate::new(&restored.data(), None)
        .evaluate()
        .await
        .unwrap();
    assert_eq!(again.posture(), RecoveryPosture::AdmittedPreviously);

    // The control is not vacuous: the anchor holds the whole history, the backup and
    // the restored directory hold the same records byte for byte, and each of the
    // three really has them as protected facts.
    let anchor = economic_objects(&scenario.live);
    assert_eq!(anchor.len(), 4, "{anchor:#?}");
    assert_eq!(economic_objects(&scenario.backup_database()), anchor);
    assert_eq!(economic_objects(&restored.data()), anchor);
    let economic: BTreeSet<(String, String, String)> = anchor
        .keys()
        .map(|key| {
            let mut parts = key.splitn(3, '/');
            (
                parts.next().unwrap().to_owned(),
                parts.next().unwrap().to_owned(),
                parts.next().unwrap().to_owned(),
            )
        })
        .collect();
    for (name, database) in [
        ("backup", scenario.backup_database()),
        ("anchor", scenario.live.clone()),
        ("restored", restored.data()),
    ] {
        let protected: BTreeSet<_> = read_control_plane_facts(&database)
            .await
            .unwrap()
            .protected_objects
            .keys()
            .cloned()
            .collect();
        assert!(
            economic.is_subset(&protected),
            "{name}: the economic records are not protected facts: {protected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. revocation: the cleanup keeps the records, so they stay visible
// ---------------------------------------------------------------------------

/// A claim that the cleanup redacts is invisible to the comparison afterwards (the
/// known, narrow gap `restore_coverage_v42.rs` characterizes). The economic records
/// are the other kind: the cleanup of the revoked source keeps them as they were, so
/// a backup that predates them is still behind the anchor after the revocation.
#[tokio::test]
async fn a_record_the_cleanup_kept_still_isolates_a_backup_that_predates_it() {
    let mut scenario = Scenario::begin().await;
    scenario.take_backup().await;
    scenario.run_history().await;
    let before = economic_objects(&scenario.live);
    assert_eq!(before.len(), 4, "{before:#?}");

    // the source of the experiment's selection is revoked on the anchor and the
    // cleanup runs to its end; the four records are exactly as they were
    let revocation = scenario.revoke_and_clean(REVOKED_RUN).await;
    assert_eq!(
        economic_objects(&scenario.live),
        before,
        "the cleanup kept the records"
    );
    scenario
        .assert_only_economic_records_differ("a revoked source, a cleaned closure")
        .await;

    let delta = scenario.delta("delta.json", &[revocation]);
    scenario
        .restore("restored", &delta)
        .assert_isolated("economic records kept by the cleanup, absent from the backup");
}

/// The other side: a backup that already holds the history restores through a
/// revocation that came after it, and the gate verifies the result. Equality of the
/// records is not disturbed by the revocation, because the cleanup keeps them.
#[tokio::test]
async fn a_backup_holding_the_history_restores_through_a_later_revocation_of_its_source() {
    let mut scenario = Scenario::begin().await;
    scenario.run_history().await;
    scenario.take_backup().await;
    let revocation = scenario.revoke_and_clean(REVOKED_RUN).await;

    let delta = scenario.delta("delta.json", &[revocation]);
    let restored = scenario.restore("restored", &delta);
    restored.assert_restored("history in the backup, source revoked after it", 1);
    let decision = StartupGate::new(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);

    // not vacuous: the four records are on every side, equal, and protected
    let anchor = economic_objects(&scenario.live);
    assert_eq!(anchor.len(), 4, "{anchor:#?}");
    assert_eq!(economic_objects(&restored.data()), anchor);
    let protected: BTreeSet<String> = read_control_plane_facts(&restored.data())
        .await
        .unwrap()
        .protected_objects
        .keys()
        .map(|(namespace, kind, id)| format!("{namespace}/{kind}/{id}"))
        .collect();
    for record in anchor.keys() {
        assert!(
            protected.contains(record),
            "{record} is not a protected fact of the restored directory"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. negative control: schemas next to the envelope do not isolate a backup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn schemas_next_to_the_envelope_do_not_isolate_a_backup() {
    let mut scenario = Scenario::begin().await;
    scenario.take_backup().await;
    scenario.put_look_alikes().await;
    scenario.restore_without_events("restored").assert_restored(
        "a v2 envelope, a payload schema and an unrelated artifact",
        0,
    );
}
