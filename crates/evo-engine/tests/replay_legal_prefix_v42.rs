//! Observed-parent legality in lookup-only replay. These are synthetic world/source
//! fixtures, not real model executions or evidence of formal benefit.
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

fn d(value: &str) -> String {
    hash(value.as_bytes())
}
fn valid(quality_micros: u32) -> ObservedStatus {
    ObservedStatus::Valid { quality_micros }
}
fn failure() -> ObservedStatus {
    ObservedStatus::RepairableFailure {
        episode_id: "episode".into(),
        failure_kind: FailureKind::Compile,
        repair_template_digest: d("repair"),
        environment_reset: true,
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
    outcome: ReplayTransitionOutcome,
}
fn root(seq: u32, branch: u32, status: ObservedStatus) -> Step {
    Step {
        seq,
        branch,
        depth: 1,
        kind: ActionKindV1::Widen { root_slot: branch },
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
        outcome: observed(status),
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
            estimated_cost_upper_micros: Some(10),
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
fn profile(width: u8, probes: u8) -> ReplaySimulationProfile {
    let mut p = ReplaySimulationProfile::default_v2(width, d("pool"));
    p.probe_budget = probes;
    p.horizon = probes;
    p.fixed_seed = 17;
    p
}
fn run(steps: &[Step], width: u8, probes: u8) -> ReplayReportV2 {
    let w = world("ag074-local", WorldPartition::Select, steps);
    run_world(&w, &profile(width, probes))
}
fn run_world(w: &ReplayWorldV2, p: &ReplaySimulationProfile) -> ReplayReportV2 {
    w.validate_sealed().unwrap();
    run_replay(
        w,
        &ElasticPolicyV1::default(),
        p,
        &ExplorationCapsV1::online(),
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
fn repeated_parent_steps(child: ObservedStatus) -> Vec<Step> {
    vec![
        root(1, 1, valid(800_000)),
        deepen(2, 1, 1, 2, child),
        deepen(3, 1, 1, 2, valid(950_000)),
    ]
}
fn legacy_steps(kind: &str) -> Vec<Step> {
    match kind {
        "affected" => repeated_parent_steps(valid(900_000)),
        "chain" => vec![
            root(1, 1, valid(800_000)),
            deepen(2, 1, 1, 2, valid(900_000)),
            deepen(3, 2, 1, 3, valid(950_000)),
        ],
        "recover" => vec![
            root(1, 1, failure()),
            Step {
                seq: 2,
                branch: 1,
                depth: 2,
                kind: ActionKindV1::Recover {
                    failed_node_seq: 1,
                    episode_id: "episode".into(),
                },
                outcome: observed(valid(900_000)),
            },
            deepen(3, 2, 1, 3, valid(950_000)),
        ],
        _ => panic!("unknown test fixture"),
    }
}

#[test]
fn normal_deepen_does_not_reuse_an_observed_parent_under_another_record_sequence() {
    let report = run(&repeated_parent_steps(valid(900_000)), 1, 3);
    println!(
        "sealed repeated-parent world: {:?}; terminal {:?}",
        selected(&report),
        report.terminal
    );
    assert_eq!(selected(&report), vec![1, 2]);
    assert_eq!(report.coverage.observed_actions, 2);
}
#[test]
fn hard_failed_child_does_not_return_the_normal_parent_in_replay() {
    let report = run(&repeated_parent_steps(ObservedStatus::HardFailure), 1, 3);
    println!(
        "sealed hard-child world: {:?}; terminal {:?}",
        selected(&report),
        report.terminal
    );
    assert_eq!(selected(&report), vec![1, 2]);
    assert_eq!(report.coverage.observed_actions, 2);
}
#[test]
fn the_available_projection_removes_expanded_parent_waits_but_keeps_a_valid_child() {
    let mut steps = repeated_parent_steps(valid(900_000));
    steps.push(deepen(4, 2, 1, 3, valid(950_000)));
    let report = run(&steps, 1, 2);
    assert_eq!(selected(&report), vec![1, 2]);
    let prefix = report.revealed_prefix.unwrap();
    prefix.validate().unwrap();
    assert!(
        !prefix.waits.iter().any(|w| w.action_seq == 3),
        "expanded-parent opportunity leaked through replay available_actions"
    );
    assert!(prefix.waits.iter().any(|w| w.action_seq == 4));
}

#[test]
fn legal_child_continuations_and_other_branches_survive_at_every_supported_width() {
    let steps = vec![
        root(1, 1, valid(800_000)),
        root(2, 2, valid(800_000)),
        deepen(3, 1, 1, 2, valid(900_000)),
        deepen(4, 1, 1, 2, valid(1_000_000)),
        deepen(5, 2, 2, 2, valid(850_000)),
        deepen(6, 3, 1, 3, valid(950_000)),
        deepen(7, 5, 2, 3, valid(900_000)),
    ];
    for width in [1, 2, 4] {
        let report = run(&steps, width, 6);
        assert_eq!(
            selected(&report).into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 2, 3, 5, 6, 7])
        );
        let mut revealed = BTreeSet::new();
        for batch in &report.batches {
            assert!(batch.action_seqs.len() <= usize::from(width));
            let mut branches = BTreeSet::new();
            for seq in &batch.action_seqs {
                let step = steps.iter().find(|step| step.seq == *seq).unwrap();
                assert!(branches.insert(step.branch));
                if let ActionKindV1::Deepen { parent_node_seq } = step.kind {
                    assert!(revealed.contains(&parent_node_seq));
                }
            }
            revealed.extend(batch.record_seqs.iter().copied());
        }
        report.revealed_prefix.unwrap().validate().unwrap();
    }
}

#[test]
fn missing_out_of_support_and_censored_successors_are_attempted_without_inventing_children() {
    for mode in ["missing", "out_of_support", "censored"] {
        let mut w = world(
            &format!("ag074-{mode}"),
            WorldPartition::Select,
            &repeated_parent_steps(valid(900_000)),
        );
        if mode == "missing" {
            w.transitions.retain(|t| t.record_seq != 2);
        } else {
            let t = w
                .transitions
                .iter_mut()
                .find(|t| t.record_seq == 2)
                .unwrap();
            t.outcome = if mode == "censored" {
                ReplayTransitionOutcome::Censored {
                    reason: "historical observation unavailable".into(),
                }
            } else {
                ReplayTransitionOutcome::OutOfSupport {
                    reason: "no supported successor".into(),
                }
            };
        }
        refresh(&mut w);
        let before = fingerprint(&w).unwrap();
        let report = run_world(&w, &profile(1, 3));
        assert_eq!(selected(&report), vec![1, 2]);
        assert_eq!(report.coverage.attempted_actions, 2);
        assert_eq!(report.coverage.observed_actions, 1);
        assert!(!report.coverage.complete_support);
        assert_eq!(report.score_v2_micros, None);
        assert_eq!(
            report.terminal,
            if mode == "censored" {
                ReplayTerminal::Censored
            } else {
                ReplayTerminal::OutOfSupport
            }
        );
        assert_eq!(
            report.coverage.censored_actions,
            u32::from(mode == "censored")
        );
        assert_eq!(
            report.coverage.out_of_support_actions,
            u32::from(mode != "censored")
        );
        let prefix = report.revealed_prefix.unwrap();
        prefix.validate().unwrap();
        assert_eq!(prefix.nodes.len(), 1);
        assert!(prefix.waits.iter().any(|wait| wait.action_seq == 3));
        assert_eq!(fingerprint(&w).unwrap(), before);
    }
}

#[test]
fn an_observed_usage_uncertain_child_consumes_the_parent_even_at_a_censored_terminal() {
    let report = run(&repeated_parent_steps(ObservedStatus::UsageUncertain), 1, 3);
    assert_eq!(selected(&report), vec![1, 2]);
    assert_eq!(report.terminal, ReplayTerminal::Censored);
    assert_eq!(report.coverage.observed_actions, 2);
    assert_eq!(report.coverage.censored_actions, 1);
    let prefix = report.revealed_prefix.unwrap();
    prefix.validate().unwrap();
    assert_eq!(prefix.nodes.len(), 2);
    assert!(!prefix.waits.iter().any(|wait| wait.action_seq == 3));
}

#[test]
fn unrevealed_scores_and_opaque_records_cannot_change_earlier_batches_or_reopen_a_parent() {
    let mut steps = repeated_parent_steps(valid(900_000));
    steps.push(deepen(4, 2, 1, 3, valid(950_000)));
    let original = world("ag074-future", WorldPartition::Select, &steps);
    let mut changed = original.clone();
    for t in &mut changed.transitions {
        t.record_id = format!("different-audit-identity-{}", t.record_seq);
        if t.record_seq >= 3 {
            t.outcome = observed(valid(if t.record_seq == 3 { 1_000_000 } else { 0 }));
        }
    }
    changed.manifest.action_catalog.reverse();
    refresh(&mut changed);
    for width in [1, 2, 4] {
        let early = run_world(&original, &profile(width, 2));
        let changed_early = run_world(&changed, &profile(width, 2));
        assert_eq!(early.batches, changed_early.batches);
        assert_eq!(selected(&early), vec![1, 2]);
        for w in [&original, &changed] {
            let report = run_world(w, &profile(width, 3));
            assert_eq!(selected(&report), vec![1, 2, 4]);
            report.revealed_prefix.unwrap().validate().unwrap();
        }
    }
}

#[test]
fn the_unaffected_normal_chain_preserves_v1_auc_interpretation() {
    let w = world(
        "ag074-v1-control",
        WorldPartition::Select,
        &legacy_steps("chain"),
    );
    let mut p = profile(1, 3);
    p.objective = ReplayObjective::AttainmentAucV1;
    let report = run_world(&w, &p);
    assert_eq!(selected(&report), vec![1, 2, 3]);
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert!(report.coverage.complete_support);
    assert_eq!(report.attainment_auc_micros, Some(883_333));
    assert_eq!(report.q_auc_sim_micros, Some(883_333));
    assert_eq!(report.score_v2_micros, None);
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
        let worker = Context::new("ag074", "worker", Role::Worker).unwrap();
        let evaluator = Context::new("ag074", "evaluator", Role::Evaluator).unwrap();
        let worlds = vec![
            world(
                &format!("ag074-{kind}-train"),
                WorldPartition::Train,
                &[root(1, 1, valid(800_000))],
            ),
            world(
                &format!("ag074-{kind}-select"),
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
        let mut p = profile(1, 3);
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
            &ExplorationCapsV1::online(),
        )
        .await
    }
}

// Optional explicit baseline capture. The implementation call is unchanged and
// files are created once, so a later rerun cannot replace frozen old evidence.
#[tokio::test]
async fn baseline_generates_unmodified_reports_through_the_persistent_engine() {
    let Some(directory) = std::env::var_os("RSIA_AG074_LEGACY_REPORT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    for kind in ["affected", "chain", "recover"] {
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

// Compare the API's serialized immutable artifact payload, including the full
// old report. This does not assert byte identity of the SQLite database file.
async fn stored_payload_bytes(env: &Persistent, id: &str) -> Vec<u8> {
    let mut session = env.store.session().await.unwrap();
    let value: serde_json::Value = session.need(&env.worker, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    serde_json::to_vec(&value).unwrap()
}

#[tokio::test]
async fn the_actual_old_affected_report_stays_readable_but_recomputation_cannot_overwrite_it() {
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
    let old_member = &old.member_reports[0].report;
    let new_member = &new.member_reports[0].report;
    assert_eq!(selected(old_member), vec![1, 2, 3]);
    assert_eq!(selected(new_member), vec![1, 2]);
    assert_eq!(old_member.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(new_member.terminal, ReplayTerminal::WorldExhausted);
    assert!(old_member.coverage.complete_support);
    assert!(!new_member.coverage.complete_support);
    assert_eq!(old_member.final_quality_micros, 950_000);
    assert_eq!(new_member.final_quality_micros, 900_000);
    assert!(old_member.score_v2_micros.is_some());
    assert_eq!(new_member.score_v2_micros, None);
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
async fn the_actual_old_normal_chain_and_recover_control_remain_verified_and_idempotent() {
    for (kind, bytes, digest, worlds_digest) in [
        (
            "chain",
            LEGACY_CHAIN,
            LEGACY_CHAIN_SHA256,
            LEGACY_CHAIN_WORLDS_SHA256,
        ),
        (
            "recover",
            LEGACY_RECOVER,
            LEGACY_RECOVER_SHA256,
            LEGACY_RECOVER_WORLDS_SHA256,
        ),
    ] {
        assert_eq!(hash(bytes.as_bytes()), digest);
        let env = Persistent::new(kind).await;
        assert_eq!(
            hash(&serde_json::to_vec(&env.worlds).unwrap()),
            worlds_digest
        );
        let old: StoredReplayReportV1 = serde_json::from_str(bytes).unwrap();
        old.validate_static(&env.pool).unwrap();
        put_replay_report(&env.worker, &env.store, &old)
            .await
            .unwrap();
        let before = stored_payload_bytes(&env, &old.report_id).await;
        let loaded = load_live_replay_report(&env.evaluator, &env.store, &old.report_id)
            .await
            .unwrap();
        assert_eq!(serde_json::to_string(&loaded).unwrap(), bytes);
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
    }
}

// Actual complete reports generated before the product edit, at baseline
// 212c78488b403397eaebaa65a6d280b9c6be3899, through the public persistent engine.
// CPU measurements are retained. The worlds and sources are synthetic fixtures.
const LEGACY_AFFECTED: &str = r##"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-0d08cd4b186e63d0eb79eed5c328d4316fe6becccb5bd2470b44ee08d558dd6a","namespace":"ag074","purpose":"development","partition":"select","pool_digest":"a4418af9726e8480b097d4ca68190c282a71a159341b333f865757453386a5af","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":3,"horizon":3,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"a4418af9726e8480b097d4ca68190c282a71a159341b333f865757453386a5af","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"9ef10a58eddabb3e829e5f0b42aa6670234da69afeb3f06cc7eea32f74a3cecb","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"ag074-affected-select","world_digest":"0c5b62d8c9c0e45261790ba2dfb524fc04466b3022696bf0914429e9488bfcdb","report":{"schema_version":"rsia.replay_report.v2","world_id":"ag074-affected-select","world_digest":"0c5b62d8c9c0e45261790ba2dfb524fc04466b3022696bf0914429e9488bfcdb","cluster_id":"cluster-ag074-affected-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"9ef10a58eddabb3e829e5f0b42aa6670234da69afeb3f06cc7eea32f74a3cecb","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[800000]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[900000]},{"decision_round":3,"action_seqs":[3],"record_seqs":[3],"revealed_quality_micros":[950000]}],"probe_best_quality_micros":[800000,900000,950000],"round_best_quality_micros":[800000,900000,950000],"final_quality_micros":950000,"probes":3,"simulated_rounds":3,"parallel_penalty":{"numerator":3,"denominator":3},"no_probe":false,"attainment_auc_micros":883333,"q_auc_sim_micros":883333,"score_v2_micros":783333,"historical_usage":{"input_tokens":3,"output_tokens":3,"cost_micros":6,"latency_millis":3},"replay_cpu_nanos":949750,"coverage":{"attempted_actions":3,"observed_actions":3,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"valid","quality_micros":800000},"best_valid_ancestor_micros":800000,"recent_valid_gains_micros":[300000],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"valid","quality_micros":900000},"best_valid_ancestor_micros":900000,"recent_valid_gains_micros":[300000,100000],"repair_failures_dispatched":0},{"node_seq":3,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"valid","quality_micros":950000},"best_valid_ancestor_micros":950000,"recent_valid_gains_micros":[300000,150000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":3,"decisions_completed":3,"waits":[],"nodes_used":3,"recovery_dispatches_used":0}}}],"reports_body_digest":"34e43d9fa3b5b4a4849fd8e2579f76f452976e471d80037e06ffc9cadd15fd1b","semantic_reports_digest":"293c5561401ab0d9327213660f9fe2f3c84503119c266632cea5ddcf6d3307f9"}"##;
const LEGACY_AFFECTED_SHA256: &str =
    "4e12d213a766f20c1408e63b23a9ddf24496b672a2651cfb71ea337a42b8f97b";
const LEGACY_AFFECTED_WORLDS_SHA256: &str =
    "cc5bb3957df6fadffb9f81eb7641963627106ade93a64065bcf9975a5519593b";
const LEGACY_CHAIN: &str = r##"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-578ae8c45ba4d99a607461d32cb60701110542ffa6cbdd9caa47dd90ccc6a797","namespace":"ag074","purpose":"development","partition":"select","pool_digest":"6fb6142a3e60c8441722e5739ae570d79680db58a177648e1d44c2d7105c6a5e","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":3,"horizon":3,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"6fb6142a3e60c8441722e5739ae570d79680db58a177648e1d44c2d7105c6a5e","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"5e9937f37b5322c613a65153d31dc35d3dcd7a9904a6b54695cd20af49efdf4c","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"ag074-chain-select","world_digest":"089d071f00476a4f850306afcddf954968726f5b062a30059dd16e0d7b379046","report":{"schema_version":"rsia.replay_report.v2","world_id":"ag074-chain-select","world_digest":"089d071f00476a4f850306afcddf954968726f5b062a30059dd16e0d7b379046","cluster_id":"cluster-ag074-chain-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"5e9937f37b5322c613a65153d31dc35d3dcd7a9904a6b54695cd20af49efdf4c","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[800000]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[900000]},{"decision_round":3,"action_seqs":[3],"record_seqs":[3],"revealed_quality_micros":[950000]}],"probe_best_quality_micros":[800000,900000,950000],"round_best_quality_micros":[800000,900000,950000],"final_quality_micros":950000,"probes":3,"simulated_rounds":3,"parallel_penalty":{"numerator":3,"denominator":3},"no_probe":false,"attainment_auc_micros":883333,"q_auc_sim_micros":883333,"score_v2_micros":783333,"historical_usage":{"input_tokens":3,"output_tokens":3,"cost_micros":6,"latency_millis":3},"replay_cpu_nanos":535583,"coverage":{"attempted_actions":3,"observed_actions":3,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"valid","quality_micros":800000},"best_valid_ancestor_micros":800000,"recent_valid_gains_micros":[300000],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"valid","quality_micros":900000},"best_valid_ancestor_micros":900000,"recent_valid_gains_micros":[300000,100000],"repair_failures_dispatched":0},{"node_seq":3,"branch_seq":1,"search_parent_seq":2,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":3,"status":{"status":"valid","quality_micros":950000},"best_valid_ancestor_micros":950000,"recent_valid_gains_micros":[100000,50000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":3,"decisions_completed":3,"waits":[],"nodes_used":3,"recovery_dispatches_used":0}}}],"reports_body_digest":"ebeccb83aa99246b310b0abf21ae93f4d23852337a87870d6f54889b230c28b3","semantic_reports_digest":"7bfde2be36e197925b45c653a0904e5ba401578c86c4847b2d1d1dfd5b1ec5bd"}"##;
const LEGACY_CHAIN_SHA256: &str =
    "19be67813d2ef2116e10896278b8da604ef7047e7abe00b2249aecae06f9c715";
const LEGACY_CHAIN_WORLDS_SHA256: &str =
    "efa6cef6b7d18d6f327c873d8d3b7e0938ee2afef20e2afc7ccc46d238682c39";
const LEGACY_RECOVER: &str = r##"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-73814c4cab969d26cf15a0bd77a63d60c90de6993f360a90d17c3fc1c9aed60d","namespace":"ag074","purpose":"development","partition":"select","pool_digest":"464b2cc77d435e39ab590a6e0834cde21f319e27a9f31794c4d2809f7d5764e5","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":3,"horizon":3,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":17,"global_recovery_dispatch_limit":1,"pool_digest":"464b2cc77d435e39ab590a6e0834cde21f319e27a9f31794c4d2809f7d5764e5","purpose":"development","target_runtime_profile":"simulation_only"},"profile_digest":"140c0e350ff50f22df1e6f0e9062d020c23f452cbdd359895c55997261c2ea1c","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"ag074-recover-select","world_digest":"a525661e90c89e3bbb543bd9816f198b6f96f9210588e78884a87567f124efc0","report":{"schema_version":"rsia.replay_report.v2","world_id":"ag074-recover-select","world_digest":"a525661e90c89e3bbb543bd9816f198b6f96f9210588e78884a87567f124efc0","cluster_id":"cluster-ag074-recover-select","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"140c0e350ff50f22df1e6f0e9062d020c23f452cbdd359895c55997261c2ea1c","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[null]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[900000]},{"decision_round":3,"action_seqs":[3],"record_seqs":[3],"revealed_quality_micros":[950000]}],"probe_best_quality_micros":[500000,900000,950000],"round_best_quality_micros":[500000,900000,950000],"final_quality_micros":950000,"probes":3,"simulated_rounds":3,"parallel_penalty":{"numerator":3,"denominator":3},"no_probe":false,"attainment_auc_micros":783333,"q_auc_sim_micros":783333,"score_v2_micros":683333,"historical_usage":{"input_tokens":3,"output_tokens":3,"cost_micros":6,"latency_millis":3},"replay_cpu_nanos":513208,"coverage":{"attempted_actions":3,"observed_actions":3,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","initial_baseline_quality_micros":500000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":1,"status":{"status":"repairable_failure","episode_id":"episode","failure_kind":"compile","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","environment_reset":true,"dispatched_repairs":1},"best_valid_ancestor_micros":500000,"recent_valid_gains_micros":[],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":2,"status":{"status":"valid","quality_micros":900000},"best_valid_ancestor_micros":900000,"recent_valid_gains_micros":[400000],"repair_failures_dispatched":0},{"node_seq":3,"branch_seq":1,"search_parent_seq":2,"approved_parent_digest":"2687f86ed6784b8a5fca36e6c468e12aa44dc3c7e8137e3160d1a95079bdcd02","depth":3,"status":{"status":"valid","quality_micros":950000},"best_valid_ancestor_micros":950000,"recent_valid_gains_micros":[400000,50000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":3,"decisions_completed":3,"waits":[],"nodes_used":3,"recovery_dispatches_used":1}}}],"reports_body_digest":"58fb02577374911036b01353594a0f8f01dcaf12e6da68418c21c0ff1c4de742","semantic_reports_digest":"46c25f1d8a34af7fe4ce7db9affdeb190fa077f3387ea2874cfecbf24a1b7684"}"##;
const LEGACY_RECOVER_SHA256: &str =
    "0a3c74a2a998331d03e51b85bf7c90fdb9b30eae2bbc5f6ff3865be693319781";
const LEGACY_RECOVER_WORLDS_SHA256: &str =
    "b2da6ec08aff894196ebc3dfe781f9e3667ceb471794b0092fce60abcea2cb3e";
