use evo_core::evaluation::DataUse;
use evo_core::hash;
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ConsolidationClass, ElasticPolicyV1,
    ExplorationCapsV1, FailureKind, HistoryOutcome, HistoryQuery, LegalActionV1, LegalActionsV1,
    ObservedStatus, OpportunityWait, OptimizationHistoryEntry, PracticePlan, PrefixNodeV2,
    PrefixViewV2, RetryDecision, SimulationContext, SkillGroupCandidate,
    build_combined_skill_candidate, classify_consolidation_pair, decide_elastic,
    deterministic_retry_decision, select_optimization_history, validate_skill_groups,
};

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn node(seq: u32, branch: u32, quality: u32, gains: Vec<i32>) -> PrefixNodeV2 {
    PrefixNodeV2 {
        node_seq: seq,
        branch_seq: branch,
        search_parent_seq: None,
        approved_parent_digest: d("approved"),
        depth: 1,
        status: ObservedStatus::Valid {
            quality_micros: quality,
        },
        best_valid_ancestor_micros: Some(quality.max(500_000)),
        recent_valid_gains_micros: gains,
        repair_failures_dispatched: 0,
    }
}

fn prefix(nodes: Vec<PrefixNodeV2>, current: Option<u32>) -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("context-v1"),
        approved_parent_digest: d("approved"),
        initial_baseline_quality_micros: 500_000,
        nodes_used: nodes.len() as u8,
        nodes,
        current_branch_seq: current,
        current_branch_focus_actions: 0,
        decisions_completed: 0,
        waits: vec![],
        recovery_dispatches_used: 0,
    }
}

fn action(seq: u32, branch: u32, kind: ActionKindV1, cost: Option<u64>) -> LegalActionV1 {
    let target_depth = if matches!(kind, ActionKindV1::Widen { .. }) {
        1
    } else {
        2
    };
    LegalActionV1 {
        action_id: format!("opaque-{seq}"),
        action_seq: seq,
        branch_seq: branch,
        target_depth,
        kind,
        estimated_cost_upper_micros: cost,
    }
}

fn decide(prefix: &PrefixViewV2, actions: Vec<LegalActionV1>) -> BatchActionV1 {
    decide_elastic(
        &ElasticPolicyV1::default(),
        prefix,
        &LegalActionsV1 {
            schema_version: "rsia.legal_actions.v1".into(),
            actions,
        },
        &BudgetViewV1 {
            remaining_nodes: 12,
            remaining_recovery_dispatches: 4,
            remaining_root_micros: 10_000,
        },
        &ExplorationCapsV1::online(),
        SimulationContext::Online { fixed_seed: 7 },
    )
    .unwrap()
}

fn selected_seq(decision: &BatchActionV1) -> u32 {
    match decision {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs[0],
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

#[test]
fn first_root_uses_stable_reveal_order_not_opaque_id() {
    let p = prefix(vec![], None);
    let mut actions = vec![
        action(2, 2, ActionKindV1::Widen { root_slot: 2 }, Some(10)),
        action(1, 1, ActionKindV1::Widen { root_slot: 1 }, Some(10)),
    ];
    assert_eq!(selected_seq(&decide(&p, actions.clone())), 1);
    actions[0].action_id = "renamed-z".into();
    actions[1].action_id = "renamed-a".into();
    assert_eq!(selected_seq(&decide(&p, actions)), 1);
}

#[test]
fn fairness_preempts_high_quality_after_four_rounds() {
    let mut p = prefix(vec![node(1, 1, 900_000, vec![30_000])], Some(1));
    p.waits = vec![OpportunityWait {
        action_seq: 2,
        waited_rounds: 4,
    }];
    let actions = vec![
        action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, Some(10)),
        action(2, 2, ActionKindV1::Widen { root_slot: 2 }, Some(10)),
    ];
    assert_eq!(selected_seq(&decide(&p, actions)), 2);
}

#[test]
fn stagnation_and_focus_force_an_available_alternative() {
    let mut p = prefix(vec![node(1, 1, 800_000, vec![5_000, -5_000])], Some(1));
    p.current_branch_focus_actions = 2;
    let actions = vec![
        action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, Some(10)),
        action(2, 2, ActionKindV1::Widen { root_slot: 2 }, Some(10)),
    ];
    assert_eq!(selected_seq(&decide(&p, actions)), 2);
}

#[test]
fn high_value_repair_beats_low_branch_but_never_overrides_legality() {
    let ancestor = node(1, 1, 900_000, vec![0]);
    let mut middle = node(2, 1, 900_000, vec![0]);
    middle.search_parent_seq = Some(1);
    middle.depth = 2;
    let mut failed = node(3, 1, 900_000, vec![0]);
    failed.search_parent_seq = Some(2);
    failed.depth = 3;
    failed.status = ObservedStatus::RepairableFailure {
        episode_id: "episode-1".into(),
        failure_kind: FailureKind::Implementation,
        repair_template_digest: d("repair-v1"),
        environment_reset: true,
        dispatched_repairs: 0,
    };

    let p = prefix(
        vec![ancestor, middle, failed, node(4, 2, 200_000, vec![0])],
        Some(2),
    );
    let mut recovery = action(
        1,
        1,
        ActionKindV1::Recover {
            failed_node_seq: 3,
            episode_id: "episode-1".into(),
        },
        Some(10),
    );
    recovery.target_depth = 4;
    let deepen = action(2, 2, ActionKindV1::Deepen { parent_node_seq: 4 }, Some(10));
    assert_eq!(
        selected_seq(&decide(&p, vec![recovery.clone(), deepen.clone()])),
        1
    );

    let mut illegal = p.clone();
    if let ObservedStatus::RepairableFailure {
        environment_reset, ..
    } = &mut illegal.nodes[2].status
    {
        *environment_reset = false;
    }
    assert_eq!(selected_seq(&decide(&illegal, vec![recovery, deepen])), 2);
}

#[test]
fn unknown_cost_and_exhausted_budget_stop_without_free_dispatch() {
    let p = prefix(vec![node(1, 1, 500_000, vec![0])], Some(1));
    let result = decide(
        &p,
        vec![action(
            1,
            1,
            ActionKindV1::Deepen { parent_node_seq: 1 },
            None,
        )],
    );
    assert!(matches!(result, BatchActionV1::Stop { .. }));
}

#[test]
fn offline_width_is_control_plane_and_online_remains_one() {
    let p = prefix(vec![node(1, 1, 500_000, vec![0])], Some(1));
    let actions = LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, Some(10)),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, Some(10)),
        ],
    };
    let budget = BudgetViewV1 {
        remaining_nodes: 12,
        remaining_recovery_dispatches: 4,
        remaining_root_micros: 10_000,
    };
    let offline = decide_elastic(
        &ElasticPolicyV1::default(),
        &p,
        &actions,
        &budget,
        &ExplorationCapsV1::online(),
        SimulationContext::Offline {
            w_sim: 2,
            fixed_seed: 7,
        },
    )
    .unwrap();
    assert!(
        matches!(offline, BatchActionV1::Dispatch { ref action_ids, .. } if action_ids.len() == 2)
    );
    assert!(
        SimulationContext::Offline {
            w_sim: 3,
            fixed_seed: 7,
        }
        .width()
        .is_err()
    );
}

#[test]
fn batch_reserves_total_nodes_cost_and_recovery_without_mutating_prefix() {
    let p = prefix(vec![node(1, 1, 500_000, vec![0])], Some(1));
    let actions = LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, Some(60)),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, Some(60)),
        ],
    };
    let result = decide_elastic(
        &ElasticPolicyV1::default(),
        &p,
        &actions,
        &BudgetViewV1 {
            remaining_nodes: 1,
            remaining_recovery_dispatches: 0,
            remaining_root_micros: 100,
        },
        &ExplorationCapsV1::online(),
        SimulationContext::Offline {
            w_sim: 4,
            fixed_seed: 7,
        },
    )
    .unwrap();
    assert!(matches!(
        result,
        BatchActionV1::Dispatch {
            ref action_ids,
            estimated_cost_upper_micros: 60,
            ..
        } if action_ids.len() == 1
    ));
    assert_eq!(p.current_branch_focus_actions, 0);
    assert!(p.waits.is_empty());
}

#[test]
fn malformed_prefix_and_wrong_branch_actions_are_never_selected() {
    let mut malformed = prefix(vec![node(1, 1, 500_000, vec![i32::MIN])], Some(1));
    assert!(malformed.validate().is_err());
    malformed.nodes[0].recent_valid_gains_micros = vec![0];
    let wrong = action(1, 2, ActionKindV1::Deepen { parent_node_seq: 1 }, Some(1));
    assert!(matches!(
        decide(&malformed, vec![wrong]),
        BatchActionV1::Stop { .. }
    ));
}

#[test]
fn v2_root_uses_typed_parent_relationships_without_string_inequality() {
    let p = prefix(vec![], None);
    p.validate().unwrap();
}

fn history(sequence: u64, summary: &str) -> OptimizationHistoryEntry {
    OptimizationHistoryEntry {
        entry_id: format!("h{sequence}"),
        sequence,
        parent_digest: d("parent"),
        environment_digest: d("env"),
        task_family: "family".into(),
        source_watermark: 1,
        input_digest: d("input"),
        patch_digest: d("patch"),
        evidence_digest: d("evidence"),
        outcome: HistoryOutcome::CompileMismatch,
        deterministic_error: true,
        data_use: DataUse::Development,
        summary: summary.into(),
    }
}

#[test]
fn history_is_matching_recent_bounded_and_deterministic_errors_are_not_repaid() {
    let mut entries: Vec<_> = (1..=10)
        .map(|sequence| history(sequence, "small"))
        .collect();
    let mut hidden = history(11, "hidden holdout feedback");
    hidden.data_use = DataUse::AcceptanceEpoch;
    entries.push(hidden);
    let selected = select_optimization_history(
        &entries,
        HistoryQuery {
            parent_digest: &d("parent"),
            environment_digest: &d("env"),
            task_family: "family",
            source_watermark: 1,
        },
    )
    .unwrap();
    assert_eq!(selected.len(), 8);
    assert_eq!(selected.first().unwrap().sequence, 3);
    assert_eq!(
        deterministic_retry_decision(
            &entries,
            &d("parent"),
            &d("input"),
            &d("patch"),
            &d("env"),
            &d("evidence"),
        ),
        RetryDecision::ReuseDeterministicDiagnostic
    );
    assert_eq!(
        deterministic_retry_decision(
            &entries,
            &d("parent"),
            &d("input"),
            &d("patch"),
            &d("env"),
            &d("new-evidence"),
        ),
        RetryDecision::ReconsiderWithNewEvidence
    );
}

#[test]
fn practice_and_skill_groups_keep_attempts_bounded_without_inflating_n() {
    let default = PracticePlan::new("cluster-1", vec!["task-1".into()], 1, None).unwrap();
    assert_eq!(default.independent_cluster_count(), 1);
    assert!(PracticePlan::new("cluster-1", vec!["task-1".into()], 3, None).is_err());
    let expanded = PracticePlan::new(
        "cluster-1",
        vec!["task-1".into(), "task-2".into()],
        3,
        Some(d("admin-receipt")),
    )
    .unwrap();
    assert_eq!(expanded.independent_cluster_count(), 1);
    assert!(
        validate_skill_groups(&[
            SkillGroupCandidate {
                group_id: "g1".into(),
                candidate_digest: d("c1"),
            },
            SkillGroupCandidate {
                group_id: "g2".into(),
                candidate_digest: d("c2"),
            },
        ])
        .is_ok()
    );
    assert!(
        validate_skill_groups(&[
            SkillGroupCandidate {
                group_id: "g1".into(),
                candidate_digest: d("c1"),
            },
            SkillGroupCandidate {
                group_id: "g1".into(),
                candidate_digest: d("c2"),
            },
        ])
        .is_err()
    );
    let combined = build_combined_skill_candidate(vec![SkillGroupCandidate {
        group_id: "g1".into(),
        candidate_digest: d("c1"),
    }])
    .unwrap();
    assert!(combined.requires_full_development_rerun);
    assert!(combined.requires_full_formal_evaluation);
    assert_eq!(
        classify_consolidation_pair(false, true),
        ConsolidationClass::Improved
    );
}
