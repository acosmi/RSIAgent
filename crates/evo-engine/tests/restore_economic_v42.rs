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

/// Exact retained prepare originals, shared by this file's real producer fixtures.
struct RestoreFixtureInputs {
    config: evo_engine::service::HostPrepareConfig,
    reader: PathBuf,
    originals: PathBuf,
}

impl RestoreFixtureInputs {
    fn new(root: &Path) -> Self {
        #[cfg(not(unix))]
        {
            let _ = root;
            panic!("unsupported restore fixture platform");
        }
        #[cfg(unix)]
        {
            use evo_core::contract::{CapabilityLevel, HostCapabilities, SystemSnapshot};
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
            let config = evo_engine::service::HostPrepareConfig {
                profile_id: "profile-a".into(),
                system_snapshot: SystemSnapshot {
                    schema_version: "rsia.system_snapshot.v2".into(),
                    profile_id: "profile-a".into(),
                    host_id: "reference-host".into(),
                    host_version: "1.0.0".into(),
                    model_id: "disabled".into(),
                    tools: vec!["read_config".into()],
                    mandatory_context_digest: hash(b"restore-prepare-mandatory"),
                },
                host_surface_id: "reference-surface".into(),
                host_capabilities: HostCapabilities {
                    available: ["fs_read".into()].into(),
                    granted: ["fs_read".into()].into(),
                },
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            };
            config.validate().unwrap();
            let test = std::env::current_exe().unwrap().canonicalize().unwrap();
            let deps = test.parent().unwrap();
            assert_eq!(deps.file_name().unwrap(), "deps");
            let reader = deps.parent().unwrap().join("rsia").canonicalize().unwrap();
            let metadata = std::fs::metadata(&reader).unwrap();
            assert!(metadata.is_file() && metadata.nlink() == 1);
            let originals = root.join("prepare-originals.json");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&originals)
                .unwrap();
            serde_json::to_writer(&mut file, std::slice::from_ref(&config)).unwrap();
            file.sync_all().unwrap();
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            Self {
                config,
                reader,
                originals,
            }
        }
    }

    async fn open(&self, path: &Path) -> Store {
        Store::open_with_prepare_configs(path, vec![self.config.clone()])
            .await
            .unwrap()
    }

    async fn close(&self, store: Store, path: &Path) {
        store.close().await;
        // Failed native close retains the real OS lock; require actual handback.
        let closed = evo_storage::lifecycle::DirectoryWriterGuard::acquire_new_mount_checked(
            path,
            std::slice::from_ref(&self.config),
        )
        .unwrap();
        drop(closed);
    }

    fn gate(&self, path: &Path, anchor: Option<&Path>) -> StartupGate {
        StartupGate::new(path, anchor)
            .with_prepare_configs(std::slice::from_ref(&self.config))
            .unwrap()
    }

    async fn prepare(
        &self,
        store: &Store,
    ) -> (evo_core::Prepared, Context, Context, evo_core::Prepare) {
        use evo_core::Prepare;
        use evo_core::contract::{HostSurfaceManifest, SurfaceCoverage, SurfaceItem};
        use evo_engine::release_store::ReleaseStore;
        use evo_engine::service::HostService;
        let config = &self.config;
        ReleaseStore::register_host_surface(
            &admin(),
            store,
            "reference-surface",
            HostSurfaceManifest {
                schema_version: "rsia.host_surface.v1".into(),
                host: "reference-host".into(),
                host_version: "1.0.0".into(),
                adapter_version: "1.0.0".into(),
                source_digest: hash(b"restore-reference-surface"),
                items: vec![SurfaceItem {
                    name: "model".into(),
                    coverage: SurfaceCoverage::Supported,
                    mapped_field: Some("host.model".into()),
                    consumer: Some("reference-host".into()),
                    reason: "fixed reference fixture".into(),
                }],
            },
            vec!["model".into()],
        )
        .await
        .unwrap();
        let host = Context::new(NS, "gateway-host", Role::Host).unwrap();
        let agent = Context::new(NS, "restore-agent", Role::Agent).unwrap();
        let service = HostService::new((*store).clone(), host.clone()).unwrap();
        let request = Prepare {
            request_key: "restore-prepare-1".into(),
            goal: "preserve the real prepare cache".into(),
            capabilities: vec!["fs_read".into()],
        };
        let prepared = service
            .prepare(&agent, request.clone(), config)
            .await
            .unwrap();
        (prepared, host, agent, request)
    }
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
    inputs: RestoreFixtureInputs,
}

impl Scenario {
    /// A live database with namespace `n` at revoke watermark 1, the replay world,
    /// pool and report an experiment selects from, the root budget and one finalized
    /// call. No economic record and no backup yet.
    async fn begin() -> Self {
        let guard = tempfile::tempdir().unwrap();
        // the script refuses symlinks in any supplied path (`/var` on macOS)
        let root = guard.path().canonicalize().unwrap();
        #[cfg(unix)]
        use std::os::unix::fs::DirBuilderExt;
        let live_dir = root.join("live");
        #[cfg(unix)]
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&live_dir)
            .unwrap();
        let inputs = RestoreFixtureInputs::new(&root);
        let live = live_dir.join("rsia.sqlite3");
        let store = inputs.open(&live).await;
        let experiment = prerequisites(&store).await;
        spend_before_the_backup(&store).await;
        let _ = inputs.prepare(&store).await;
        inputs.close(store, &live).await;
        Self {
            _guard: guard,
            backup: root.join("backup"),
            root,
            live,
            base: None,
            manifest_bytes: Vec::new(),
            experiment,
            job_id: None,
            inputs,
        }
    }

    /// Take the real backup of the live database as it is now.
    async fn take_backup(&mut self) {
        let store = self.inputs.open(&self.live).await;
        store.backup(&self.backup).await.unwrap();
        self.inputs.close(store, &self.live).await;
        self.manifest_bytes = std::fs::read(self.backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&self.manifest_bytes).unwrap();
        assert_eq!(manifest.watermarks.len(), 1);
        self.base = Some(manifest.watermarks[0].clone());
    }

    fn backup_database(&self) -> PathBuf {
        self.backup.join("rsia.sqlite3")
    }

    /// Arrange deliberately unqualified stored bytes only for diagnostic/negative controls.
    /// No Store or producer remains live; this is not a healthy producer fixture.
    fn inject_look_alikes(&self, values: &[(&str, Value)]) {
        let program = r#"
import fcntl, json, os, pathlib, sqlite3, sys
database = pathlib.Path(sys.argv[1]).resolve()
directory = os.open(database.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
legacy = None
con = None
try:
    fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
    legacy = os.open(database.parent / ".rsia.lock", os.O_RDONLY | os.O_NOFOLLOW)
    fcntl.flock(legacy, fcntl.LOCK_EX | fcntl.LOCK_NB)
    con = sqlite3.connect(database.as_uri() + "?mode=rw", uri=True, isolation_level=None)
    con.execute("BEGIN IMMEDIATE")
    for identity, body in json.loads(sys.argv[2]):
        con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,'artifact',?,?,?)",
                    ("n", identity, "admin", json.dumps(body, separators=(",", ":"))))
    con.execute("COMMIT")
finally:
    if con is not None:
        con.close()
    if legacy is not None:
        os.close(legacy)
    os.close(directory)
"#;
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(program)
            .arg(&self.live)
            .arg(serde_json::to_string(values).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
    }

    async fn put_unprotected_look_alike(&self) {
        let store = self.inputs.open(&self.live).await;
        let admin = admin();
        let (id, body) = look_alikes().into_iter().nth(2).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "artifact", id, admin.actor(), &body)
            .await
            .unwrap();
        session.commit().await.unwrap();
        self.inputs.close(store, &self.live).await;
    }

    /// Take one step on the anchor, with the engine's own coordinator.
    async fn step(&mut self, step: Step) -> Stepped {
        let store = self.inputs.open(&self.live).await;
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
        self.inputs.close(store, &self.live).await;
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
        let store = self.inputs.open(&self.live).await;
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
        self.inputs.close(store, &self.live).await;
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
        self.restore_with_sources(name, delta, &self.backup, &self.live)
    }

    fn restore_with_sources(
        &self,
        name: &str,
        delta: &Path,
        backup: &Path,
        anchor: &Path,
    ) -> Restored {
        let dest = self.root.join(name);
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--native-reader")
            .arg(&self.inputs.reader)
            .arg("--prepare-configs-file")
            .arg(&self.inputs.originals)
            .arg("--backup")
            .arg(backup)
            .arg("--dest")
            .arg(&dest)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(anchor)
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
async fn quarantine_reason(
    scenario: &Scenario,
    step: Step,
    data: &Path,
    anchor: &Path,
    directory: &Path,
) -> String {
    let before = snapshot(directory);
    let error = match scenario.inputs.gate(data, Some(anchor)).evaluate().await {
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
    scenario.inject_look_alikes(&look_alikes());

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
        let verified = scenario
            .inputs
            .gate(&restored.data(), Some(&scenario.live))
            .evaluate()
            .await
            .unwrap_or_else(|error| panic!("{step:?}: {error}"));
        assert_eq!(verified.posture(), RecoveryPosture::RestoredVerified);

        // ... and stops verifying the moment the anchor takes the step.
        let stepped = scenario.step(step).await;
        let reason = quarantine_reason(
            &scenario,
            step,
            &restored.data(),
            &scenario.live,
            &restored.dest,
        )
        .await;
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
    let decision = scenario
        .inputs
        .gate(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    let reopened = scenario.inputs.open(&restored.data()).await;
    scenario.inputs.close(reopened, &restored.data()).await;
    assert!(!restored.dest.join(RESTORE_ADMISSION_FILE).exists());
    decision.admit().unwrap();
    assert!(restored.dest.join(RESTORE_ADMISSION_FILE).is_file());
    // the next start needs no anchor
    let again = scenario
        .inputs
        .gate(&restored.data(), None)
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
    let decision = scenario
        .inputs
        .gate(&restored.data(), Some(&scenario.live))
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
    scenario.put_unprotected_look_alike().await;
    scenario
        .restore_without_events("restored")
        .assert_restored("an unrelated artifact", 0);

    // Selection is not qualification. Unknown rsia.* bytes remain unprotected
    // selector diagnostics, but may not license restore or any temporal write.
    // Pending secondary acceptance: the source supplies no healthy unknown-schema contract.
    for value in look_alikes().into_iter().take(2) {
        let mut invalid = Scenario::begin().await;
        invalid.take_backup().await;
        invalid
            .restore_without_events("healthy-before-fault")
            .assert_restored("the genuine fixture before unknown-schema injection", 0);
        invalid.inject_look_alikes(std::slice::from_ref(&value));
        assert!(!script_selection(&invalid.live).contains(&format!("{NS}/artifact/{}", value.0)));
        let before = snapshot(invalid.live.parent().unwrap());
        let refused = invalid.restore_without_events("unqualified");
        assert_eq!(refused.code, Some(1), "{}", refused.output());
        assert_eq!(
            refused.stderr,
            "ISOLATE native restore refused: clock_regressed; not mounting\n"
        );
        assert!(refused.stdout.is_empty() && !refused.dest.exists());
        assert_eq!(snapshot(invalid.live.parent().unwrap()), before);
        let prefix = format!(
            ".{}.restore-",
            refused.dest.file_name().unwrap().to_string_lossy()
        );
        assert!(!std::fs::read_dir(&invalid.root).unwrap().any(|row| {
            row.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&prefix)
        }));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn restore_real_prepare() {
    use evo_engine::service::HostService;
    use std::os::unix::fs::DirBuilderExt;

    let owner = tempfile::tempdir().unwrap();
    let root = owner.path().canonicalize().unwrap();
    let source_dir = root.join("live");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&source_dir)
        .unwrap();
    let live = source_dir.join("rsia.sqlite3");
    let inputs = RestoreFixtureInputs::new(&root);
    let config = inputs.config.clone();
    let store = Store::open_with_prepare_configs(&live, vec![config.clone()])
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin(), &hash(b"restore-prepare-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let (prepared, host, agent, request) = inputs.prepare(&store).await;
    let backup = root.join("backup");
    store.backup(&backup).await.unwrap();
    let raw = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest.watermarks.len(), 1);
    assert_eq!(manifest.watermarks[0].namespace, NS);

    inputs.close(store, &live).await;
    let live_before = snapshot(&source_dir);
    let backup_before = snapshot(&backup);
    let delta = root.join("empty-delta.json");
    std::fs::write(
        &delta,
        serde_json::to_vec(&json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&raw),
            "namespaces":manifest.watermarks.iter().map(|row| json!({
                "namespace":row.namespace,"base_seq":row.seq,"base_digest":row.digest,
                "events":[],"latest_seq":row.seq,"latest_digest":row.digest
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();

    let dest = root.join("restored");
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(RESTORE_SCRIPT)
        .arg("--backup")
        .arg(&backup)
        .arg("--dest")
        .arg(&dest)
        .arg("--revoke-delta")
        .arg(&delta)
        .arg("--trusted-revocations-db")
        .arg(&live)
        .arg("--native-reader")
        .arg(&inputs.reader)
        .arg("--prepare-configs-file")
        .arg(&inputs.originals)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        b"RESTORE_OK events=0 migrations_applied=none\n"
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(dest.join("restore.json")).unwrap()).unwrap();
    assert_eq!(receipt["schema_version"], "rsia.restore_receipt.v2");
    assert_eq!(receipt["backup_manifest_sha256"], hash(&raw));
    assert_eq!(receipt["replayed_revoke_events"], 0);
    assert_eq!(receipt["isolated"], false);
    assert_eq!(snapshot(&source_dir), live_before);
    assert_eq!(snapshot(&backup), backup_before);

    let restored_db = dest.join("rsia.sqlite3");
    let restored = Store::open_with_prepare_configs(&restored_db, vec![config.clone()])
        .await
        .unwrap();
    let reopened = HostService::new(restored.clone(), host).unwrap();
    let replayed = reopened.prepare(&agent, request, &config).await.unwrap();
    assert_eq!(
        fingerprint(&replayed).unwrap(),
        fingerprint(&prepared).unwrap()
    );
    inputs.close(restored, &restored_db).await;
}

// §18.4 replacements: raw preservation stays in the storage test; genuine
// producer compatibility/admission is tested here. No physical legacy
// event/evaluation business-source equivalence or paid/consumer credit is claimed.
#[cfg(unix)]
mod qualified_prefix_replacements {
    use super::*;
    const PYTHON: &str = r####"
import fcntl, hashlib, json, os, pathlib, shutil, sqlite3, stat, sys
database = pathlib.Path(sys.argv[1]).resolve()
operation = sys.argv[2]
arg = json.loads(sys.argv[3])
def quoted(name):
    return '"' + name.replace('"', '""') + '"'
def readonly(path):
    for suffix in ["-wal", "-shm", "-journal"]:
        sidecar = pathlib.Path(str(path) + suffix)
        assert not sidecar.exists() or suffix == "-wal" and sidecar.stat().st_size == 0, str(sidecar)
    return sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)
def schema(con):
    return list(con.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name"))
def names(con):
    return [r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
def columns(con, table, with_rowid=True):
    result = [r[1] for r in con.execute("PRAGMA table_info(" + quoted(table) + ")")]
    if with_rowid:
        try:
            con.execute("SELECT rowid FROM " + quoted(table) + " LIMIT 0")
            result.insert(0, "rowid")
        except sqlite3.OperationalError:
            pass
    return result
def table_rows(con, table):
    cols = columns(con, table)
    return list(con.execute("SELECT " + ",".join(map(quoted, cols)) + " FROM " + quoted(table)))
def inspect(con):
    ledger = list(con.execute("SELECT quote(version),quote(description),quote(success),quote(checksum) FROM _sqlx_migrations ORDER BY version"))
    tables = []
    for table in names(con):
        if table == "_sqlx_migrations":
            continue
        expressions = ",".join("quote(" + quoted(c) + ")" for c in columns(con, table))
        rows = sorted(list(con.execute("SELECT " + expressions + " FROM " + quoted(table))))
        tables.append([table, rows])
    assert con.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    assert not list(con.execute("PRAGMA foreign_key_check"))
    timings = list(con.execute("SELECT version,CAST(installed_on AS TEXT),typeof(installed_on),execution_time,typeof(execution_time) FROM _sqlx_migrations ORDER BY version"))
    return {"image":{"schema":schema(con),"ledger":ledger,"tables":tables},"timings":timings}
def safe_copy_tree(source, dest):
    if not source.exists():
        return
    for item in source.rglob("*"):
        meta = item.lstat()
        assert not stat.S_ISLNK(meta.st_mode)
        assert stat.S_ISDIR(meta.st_mode) or stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1
    shutil.copytree(source, dest)
def manifest_for(path, created_at):
    con = readonly(path)
    migrations = [{"version":v,"description":d,"checksum_hex":bytes(c).hex(),"success":bool(s)}
        for v,d,c,s in con.execute("SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version")]
    watermarks = [{"namespace":n,"seq":s,"digest":d} for n,s,d in con.execute("SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace")]
    con.close()
    data = path.read_bytes()
    blobs = []
    root = path.parent / "blobs"
    if root.exists():
        for item in sorted(root.rglob("*")):
            if item.is_file():
                body = item.read_bytes()
                digest = hashlib.sha256(body).hexdigest()
                assert item.name == digest
                blobs.append({"path":item.relative_to(path.parent).as_posix(),"sha256":digest,"bytes":len(body)})
    return {"schema_version":"rsia.backup.v2","complete":True,
        "database":{"path":"rsia.sqlite3","sha256":hashlib.sha256(data).hexdigest(),"bytes":len(data)},
        "blobs":blobs,"migrations":migrations,"watermarks":watermarks,"created_at":created_at}
if operation == "inspect":
    con = readonly(database)
    result = inspect(con)
    con.close()
elif operation == "prefix":
    source = readonly(database)
    specs = arg["migrations"]
    versions = arg["versions"]
    assert versions in [[1,2,4,5],[1,2,4,5,6]]
    ledger_columns = columns(source, "_sqlx_migrations")
    ledger = list(source.execute("SELECT " + ",".join(map(quoted, ledger_columns)) + " FROM _sqlx_migrations ORDER BY version"))
    by_name = lambda row:dict(zip(ledger_columns,row))
    records = [by_name(r) for r in ledger]
    assert [r["version"] for r in records] == [r["version"] for r in specs]
    for actual, spec in zip(records, specs):
        assert actual["description"] == spec["description"] and actual["success"] == 1
        assert bytes(actual["checksum"]).hex() == hashlib.sha384(spec["sql"].encode()).hexdigest()
    full = sqlite3.connect(":memory:")
    for spec in specs:
        full.executescript(spec["sql"])
    destination = pathlib.Path(arg["dest"])
    assert not destination.exists()
    fd = os.open(destination, os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
    os.close(fd)
    target = sqlite3.connect(destination)
    target.execute("PRAGMA foreign_keys=OFF")
    for spec in specs:
        if spec["version"] in versions:
            target.executescript(spec["sql"])
    target.execute(source.execute("SELECT sql FROM sqlite_master WHERE name='_sqlx_migrations'").fetchone()[0])
    expected_all = set(names(full)) | {"_sqlx_migrations"}
    assert set(names(source)) == expected_all, "unclassified source table"
    omitted = expected_all - set(names(target))
    assert omitted == ({"revoke_cleanup_jobs","revoke_cleanup_frontier","revoke_cleanup_events"} if versions[-1] == 5 else set())
    for table in omitted:
        assert not table_rows(source, table), "nonempty later table: " + table
    assert not source.execute("SELECT 1 FROM objects WHERE kind='formal_parent' LIMIT 1").fetchone()
    for table in names(target):
        assert columns(target, table) == columns(source, table), table
        if table == "_sqlx_migrations":
            cols = columns(target, table)
            rows = [r for r in ledger if by_name(r)["version"] in versions]
        elif table == "skill_fts" or table.startswith("skill_fts_"):
            assert sorted(table_rows(source, table)) == sorted(table_rows(target, table)), "nonempty FTS needs explicit representation"
            continue
        else:
            cols = columns(target, table)
            rows = table_rows(source, table)
        target.execute("DELETE FROM " + quoted(table))
        if rows:
            target.executemany("INSERT INTO " + quoted(table) + "(" + ",".join(map(quoted,cols)) + ") VALUES(" + ",".join("?" for c in cols) + ")", rows)
    target.commit()
    target.execute("PRAGMA foreign_keys=ON")
    result = inspect(target)
    assert [r[0] for r in target.execute("SELECT version FROM _sqlx_migrations ORDER BY version")] == versions
    assert list(target.execute("SELECT " + ",".join(map(quoted, ledger_columns)) + " FROM _sqlx_migrations ORDER BY version")) == [r for r in ledger if by_name(r)["version"] in versions]
    target.close()
    full.close()
    source.close()
    safe_copy_tree(database.parent / "blobs", destination.parent / "blobs")
elif operation == "copy":
    destination = pathlib.Path(arg["dest"])
    assert not destination.exists()
    source = readonly(database)
    fd = os.open(destination, os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
    os.close(fd)
    target = sqlite3.connect(destination)
    source.backup(target)
    result = inspect(target)
    target.close()
    source.close()
    safe_copy_tree(database.parent / "blobs", destination.parent / "blobs")
elif operation == "manifest":
    result = manifest_for(database, arg["created_at"])
    fd = os.open(database.parent / "backup-manifest.json", os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
    os.close(fd)
elif operation == "fault":
    meta = database.lstat()
    assert stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1 and meta.st_uid == os.getuid()
    directory = os.open(database.parent, os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try:
        fcntl.flock(directory, fcntl.LOCK_EX|fcntl.LOCK_NB)
        con = sqlite3.connect(database, isolation_level=None)
        # This private, closed negative fixture may deliberately violate FK links.
        # The production collector and every production guard remain unchanged.
        con.execute("PRAGMA foreign_keys=OFF")
        con.execute("BEGIN IMMEDIATE")
        for sql in arg["sql"]:
            cursor = con.execute(sql)
            if sql.lstrip().upper().startswith("UPDATE"):
                assert cursor.rowcount > 0, sql
        con.execute("COMMIT")
        con.close()
    finally:
        os.close(directory)
    result = {"sha256":hashlib.sha256(database.read_bytes()).hexdigest(),"bytes":database.stat().st_size}
else:
    raise AssertionError(operation)
print(json.dumps(result, ensure_ascii=False, separators=(",",":")))
"####;

    const VERSIONS: [i64; 6] = [1, 2, 4, 5, 6, 7];
    const LEGACY_BLOB: &[u8] = b"legacy blob bytes";

    fn migration_specs() -> Value {
        json!([
            {"version":1,"description":"runtime","sql":include_str!("../../evo-storage/migrations/0001_runtime.sql")},
            {"version":2,"description":"revoke graph","sql":include_str!("../../evo-storage/migrations/0002_revoke_graph.sql")},
            {"version":4,"description":"replay worlds","sql":include_str!("../../evo-storage/migrations/0004_replay_worlds.sql")},
            {"version":5,"description":"root budget","sql":include_str!("../../evo-storage/migrations/0005_root_budget.sql")},
            {"version":6,"description":"revoke cleanup","sql":include_str!("../../evo-storage/migrations/0006_revoke_cleanup.sql")},
            {"version":7,"description":"formal parent kind","sql":include_str!("../../evo-storage/migrations/0007_formal_parent_kind.sql")}
        ])
    }

    fn fixture_python(database: &Path, operation: &str, argument: Value) -> Value {
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(PYTHON)
            .arg(database)
            .arg(operation)
            .arg(serde_json::to_string(&argument).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{operation}: {:?}", output.stderr);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn private_directory(root: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::DirBuilderExt;
        let path = root.join(name);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        path
    }

    fn inspect(database: &Path) -> Value {
        fixture_python(database, "inspect", json!({}))
    }

    fn copy_database(source: &Path, root: &Path, name: &str) -> PathBuf {
        let dest = private_directory(root, name).join("rsia.sqlite3");
        fixture_python(source, "copy", json!({"dest":dest}));
        dest
    }

    /// Current producer facts materialized into the actual older SQL schema.
    /// This is a compatibility fixture, not a claim of old-version business origin.
    /// Every original prefix ledger column is copied, and a nonempty later table,
    /// unknown table, nonempty unhandled FTS state or formal_parent fails construction.
    struct PrefixFixture {
        base: Scenario,
        versions: Vec<i64>,
        prefix: PathBuf,
        anchor: PathBuf,
        reference: PathBuf,
        backup: PathBuf,
        manifest: Vec<u8>,
        checksums: BTreeMap<i64, String>,
        prepared: evo_core::Prepared,
        host: Context,
        agent: Context,
        request: evo_core::Prepare,
    }

    impl PrefixFixture {
        async fn new(versions: &[i64]) -> Self {
            let mut base = Scenario::begin().await;
            let store = base.inputs.open(&base.live).await;
            let (prepared, host, agent, request) = base.inputs.prepare(&store).await;
            assert_eq!(
                store.put_blob(&admin(), LEGACY_BLOB).await.unwrap(),
                hash(LEGACY_BLOB)
            );
            let capture = base.root.join("prefix-current-capture");
            store.backup(&capture).await.unwrap();
            base.inputs.close(store, &base.live).await;
            base.manifest_bytes = std::fs::read(capture.join("backup-manifest.json")).unwrap();

            let source_before = snapshot(base.live.parent().unwrap());
            let prefix = private_directory(&base.root, "compat-prefix").join("rsia.sqlite3");
            if versions == VERSIONS.as_slice() {
                fixture_python(&base.live, "copy", json!({"dest":prefix}));
            } else {
                fixture_python(
                    &base.live,
                    "prefix",
                    json!({
                        "dest":prefix,"versions":versions,"migrations":migration_specs()
                    }),
                );
            }
            assert_eq!(snapshot(base.live.parent().unwrap()), source_before);
            let prefix_before = snapshot(prefix.parent().unwrap());
            let anchor = copy_database(&prefix, &base.root, "prefix-anchor");
            let store = base.inputs.open(&anchor).await;
            base.inputs.close(store, &anchor).await;
            let reference = copy_database(&prefix, &base.root, "prefix-store-open");
            let store = base.inputs.open(&reference).await;
            base.inputs.close(store, &reference).await;
            let backup_db = copy_database(&prefix, &base.root, "prefix-backup");
            let backup = backup_db.parent().unwrap().to_path_buf();
            let manifest_value = fixture_python(
                &backup_db,
                "manifest",
                json!({"created_at":evo_core::now().unwrap()}),
            );
            let manifest = serde_json::to_vec_pretty(&manifest_value).unwrap();
            std::fs::write(backup.join("backup-manifest.json"), &manifest).unwrap();
            assert_eq!(snapshot(prefix.parent().unwrap()), prefix_before);
            let genuine: BackupManifest = serde_json::from_slice(&base.manifest_bytes).unwrap();
            let checksums = genuine
                .migrations
                .into_iter()
                .map(|r| (r.version, r.checksum_hex))
                .collect();
            let compatible: BackupManifest = serde_json::from_slice(&manifest).unwrap();
            assert_eq!(compatible.watermarks.len(), 1);
            base.base = Some(compatible.watermarks[0].clone());
            base.manifest_bytes = manifest.clone();
            Self {
                base,
                versions: versions.to_vec(),
                prefix,
                anchor,
                reference,
                backup,
                manifest,
                checksums,
                prepared,
                host,
                agent,
                request,
            }
        }

        fn checksum(&self, version: i64) -> String {
            self.checksums[&version].clone()
        }

        fn restore(&self, name: &str, revocation: Option<&Revocation>) -> Restored {
            let events = revocation.map_or(&[][..], std::slice::from_ref);
            let delta = self.base.delta(&format!("{name}-delta.json"), events);
            self.base
                .restore_with_sources(name, &delta, &self.backup, &self.anchor)
        }

        async fn assert_success(&self, name: &str, revocation: Option<&Revocation>) {
            let anchor_before = snapshot(self.anchor.parent().unwrap());
            let backup_before = snapshot(&self.backup);
            let prefix_before = snapshot(self.prefix.parent().unwrap());
            let events = usize::from(revocation.is_some());
            let applied = VERSIONS
                .into_iter()
                .filter(|version| !self.versions.contains(version))
                .collect::<Vec<_>>();
            let applied_text = if applied.is_empty() {
                "none".into()
            } else {
                applied
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let result = self.restore(name, revocation);
            result.assert_restored("qualified compatible prefix", events);
            assert_eq!(
                result.stdout,
                format!("RESTORE_OK events={events} migrations_applied={applied_text}\n")
            );
            assert!(result.stderr.is_empty());
            assert_eq!(snapshot(self.anchor.parent().unwrap()), anchor_before);
            assert_eq!(snapshot(&self.backup), backup_before);
            assert_eq!(snapshot(self.prefix.parent().unwrap()), prefix_before);

            let mut entries = std::fs::read_dir(&result.dest)
                .unwrap()
                .map(|row| row.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            entries.sort();
            let mut expected = vec!["blobs", "restore.json", "rsia.sqlite3"];
            if !applied.is_empty() {
                expected.push("restore.migrations.json");
                expected.sort();
            }
            assert_eq!(entries, expected);
            assert_eq!(
                std::fs::read(
                    result
                        .dest
                        .join(format!("blobs/{NS}/{}", hash(LEGACY_BLOB)))
                )
                .unwrap(),
                LEGACY_BLOB
            );
            let receipt_text = std::fs::read_to_string(result.dest.join("restore.json")).unwrap();
            let receipt = evo_storage::lifecycle::parse_strict_json(&receipt_text).unwrap();
            let mut keys = receipt
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "backup_manifest_sha256",
                    "isolated",
                    "receipt_scope",
                    "replayed_revoke_events",
                    "schema_version",
                    "trusted_anchor"
                ]
            );
            let mut keys = receipt["trusted_anchor"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "control_plane_digest",
                    "path_local_only",
                    "scope",
                    "verified_at",
                    "watermarks"
                ]
            );
            assert_eq!(receipt["schema_version"], "rsia.restore_receipt.v2");
            assert_eq!(receipt["isolated"], false);
            assert_eq!(receipt["receipt_scope"], "local_only_not_for_export");
            assert_eq!(receipt["backup_manifest_sha256"], hash(&self.manifest));
            assert_eq!(receipt["replayed_revoke_events"], events);
            let data = read_control_plane_facts(&result.data()).await.unwrap();
            let anchor = read_control_plane_facts(&self.anchor).await.unwrap();
            assert_eq!(data, anchor);
            assert_eq!(
                data.watermarks
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [NS]
            );
            for (namespace, mark) in &data.watermarks {
                assert_eq!(
                    receipt["trusted_anchor"]["watermarks"][namespace]["seq"],
                    mark.seq
                );
                assert_eq!(
                    receipt["trusted_anchor"]["watermarks"][namespace]["digest"],
                    mark.digest
                );
            }
            assert_eq!(
                receipt["trusted_anchor"]["watermarks"]
                    .as_object()
                    .unwrap()
                    .len(),
                data.watermarks.len()
            );
            assert!(
                data.root_budget_tables
                    .get("root_budgets")
                    .is_some_and(|rows| rows.len() == 1)
            );

            let actual = inspect(&result.data());
            let reference = inspect(&self.reference);
            if revocation.is_none() {
                assert_eq!(actual["image"]["schema"], reference["image"]["schema"]);
                assert_eq!(actual["image"]["ledger"], reference["image"]["ledger"]);
                assert_eq!(actual["image"]["tables"], reference["image"]["tables"]);
                assert_eq!(actual["image"], reference["image"]);
                assert_eq!(actual["image"], inspect(&self.anchor)["image"]);
                assert_eq!(
                    data,
                    read_control_plane_facts(&self.reference).await.unwrap()
                );
            }
            let old_timings = inspect(&self.backup.join("rsia.sqlite3"))["timings"]
                .as_array()
                .unwrap()
                .clone();
            assert_eq!(
                &actual["timings"].as_array().unwrap()[..self.versions.len()],
                old_timings.as_slice()
            );
            for row in &actual["timings"].as_array().unwrap()[self.versions.len()..] {
                assert!(applied.contains(&row[0].as_i64().unwrap()));
                assert_eq!(row[2], "text");
                assert_eq!(row[4], "integer");
                assert!(row[3].as_i64().unwrap() >= 0);
            }
            assert_eq!(
                actual["image"]["ledger"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row[0].as_str().unwrap().parse::<i64>().unwrap())
                    .collect::<Vec<_>>(),
                VERSIONS
            );
            for row in actual["image"]["ledger"].as_array().unwrap() {
                let version = row[0].as_str().unwrap().parse::<i64>().unwrap();
                assert_eq!(row[2], "1");
                assert_eq!(
                    row[3].as_str().unwrap().to_ascii_lowercase(),
                    format!("x'{}'", self.checksum(version))
                );
            }
            let record_path = result.dest.join("restore.migrations.json");
            if applied.is_empty() {
                assert!(!record_path.exists());
            } else {
                let text = std::fs::read_to_string(record_path).unwrap();
                assert!(text.ends_with('\n'));
                let record = evo_storage::lifecycle::parse_strict_json(&text).unwrap();
                let mut keys = record
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                keys.sort_unstable();
                assert_eq!(
                    keys,
                    ["applied", "current_versions", "schema", "source_versions"]
                );
                let positions = ["schema", "source_versions", "applied", "current_versions"]
                    .map(|key| text.find(&format!("\"{key}\":")).unwrap());
                assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
                assert_eq!(
                    record,
                    json!({
                        "schema":"rsia.restore_migrations.v1","source_versions":self.versions,
                        "applied":applied.iter().map(|version| json!({
                            "version":version,"checksum_sha384":self.checksum(*version)
                        })).collect::<Vec<_>>(),"current_versions":VERSIONS,
                    })
                );
            }
            assert_eq!(
                self.base
                    .inputs
                    .gate(&result.data(), Some(&self.anchor))
                    .evaluate()
                    .await
                    .unwrap()
                    .posture(),
                RecoveryPosture::RestoredVerified
            );
            let before_open = inspect(&result.data());
            let store = self.base.inputs.open(&result.data()).await;
            let service =
                evo_engine::service::HostService::new(store.clone(), self.host.clone()).unwrap();
            let replayed = service
                .prepare(&self.agent, self.request.clone(), &self.base.inputs.config)
                .await
                .unwrap();
            assert_eq!(
                fingerprint(&replayed).unwrap(),
                fingerprint(&self.prepared).unwrap()
            );
            if let Some(event) = revocation {
                let mut session = store.session().await.unwrap();
                let replayed: RevokeTombstone = session
                    .need(&admin(), "tombstone", &event.source)
                    .await
                    .unwrap();
                assert_eq!(replayed.watermark_digest, event.digest);
                assert_eq!(
                    session.watermark(&admin()).await.unwrap(),
                    Some((event.seq, event.digest.clone()))
                );
                session.commit().await.unwrap();
                let status = LifecycleStore::cleanup_status(
                    &admin(),
                    &store,
                    &format!(
                        "revoke-{}",
                        &fingerprint(&(
                            NS,
                            "run",
                            &event.source,
                            replayed.watermark_seq,
                            &event.digest
                        ))
                        .unwrap()[..32]
                    ),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(status.state, CleanupState::Pending);
                assert_eq!(
                    data.tombstones[&(NS.to_owned(), event.source.clone())]["watermark_seq"],
                    event.seq
                );
            }
            self.base.inputs.close(store, &result.data()).await;
            assert_eq!(inspect(&result.data()), before_open);
        }

        async fn begin_revoke(&self) -> Revocation {
            let store = self.base.inputs.open(&self.anchor).await;
            let status = LifecycleStore::begin_revoke(
                &admin(),
                &store,
                TypedObjectRef {
                    kind: "run".into(),
                    id: REVOKED_RUN.into(),
                },
                "revoked after the upgrade",
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(status.state, CleanupState::Pending);
            let mut session = store.session().await.unwrap();
            let tombstone: RevokeTombstone = session
                .need(&admin(), "tombstone", REVOKED_RUN)
                .await
                .unwrap();
            session.commit().await.unwrap();
            self.base.inputs.close(store, &self.anchor).await;
            Revocation {
                source: REVOKED_RUN.into(),
                seq: i64::try_from(tombstone.watermark_seq).unwrap(),
                digest: tombstone.watermark_digest.clone(),
                tombstone_digest: hash(&serde_json::to_vec(&tombstone).unwrap()),
                reason: tombstone.reason.clone(),
                created_at: tombstone.created_at,
            }
        }

        fn select_backup_copy(&mut self, source: &Path, name: &str) {
            let before = snapshot(source);
            let database = copy_database(&source.join("rsia.sqlite3"), &self.base.root, name);
            let directory = database.parent().unwrap().to_path_buf();
            let manifest = std::fs::read(source.join("backup-manifest.json")).unwrap();
            std::fs::copy(
                source.join("backup-manifest.json"),
                directory.join("backup-manifest.json"),
            )
            .unwrap();
            assert_eq!(snapshot(&directory), before);
            assert_eq!(snapshot(source), before);
            self.backup = directory;
            self.manifest = manifest;
            self.base.manifest_bytes = self.manifest.clone();
        }

        async fn capture_pending_current(&mut self) {
            assert_eq!(self.versions, VERSIONS);
            let _event = self.begin_revoke().await;
            let store = self.base.inputs.open(&self.anchor).await;
            let backup = self.base.root.join("pending-current-backup");
            store.backup(&backup).await.unwrap();
            self.base.inputs.close(store, &self.anchor).await;
            self.backup = backup;
            self.manifest = std::fs::read(self.backup.join("backup-manifest.json")).unwrap();
            let manifest: BackupManifest = serde_json::from_slice(&self.manifest).unwrap();
            self.base.base = Some(
                manifest
                    .watermarks
                    .into_iter()
                    .find(|r| r.namespace == NS)
                    .unwrap(),
            );
            self.base.manifest_bytes = self.manifest.clone();
            self.reference = copy_database(&self.anchor, &self.base.root, "pending-reference");
            self.prefix = copy_database(&self.anchor, &self.base.root, "pending-prefix");
        }

        fn fault(&mut self, sql: &[&str], edit: impl FnOnce(&mut Value)) {
            let path = self.backup.join("rsia.sqlite3");
            if !sql.is_empty() {
                fixture_python(&path, "fault", json!({"sql":sql}));
            }
            let mut document: Value = serde_json::from_slice(&self.manifest).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            document["database"]["sha256"] = json!(hash(&bytes));
            document["database"]["bytes"] = json!(bytes.len());
            edit(&mut document);
            self.manifest = serde_json::to_vec_pretty(&document).unwrap();
            self.base.manifest_bytes = self.manifest.clone();
            std::fs::write(self.backup.join("backup-manifest.json"), &self.manifest).unwrap();
        }

        fn assert_refused(&self, name: &str, reason: &str) {
            let anchor_before = snapshot(self.anchor.parent().unwrap());
            let backup_before = snapshot(&self.backup);
            let prefix_before = snapshot(self.prefix.parent().unwrap());
            let result = self.restore(name, None);
            assert_eq!(result.code, Some(1), "{}", result.output());
            assert_eq!(result.stderr, format!("ISOLATE {reason}; not mounting\n"));
            assert!(result.stdout.is_empty() && !result.dest.exists());
            let staging = format!(".{name}.restore-");
            assert!(!std::fs::read_dir(&self.base.root).unwrap().any(|row| {
                row.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&staging)
            }));
            assert_eq!(snapshot(self.anchor.parent().unwrap()), anchor_before);
            assert_eq!(snapshot(&self.backup), backup_before);
            assert_eq!(snapshot(self.prefix.parent().unwrap()), prefix_before);
        }
    }

    // Original restore_prefix_upgrade_v42.rs mapping:
    // upgrade_matches_store_open's raw rows/timings/checksums stay in storage.
    // Its qualified restore/receipt/anchor/gate/no-further-upgrade assertions
    // are exercised here over genuine producers, without claiming legacy
    // physical event/evaluation business-source equivalence.
    #[tokio::test]
    async fn real_producer_prefix_0005_restores_like_store_open() {
        PrefixFixture::new(&[1, 2, 4, 5])
            .await
            .assert_success("restored-prefix-five", None)
            .await;
    }

    #[tokio::test]
    async fn real_producer_prefix_0006_restores_like_store_open() {
        PrefixFixture::new(&[1, 2, 4, 5, 6])
            .await
            .assert_success("restored-prefix-six", None)
            .await;
    }

    #[tokio::test]
    async fn real_producer_prefix_0005_replays_later_revocation() {
        let fixture = PrefixFixture::new(&[1, 2, 4, 5]).await;
        fixture
            .assert_success("healthy-before-revocation", None)
            .await;
        let event = fixture.begin_revoke().await;
        fixture
            .assert_success("restored-later-revocation", Some(&event))
            .await;
    }

    #[tokio::test]
    async fn real_producer_current_backup_applies_no_migration() {
        let current = PrefixFixture::new(&VERSIONS).await;
        current.assert_success("restored-current", None).await;
        let five = PrefixFixture::new(&[1, 2, 4, 5]).await;
        let six = PrefixFixture::new(&[1, 2, 4, 5, 6]).await;
        let late = PrefixFixture::new(&[1, 2, 4, 5]).await;
        five.assert_success("python-five-control", None).await;
        six.assert_success("python-six-control", None).await;
        late.assert_success("python-late-before", None).await;
        let event = late.begin_revoke().await;
        late.assert_success("python-late-control", Some(&event))
            .await;
        let entry = |f: &PrefixFixture, event: Option<&Revocation>| {
            assert_eq!(
                std::fs::read(&f.base.inputs.originals).unwrap(),
                std::fs::read(&current.base.inputs.originals).unwrap()
            );
            let delta = f.base.delta(
                "python-empty-or-late.json",
                event.map_or(&[][..], std::slice::from_ref),
            );
            json!({"prefix":f.prefix,"anchor":f.anchor,"backup":f.backup,
                "reference":f.reference,"delta":delta})
        };
        let fixtures = json!({"5":entry(&five,None),"6":entry(&six,None),
            "current":entry(&current,None),"late":entry(&late,Some(&event))});
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(current.base.root.join("python-fixtures.json"))
            .unwrap();
        file.write_all(&serde_json::to_vec(&fixtures).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        let before = [&five, &six, &current, &late].map(|f| {
            (
                snapshot(f.prefix.parent().unwrap()),
                snapshot(f.anchor.parent().unwrap()),
                snapshot(&f.backup),
            )
        });
        let output = Command::new("python3")
            .arg("-B")
            .args(["-m", "unittest", "-v", "scripts.test_restore_backup"])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("RSIA_TEST_NATIVE_READER", &current.base.inputs.reader)
            .env(
                "RSIA_TEST_PREPARE_CONFIGS_FILE",
                &current.base.inputs.originals,
            )
            .env("RSIA_TEST_RESTORE_FIXTURE_ROOT", &current.base.root)
            .output()
            .unwrap();
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stdout.is_empty(), "{:?}", output.stdout);
        let terminal = String::from_utf8(output.stderr).unwrap();
        assert!(terminal.contains("\nRan 18 tests in "));
        assert!(terminal.ends_with("\nOK\n"));
        assert_eq!(
            [&five, &six, &current, &late].map(|f| (
                snapshot(f.prefix.parent().unwrap()),
                snapshot(f.anchor.parent().unwrap()),
                snapshot(&f.backup)
            )),
            before
        );
    }

    #[tokio::test]
    async fn every_original_ledger_fault_has_an_independently_healthy_control() {
        const NOT_A_PREFIX: &str =
            "unsupported manifest migrations: not a prefix of the current migration set";
        const PREDATES: &str =
            "backup predates the root budget ledger (0005); protected facts cannot be verified";
        let mut six = PrefixFixture::new(&[1, 2, 4, 5, 6]).await;
        let six_backup = six.backup.clone();
        let mut current = PrefixFixture::new(&VERSIONS).await;
        let current_backup = current.backup.clone();
        for name in [
            "gap",
            "predates",
            "future",
            "short-manifest",
            "long-manifest",
            "checksum",
            "checksum-db",
            "unsuccessful",
            "unsuccessful-db",
            "stale-anchor",
            "conflict",
        ] {
            let fixture = if matches!(name, "gap" | "predates" | "future" | "short-manifest") {
                current.select_backup_copy(&current_backup, &format!("ledger-{name}-backup"));
                &mut current
            } else {
                six.select_backup_copy(&six_backup, &format!("ledger-{name}-backup"));
                &mut six
            };
            fixture
                .assert_success(&format!("healthy-before-{name}"), None)
                .await;
            let reason = match name {
                "gap" => {
                    fixture.fault(&["DELETE FROM _sqlx_migrations WHERE version=6"], |m| {
                        m["migrations"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|r| r["version"] != 6)
                    });
                    NOT_A_PREFIX
                }
                "predates" => {
                    fixture.fault(&["DELETE FROM _sqlx_migrations WHERE version>=5"], |m| {
                        m["migrations"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|r| r["version"].as_i64().unwrap() < 5)
                    });
                    PREDATES
                }
                "future" => {
                    fixture.fault(
                        &["INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(999,'future',TRUE,zeroblob(48),0)"],
                        |m| m["migrations"].as_array_mut().unwrap().push(json!({
                            "version":999,"description":"future","checksum_hex":"00".repeat(48),"success":true,
                        })),
                    );
                    NOT_A_PREFIX
                }
                "short-manifest" => {
                    fixture.fault(&[], |m| {
                        m["migrations"].as_array_mut().unwrap().pop();
                    });
                    "backup database migrations differ from its manifest"
                }
                "long-manifest" => {
                    let checksum = fixture.checksum(7);
                    fixture.fault(&[], |m| m["migrations"].as_array_mut().unwrap().push(json!({
                        "version":7,"description":"formal parent kind","checksum_hex":checksum,"success":true,
                    })));
                    "backup database migrations differ from its manifest"
                }
                "checksum" => {
                    let tamper =
                        "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1";
                    fixture.fault(&[tamper], |m| {
                        m["migrations"][0]["checksum_hex"] = json!("00".repeat(48))
                    });
                    "manifest migration checksums differ from current code"
                }
                "checksum-db" => {
                    let tamper =
                        "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1";
                    fixture.fault(&[tamper], |_| {});
                    "native restore refused: clock_regressed"
                }
                "unsuccessful" => {
                    let failed = "UPDATE _sqlx_migrations SET success=0 WHERE version=5";
                    fixture.fault(&[failed], |m| m["migrations"][3]["success"] = json!(false));
                    "migration is not successful"
                }
                "unsuccessful-db" => {
                    let failed = "UPDATE _sqlx_migrations SET success=0 WHERE version=5";
                    fixture.fault(&[failed], |_| {});
                    "native restore refused: clock_regressed"
                }
                "stale-anchor" => {
                    let original = fixture.anchor.clone();
                    fixture.anchor =
                        copy_database(&fixture.prefix, &fixture.base.root, "stale-live");
                    fixture.assert_refused(
                        "restored-stale-anchor",
                        "database migration checksums differ from current code",
                    );
                    fixture.anchor = original;
                    continue;
                }
                "conflict" => {
                    fixture.fault(&["CREATE TABLE objects_formal_parent(x)"], |_| {});
                    let refused = copy_database(
                        &fixture.backup.join("rsia.sqlite3"),
                        &fixture.base.root,
                        "conflict-store-open",
                    );
                    let before = snapshot(refused.parent().unwrap());
                    match Store::open_with_prepare_configs(
                        &refused,
                        vec![fixture.base.inputs.config.clone()],
                    )
                    .await
                    {
                        Err(evo_core::Error::ClockRegressed) => {}
                        Err(error) => panic!("unexpected refusal: {error:?}"),
                        Ok(store) => {
                            fixture.base.inputs.close(store, &refused).await;
                            panic!("Store::open applied 0007 over a conflicting table");
                        }
                    }
                    assert_eq!(snapshot(refused.parent().unwrap()), before);
                    "native restore refused: clock_regressed"
                }
                _ => unreachable!(),
            };
            fixture.assert_refused(&format!("restored-{name}"), reason);
        }
    }

    #[tokio::test]
    async fn each_unqualified_legacy_family_is_refused_without_another_bad_family_masking_it() {
        let mut fixture = PrefixFixture::new(&[1, 2, 4, 5, 6]).await;
        let original = fixture.backup.clone();
        for (name, sql) in [
            (
                "audit",
                "UPDATE audit SET payload=' { \"event\" : \"old\" } ',previous_hash='previous',digest='digest' WHERE seq=(SELECT MIN(seq) FROM audit)",
            ),
            (
                "world",
                "UPDATE replay_worlds SET manifest=' { \"sealed\" : true } ' WHERE id='world-select'",
            ),
            (
                "root",
                "UPDATE root_budgets SET authorization_receipt_digest='authorization' WHERE billing_scope='economic-scope'",
            ),
            (
                "group",
                "UPDATE root_budget_dispatch_groups SET owner_namespace='legacy-a' WHERE dispatch_group_id='group-fixed'",
            ),
            (
                "call",
                "UPDATE root_budget_calls SET actual_input_digest='input' WHERE call_id='fixed-call'",
            ),
            (
                "typed-ledger",
                "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('legacy-a','artifact','ledger-1','admin','{\"id\":\"ledger-1\",\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"record_kind\":\"exposure_ledger_v1\",\"payload\":{\"spent\":1}}')",
            ),
            (
                "evaluation",
                "INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES(970,'legacy-b','evaluation','eval-1','admin','{\"id\":\"eval-1\",\"score\":0.5}',7)",
            ),
        ] {
            fixture.select_backup_copy(&original, &format!("family-{name}-backup"));
            fixture
                .assert_success(&format!("healthy-before-{name}"), None)
                .await;
            fixture.fault(&[sql], |_| {});
            fixture.assert_refused(
                &format!("refused-{name}"),
                "native restore refused: clock_regressed",
            );
        }
        for kind in ["run", "event", "release", "receipt", "candidate"] {
            fixture.select_backup_copy(&original, &format!("bare-{kind}-backup"));
            fixture
                .assert_success(&format!("healthy-before-bare-{kind}"), None)
                .await;
            let id = if kind == "run" { "run-1" } else { "shared" };
            let sql = format!(
                "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('legacy-a','{kind}','{id}','owner-0',' {{ \"id\" : \"{id}\", \"kind\" : \"{kind}\", \"state\" : \"queued\" }} ',2)"
            );
            fixture.fault(&[&sql], |_| {});
            fixture.assert_refused(
                &format!("refused-bare-{kind}"),
                "native restore refused: clock_regressed",
            );
        }
        // feedback/pointer/dataset/budget/reservation/job/improvement and plain
        // artifact do not become authorized business evidence by parsing. Their
        // exact original seed bytes remain in the storage mechanical tests.
    }

    #[tokio::test]
    async fn cleanup_identity_faults_are_not_hidden_by_an_invalid_placeholder_job() {
        let mut fixture = PrefixFixture::new(&VERSIONS).await;
        fixture.capture_pending_current().await;
        let original = fixture.backup.clone();
        for (name, sql) in [
            (
                "job",
                "UPDATE revoke_cleanup_jobs SET watermark_digest='watermark'",
            ),
            (
                "frontier",
                "UPDATE revoke_cleanup_frontier SET job_id='cleanup' WHERE seq=(SELECT MIN(seq) FROM revoke_cleanup_frontier)",
            ),
            (
                "event",
                "UPDATE revoke_cleanup_events SET job_id='cleanup' WHERE seq=(SELECT MIN(seq) FROM revoke_cleanup_events)",
            ),
        ] {
            fixture.select_backup_copy(&original, &format!("cleanup-{name}-backup"));
            fixture
                .assert_success(&format!("healthy-before-cleanup-{name}"), None)
                .await;
            fixture.fault(&[sql], |_| {});
            fixture.assert_refused(
                &format!("refused-cleanup-{name}"),
                "native restore refused: clock_regressed",
            );
        }
        // Ordinary valid JSON details such as { "old" : true } are not claimed
        // invalid on their own. The originals stay mechanically preserved; no
        // current consumer, provider, paid capability or deletion proof is added.
    }
}
