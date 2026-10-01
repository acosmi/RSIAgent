//! Lookup-only recovery facts; fixtures do not establish a production repair capability.
use evo_core::evidence::Purpose;
use evo_core::replay::*;
use evo_core::strategy::{
    ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind, ObservedStatus,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::replay::{
    LiveWorldAuthority, replay_v2_is_not_formal, run_and_persist_pool_replay, run_replay,
    verified_replay_report_view,
};
use evo_storage::Store;
use evo_storage::replay::{
    load_live_replay_report, put_replay_report, register_replay_pool, seal_replay_world,
};
use serde_json::json;
use std::collections::BTreeSet;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn failure(episode: &str, counter: u8) -> ObservedStatus {
    ObservedStatus::RepairableFailure {
        episode_id: episode.into(),
        failure_kind: FailureKind::Compile,
        repair_template_digest: d("repair"),
        environment_reset: true,
        dispatched_repairs: counter,
    }
}
fn valid() -> ObservedStatus {
    ObservedStatus::Valid {
        quality_micros: 800_000,
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
    outcome: ReplayTransitionOutcome,
    cost: Option<u64>,
}
fn root(seq: u32, branch: u32, status: ObservedStatus) -> Step {
    Step {
        seq,
        branch,
        depth: 1,
        kind: ActionKindV1::Widen { root_slot: branch },
        outcome: observed(status),
        cost: Some(20),
    }
}
fn recover(
    seq: u32,
    branch: u32,
    parent: u32,
    episode: &str,
    depth: u8,
    status: ObservedStatus,
) -> Step {
    Step {
        seq,
        branch,
        depth,
        kind: ActionKindV1::Recover {
            failed_node_seq: parent,
            episode_id: episode.into(),
        },
        outcome: observed(status),
        cost: Some(10),
    }
}

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

fn world(id: &str, partition: WorldPartition, steps: &[Step]) -> ReplayWorldV2 {
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
            initial_baseline_quality_micros: 500_000,
            baseline_observation_source_id: format!("{id}-baseline"),
            source_closure: vec![],
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

fn profile(width: u8, global: u8, probes: u8) -> ReplaySimulationProfile {
    let mut p = ReplaySimulationProfile::default_v2(width, d("pool"));
    p.global_recovery_dispatch_limit = global;
    p.probe_budget = probes;
    p.horizon = probes;
    p.fixed_seed = 17;
    p
}
fn run_with(
    world: &ReplayWorldV2,
    p: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> evo_core::Result<ReplayReportV2> {
    run_replay(
        world,
        &ElasticPolicyV1::default(),
        p,
        caps,
        &LiveWorldAuthority {
            revoke_watermark: 1,
            revoked_source_ids: BTreeSet::new(),
        },
    )
}
fn run(steps: &[Step], width: u8, global: u8) -> ReplayReportV2 {
    run_with(
        &world("select", WorldPartition::Select, steps),
        &profile(width, global, 12),
        &ExplorationCapsV1::online(),
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
fn counter(report: &ReplayReportV2, seq: u32) -> u8 {
    match &report
        .revealed_prefix
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.node_seq == seq)
        .unwrap()
        .status
    {
        ObservedStatus::RepairableFailure {
            dispatched_repairs, ..
        } => *dispatched_repairs,
        other => panic!("expected failure, got {other:?}"),
    }
}
fn cross_roots(second_episode: &str, counter: u8) -> Vec<Step> {
    vec![
        root(1, 1, failure("same", counter)),
        recover(2, 1, 1, "same", 2, valid()),
        root(3, 2, failure(second_episode, counter)),
        recover(4, 2, 3, second_episode, 2, valid()),
    ]
}

#[test]
fn later_root_of_an_episode_cannot_reopen_its_used_recovery() {
    let report = run(&cross_roots("same", 0), 1, 2);
    assert_eq!(selected(&report), vec![1, 2, 3]);
    assert_eq!((counter(&report, 1), counter(&report, 3)), (1, 1));
    assert_eq!(
        report
            .revealed_prefix
            .as_ref()
            .unwrap()
            .recovery_dispatches_used,
        1
    );
    assert_eq!(report.coverage.observed_actions, 3);
    assert_eq!(report.historical_usage.cost_micros, Some(6));
    let independent = run(&cross_roots("other", 0), 1, 2);
    assert_eq!(selected(&independent), vec![1, 2, 3, 4]);
    assert_eq!(independent.historical_usage.cost_micros, Some(10));
    assert_eq!(
        independent
            .revealed_prefix
            .unwrap()
            .recovery_dispatches_used,
        2
    );
}

#[test]
fn failed_repair_child_cannot_retry_under_the_same_or_a_new_episode() {
    for episode in ["same", "renamed"] {
        for next in [valid(), failure("third", 0)] {
            let steps = vec![
                root(1, 1, failure("same", 0)),
                recover(2, 1, 1, "same", 2, failure(episode, 0)),
                recover(3, 1, 2, episode, 3, next),
                root(4, 2, valid()),
            ];
            let w = world("lineage", WorldPartition::Select, &steps);
            let result = run_with(&w, &profile(1, 2, 12), &ExplorationCapsV1::online());
            assert!(
                result.is_ok(),
                "must stop before a second failed repair corrupts the prefix: {result:?}"
            );
            let report = result.unwrap();
            assert_eq!(selected(&report), vec![1, 2, 4]);
            assert_eq!(counter(&report, 2), u8::from(episode == "same"));
            assert_eq!(
                report.revealed_prefix.as_ref().unwrap().nodes[1].repair_failures_dispatched,
                1
            );
            assert_eq!(
                report
                    .revealed_prefix
                    .as_ref()
                    .unwrap()
                    .recovery_dispatches_used,
                1
            );
            assert_eq!(report.historical_usage.cost_micros, Some(7));
        }
    }
}

#[test]
fn historical_counters_do_not_replace_revealed_dispatch_facts() {
    for supplied in [0, 1, 255] {
        let steps = vec![
            root(1, 1, failure("same", supplied)),
            recover(2, 1, 1, "same", 2, failure("same", supplied)),
            root(3, 2, failure("same", supplied)),
        ];
        let w = world("counter", WorldPartition::Select, &steps);
        w.validate_sealed().unwrap();
        let original = serde_json::to_vec(&w).unwrap();
        let report = run_with(&w, &profile(1, 2, 12), &ExplorationCapsV1::online()).unwrap();
        assert_eq!(selected(&report), vec![1, 2, 3], "supplied {supplied}");
        for seq in 1..=3 {
            assert_eq!(counter(&report, seq), 1);
        }
        assert_eq!(report.revealed_prefix.unwrap().recovery_dispatches_used, 1);
        assert_eq!(serde_json::to_vec(&w).unwrap(), original);
        let unused = run(&[root(1, 1, failure("same", supplied))], 1, 0);
        assert_eq!(counter(&unused, 1), 0);
    }
}

#[test]
fn wide_replay_deduplicates_an_episode_before_the_batch_reveal() {
    let steps = vec![
        root(1, 1, failure("same", 0)),
        root(2, 2, failure("same", 0)),
        recover(3, 1, 1, "same", 2, valid()),
        recover(4, 2, 2, "same", 2, valid()),
        root(5, 3, failure("other", 0)),
        recover(6, 3, 5, "other", 2, valid()),
        root(7, 4, valid()),
        Step {
            seq: 8,
            branch: 4,
            depth: 2,
            kind: ActionKindV1::Deepen { parent_node_seq: 7 },
            outcome: observed(valid()),
            cost: Some(10),
        },
    ];
    for width in [1, 2, 4] {
        let report = run(&steps, width, 2);
        assert!(selected(&report).contains(&3));
        assert!(
            !selected(&report).contains(&4),
            "width {width}: {:?}",
            selected(&report)
        );
        assert!(selected(&report).contains(&6));
        assert!(selected(&report).contains(&8));
        assert_eq!((counter(&report, 1), counter(&report, 2)), (1, 1));
        assert_eq!(
            report
                .revealed_prefix
                .as_ref()
                .unwrap()
                .recovery_dispatches_used,
            2
        );
        assert_eq!(report.historical_usage.cost_micros, Some(32));
        assert_eq!(report.coverage.observed_actions, 7);
        if width > 1 {
            assert_eq!(&report.batches[0].action_seqs[..2], &[1, 2]);
        }
        for batch in &report.batches {
            assert!(batch.action_seqs.len() <= usize::from(width));
            assert!(!(batch.action_seqs.contains(&3) && batch.action_seqs.contains(&4)));
            if batch.action_seqs.contains(&3) {
                assert!(!batch.action_seqs.contains(&1));
            }
        }
    }
}

#[test]
fn observed_cancel_and_uncertain_consume_but_lookup_terminals_do_not() {
    for status in [ObservedStatus::Cancelled, ObservedStatus::UsageUncertain] {
        let report = run(
            &[
                root(1, 1, failure("same", 0)),
                recover(2, 1, 1, "same", 2, status.clone()),
            ],
            1,
            2,
        );
        assert_eq!(selected(&report), vec![1, 2]);
        assert_eq!(counter(&report, 1), 1);
        assert_eq!(report.revealed_prefix.unwrap().recovery_dispatches_used, 1);
        assert_eq!(
            report.terminal,
            if matches!(status, ObservedStatus::UsageUncertain) {
                ReplayTerminal::Censored
            } else {
                ReplayTerminal::WorldExhausted
            }
        );
    }
    for mode in 0..3 {
        let mut steps = vec![
            root(1, 1, failure("same", 0)),
            recover(2, 1, 1, "same", 2, valid()),
        ];
        steps[1].outcome = if mode == 1 {
            ReplayTransitionOutcome::Censored {
                reason: "historical truncation".into(),
            }
        } else {
            ReplayTransitionOutcome::OutOfSupport {
                reason: "unsupported transition".into(),
            }
        };
        let mut w = world("terminal", WorldPartition::Select, &steps);
        if mode == 0 {
            w.transitions.pop();
            refresh(&mut w);
        }
        let report = run_with(&w, &profile(1, 2, 12), &ExplorationCapsV1::online()).unwrap();
        assert_eq!(selected(&report), vec![1, 2]);
        assert_eq!(counter(&report, 1), 0);
        assert_eq!(report.revealed_prefix.unwrap().recovery_dispatches_used, 0);
        assert_eq!(report.coverage.observed_actions, 1);
        assert_eq!(
            report.terminal,
            if mode == 1 {
                ReplayTerminal::Censored
            } else {
                ReplayTerminal::OutOfSupport
            }
        );
    }
}

#[test]
fn successful_recovery_keeps_a_normal_deepen_legal() {
    let steps = vec![
        root(1, 1, failure("same", 0)),
        recover(2, 1, 1, "same", 2, valid()),
        Step {
            seq: 3,
            branch: 1,
            depth: 3,
            kind: ActionKindV1::Deepen { parent_node_seq: 2 },
            outcome: observed(valid()),
            cost: Some(10),
        },
    ];
    let report = run(&steps, 1, 2);
    assert_eq!(selected(&report), vec![1, 2, 3]);
    assert_eq!(
        report.revealed_prefix.as_ref().unwrap().nodes[2].repair_failures_dispatched,
        0
    );
    assert_eq!(
        report
            .revealed_prefix
            .as_ref()
            .unwrap()
            .recovery_dispatches_used,
        1
    );
    assert!(replay_v2_is_not_formal(&report).is_err());
}

#[test]
fn recovery_still_respects_frozen_caps_and_unknown_cost() {
    for mode in 0..6 {
        let mut steps = cross_roots("other", 0);
        let mut caps = ExplorationCapsV1::online();
        let mut p = profile(1, 2, 12);
        match mode {
            0 => p.global_recovery_dispatch_limit = 0,
            1 => p.global_recovery_dispatch_limit = 1,
            2 => caps.max_repair_dispatches_per_episode = 0,
            3 => {
                caps.max_depth = 1;
            }
            4 => {
                steps[1].cost = None;
                steps[3].cost = None;
            }
            _ => {
                for step in steps.iter_mut().step_by(2) {
                    if let ReplayTransitionOutcome::Observed {
                        status:
                            ObservedStatus::RepairableFailure {
                                environment_reset, ..
                            },
                    } = &mut step.outcome
                    {
                        *environment_reset = false;
                    }
                }
            }
        }
        let report = run_with(
            &world("negative", WorldPartition::Select, &steps),
            &p,
            &caps,
        )
        .unwrap();
        assert!(!selected(&report).contains(&4), "mode {mode}");
        if mode != 1 {
            assert!(!selected(&report).contains(&2), "mode {mode}");
        } else {
            assert!(selected(&report).contains(&2));
        }
    }
    let mut steps = cross_roots("other", 0);
    if let ReplayTransitionOutcome::Observed {
        status: ObservedStatus::RepairableFailure { failure_kind, .. },
    } = &mut steps[0].outcome
    {
        *failure_kind = FailureKind::Safety;
    }
    assert!(matches!(
        run_with(
            &world("invalid-kind", WorldPartition::Select, &steps),
            &profile(1, 2, 12),
            &ExplorationCapsV1::online()
        ),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn future_quality_and_opaque_names_do_not_change_earlier_batches() {
    let steps = cross_roots("other", 0);
    let w = world("deterministic", WorldPartition::Select, &steps);
    let p = profile(1, 2, 12);
    let caps = ExplorationCapsV1::online();
    let first = run_with(&w, &p, &caps).unwrap();
    let second = run_with(&w, &p, &caps).unwrap();
    let strip_cpu = |report: ReplayReportV2| {
        let mut value = serde_json::to_value(report).unwrap();
        value.as_object_mut().unwrap().remove("replay_cpu_nanos");
        value
    };
    assert_eq!(strip_cpu(first.clone()), strip_cpu(second));
    let mut changed = w.clone();
    changed.transitions[3].outcome = observed(ObservedStatus::Valid { quality_micros: 1 });
    for t in &mut changed.transitions {
        t.record_id = format!("renamed-{}", 100 - t.record_seq);
    }
    refresh(&mut changed);
    let other = run_with(&changed, &p, &caps).unwrap();
    assert_eq!(
        first
            .batches
            .iter()
            .map(|b| &b.action_seqs)
            .collect::<Vec<_>>(),
        other
            .batches
            .iter()
            .map(|b| &b.action_seqs)
            .collect::<Vec<_>>()
    );
}

fn legacy_steps(kind: &str) -> Vec<Step> {
    let status = match kind {
        "affected" => failure("same", 0),
        "valid" => valid(),
        "hard" => ObservedStatus::HardFailure,
        _ => panic!("unknown fixture"),
    };
    vec![
        root(1, 1, failure("same", 0)),
        recover(2, 1, 1, "same", 2, status),
    ]
}
struct Persistent {
    _dir: tempfile::TempDir,
    store: Store,
    worker: Context,
    evaluator: Context,
    pool: ReplayPoolManifestV1,
    p: ReplaySimulationProfile,
}
impl Persistent {
    async fn new(kind: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("replay.sqlite3"))
            .await
            .unwrap();
        let worker = Context::new("n", "worker", Role::Worker).unwrap();
        let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
        let worlds = [
            world(
                &format!("{kind}-train"),
                WorldPartition::Train,
                &[root(1, 1, valid())],
            ),
            world(
                &format!("{kind}-select"),
                WorldPartition::Select,
                &legacy_steps(kind),
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
                session.put(&worker, "run", &source.source_id, "host", &json!({"schema_version":"rsia.optimization.source.v1", "record":{"id":source.source_id,"body":body,"parent_family":format!("family-{}",source.source_id),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"}, "trace":{"run_id":source.source_id,"parent_family":format!("family-{}",source.source_id),"source_digest":source.content_digest,"purpose":"development","outcome":"success","diagnosis":null,"excerpt":excerpt,"seed":1}, "excerpt_start":0,"excerpt_end":excerpt.len()})).await.unwrap();
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
        let mut p = profile(1, 1, 2);
        p.pool_digest = pool.pool_digest.clone();
        Self {
            _dir: dir,
            store,
            worker,
            evaluator,
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
            &ExplorationCapsV1::online(),
        )
        .await
    }
}

// Actual StoredReplayReportV1 bytes from unchanged product 6f0e5fa35b582f0f4cde4207ddf59d65a51b180a.
// Generated with the baseline-only capture_baseline_report_bytes test via the public
// run_and_persist_pool_replay API, using Persistent::new and legacy_steps above.
// CPU measurements are retained; these are synthetic evidence fixtures, not real repairs.
// SHA256 (affected): 64b34091cdbd9b520257d526153257e164bd9db247f332ebfde0990fda5e1137
const LEGACY_AFFECTED: &str = r#"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-5c7d5c88d79fe7e15b4a16560f23eb439d7062a7216ef5b9689faa955e4ef14c","namespace":"n","purpose":"development","partition":"select","pool_digest":"8560bdbe5148a706998caf0c44fb28eaf910053f320f4eb53ce1284d1c4b0847","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":2,"horizon":2,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"8560bdbe5148a706998caf0c44fb28eaf910053f320f4eb53ce1284d1c4b0847","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"0febb61436c07cecfca05ef16a00fef938b3d8f794be474e34da046f963adb55","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"affected-select","world_digest":"511f87138b1f02868f36ffac151c3de7015bf090dcf38c5c5f25e5c2131afcd5","report":{"schema_version":"rsia.replay_report.v2","world_id":"affected-select","world_digest":"511f87138b1f02868f36ffac151c3de7015bf090dcf38c5c5f25e5c2131afcd5","cluster_id":"cluster-affected-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"0febb61436c07cecfca05ef16a00fef938b3d8f794be474e34da046f963adb55","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[null]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[null]}],"probe_best_quality_micros":[500000,500000],"round_best_quality_micros":[500000,500000],"final_quality_micros":500000,"probes":2,"simulated_rounds":2,"parallel_penalty":{"numerator":2,"denominator":2},"no_probe":false,"attainment_auc_micros":500000,"q_auc_sim_micros":500000,"score_v2_micros":400000,"historical_usage":{"input_tokens":2,"output_tokens":2,"cost_micros":3,"latency_millis":2},"replay_cpu_nanos":520666,"coverage":{"attempted_actions":2,"observed_actions":2,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"repairable_failure","episode_id":"same","failure_kind":"compile","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","environment_reset":true,"dispatched_repairs":1},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"repairable_failure","episode_id":"same","failure_kind":"compile","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","environment_reset":true,"dispatched_repairs":0},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":1}],"current_branch_seq":1,"current_branch_focus_actions":2,"decisions_completed":2,"waits":[],"nodes_used":2,"recovery_dispatches_used":1}}}],"reports_body_digest":"dd06a3178533a9dfcc98a21bf626463c2f2cee8d02f24b4f30755997ec7260a6","semantic_reports_digest":"cc503e52046f0de005793ccad5e87fc8753b1dbe5c9d2558941d4634f97dc374"}"#;
const LEGACY_AFFECTED_SHA256: &str =
    "64b34091cdbd9b520257d526153257e164bd9db247f332ebfde0990fda5e1137";
// SHA256 (valid): 942a65b68e50ec0c2b9c211b6d6607502e137ee67d17499671d6a4243a2a087c
const LEGACY_VALID: &str = r#"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-634c4c6a51f68f95b93d44784ba565859a1138aa15bf28c547cd4a3db353cda9","namespace":"n","purpose":"development","partition":"select","pool_digest":"55c23b628a106841a6521792c166a76a39d4faeaa1e8dbb7ac703184f7248fac","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":2,"horizon":2,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"55c23b628a106841a6521792c166a76a39d4faeaa1e8dbb7ac703184f7248fac","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"db68c480d6935c6fa92af6e4d18efe7cb377ec2811556ffdf86c0b7af90b5976","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"valid-select","world_digest":"600cf8d5e98b3d921aca0bed68006a873bc005e0e843cdf1f5740fd324ffd7a6","report":{"schema_version":"rsia.replay_report.v2","world_id":"valid-select","world_digest":"600cf8d5e98b3d921aca0bed68006a873bc005e0e843cdf1f5740fd324ffd7a6","cluster_id":"cluster-valid-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"db68c480d6935c6fa92af6e4d18efe7cb377ec2811556ffdf86c0b7af90b5976","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[null]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[800000]}],"probe_best_quality_micros":[500000,800000],"round_best_quality_micros":[500000,800000],"final_quality_micros":800000,"probes":2,"simulated_rounds":2,"parallel_penalty":{"numerator":2,"denominator":2},"no_probe":false,"attainment_auc_micros":650000,"q_auc_sim_micros":650000,"score_v2_micros":550000,"historical_usage":{"input_tokens":2,"output_tokens":2,"cost_micros":3,"latency_millis":2},"replay_cpu_nanos":407750,"coverage":{"attempted_actions":2,"observed_actions":2,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"repairable_failure","episode_id":"same","failure_kind":"compile","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","environment_reset":true,"dispatched_repairs":1},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"valid","quality_micros":800000},"best_valid_ancestor_micros":800000,"recent_valid_gains_micros":[300000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":2,"decisions_completed":2,"waits":[],"nodes_used":2,"recovery_dispatches_used":1}}}],"reports_body_digest":"287685263e02e9a8b221309eda63a9ae0f59da53cec6e74dda70949bb2a0caa0","semantic_reports_digest":"89580a1e828fcb33138b3b66cb03f920b259071907bbd3578cc0cdb495d716a4"}"#;
const LEGACY_VALID_SHA256: &str =
    "942a65b68e50ec0c2b9c211b6d6607502e137ee67d17499671d6a4243a2a087c";
// SHA256 (hard): 27132c6cb1096d34f92f5a8d75ea99490cd87cf5e55c672f331f2d8637bc4e21
const LEGACY_HARD: &str = r#"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-30932b6e414430b69e3d8bed1a27ffb279274c562cd2eb864182f5a66ef72444","namespace":"n","purpose":"development","partition":"select","pool_digest":"c0d390ad37fc4b5118dd35dcbebbed7e6dcd78280181b917109d7e78b0a32143","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":2,"horizon":2,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"c0d390ad37fc4b5118dd35dcbebbed7e6dcd78280181b917109d7e78b0a32143","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"860e7d685662e13ac66e644e7b87632b4d8f238d8cafff8c8e49f4c9e5e5e00a","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"hard-select","world_digest":"5f56df1671744bb34b32d9147998d995ab3e95e358210492bac664d445b1090e","report":{"schema_version":"rsia.replay_report.v2","world_id":"hard-select","world_digest":"5f56df1671744bb34b32d9147998d995ab3e95e358210492bac664d445b1090e","cluster_id":"cluster-hard-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"860e7d685662e13ac66e644e7b87632b4d8f238d8cafff8c8e49f4c9e5e5e00a","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[null]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[null]}],"probe_best_quality_micros":[500000,500000],"round_best_quality_micros":[500000,500000],"final_quality_micros":500000,"probes":2,"simulated_rounds":2,"parallel_penalty":{"numerator":2,"denominator":2},"no_probe":false,"attainment_auc_micros":500000,"q_auc_sim_micros":500000,"score_v2_micros":400000,"historical_usage":{"input_tokens":2,"output_tokens":2,"cost_micros":3,"latency_millis":2},"replay_cpu_nanos":407417,"coverage":{"attempted_actions":2,"observed_actions":2,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"repairable_failure","episode_id":"same","failure_kind":"compile","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","environment_reset":true,"dispatched_repairs":1},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"hard_failure"},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":2,"decisions_completed":2,"waits":[],"nodes_used":2,"recovery_dispatches_used":1}}}],"reports_body_digest":"f63576ff8231f29b0f07253c001bf52514052b5b5fb8a35ad286fa5a2c29f695","semantic_reports_digest":"cabc07e25bd21372d9f6c014c6b27a7b431259d43167eaed50086fcb6d37490d"}"#;
const LEGACY_HARD_SHA256: &str = "27132c6cb1096d34f92f5a8d75ea99490cd87cf5e55c672f331f2d8637bc4e21";

async fn stored_envelope(env: &Persistent, id: &str) -> Vec<u8> {
    let mut session = env.store.session().await.unwrap();
    let value: serde_json::Value = session.need(&env.worker, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    serde_json::to_vec(&value).unwrap()
}

#[tokio::test]
async fn old_affected_report_is_static_readable_but_semantically_rejected_without_overwrite() {
    assert_eq!(hash(LEGACY_AFFECTED.as_bytes()), LEGACY_AFFECTED_SHA256);
    let env = Persistent::new("affected").await;
    let old: StoredReplayReportV1 = serde_json::from_str(LEGACY_AFFECTED).unwrap();
    old.validate_static(&env.pool).unwrap();
    put_replay_report(&env.worker, &env.store, &old)
        .await
        .unwrap();
    let before = stored_envelope(&env, &old.report_id).await;
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
    }
    assert_eq!(stored_envelope(&env, &old.report_id).await, before);
    let fresh = Persistent::new("affected").await;
    let new = fresh.replay().await.unwrap();
    assert_eq!(old.report_id, new.report_id);
    assert_ne!(old.semantic_reports_digest, new.semantic_reports_digest);
    assert_eq!(
        selected(&old.member_reports[0].report),
        selected(&new.member_reports[0].report)
    );
    assert_eq!(counter(&old.member_reports[0].report, 2), 0);
    assert_eq!(counter(&new.member_reports[0].report, 2), 1);
    let view = verified_replay_report_view(&fresh.evaluator, &fresh.store, &new.report_id)
        .await
        .unwrap();
    assert!(!view.promotion_eligible);
    assert!(view.development_only);
    assert!(replay_v2_is_not_formal(&new.member_reports[0].report).is_err());
    assert!(
        matches!(put_replay_report(&env.worker, &env.store, &new).await, Err(Error::Conflict(ref reason)) if reason == "immutable replay report differs")
    );
    assert_eq!(stored_envelope(&env, &old.report_id).await, before);
}

#[tokio::test]
async fn unaffected_old_single_recoveries_remain_verified_and_idempotent() {
    for (kind, bytes, digest) in [
        ("valid", LEGACY_VALID, LEGACY_VALID_SHA256),
        ("hard", LEGACY_HARD, LEGACY_HARD_SHA256),
    ] {
        assert_eq!(hash(bytes.as_bytes()), digest);
        let env = Persistent::new(kind).await;
        let old: StoredReplayReportV1 = serde_json::from_str(bytes).unwrap();
        old.validate_static(&env.pool).unwrap();
        put_replay_report(&env.worker, &env.store, &old)
            .await
            .unwrap();
        let before = stored_envelope(&env, &old.report_id).await;
        let view = verified_replay_report_view(&env.evaluator, &env.store, &old.report_id)
            .await
            .unwrap();
        assert_eq!(view.semantic_digest, old.semantic_reports_digest);
        assert!(!view.promotion_eligible);
        assert!(view.development_only);
        assert_eq!(
            fingerprint(&env.replay().await.unwrap()).unwrap(),
            fingerprint(&old).unwrap()
        );
        assert_eq!(stored_envelope(&env, &old.report_id).await, before);
    }
}
