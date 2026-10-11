//! AG-103 (E09/E10; plan §7.1.1, §7.4, §7.5): the replay moves its fairness waits only
//! over legal opportunities, the actions evo-core's shared filter (`legal_candidates`)
//! leaves legal in a completed decision round, the filter the decision ranks and online
//! exploration moves its waits over. An action that filter excluded in a round (no
//! authorized cost bound, past the depth cap, a Recover that is not eligible, a
//! successor of a parent that cannot have one) neither waits nor stays among the waits.
//!
//! What a replay selects, reveals and scores, how it ends and whether its support is
//! complete are as they were: the worlds below pin them value for value. A report stored
//! before the change on a world with an excluded action stays readable, but its
//! recomputation differs and is a conflict that never overwrites it; a report without
//! one stays verified and idempotent (the AG-064/AG-074 handling of old reports). Worlds
//! and sources are synthetic fixtures, not real model executions or evidence of formal
//! benefit.
//!
//! Not declared: two boundaries, recorded as the replay behaves, not claimed as a
//! contract.
//!
//! * An action a batch made available has met no decision round yet. It enters the waits
//!   at zero and stays there for one round, until a completed round judges it; when the
//!   replay ends first, it stays at zero in the final prefix. It never waits a round.
//! * An action that becomes ineligible only after the last completed round keeps the wait
//!   entry it had, as before this change: no later round judges it.
use evo_core::evidence::Purpose;
use evo_core::hash;
use evo_core::replay::*;
use evo_core::strategy::{
    ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind, ObservedStatus,
};
use evo_core::{Context, Error, Role, fingerprint};
use evo_engine::replay::{
    LiveWorldAuthority, run_and_persist_pool_replay, run_replay, verified_replay_report_view,
};
use evo_storage::Store;
use evo_storage::replay::{
    load_live_replay_report, put_replay_report, register_replay_pool, seal_replay_world,
};
use serde_json::json;
use std::collections::BTreeSet;

const COST: u64 = 10;
const STOP_REASON: &str = "no_legal_action_with_authorized_cost";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}
fn valid(quality_micros: u32) -> ObservedStatus {
    ObservedStatus::Valid { quality_micros }
}
/// A repairable failure whose environment was not reset: no Recover of it is eligible.
fn unreset_failure(episode: &str) -> ObservedStatus {
    ObservedStatus::RepairableFailure {
        episode_id: episode.into(),
        failure_kind: FailureKind::Compile,
        repair_template_digest: d("repair"),
        environment_reset: false,
        dispatched_repairs: 0,
    }
}
fn observed(status: ObservedStatus) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed { status }
}

#[derive(Clone)]
struct Step {
    seq: u32,
    branch: u32,
    depth: u8,
    kind: ActionKindV1,
    cost: Option<u64>,
    outcome: ReplayTransitionOutcome,
}
fn root(seq: u32, branch: u32, cost: Option<u64>, status: ObservedStatus) -> Step {
    Step {
        seq,
        branch,
        depth: 1,
        kind: ActionKindV1::Widen { root_slot: branch },
        cost,
        outcome: observed(status),
    }
}
fn deepen(seq: u32, parent: u32, branch: u32, depth: u8, status: ObservedStatus) -> Step {
    Step {
        seq,
        branch,
        depth,
        kind: ActionKindV1::Deepen {
            parent_node_seq: parent,
        },
        cost: Some(COST),
        outcome: observed(status),
    }
}
fn recover(seq: u32, failed: u32, branch: u32, depth: u8, status: ObservedStatus) -> Step {
    Step {
        seq,
        branch,
        depth,
        kind: ActionKindV1::Recover {
            failed_node_seq: failed,
            episode_id: "episode-1".into(),
        },
        cost: Some(COST),
        outcome: observed(status),
    }
}

/// Seals `world` over a source closure rebuilt from its manifest and transitions.
fn refresh(world: &mut ReplayWorldV2) {
    world.sealed_digest = None;
    world.manifest.source_closure = std::iter::once(ReplaySourceRef {
        source_id: world.manifest.baseline_observation_source_id.clone(),
        content_digest: replay_baseline_observation_digest(&world.manifest).unwrap(),
    })
    .chain(world.transitions.iter().map(|t| ReplaySourceRef {
        source_id: t.observation_source_id.clone(),
        content_digest: String::new(),
    }))
    .collect();
    let digests: Vec<_> = world
        .transitions
        .iter()
        .map(|t| {
            (
                t.observation_source_id.clone(),
                replay_observation_digest(&world.manifest, t).unwrap(),
            )
        })
        .collect();
    for (id, digest) in digests {
        world
            .manifest
            .source_closure
            .iter_mut()
            .find(|s| s.source_id == id)
            .unwrap()
            .content_digest = digest;
    }
    world.seal().unwrap();
}

/// A sealed world whose catalog holds `steps` (in this order) and whose transitions
/// reveal each step's outcome, from the baseline quality `baseline_micros`.
fn world(
    id: &str,
    partition: WorldPartition,
    baseline_micros: u32,
    steps: &[Step],
) -> ReplayWorldV2 {
    let context = |seq| d(&format!("ctx-{seq}"));
    let mut actions = Vec::new();
    let mut transitions = Vec::new();
    for step in steps {
        let parent = match step.kind {
            ActionKindV1::Widen { .. } => d("baseline"),
            ActionKindV1::Deepen { parent_node_seq } => context(parent_node_seq),
            ActionKindV1::Recover {
                failed_node_seq, ..
            } => context(failed_node_seq),
        };
        actions.push(ReplayActionSpecV1 {
            record_seq: step.seq,
            generation_signature: d("generation"),
            parent_context_signature: parent.clone(),
            branch_seq: step.branch,
            target_depth: step.depth,
            action_kind: step.kind.clone(),
            estimated_cost_upper_micros: step.cost,
            writes_shared_workspace: false,
        });
        let source = format!("{id}-source-{}", step.seq);
        transitions.push(ReplayTransitionV2 {
            record_id: format!("opaque-{}", step.seq),
            record_seq: step.seq,
            generation_signature: d("generation"),
            parent_context_signature: parent,
            action_kind: step.kind.clone(),
            next_context_signature: context(step.seq),
            outcome: step.outcome.clone(),
            actual_usage: HistoricalUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: Some(u64::from(step.seq)),
                latency_millis: Some(1),
            },
            source_ids: vec![source.clone()],
            observation_source_id: source,
        });
    }
    let contexts: BTreeSet<_> = std::iter::once(d("baseline"))
        .chain(steps.iter().map(|s| context(s.seq)))
        .collect();
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: id.into(),
            cluster_id: format!("cluster-{id}"),
            partition,
            purpose: Purpose::Development,
            generation_signature: d("generation"),
            world_context_signature: d("world-context"),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("approved"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: baseline_micros,
            baseline_observation_source_id: format!("{id}-baseline"),
            source_closure: Vec::new(),
            revoke_watermark: 1,
            prefix_coverage: contexts
                .into_iter()
                .map(|context_signature| PrefixCoverageV1 {
                    context_signature,
                    exhausted: true,
                })
                .collect(),
            action_catalog: actions,
        },
        transitions,
        sealed_digest: None,
    };
    refresh(&mut world);
    world
}

/// The unknown-cost counterexample: root 1 (cost 10, quality 400000) opens a chain of
/// deepenings 3 → 4 → 5, and root 2 reveals the best quality of the world (900000) but
/// has no authorized cost bound, so it is never a legal opportunity.
fn unknown_cost_steps() -> Vec<Step> {
    vec![
        root(1, 1, Some(COST), valid(400_000)),
        root(2, 2, None, valid(900_000)),
        deepen(3, 1, 1, 2, valid(800_000)),
        deepen(4, 3, 1, 3, valid(850_000)),
        deepen(5, 4, 1, 4, valid(900_000)),
    ]
}

/// Root 1 fails without its environment reset, so its Recover 2 is not eligible and its
/// deepening 5 has no valid parent (both reveal better qualities than anything legal);
/// root 3 opens the chain 4 → 6.
fn excluded_successor_steps() -> Vec<Step> {
    vec![
        root(1, 1, Some(COST), unreset_failure("episode-1")),
        recover(2, 1, 1, 2, valid(950_000)),
        root(3, 2, Some(COST), valid(600_000)),
        deepen(4, 3, 2, 2, valid(700_000)),
        deepen(5, 1, 1, 2, valid(990_000)),
        deepen(6, 4, 2, 3, valid(800_000)),
    ]
}

fn profile(probes: u8) -> ReplaySimulationProfile {
    let mut p = ReplaySimulationProfile::default_v2(1, d("pool"));
    p.probe_budget = probes;
    p.horizon = probes;
    p.fixed_seed = 17;
    p
}
fn caps() -> ExplorationCapsV1 {
    ExplorationCapsV1::online()
}
fn run(w: &ReplayWorldV2, p: &ReplaySimulationProfile, caps: &ExplorationCapsV1) -> ReplayReportV2 {
    w.validate_sealed().unwrap();
    run_replay(
        w,
        &ElasticPolicyV1::default(),
        p,
        caps,
        &LiveWorldAuthority {
            revoke_watermark: 1,
            revoked_source_ids: BTreeSet::new(),
        },
    )
    .unwrap()
}
fn selected(report: &ReplayReportV2) -> Vec<u32> {
    report
        .batches
        .iter()
        .flat_map(|b| b.action_seqs.clone())
        .collect()
}
/// The waits a report's revealed prefix ends with, as `(action_seq, waited_rounds)`.
fn waits(report: &ReplayReportV2) -> Vec<(u32, u32)> {
    report
        .revealed_prefix
        .as_ref()
        .unwrap()
        .waits
        .iter()
        .map(|wait| (wait.action_seq, wait.waited_rounds))
        .collect()
}

#[test]
fn the_unknown_cost_world_selects_ends_and_scores_as_before() {
    let w = world(
        "ag103-unknown-cost",
        WorldPartition::Select,
        100_000,
        &unknown_cost_steps(),
    );
    // A probe budget of one to four rounds ends the replay after that round.
    for probes in 1..=4u8 {
        let report = run(&w, &profile(probes), &caps());
        assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted, "{probes}");
        assert_eq!(
            selected(&report),
            [1, 3, 4, 5][..usize::from(probes)].to_vec(),
            "{probes}"
        );
        assert!(report.coverage.complete_support, "{probes}");
    }
    // With a fifth probe nothing legal is left: a policy stop that keeps its quality.
    let report = run(&w, &profile(5), &caps());
    assert_eq!(selected(&report), vec![1, 3, 4, 5]);
    assert_eq!(report.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(report.terminal_reason, STOP_REASON);
    assert!(report.coverage.complete_support);
    let curve = vec![400_000, 800_000, 850_000, 900_000, 900_000];
    assert_eq!(report.probe_best_quality_micros, curve);
    assert_eq!(report.round_best_quality_micros, curve);
    assert_eq!((report.probes, report.simulated_rounds), (4, 4));
    assert_eq!(report.final_quality_micros, 900_000);
    assert_eq!(report.attainment_auc_micros, Some(770_000));
    assert_eq!(report.q_auc_sim_micros, Some(770_000));
    assert_eq!(report.score_v2_micros, Some(690_000));
}

#[test]
fn an_unknown_cost_root_never_waits_while_the_chain_it_cannot_join_is_selected() {
    let w = world(
        "ag103-unknown-cost",
        WorldPartition::Select,
        100_000,
        &unknown_cost_steps(),
    );
    // After each round in turn, and at the policy stop: root 2 is never among the waits
    // (it used to wait 1, 2, 3 and 4 rounds), and the deepening a round made available
    // enters at zero.
    let after: [&[(u32, u32)]; 5] = [&[(3, 0)], &[(4, 0)], &[(5, 0)], &[], &[]];
    for (probes, expected) in (1..=5u8).zip(after) {
        let report = run(&w, &profile(probes), &caps());
        assert_eq!(waits(&report), expected, "probe budget {probes}");
        report.revealed_prefix.unwrap().validate().unwrap();
    }
}

#[test]
fn a_depth_capped_world_still_stops_by_policy_and_keeps_its_quality() {
    let w = world(
        "ag103-depth-cap",
        WorldPartition::Select,
        100_000,
        &[
            root(1, 1, Some(COST), valid(400_000)),
            deepen(2, 1, 1, 2, valid(800_000)),
        ],
    );
    let mut shallow = caps();
    shallow.max_depth = 1;
    let report = run(&w, &profile(4), &shallow);
    assert_eq!(selected(&report), vec![1]);
    assert_eq!(report.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(report.terminal_reason, STOP_REASON);
    assert!(report.coverage.complete_support);
    assert_eq!(report.probe_best_quality_micros, vec![400_000; 4]);
    assert_eq!(report.round_best_quality_micros, vec![400_000; 4]);
    assert_eq!((report.probes, report.simulated_rounds), (1, 1));
    assert_eq!(report.final_quality_micros, 400_000);
    assert_eq!(report.attainment_auc_micros, Some(400_000));
    assert_eq!(report.q_auc_sim_micros, Some(400_000));
    assert_eq!(report.score_v2_micros, Some(375_000));
    // The deepening past the cap never waited a round. The policy stop leaves it at zero
    // in the final prefix (the first boundary above, not declared).
    assert!(
        report
            .revealed_prefix
            .unwrap()
            .waits
            .iter()
            .all(|wait| wait.waited_rounds == 0)
    );
}

#[test]
fn the_excluded_successor_world_selects_ends_and_scores_as_before() {
    let w = world(
        "ag103-excluded-successors",
        WorldPartition::Select,
        500_000,
        &excluded_successor_steps(),
    );
    for probes in 1..=4u8 {
        let report = run(&w, &profile(probes), &caps());
        assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted, "{probes}");
        assert_eq!(
            selected(&report),
            [1, 3, 4, 6][..usize::from(probes)].to_vec(),
            "{probes}"
        );
    }
    let report = run(&w, &profile(5), &caps());
    assert_eq!(selected(&report), vec![1, 3, 4, 6]);
    assert_eq!(report.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(report.terminal_reason, STOP_REASON);
    assert!(report.coverage.complete_support);
    assert_eq!(
        report.probe_best_quality_micros,
        vec![500_000, 600_000, 700_000, 800_000, 800_000]
    );
    assert_eq!(report.final_quality_micros, 800_000);
    assert_eq!(report.attainment_auc_micros, Some(680_000));
    assert_eq!(report.score_v2_micros, Some(600_000));
}

#[test]
fn an_ineligible_recover_and_a_successor_of_a_failed_parent_never_wait() {
    let w = world(
        "ag103-excluded-successors",
        WorldPartition::Select,
        500_000,
        &excluded_successor_steps(),
    );
    // Round 1 reveals the failure and so makes its Recover 2 and its deepening 5
    // available: no round has judged them yet, so they enter at zero (the first boundary
    // above, not declared), and root 3, a legal opportunity of round 1 that was not
    // chosen, waits one round. Round 2 excludes 2 and 5 (an environment that was not
    // reset, a parent that is no valid node): from then on neither waits nor stays among
    // the waits.
    let after: [&[(u32, u32)]; 5] = [&[(2, 0), (3, 1), (5, 0)], &[(4, 0)], &[(6, 0)], &[], &[]];
    for (probes, expected) in (1..=5u8).zip(after) {
        let report = run(&w, &profile(probes), &caps());
        assert_eq!(waits(&report), expected, "probe budget {probes}");
        report.revealed_prefix.unwrap().validate().unwrap();
    }
}

#[test]
fn a_legal_opportunity_left_at_the_end_of_the_probe_budget_keeps_its_wait() {
    // The node budget ends the replay; it does not judge the opportunity left open.
    let report = run(
        &world(
            "ag103-leftover",
            WorldPartition::Select,
            500_000,
            &leftover_steps(),
        ),
        &profile(2),
        &caps(),
    );
    assert_eq!(selected(&report), vec![1, 3]);
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(waits(&report), vec![(2, 2)]);
}

/// Two roots and a deepening of the first: nothing is ever excluded. With two probes the
/// replay takes root 1 and its deepening 3, and root 2 is left waiting.
fn leftover_steps() -> Vec<Step> {
    vec![
        root(1, 1, Some(COST), valid(800_000)),
        root(2, 2, Some(COST), valid(700_000)),
        deepen(3, 1, 1, 2, valid(900_000)),
    ]
}

/// The select world of each stored report and its probe budget.
fn legacy(kind: &str) -> (u32, Vec<Step>, u8) {
    match kind {
        "affected" => (100_000, unknown_cost_steps(), 5),
        "unaffected" => (500_000, leftover_steps(), 2),
        _ => panic!("unknown test fixture"),
    }
}

struct Persistent {
    _dir: tempfile::TempDir,
    store: Store,
    worker: Context,
    evaluator: Context,
    worlds: Vec<ReplayWorldV2>,
    pool: ReplayPoolManifestV1,
    p: ReplaySimulationProfile,
}
impl Persistent {
    async fn new(kind: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("replay.sqlite3"))
            .await
            .unwrap();
        let worker = Context::new("ag103", "worker", Role::Worker).unwrap();
        let evaluator = Context::new("ag103", "evaluator", Role::Evaluator).unwrap();
        let (baseline_micros, steps, probes) = legacy(kind);
        let worlds = vec![
            world(
                &format!("ag103-{kind}-train"),
                WorldPartition::Train,
                500_000,
                &[root(1, 1, Some(COST), valid(800_000))],
            ),
            world(
                &format!("ag103-{kind}-select"),
                WorldPartition::Select,
                baseline_micros,
                &steps,
            ),
        ];
        let mut session = store.session().await.unwrap();
        for w in &worlds {
            for source in &w.manifest.source_closure {
                let body = if source.source_id == w.manifest.baseline_observation_source_id {
                    replay_baseline_observation_bytes(&w.manifest).unwrap()
                } else {
                    replay_observation_bytes(
                        &w.manifest,
                        w.transitions
                            .iter()
                            .find(|t| t.observation_source_id == source.source_id)
                            .unwrap(),
                    )
                    .unwrap()
                };
                let excerpt = String::from_utf8(body.clone()).unwrap();
                session.put(&worker, "run", &source.source_id, "host", &json!({
                    "schema_version":"rsia.optimization.source.v1",
                    "record":{"id":source.source_id,"body":body,"parent_family":format!("family-{}",source.source_id),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
                    "trace":{"run_id":source.source_id,"parent_family":format!("family-{}",source.source_id),"source_digest":source.content_digest,"purpose":"development","outcome":"success","diagnosis":null,"excerpt":excerpt,"seed":1},
                    "excerpt_start":0,"excerpt_end":excerpt.len()
                })).await.unwrap();
            }
        }
        session
            .bump_watermark(&worker, &d("watermark"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        for w in &worlds {
            seal_replay_world(&worker, &store, w).await.unwrap();
        }
        let pool = register_replay_pool(
            &worker,
            &store,
            &worlds
                .iter()
                .map(|w| w.manifest.world_id.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let mut p = profile(probes);
        p.pool_digest = pool.pool_digest.clone();
        Self {
            _dir: dir,
            store,
            worker,
            evaluator,
            worlds,
            pool,
            p,
        }
    }
    async fn replay(&self) -> evo_core::Result<StoredReplayReportV1> {
        run_and_persist_pool_replay(
            &self.worker,
            &self.store,
            &self.pool.pool_digest,
            WorldPartition::Select,
            &ElasticPolicyV1::default(),
            &self.p,
            &caps(),
        )
        .await
    }
}

// Optional explicit baseline capture. The implementation call is unchanged and files are
// created once, so a later rerun cannot replace frozen old evidence.
#[tokio::test]
async fn baseline_generates_unmodified_reports_through_the_persistent_engine() {
    let Some(directory) = std::env::var_os("RSIA_AG103_LEGACY_REPORT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    for kind in ["affected", "unaffected"] {
        let env = Persistent::new(kind).await;
        let report = env.replay().await.unwrap();
        report.validate_static(&env.pool).unwrap();
        let view = verified_replay_report_view(&env.evaluator, &env.store, &report.report_id)
            .await
            .unwrap();
        assert!(view.development_only);
        assert!(!view.promotion_eligible);
        for (suffix, bytes) in [
            ("worlds", serde_json::to_vec(&env.worlds).unwrap()),
            ("report", serde_json::to_vec(&report).unwrap()),
        ] {
            use std::io::Write;
            let path = directory.join(format!("{kind}-legacy-{suffix}.json"));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
            println!(
                "actual baseline capture {} sha256={}",
                path.display(),
                hash(&bytes)
            );
        }
    }
}

// Compare the API's serialized immutable artifact payload, including the full old
// report. This does not assert byte identity of the SQLite database file.
async fn stored_payload_bytes(env: &Persistent, id: &str) -> Vec<u8> {
    let mut session = env.store.session().await.unwrap();
    let value: serde_json::Value = session.need(&env.worker, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    serde_json::to_vec(&value).unwrap()
}

fn member(report: &StoredReplayReportV1) -> &ReplayReportV2 {
    &report.member_reports[0].report
}

#[tokio::test]
async fn the_affected_old_report_stays_readable_but_recomputation_cannot_overwrite_it() {
    assert_eq!(hash(LEGACY_AFFECTED.as_bytes()), LEGACY_AFFECTED_SHA256);
    let env = Persistent::new("affected").await;
    assert_eq!(
        hash(&serde_json::to_vec(&env.worlds).unwrap()),
        LEGACY_AFFECTED_WORLDS_SHA256
    );
    let old: StoredReplayReportV1 = serde_json::from_str(LEGACY_AFFECTED).unwrap();
    old.validate_static(&env.pool).unwrap();
    put_replay_report(&env.worker, &env.store, &old)
        .await
        .unwrap();
    let before = stored_payload_bytes(&env, &old.report_id).await;
    let loaded = load_live_replay_report(&env.evaluator, &env.store, &old.report_id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), LEGACY_AFFECTED);
    for result in [
        verified_replay_report_view(&env.evaluator, &env.store, &old.report_id)
            .await
            .map(|_| ()),
        env.replay().await.map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(Error::Conflict(ref reason)) if reason == "stored replay report differs from semantic recomputation"),
            "{result:?}"
        );
        assert_eq!(stored_payload_bytes(&env, &old.report_id).await, before);
    }

    let fresh = Persistent::new("affected").await;
    let new = fresh.replay().await.unwrap();
    assert_eq!(old.report_id, new.report_id);
    assert_ne!(old.semantic_reports_digest, new.semantic_reports_digest);
    // Only the waits differ: what was selected and revealed, the curves, the terminal,
    // the support and the score are the same.
    let (old_member, new_member) = (member(&old), member(&new));
    assert_eq!(old_member.batches, new_member.batches);
    assert_eq!(selected(new_member), vec![1, 3, 4, 5]);
    assert_eq!(
        old_member.probe_best_quality_micros,
        new_member.probe_best_quality_micros
    );
    assert_eq!(
        old_member.round_best_quality_micros,
        new_member.round_best_quality_micros
    );
    assert_eq!(old_member.final_quality_micros, new_member.final_quality_micros);
    assert_eq!(old_member.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(new_member.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(old_member.terminal_reason, new_member.terminal_reason);
    assert_eq!(old_member.coverage, new_member.coverage);
    assert!(new_member.coverage.complete_support);
    assert_eq!(old_member.attainment_auc_micros, new_member.attainment_auc_micros);
    assert_eq!(old_member.score_v2_micros, Some(690_000));
    assert_eq!(new_member.score_v2_micros, Some(690_000));
    assert_eq!(waits(old_member), vec![(2, 4)]);
    assert_eq!(waits(new_member), Vec::<(u32, u32)>::new());
    let mut old_prefix = old_member.revealed_prefix.clone().unwrap();
    old_prefix.waits.clear();
    assert_eq!(
        fingerprint(&old_prefix).unwrap(),
        fingerprint(new_member.revealed_prefix.as_ref().unwrap()).unwrap()
    );
    let view = verified_replay_report_view(&fresh.evaluator, &fresh.store, &new.report_id)
        .await
        .unwrap();
    assert!(view.development_only);
    assert!(!view.promotion_eligible);

    assert!(
        matches!(put_replay_report(&env.worker, &env.store, &new).await, Err(Error::Conflict(ref reason)) if reason == "immutable replay report differs")
    );
    assert_eq!(stored_payload_bytes(&env, &old.report_id).await, before);
    let loaded = load_live_replay_report(&env.evaluator, &env.store, &old.report_id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), LEGACY_AFFECTED);
}

#[tokio::test]
async fn the_old_report_without_an_excluded_action_stays_verified_and_idempotent() {
    assert_eq!(hash(LEGACY_UNAFFECTED.as_bytes()), LEGACY_UNAFFECTED_SHA256);
    let env = Persistent::new("unaffected").await;
    assert_eq!(
        hash(&serde_json::to_vec(&env.worlds).unwrap()),
        LEGACY_UNAFFECTED_WORLDS_SHA256
    );
    let old: StoredReplayReportV1 = serde_json::from_str(LEGACY_UNAFFECTED).unwrap();
    old.validate_static(&env.pool).unwrap();
    put_replay_report(&env.worker, &env.store, &old)
        .await
        .unwrap();
    let before = stored_payload_bytes(&env, &old.report_id).await;
    let loaded = load_live_replay_report(&env.evaluator, &env.store, &old.report_id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), LEGACY_UNAFFECTED);
    let view = verified_replay_report_view(&env.evaluator, &env.store, &old.report_id)
        .await
        .unwrap();
    assert_eq!(view.semantic_digest, old.semantic_reports_digest);
    assert!(view.development_only);
    assert!(!view.promotion_eligible);
    assert_eq!(
        fingerprint(&env.replay().await.unwrap()).unwrap(),
        fingerprint(&old).unwrap()
    );
    assert_eq!(stored_payload_bytes(&env, &old.report_id).await, before);
    // Its last legal opportunity still waits at the end of the probe budget.
    assert_eq!(member(&old).terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(waits(member(&old)), vec![(2, 2)]);
}

// Actual stored reports generated before the product edit, at baseline
// 98776f1717b37a47e4a7fcc4349ff0faff68ce1b, through the public persistent engine
// (`baseline_generates_unmodified_reports_through_the_persistent_engine`). CPU
// measurements are retained. The worlds and sources are synthetic fixtures.
const LEGACY_AFFECTED: &str = r##"PENDING_BASELINE_CAPTURE"##;
const LEGACY_AFFECTED_SHA256: &str = "PENDING_BASELINE_CAPTURE";
const LEGACY_AFFECTED_WORLDS_SHA256: &str = "PENDING_BASELINE_CAPTURE";
const LEGACY_UNAFFECTED: &str = r##"PENDING_BASELINE_CAPTURE"##;
const LEGACY_UNAFFECTED_SHA256: &str = "PENDING_BASELINE_CAPTURE";
const LEGACY_UNAFFECTED_WORLDS_SHA256: &str = "PENDING_BASELINE_CAPTURE";
