//! AG147 terminal semantics over plain legacy data and actual qualified V3 rows.
//! The Current fixture uses the existing controlled local Host issuer, E09
//! registration, collector, seal and full loader. It does not forge a guard,
//! rewrite old run provenance, call a Provider, or prove physical Host authority.
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::replay::*;
use evo_core::strategy::{
    ActionKindV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind, LegalActionV1,
    LegalActionsV1, ObservedStatus, PrefixViewV2, SimulationContext, decide_elastic,
    legal_candidates,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::replay::{
    LiveWorldAuthority, ReplayWorldV3Seed, collect_replay_transition_v3,
    collect_replay_world_v3_draft, prepare_replay_world_v3, run_current_replay, run_replay,
    seal_replay_world_v3,
};
use evo_storage::Store;
use evo_storage::replay::load_replay_world_v3;
use std::collections::BTreeSet;

fn d(label: &str) -> String {
    hash(label.as_bytes())
}
fn valid(q: u32) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::Valid { quality_micros: q },
    }
}
fn action(
    seq: u32,
    parent: &str,
    branch: u32,
    depth: u8,
    kind: ActionKindV1,
    cost: Option<u64>,
) -> ReplayActionSpecV1 {
    ReplayActionSpecV1 {
        record_seq: seq,
        generation_signature: d("generation"),
        parent_context_signature: parent.into(),
        branch_seq: branch,
        target_depth: depth,
        action_kind: kind,
        estimated_cost_upper_micros: cost,
        writes_shared_workspace: false,
    }
}
fn observation(
    action: &ReplayActionSpecV1,
    outcome: ReplayTransitionOutcome,
) -> ReplayTransitionV2 {
    let source = format!("observation-{}", action.record_seq);
    ReplayTransitionV2 {
        record_id: format!("opaque-{}", action.record_seq),
        record_seq: action.record_seq,
        generation_signature: action.generation_signature.clone(),
        parent_context_signature: action.parent_context_signature.clone(),
        action_kind: action.action_kind.clone(),
        next_context_signature: d(&format!("context-{}", action.record_seq)),
        outcome,
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_micros: Some(1),
            latency_millis: Some(1),
        },
        source_ids: vec![source.clone()],
        observation_source_id: source,
    }
}
fn reseal(world: &mut ReplayWorldV2) {
    world.sealed_digest = None;
    let baseline = replay_baseline_observation_digest(&world.manifest).unwrap();
    let baseline_id = world.manifest.baseline_observation_source_id.clone();
    world
        .manifest
        .source_closure
        .iter_mut()
        .find(|s| s.source_id == baseline_id)
        .unwrap()
        .content_digest = baseline;
    let observations: Vec<_> = world
        .transitions
        .iter()
        .map(|t| {
            (
                t.observation_source_id.clone(),
                replay_observation_digest(&world.manifest, t).unwrap(),
            )
        })
        .collect();
    for (id, digest) in observations {
        world
            .manifest
            .source_closure
            .iter_mut()
            .find(|s| s.source_id == id)
            .unwrap()
            .content_digest = digest;
    }
    world.seal().unwrap();
    world.validate_sealed().unwrap();
}
fn legacy(actions: Vec<ReplayActionSpecV1>, transitions: Vec<ReplayTransitionV2>) -> ReplayWorldV2 {
    let ids: BTreeSet<_> = transitions
        .iter()
        .flat_map(|t| t.source_ids.iter().cloned())
        .chain(std::iter::once("baseline-source".into()))
        .collect();
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: "legacy-world".into(),
            cluster_id: "cluster".into(),
            partition: WorldPartition::Select,
            purpose: Purpose::Development,
            generation_signature: d("generation"),
            world_context_signature: d("world-context"),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("grader"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-source".into(),
            source_closure: ids
                .into_iter()
                .map(|source_id| ReplaySourceRef {
                    source_id,
                    content_digest: d("pending"),
                })
                .collect(),
            revoke_watermark: 7,
            prefix_coverage: vec![PrefixCoverageV1 {
                context_signature: d("baseline"),
                exhausted: false,
            }],
            action_catalog: actions,
        },
        transitions,
        sealed_digest: None,
    };
    reseal(&mut world);
    world
}
fn healthy_legacy() -> ReplayWorldV2 {
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let child = action(
        2,
        &d("context-1"),
        1,
        2,
        ActionKindV1::Deepen { parent_node_seq: 1 },
        Some(10),
    );
    legacy(
        vec![root.clone(), child.clone()],
        vec![
            observation(&root, valid(400_000)),
            observation(&child, valid(800_000)),
        ],
    )
}
fn profile(budget: u8, width: u8) -> ReplaySimulationProfile {
    let mut p = ReplaySimulationProfile::default_v2(width, d("pool"));
    p.probe_budget = budget;
    p.horizon = budget;
    p.fixed_seed = 7;
    p
}
fn authority(watermark: u64) -> LiveWorldAuthority {
    LiveWorldAuthority {
        revoke_watermark: watermark,
        revoked_source_ids: BTreeSet::new(),
    }
}
fn replay(
    world: &ReplayWorldV2,
    p: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> ReplayReportV3 {
    run_replay(world, &ElasticPolicyV1::default(), p, caps, &authority(7)).unwrap()
}
fn unscored(report: &ReplayReportV3) {
    assert!(!report.coverage.complete_support);
    assert_eq!(report.attainment_auc_micros, None);
    assert_eq!(report.q_auc_sim_micros, None);
    assert_eq!(report.score_v2_micros, None);
}
fn complete(report: &ReplayReportV3) {
    assert!(report.coverage.complete_support);
    assert!(report.attainment_auc_micros.is_some());
    assert!(report.q_auc_sim_micros.is_some());
    assert!(report.score_v2_micros.is_some());
}

#[test]
fn legacy_b2_n2_remains_healthy_but_does_not_acquire_proved_roots() {
    let report = replay(
        &healthy_legacy(),
        &profile(2, 1),
        &ExplorationCapsV1::online(),
    );
    assert_eq!(report.schema_version, REPLAY_REPORT_SCHEMA_V3);
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(report.root_contract_state, ReplayRootContractState::Unknown);
    assert_eq!(report.probes, 2);
    assert_eq!(report.coverage.attempted_actions, 2);
    assert_eq!(report.probe_best_quality_micros, vec![400_000, 800_000]);
    assert_eq!(report.attainment_auc_micros, Some(600_000));
    complete(&report);
}

#[test]
fn invalid_input_precedes_revocation_and_revocation_precedes_any_probe() {
    let healthy = healthy_legacy();
    let mut broken = healthy.clone();
    broken.sealed_digest = Some("0".repeat(64));
    let revoked = authority(8);
    let invalid = run_replay(
        &broken,
        &ElasticPolicyV1::default(),
        &profile(2, 1),
        &ExplorationCapsV1::online(),
        &revoked,
    )
    .unwrap();
    assert_eq!(invalid.terminal, ReplayTerminal::InvalidWorld);
    assert_eq!(invalid.probes, 0);
    unscored(&invalid);
    for by_watermark in [false, true] {
        let mut source = authority(if by_watermark { 8 } else { 7 });
        if !by_watermark {
            source.revoked_source_ids.insert("baseline-source".into());
        }
        let report = run_replay(
            &healthy,
            &ElasticPolicyV1::default(),
            &profile(2, 1),
            &ExplorationCapsV1::online(),
            &source,
        )
        .unwrap();
        assert_eq!(report.terminal, ReplayTerminal::SourceRevoked);
        assert_eq!(report.probes, 0);
        assert_eq!(report.coverage.attempted_actions, 0);
        unscored(&report);
    }
}

#[test]
fn missing_and_explicit_unsupported_records_never_count_as_real_probes() {
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    for explicit in [false, true] {
        let records = if explicit {
            vec![observation(
                &root,
                ReplayTransitionOutcome::OutOfSupport {
                    reason: "explicit gap".into(),
                },
            )]
        } else {
            vec![]
        };
        let report = replay(
            &legacy(vec![root.clone()], records),
            &profile(2, 1),
            &ExplorationCapsV1::online(),
        );
        assert_eq!(report.terminal, ReplayTerminal::OutOfSupport);
        assert_eq!(report.probes, 0);
        assert_eq!(report.simulated_rounds, 0);
        assert!(report.probe_best_quality_micros.is_empty());
        assert_eq!(report.coverage.attempted_actions, 1);
        assert_eq!(report.coverage.out_of_support_actions, 1);
        assert!(report.batches[0].record_seqs.is_empty());
        assert_eq!(
            report.historical_usage.cost_micros,
            if explicit { Some(1) } else { None }
        );
        unscored(&report);
    }
}

#[test]
fn censored_and_usage_uncertain_each_count_once_without_a_score() {
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    for outcome in [
        ReplayTransitionOutcome::Censored {
            reason: "source censored".into(),
        },
        ReplayTransitionOutcome::Observed {
            status: ObservedStatus::UsageUncertain,
        },
    ] {
        let report = replay(
            &legacy(vec![root.clone()], vec![observation(&root, outcome)]),
            &profile(1, 1),
            &ExplorationCapsV1::online(),
        );
        assert_eq!(report.terminal, ReplayTerminal::Censored);
        assert_eq!(report.probes, 1);
        assert_eq!(report.simulated_rounds, 1);
        assert_eq!(report.coverage.attempted_actions, 1);
        assert_eq!(report.coverage.observed_actions, 0);
        assert_eq!(report.coverage.censored_actions, 1);
        assert_eq!(report.probe_best_quality_micros, vec![100_000]);
        assert_eq!(report.batches[0].record_seqs, vec![1]);
        unscored(&report);
    }
}

#[test]
fn a_whole_batch_censored_precedes_missing_explicit_oos_and_the_budget_boundary() {
    let a = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let b = action(
        2,
        &d("baseline"),
        2,
        1,
        ActionKindV1::Widen { root_slot: 2 },
        Some(10),
    );
    for second in [
        None,
        Some(ReplayTransitionOutcome::OutOfSupport {
            reason: "unsupported".into(),
        }),
        Some(valid(600_000)),
    ] {
        let has_observed = matches!(&second, Some(ReplayTransitionOutcome::Observed { .. }));
        let mut records = vec![observation(
            &a,
            ReplayTransitionOutcome::Censored {
                reason: "first-censored".into(),
            },
        )];
        if let Some(outcome) = second {
            records.push(observation(&b, outcome));
        }
        let report = replay(
            &legacy(vec![a.clone(), b.clone()], records),
            &profile(2, 2),
            &ExplorationCapsV1::online(),
        );
        assert_eq!(report.terminal, ReplayTerminal::Censored);
        assert_eq!(report.terminal_reason, "first-censored");
        assert_eq!(report.probes, if has_observed { 2 } else { 1 });
        assert_eq!(report.coverage.attempted_actions, 2);
        assert_eq!(report.coverage.censored_actions, 1);
        assert_eq!(report.coverage.observed_actions, u32::from(has_observed));
        assert_eq!(
            report.coverage.out_of_support_actions,
            u32::from(!has_observed)
        );
        assert_eq!(report.batches.len(), 1);
        assert_eq!(report.batches[0].action_seqs, vec![1, 2]);
        assert_eq!(
            report.batches[0].record_seqs,
            if has_observed { vec![1, 2] } else { vec![1] }
        );
        unscored(&report);
    }
}

#[test]
fn unknown_roots_do_not_become_complete_from_empty_catalog_exhaustion_or_depth() {
    let healthy = healthy_legacy();
    assert_eq!(
        replay(&healthy, &profile(2, 1), &ExplorationCapsV1::online()).probes,
        2
    );
    for exhausted in [false, true] {
        let mut empty = legacy(vec![], vec![]);
        empty.manifest.prefix_coverage[0].exhausted = exhausted;
        reseal(&mut empty);
        let report = replay(&empty, &profile(2, 1), &ExplorationCapsV1::online());
        assert_eq!(report.terminal, ReplayTerminal::OutOfSupport);
        assert_eq!(report.terminal_reason, "frontier_not_covered");
        assert_eq!(report.probes, 0);
        assert_eq!(report.root_contract_state, ReplayRootContractState::Unknown);
        unscored(&report);
    }
    let mut caps = ExplorationCapsV1::online();
    caps.max_depth = 1;
    let report = replay(&healthy, &profile(2, 1), &caps);
    assert_eq!(report.terminal, ReplayTerminal::OutOfSupport);
    assert_eq!(report.terminal_reason, "frontier_not_covered");
    assert_eq!(report.probes, 1);
    unscored(&report);
}

#[test]
fn historical_usage_overflow_is_typed_and_never_a_scored_report() {
    assert_eq!(
        replay(
            &healthy_legacy(),
            &profile(2, 1),
            &ExplorationCapsV1::online()
        )
        .probes,
        2
    );
    for money in [false, true] {
        let mut world = healthy_legacy();
        if money {
            world.transitions[0].actual_usage.cost_micros = Some(u64::MAX);
        } else {
            world.transitions[0].actual_usage.input_tokens = u64::MAX;
        }
        reseal(&mut world);
        let error = run_replay(
            &world,
            &ElasticPolicyV1::default(),
            &profile(2, 1),
            &ExplorationCapsV1::online(),
            &authority(7),
        )
        .unwrap_err();
        let expected = if money {
            "historical cost overflow"
        } else {
            "historical input token overflow"
        };
        assert!(matches!(error, Error::Invalid(ref message) if message == expected));
    }
}

fn empty_prefix() -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("world-context"),
        approved_parent_digest: d("parent"),
        initial_baseline_quality_micros: 100_000,
        nodes: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decisions_completed: 0,
        waits: vec![],
        nodes_used: 0,
        recovery_dispatches_used: 0,
    }
}
#[test]
fn the_shared_core_typed_guards_reject_forged_duplicate_actions_and_bad_prefix() {
    let prefix = empty_prefix();
    let caps = ExplorationCapsV1::online();
    let budget = BudgetViewV1 {
        remaining_nodes: 2,
        remaining_recovery_dispatches: 1,
        remaining_root_micros: 100,
    };
    let legal = LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: vec![LegalActionV1 {
            action_id: "action-1".into(),
            action_seq: 1,
            branch_seq: 1,
            target_depth: 1,
            kind: ActionKindV1::Widen { root_slot: 1 },
            estimated_cost_upper_micros: Some(10),
        }],
    };
    assert_eq!(
        legal_candidates(&prefix, &legal, &budget, &caps)
            .unwrap()
            .len(),
        1
    );
    let mut duplicate = legal.clone();
    duplicate.actions.push(legal.actions[0].clone());
    assert!(matches!(
        legal_candidates(&prefix, &duplicate, &budget, &caps),
        Err(Error::Invalid(_))
    ));
    let mut forged = legal.clone();
    forged.actions[0].action_id.clear();
    assert!(matches!(
        legal_candidates(&prefix, &forged, &budget, &caps),
        Err(Error::Invalid(_))
    ));
    let mut bad_prefix = prefix.clone();
    bad_prefix.nodes_used = 1;
    assert!(matches!(
        decide_elastic(
            &ElasticPolicyV1::default(),
            &bad_prefix,
            &legal,
            &budget,
            &caps,
            SimulationContext::Offline {
                w_sim: 1,
                fixed_seed: 7
            }
        ),
        Err(Error::Invalid(_))
    ));
}

// Current semantic fixtures reuse the existing issuer/E09 path. Every source is
// a new controlled fixture source; no historical bytes or labels are altered.
struct CurrentFixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    host: Context,
    registered: ExplorationWorldV1,
}
async fn issue(f: &CurrentFixture, id: &str, body: Vec<u8>) {
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.clone(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(&body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: String::from_utf8(body.clone()).unwrap(),
            seed: 7,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(&f.store, &f.host, &authority)
        .await
        .unwrap();
}
async fn current_fixture(two_roots: bool) -> CurrentFixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("terminal.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("terminal-ns", "admin", Role::Admin).unwrap();
    let host = Context::new("terminal-ns", "host", Role::Host).unwrap();
    let mut registered = ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: "terminal-registered".into(),
        approved_parent_digest: d("parent"),
        context_signature: String::new(),
        parent_skill_digest: d("skill"),
        parent_bundle_digest: d("bundle"),
        environment_digest: d("environment"),
        model_digest: d("model"),
        tools_digest: d("tools"),
        grader_digest: d("grader"),
        rules_digest: d("rules"),
        source_watermark: 1,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        root_opportunities: (1..=if two_roots { 2 } else { 1 })
            .map(|seq| RootOpportunity {
                root_slot: seq,
                branch_seq: seq,
                action_seq: seq,
                estimated_cost_upper_micros: 10,
            })
            .collect(),
        dependencies: vec![ExplorationDependency {
            kind: "run".into(),
            id: "initial-input".into(),
        }],
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 100_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 1,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    };
    registered.context_signature = fingerprint(&(
        &registered.parent_skill_digest,
        &registered.parent_bundle_digest,
        &registered.environment_digest,
        &registered.model_digest,
        &registered.tools_digest,
        &registered.grader_digest,
        &registered.rules_digest,
        registered.source_watermark,
    ))
    .unwrap();
    let f = CurrentFixture {
        _dir: dir,
        store,
        admin,
        host,
        registered,
    };
    issue(
        &f,
        "initial-input",
        b"controlled source for actual E09 registration".to_vec(),
    )
    .await;
    let mut session = f.store.session().await.unwrap();
    session
        .bump_watermark(&f.host, "initial-source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    PersistentCoordinator::new(f.store.clone(), f.admin.clone(), "admin")
        .unwrap()
        .register_world(f.registered.clone())
        .await
        .unwrap();
    f
}
async fn current_draft(f: &CurrentFixture, baseline_exhausted: bool) -> ReplayWorldV3 {
    let world = prepare_replay_world_v3(
        &f.admin,
        &f.store,
        &f.registered.id,
        ReplayWorldV3Seed {
            cluster_id: "cluster".into(),
            partition: WorldPartition::Select,
            generation_signature: d("generation"),
            world_context_signature: f.registered.context_signature.clone(),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("grader"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-source".into(),
            prefix_coverage: vec![PrefixCoverageV1 {
                context_signature: d("baseline"),
                exhausted: baseline_exhausted,
            }],
            action_catalog: vec![],
        },
    )
    .await
    .unwrap();
    issue(
        f,
        "baseline-source",
        replay_baseline_observation_bytes_v2(&world.manifest).unwrap(),
    )
    .await;
    collect_replay_world_v3_draft(&f.admin, &f.store, &world)
        .await
        .unwrap();
    world
}
async fn append(
    f: &CurrentFixture,
    world: &ReplayWorldV3,
    action: &ReplayActionSpecV1,
    outcome: ReplayTransitionOutcome,
) -> ReplayWorldV3 {
    let transition = observation(action, outcome);
    issue(
        f,
        &transition.observation_source_id,
        replay_observation_bytes_v3(&world.manifest, &transition).unwrap(),
    )
    .await;
    collect_replay_transition_v3(
        &f.admin,
        &f.store,
        &world.manifest.world_id,
        action,
        &transition,
    )
    .await
    .unwrap()
}
async fn evaluate(
    f: &CurrentFixture,
    world: &ReplayWorldV3,
    p: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> ReplayReportV3 {
    seal_replay_world_v3(&f.admin, &f.store, &world.manifest.world_id)
        .await
        .unwrap();
    f.store.close().await;
    let reopened = Store::open(&f._dir.path().join("terminal.sqlite3"))
        .await
        .unwrap();
    let reader = Context::new("terminal-ns", "reader", Role::Worker).unwrap();
    let loaded = load_replay_world_v3(
        &reader,
        &reopened,
        &world.manifest.world_id,
        Purpose::Development,
    )
    .await
    .unwrap();
    let mut session = reopened.session().await.unwrap();
    loaded
        .revalidate_in_session(&mut session, &reader)
        .await
        .unwrap();
    let watermark = u64::try_from(session.watermark(&reader).await.unwrap().unwrap().0).unwrap();
    let report = run_current_replay(
        &loaded,
        &ElasticPolicyV1::default(),
        p,
        caps,
        &authority(watermark),
    )
    .unwrap();
    session.commit().await.unwrap();
    reopened.close().await;
    assert_eq!(
        report.root_contract_state,
        ReplayRootContractState::ProvedClosed
    );
    report
}

#[tokio::test]
async fn proved_unused_roots_win_before_balance_and_recovery_exclusions() {
    for recovery in [false, true] {
        for exhausted in [false, true] {
            let f = current_fixture(true).await;
            let world = current_draft(&f, exhausted).await;
            let root = action(
                1,
                &d("baseline"),
                1,
                1,
                ActionKindV1::Widen { root_slot: 1 },
                Some(10),
            );
            let status = if recovery {
                ReplayTransitionOutcome::Observed {
                    status: ObservedStatus::RepairableFailure {
                        episode_id: "episode-root".into(),
                        failure_kind: FailureKind::Compile,
                        repair_template_digest: d("repair"),
                        environment_reset: true,
                        dispatched_repairs: 0,
                    },
                }
            } else {
                valid(400_000)
            };
            let mut world = append(&f, &world, &root, status).await;
            let kind = if recovery {
                ActionKindV1::Recover {
                    failed_node_seq: 1,
                    episode_id: "episode-root".into(),
                }
            } else {
                ActionKindV1::Deepen { parent_node_seq: 1 }
            };
            world.manifest.action_catalog.push(action(
                3,
                &d("context-1"),
                1,
                2,
                kind,
                Some(if recovery { 10 } else { 1_000 }),
            ));
            collect_replay_world_v3_draft(&f.admin, &f.store, &world)
                .await
                .unwrap();
            let mut p = profile(2, 1);
            if recovery {
                p.global_recovery_dispatch_limit = 0;
            }
            let report = evaluate(&f, &world, &p, &ExplorationCapsV1::online()).await;
            assert_eq!(report.probes, 1);
            assert_eq!(
                report.terminal,
                if exhausted {
                    ReplayTerminal::WorldExhausted
                } else {
                    ReplayTerminal::OutOfSupport
                }
            );
            assert_eq!(
                report.terminal_reason,
                if exhausted {
                    "logged_frontier_exhausted"
                } else {
                    "frontier_not_covered"
                }
            );
            unscored(&report);
            f.store.close().await;
        }
    }
}

#[tokio::test]
async fn any_uncovered_revealed_node_precedes_an_exhausted_unused_root() {
    let f = current_fixture(true).await;
    let world = current_draft(&f, true).await;
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let world = append(&f, &world, &root, valid(400_000)).await;
    let report = evaluate(&f, &world, &profile(2, 1), &ExplorationCapsV1::online()).await;
    assert_eq!(report.probes, 1);
    assert_eq!(report.terminal, ReplayTerminal::OutOfSupport);
    assert_eq!(report.terminal_reason, "frontier_not_covered");
    unscored(&report);
    f.store.close().await;
}

#[tokio::test]
async fn proved_revealed_node_missing_record_uses_its_own_exhaustion_flag() {
    for exhausted in [false, true] {
        let f = current_fixture(false).await;
        let world = current_draft(&f, false).await;
        let root = action(
            1,
            &d("baseline"),
            1,
            1,
            ActionKindV1::Widen { root_slot: 1 },
            Some(10),
        );
        let mut world = append(&f, &world, &root, valid(400_000)).await;
        world.manifest.prefix_coverage.push(PrefixCoverageV1 {
            context_signature: d("context-1"),
            exhausted,
        });
        collect_replay_world_v3_draft(&f.admin, &f.store, &world)
            .await
            .unwrap();
        let report = evaluate(&f, &world, &profile(2, 1), &ExplorationCapsV1::online()).await;
        assert_eq!(
            report.terminal,
            if exhausted {
                ReplayTerminal::WorldExhausted
            } else {
                ReplayTerminal::OutOfSupport
            }
        );
        assert_eq!(
            report.terminal_reason,
            if exhausted {
                "logged_frontier_exhausted"
            } else {
                "frontier_not_covered"
            }
        );
        unscored(&report);
        f.store.close().await;
    }
}

#[tokio::test]
async fn policy_stop_scores_only_when_every_proved_frontier_record_is_present_or_closed() {
    for case in [
        "depth",
        "no-cost",
        "balance",
        "recovery",
        "no-reset",
        "structure",
    ] {
        let f = current_fixture(false).await;
        let world = current_draft(&f, false).await;
        let outcome = match case {
            "recovery" | "no-reset" => ReplayTransitionOutcome::Observed {
                status: ObservedStatus::RepairableFailure {
                    episode_id: "episode-root".into(),
                    failure_kind: FailureKind::Compile,
                    repair_template_digest: d("repair"),
                    environment_reset: case != "no-reset",
                    dispatched_repairs: 0,
                },
            },
            "structure" => ReplayTransitionOutcome::Observed {
                status: ObservedStatus::HardFailure,
            },
            _ => valid(400_000),
        };
        let root = action(
            1,
            &d("baseline"),
            1,
            1,
            ActionKindV1::Widen { root_slot: 1 },
            Some(10),
        );
        let mut world = append(&f, &world, &root, outcome).await;
        let kind = if case == "recovery" || case == "no-reset" {
            ActionKindV1::Recover {
                failed_node_seq: 1,
                episode_id: "episode-root".into(),
            }
        } else {
            ActionKindV1::Deepen { parent_node_seq: 1 }
        };
        let cost = match case {
            "no-cost" => None,
            "balance" => Some(1_000),
            _ => Some(10),
        };
        world
            .manifest
            .action_catalog
            .push(action(2, &d("context-1"), 1, 2, kind, cost));
        collect_replay_world_v3_draft(&f.admin, &f.store, &world)
            .await
            .unwrap();
        let mut caps = ExplorationCapsV1::online();
        if case == "depth" {
            caps.max_depth = 1;
        }
        let mut p = profile(2, 1);
        if case == "recovery" {
            p.global_recovery_dispatch_limit = 0;
        }
        let report = evaluate(&f, &world, &p, &caps).await;
        assert_eq!(report.terminal, ReplayTerminal::PolicyStop, "{case}");
        assert_eq!(
            report.terminal_reason,
            "no_legal_action_with_authorized_cost"
        );
        assert_eq!(report.probes, 1);
        complete(&report);
        let counts = &report.policy_stop_exclusions;
        assert_eq!(counts.depth_limit, u32::from(case == "depth"));
        assert_eq!(counts.node_limit, 0);
        assert_eq!(
            counts.no_authorized_cost_bound,
            u32::from(case == "no-cost")
        );
        assert_eq!(counts.insufficient_balance, u32::from(case == "balance"));
        assert_eq!(
            counts.recovery_ineligible,
            u32::from(case == "recovery" || case == "no-reset")
        );
        assert_eq!(
            counts.structurally_unavailable,
            u32::from(case == "structure")
        );
        f.store.close().await;
    }
}

#[tokio::test]
async fn diagnostic_first_hit_does_not_count_one_record_in_multiple_categories() {
    let f = current_fixture(false).await;
    let world = current_draft(&f, false).await;
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let mut world = append(
        &f,
        &world,
        &root,
        ReplayTransitionOutcome::Observed {
            status: ObservedStatus::HardFailure,
        },
    )
    .await;
    // This record is past depth, lacks a cost bound, and cannot Deepen its failed parent.
    world.manifest.action_catalog.push(action(
        2,
        &d("context-1"),
        1,
        2,
        ActionKindV1::Deepen { parent_node_seq: 1 },
        None,
    ));
    collect_replay_world_v3_draft(&f.admin, &f.store, &world)
        .await
        .unwrap();
    let mut caps = ExplorationCapsV1::online();
    caps.max_depth = 1;
    let report = evaluate(&f, &world, &profile(2, 1), &caps).await;
    assert_eq!(report.terminal, ReplayTerminal::PolicyStop);
    complete(&report);
    let c = &report.policy_stop_exclusions;
    assert_eq!(
        (
            c.depth_limit,
            c.node_limit,
            c.no_authorized_cost_bound,
            c.insufficient_balance,
            c.recovery_ineligible,
            c.structurally_unavailable
        ),
        (1, 0, 0, 0, 0, 0)
    );
    f.store.close().await;
}

#[tokio::test]
async fn qualified_current_b2_n2_remains_healthy_after_real_close_and_reopen() {
    let f = current_fixture(false).await;
    let world = current_draft(&f, false).await;
    let root = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let world = append(&f, &world, &root, valid(400_000)).await;
    let child = action(
        2,
        &d("context-1"),
        1,
        2,
        ActionKindV1::Deepen { parent_node_seq: 1 },
        Some(10),
    );
    let world = append(&f, &world, &child, valid(800_000)).await;
    let report = evaluate(&f, &world, &profile(2, 1), &ExplorationCapsV1::online()).await;
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(report.probes, 2);
    assert_eq!(report.coverage.attempted_actions, 2);
    assert_eq!(report.coverage.observed_actions, 2);
    assert_eq!(report.coverage.censored_actions, 0);
    assert_eq!(
        report
            .batches
            .iter()
            .flat_map(|b| b.record_seqs.iter().copied())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(report.probe_best_quality_micros, vec![400_000, 800_000]);
    assert_eq!(report.attainment_auc_micros, Some(600_000));
    complete(&report);
}

#[tokio::test]
async fn qualified_current_batch_censored_precedes_oos_after_real_close_and_reopen() {
    let f = current_fixture(true).await;
    let world = current_draft(&f, false).await;
    let first = action(
        1,
        &d("baseline"),
        1,
        1,
        ActionKindV1::Widen { root_slot: 1 },
        Some(10),
    );
    let world = append(
        &f,
        &world,
        &first,
        ReplayTransitionOutcome::Censored {
            reason: "current-censored".into(),
        },
    )
    .await;
    let second = action(
        2,
        &d("baseline"),
        2,
        1,
        ActionKindV1::Widen { root_slot: 2 },
        Some(10),
    );
    let world = append(
        &f,
        &world,
        &second,
        ReplayTransitionOutcome::OutOfSupport {
            reason: "current-unsupported".into(),
        },
    )
    .await;
    let report = evaluate(&f, &world, &profile(2, 2), &ExplorationCapsV1::online()).await;
    assert_eq!(report.terminal, ReplayTerminal::Censored);
    assert_eq!(report.terminal_reason, "current-censored");
    assert_eq!(report.probes, 1);
    assert_eq!(report.coverage.attempted_actions, 2);
    assert_eq!(report.coverage.observed_actions, 0);
    assert_eq!(report.coverage.censored_actions, 1);
    assert_eq!(report.coverage.out_of_support_actions, 1);
    assert_eq!(report.batches[0].action_seqs, vec![1, 2]);
    assert_eq!(report.batches[0].record_seqs, vec![1]);
    unscored(&report);
}
