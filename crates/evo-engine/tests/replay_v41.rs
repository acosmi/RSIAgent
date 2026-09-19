//! Replay driver, barrier, terminal, and scoring tests.
use evo_core::evidence::Purpose;
use evo_core::hash;
use evo_core::replay::*;
use evo_core::strategy::{
    ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind, ObservedStatus,
};
use evo_core::{Context, Error, Role};
use evo_engine::replay::{
    LiveWorldAuthority, compare_complete_v2, replay_v2_is_not_formal, run_and_persist_pool_replay,
    run_persisted_replay, run_replay, score_completed_trajectory, verified_replay_report_view,
};
use evo_storage::Store;
use evo_storage::replay::{register_replay_pool, seal_replay_world};
use serde_json::json;
use std::collections::BTreeSet;

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

fn action(seq: u32, parent: &str, branch: u32, kind: ActionKindV1) -> ReplayActionSpecV1 {
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

fn transition(
    seq: u32,
    id: &str,
    parent: &str,
    next: &str,
    kind: ActionKindV1,
    outcome: ReplayTransitionOutcome,
) -> ReplayTransitionV2 {
    ReplayTransitionV2 {
        record_id: id.into(),
        record_seq: seq,
        generation_signature: d("generation-v1"),
        parent_context_signature: parent.into(),
        action_kind: kind,
        next_context_signature: next.into(),
        outcome,
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

fn valid(q: u32) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::Valid { quality_micros: q },
    }
}

fn world() -> ReplayWorldV2 {
    let actions = vec![
        action(1, &d("baseline"), 1, ActionKindV1::Widen { root_slot: 1 }),
        action(
            2,
            &d("ctx-1"),
            1,
            ActionKindV1::Deepen { parent_node_seq: 1 },
        ),
    ];
    let transitions = vec![
        transition(
            1,
            "opaque-root",
            &d("baseline"),
            &d("ctx-1"),
            ActionKindV1::Widen { root_slot: 1 },
            valid(400_000),
        ),
        transition(
            2,
            "opaque-child",
            &d("ctx-1"),
            &d("ctx-2"),
            ActionKindV1::Deepen { parent_node_seq: 1 },
            valid(800_000),
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

fn profile(objective: ReplayObjective, budget: u8, width: u8) -> ReplaySimulationProfile {
    ReplaySimulationProfile {
        simulation_version: SIMULATION_VERSION.into(),
        objective,
        w_sim: width,
        probe_budget: budget,
        horizon: budget,
        lambda_work_micros: DEFAULT_LAMBDA_MICROS,
        lambda_round_micros: DEFAULT_LAMBDA_MICROS,
        fixed_seed: 9,
        global_recovery_dispatch_limit: 1,
        pool_digest: d("pool"),
        purpose: Purpose::Development,
        target_runtime_profile: "simulation-only".into(),
    }
}

fn authority() -> LiveWorldAuthority {
    LiveWorldAuthority {
        revoke_watermark: 7,
        revoked_source_ids: BTreeSet::new(),
    }
}

#[test]
fn v1_preserves_probe_axis_auc_without_future_normalization() {
    let report = run_replay(
        &world(),
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::AttainmentAucV1, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(report.probe_best_quality_micros, vec![400_000, 800_000]);
    assert_eq!(report.attainment_auc_micros, Some(600_000));
    assert!(report.score_v2_micros.is_none());
}

#[test]
fn future_quality_and_opaque_id_rename_do_not_change_first_choice() {
    let original = world();
    let first = run_replay(
        &original,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    let mut poisoned = original.clone();
    poisoned.sealed_digest = None;
    poisoned.transitions[1].record_id = "renamed-future".into();
    poisoned.transitions[1].outcome = valid(1);
    poisoned.manifest.source_closure[1].content_digest =
        replay_observation_digest(&poisoned.manifest, &poisoned.transitions[1]).unwrap();
    poisoned.seal().unwrap();
    let second = run_replay(
        &poisoned,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(first.batches[0].action_seqs, second.batches[0].action_seqs);
    assert_eq!(first.batches[0].action_seqs, vec![1]);
}

#[test]
fn missing_transition_is_oos_not_avoided_by_future_scan() {
    let mut missing = world();
    missing.sealed_digest = None;
    missing.transitions.remove(0);
    missing.transitions.clear();
    let baseline_source_id = missing.manifest.baseline_observation_source_id.clone();
    missing
        .manifest
        .source_closure
        .retain(|source| source.source_id == baseline_source_id);
    missing.seal().unwrap();
    let report = run_replay(
        &missing,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(report.terminal, ReplayTerminal::OutOfSupport);
    assert_eq!(report.probes, 0);
    assert_eq!(report.simulated_rounds, 0);
    assert!(report.parallel_penalty.is_none());
    assert_eq!(report.coverage.attempted_actions, 1);
    assert_eq!(report.coverage.out_of_support_actions, 1);
    assert!(report.score_v2_micros.is_none());
}

#[test]
fn usage_uncertain_is_censored_and_never_gets_a_determined_score() {
    let mut uncertain = world();
    uncertain.sealed_digest = None;
    uncertain.transitions[0].outcome = ReplayTransitionOutcome::Observed {
        status: ObservedStatus::UsageUncertain,
    };
    uncertain.transitions[0].actual_usage.cost_micros = None;
    uncertain.manifest.source_closure[0].content_digest =
        replay_observation_digest(&uncertain.manifest, &uncertain.transitions[0]).unwrap();
    uncertain.seal().unwrap();
    let report = run_replay(
        &uncertain,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 1, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(report.terminal, ReplayTerminal::Censored);
    assert_eq!(report.probes, 1);
    assert_eq!(report.coverage.censored_actions, 1);
    assert!(!report.coverage.complete_support);
    assert!(report.attainment_auc_micros.is_none());
    assert!(report.q_auc_sim_micros.is_none());
    assert!(report.score_v2_micros.is_none());
    assert_eq!(report.historical_usage.cost_micros, None);
}

#[test]
fn historical_usage_overflow_is_explicitly_invalid() {
    for field in ["tokens", "cost", "latency"] {
        let mut overflow = world();
        overflow.sealed_digest = None;
        match field {
            "tokens" => {
                overflow.transitions[0].actual_usage.input_tokens = u64::MAX;
                overflow.transitions[1].actual_usage.input_tokens = 1;
            }
            "cost" => {
                overflow.transitions[0].actual_usage.cost_micros = Some(u64::MAX);
                overflow.transitions[1].actual_usage.cost_micros = Some(1);
            }
            "latency" => {
                overflow.transitions[0].actual_usage.latency_millis = Some(u64::MAX);
                overflow.transitions[1].actual_usage.latency_millis = Some(1);
            }
            _ => unreachable!(),
        }
        for index in 0..2 {
            overflow.manifest.source_closure[index].content_digest =
                replay_observation_digest(&overflow.manifest, &overflow.transitions[index])
                    .unwrap();
        }
        overflow.seal().unwrap();
        let result = run_replay(
            &overflow,
            &ElasticPolicyV1::default(),
            &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
            &ExplorationCapsV1::online(),
            &authority(),
        );
        assert!(matches!(result, Err(Error::Invalid(_))), "field={field}");
    }
}

#[test]
fn censored_policy_stop_world_exhausted_and_revoked_are_distinct() {
    let mut censored = world();
    censored.sealed_digest = None;
    censored.transitions[0].outcome = ReplayTransitionOutcome::Censored {
        reason: "redacted result".into(),
    };
    censored.manifest.source_closure[0].content_digest =
        replay_observation_digest(&censored.manifest, &censored.transitions[0]).unwrap();
    censored.seal().unwrap();
    let censored_report = run_replay(
        &censored,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(censored_report.terminal, ReplayTerminal::Censored);

    let mut stop = world();
    stop.sealed_digest = None;
    stop.manifest.action_catalog[0].estimated_cost_upper_micros = None;
    stop.seal().unwrap();
    let stop_report = run_replay(
        &stop,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(stop_report.terminal, ReplayTerminal::PolicyStop);
    assert_eq!(stop_report.probes, 0);
    assert!(stop_report.parallel_penalty.is_none());

    let mut exhausted = world();
    exhausted.sealed_digest = None;
    exhausted.manifest.action_catalog.clear();
    exhausted.transitions.clear();
    let baseline_source_id = exhausted.manifest.baseline_observation_source_id.clone();
    exhausted
        .manifest
        .source_closure
        .retain(|source| source.source_id == baseline_source_id);
    exhausted.manifest.prefix_coverage[0].exhausted = true;
    exhausted.seal().unwrap();
    let exhausted_report = run_replay(
        &exhausted,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(exhausted_report.terminal, ReplayTerminal::WorldExhausted);

    let mut revoked = authority();
    revoked.revoked_source_ids.insert("source-2".into());
    let revoked_report = run_replay(
        &world(),
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &revoked,
    )
    .unwrap();
    assert_eq!(revoked_report.terminal, ReplayTerminal::SourceRevoked);
}

#[test]
fn mixed_batches_use_whole_trajectory_ratio_and_no_benefit_probe_loses() {
    let p = profile(ReplayObjective::ParetoAttainmentV2, 12, 4);
    let mixed = score_completed_trajectory(
        &p,
        &[1, 4],
        &[300_000, 500_000, 500_000, 500_000, 500_000],
        &[300_000, 500_000],
    )
    .unwrap();
    assert_eq!(mixed.probes, 5);
    assert_eq!(mixed.simulated_rounds, 2);
    assert_eq!(
        mixed.parallel_penalty,
        Some(RationalValue {
            numerator: 2,
            denominator: 5
        })
    );
    let useful = score_completed_trajectory(&p, &[1], &[500_000], &[500_000]).unwrap();
    let padded = score_completed_trajectory(&p, &[2], &[500_000, 500_000], &[500_000]).unwrap();
    assert!(useful.score_v2_micros > padded.score_v2_micros);
}

#[test]
fn invalid_world_is_terminal_and_report_is_never_formal() {
    let mut invalid = world();
    invalid.transitions[0].next_context_signature = "changed".into();
    let report = run_replay(
        &invalid,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    assert_eq!(report.terminal, ReplayTerminal::InvalidWorld);
    assert!(replay_v2_is_not_formal(&report).is_err());
}

#[test]
fn dominated_complete_report_cannot_win_and_unknown_cost_is_not_zero() {
    let profile = profile(ReplayObjective::ParetoAttainmentV2, 2, 1);
    let better = run_replay(
        &world(),
        &ElasticPolicyV1::default(),
        &profile,
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    let mut worse = better.clone();
    worse.score_v2_micros = better.score_v2_micros.map(|score| score - 1);
    assert_eq!(
        compare_complete_v2(&better, &worse).unwrap(),
        std::cmp::Ordering::Greater
    );
    let mut unknown = better.clone();
    unknown.historical_usage.cost_micros = None;
    assert!(compare_complete_v2(&unknown, &better).is_err());

    let mut legacy = better.clone();
    legacy.objective_version = OBJECTIVE_V1.into();
    assert!(compare_complete_v2(&legacy, &better).is_err());
}

#[test]
fn score_rejects_zero_profile_nonmonotone_and_within_batch_leakage() {
    let mut invalid = profile(ReplayObjective::ParetoAttainmentV2, 12, 4);
    invalid.w_sim = 0;
    assert!(score_completed_trajectory(&invalid, &[], &[], &[]).is_err());
    let valid = profile(ReplayObjective::ParetoAttainmentV2, 12, 4);
    assert!(
        score_completed_trajectory(&valid, &[1, 1], &[500_000, 400_000], &[500_000, 400_000])
            .is_err()
    );
    assert!(score_completed_trajectory(&valid, &[2], &[300_000, 500_000], &[500_000]).is_err());
}

#[test]
fn root_branch_gain_and_ancestor_are_not_polluted_by_other_branch() {
    let mut independent = world();
    independent.sealed_digest = None;
    independent.manifest.action_catalog[1] =
        action(2, &d("baseline"), 2, ActionKindV1::Widen { root_slot: 2 });
    independent.transitions[0].outcome = valid(900_000);
    independent.transitions[1].parent_context_signature = d("baseline");
    independent.transitions[1].action_kind = ActionKindV1::Widen { root_slot: 2 };
    independent.transitions[1].outcome = valid(200_000);
    independent.manifest.source_closure[0].content_digest =
        replay_observation_digest(&independent.manifest, &independent.transitions[0]).unwrap();
    independent.manifest.source_closure[1].content_digest =
        replay_observation_digest(&independent.manifest, &independent.transitions[1]).unwrap();
    independent.seal().unwrap();
    let report = run_replay(
        &independent,
        &ElasticPolicyV1::default(),
        &profile(ReplayObjective::ParetoAttainmentV2, 2, 1),
        &ExplorationCapsV1::online(),
        &authority(),
    )
    .unwrap();
    let prefix = report.revealed_prefix.unwrap();
    let second = prefix.nodes.iter().find(|node| node.node_seq == 2).unwrap();
    assert_eq!(second.best_valid_ancestor_micros, Some(200_000));
    assert_eq!(second.recent_valid_gains_micros, vec![100_000]);
}

#[test]
fn recovery_is_counted_only_when_dispatched_and_global_limit_spans_episodes() {
    let repair = d("repair-template");
    let failure = |episode: &str| ReplayTransitionOutcome::Observed {
        status: ObservedStatus::RepairableFailure {
            episode_id: episode.into(),
            failure_kind: FailureKind::Compile,
            repair_template_digest: repair.clone(),
            environment_reset: true,
            dispatched_repairs: 0,
        },
    };
    let mut recovery = world();
    recovery.sealed_digest = None;
    recovery.manifest.action_catalog = vec![
        action(1, &d("baseline"), 1, ActionKindV1::Widen { root_slot: 1 }),
        action(
            2,
            &d("ctx-1"),
            1,
            ActionKindV1::Recover {
                failed_node_seq: 1,
                episode_id: "episode-1".into(),
            },
        ),
        action(3, &d("baseline"), 2, ActionKindV1::Widen { root_slot: 2 }),
        action(
            4,
            &d("ctx-3"),
            2,
            ActionKindV1::Recover {
                failed_node_seq: 3,
                episode_id: "episode-2".into(),
            },
        ),
    ];
    recovery.transitions = vec![
        transition(
            1,
            "failure-1",
            &d("baseline"),
            &d("ctx-1"),
            ActionKindV1::Widen { root_slot: 1 },
            failure("episode-1"),
        ),
        transition(
            2,
            "repair-1",
            &d("ctx-1"),
            &d("ctx-2"),
            ActionKindV1::Recover {
                failed_node_seq: 1,
                episode_id: "episode-1".into(),
            },
            failure("episode-1"),
        ),
        transition(
            3,
            "failure-2",
            &d("baseline"),
            &d("ctx-3"),
            ActionKindV1::Widen { root_slot: 2 },
            failure("episode-2"),
        ),
        transition(
            4,
            "repair-2",
            &d("ctx-3"),
            &d("ctx-4"),
            ActionKindV1::Recover {
                failed_node_seq: 3,
                episode_id: "episode-2".into(),
            },
            failure("episode-2"),
        ),
    ];
    recovery.manifest.source_closure = recovery
        .transitions
        .iter()
        .enumerate()
        .map(|(index, _transition)| ReplaySourceRef {
            source_id: format!("source-{}", index + 1),
            content_digest: String::new(),
        })
        .collect();
    recovery.manifest.source_closure.push(ReplaySourceRef {
        source_id: recovery.manifest.baseline_observation_source_id.clone(),
        content_digest: String::new(),
    });
    for (index, transition) in recovery.transitions.iter_mut().enumerate() {
        transition.source_ids = vec![format!("source-{}", index + 1)];
        transition.observation_source_id = format!("source-{}", index + 1);
    }
    recovery.manifest.prefix_coverage = ["ctx-1", "ctx-2", "ctx-3", "ctx-4"]
        .into_iter()
        .map(|context| PrefixCoverageV1 {
            context_signature: d(context),
            exhausted: true,
        })
        .collect();
    refresh_evidence_digests(&mut recovery);
    recovery.seal().unwrap();
    let mut simulation = profile(ReplayObjective::ParetoAttainmentV2, 4, 1);
    simulation.global_recovery_dispatch_limit = 1;
    let report = run_replay(
        &recovery,
        &ElasticPolicyV1::default(),
        &simulation,
        &ExplorationCapsV1::online(),
        &LiveWorldAuthority {
            revoke_watermark: 7,
            revoked_source_ids: BTreeSet::new(),
        },
    )
    .unwrap();
    let selected: Vec<u32> = report
        .batches
        .iter()
        .flat_map(|batch| batch.action_seqs.clone())
        .collect();
    assert!(selected.contains(&2));
    assert!(!selected.contains(&4));
    let prefix = report.revealed_prefix.unwrap();
    let original_failure = prefix.nodes.iter().find(|node| node.node_seq == 1).unwrap();
    assert!(matches!(
        original_failure.status,
        ObservedStatus::RepairableFailure {
            dispatched_repairs: 1,
            ..
        }
    ));
    let failed_repair = prefix.nodes.iter().find(|node| node.node_seq == 2).unwrap();
    assert_eq!(failed_repair.repair_failures_dispatched, 1);
    let ordinary_failure = prefix.nodes.iter().find(|node| node.node_seq == 3).unwrap();
    assert_eq!(ordinary_failure.repair_failures_dispatched, 0);
}

#[tokio::test]
async fn persisted_wrapper_rechecks_secondary_source_after_compute() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("persistent-replay.sqlite3"))
        .await
        .unwrap();
    let ctx = Context::new("n", "worker", Role::Worker).unwrap();
    let mut persisted = world();
    persisted.sealed_digest = None;
    persisted.manifest.revoke_watermark = 1;
    persisted.seal().unwrap();
    let mut train = persisted.clone();
    train.sealed_digest = None;
    train.manifest.world_id = "world-train".into();
    train.manifest.cluster_id = "cluster-train".into();
    train.manifest.partition = WorldPartition::Train;
    train.manifest.baseline_observation_source_id = "baseline-source-train".into();
    train.manifest.source_closure[2].source_id = "baseline-source-train".into();
    for (index, transition) in train.transitions.iter_mut().enumerate() {
        let source_id = format!("train-source-{}", index + 1);
        transition.source_ids = vec![source_id.clone()];
        transition.observation_source_id = source_id.clone();
        train.manifest.source_closure[index].source_id = source_id;
    }
    refresh_evidence_digests(&mut train);
    train.seal().unwrap();
    let mut session = store.session().await.unwrap();
    for replay_world in [&persisted, &train] {
        for source in &replay_world.manifest.source_closure {
            let body = if source.source_id == replay_world.manifest.baseline_observation_source_id {
                replay_baseline_observation_bytes(&replay_world.manifest).unwrap()
            } else {
                let transition = replay_world
                    .transitions
                    .iter()
                    .find(|transition| transition.observation_source_id == source.source_id)
                    .unwrap();
                replay_observation_bytes(&replay_world.manifest, transition).unwrap()
            };
            let excerpt = String::from_utf8(body.clone()).unwrap();
            session.put(&ctx, "run", &source.source_id, "host", &json!({
                "schema_version":"rsia.optimization.source.v1",
                "record":{"id":source.source_id,"body":body.clone(),"parent_family":format!("family-{}",source.source_id),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
                "trace":{"run_id":source.source_id,"parent_family":format!("family-{}",source.source_id),"source_digest":source.content_digest,"purpose":"development","outcome":"success","diagnosis":null,"excerpt":excerpt,"seed":1},
                "excerpt_start":0,"excerpt_end":body.len()
            })).await.unwrap();
        }
    }
    session.bump_watermark(&ctx, &d("watermark")).await.unwrap();
    session.commit().await.unwrap();
    seal_replay_world(&ctx, &store, &persisted).await.unwrap();
    seal_replay_world(&ctx, &store, &train).await.unwrap();
    let pool = register_replay_pool(
        &ctx,
        &store,
        &[
            persisted.manifest.world_id.clone(),
            train.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap();
    let mut simulation = profile(ReplayObjective::ParetoAttainmentV2, 2, 1);
    simulation.pool_digest = pool.pool_digest;
    let policy = ElasticPolicyV1::default();
    let caps = ExplorationCapsV1::online();
    let stored = run_and_persist_pool_replay(
        &ctx,
        &store,
        &simulation.pool_digest,
        WorldPartition::Select,
        &policy,
        &simulation,
        &caps,
    )
    .await
    .unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let view = verified_replay_report_view(&evaluator, &store, &stored.report_id)
        .await
        .unwrap();
    assert!(!view.promotion_eligible);
    assert!(view.development_only);
    assert_eq!(view.member_worlds.len(), 1);
    let cached = run_and_persist_pool_replay(
        &ctx,
        &store,
        &simulation.pool_digest,
        WorldPartition::Select,
        &policy,
        &simulation,
        &caps,
    )
    .await
    .unwrap();
    assert_eq!(cached.report_id, stored.report_id);
    assert_eq!(cached.reports_body_digest, stored.reports_body_digest);
    let mut session = store.session().await.unwrap();
    let original: serde_json::Value = session
        .need(&ctx, "artifact", &stored.report_id)
        .await
        .unwrap();
    let mut forged = original.clone();
    forged["payload"]["member_reports"][0]["report"]["final_quality_micros"] = json!(999_999);
    session
        .put(&ctx, "artifact", &stored.report_id, ctx.actor(), &forged)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        verified_replay_report_view(&evaluator, &store, &stored.report_id)
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    session
        .put(&ctx, "artifact", &stored.report_id, ctx.actor(), &original)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let replay = run_persisted_replay(
        &ctx,
        &store,
        "world-1",
        Purpose::Development,
        &policy,
        &simulation,
        &caps,
    );
    let revoke = async {
        tokio::task::yield_now().await;
        let mut session = store.session().await.unwrap();
        session
            .put(
                &ctx,
                "tombstone",
                "source-2",
                "worker",
                &json!({"id":"source-2"}),
            )
            .await
            .unwrap();
        session.bump_watermark(&ctx, &d("revoked")).await.unwrap();
        session.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(replay, revoke);
    assert!(result.is_err());
}
