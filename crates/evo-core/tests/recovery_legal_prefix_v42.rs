//! Recovery eligibility and batch slot accounting on a trusted, observed prefix.
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind,
    LegalActionV1, LegalActionsV1, ObservedStatus, OpportunityWait, PrefixNodeV2, PrefixViewV2,
    SimulationContext, decide_elastic,
};
use evo_core::{fingerprint, hash};

fn failure(seq: u32, episode: &str) -> PrefixNodeV2 {
    PrefixNodeV2 {
        node_seq: seq,
        branch_seq: seq,
        search_parent_seq: None,
        approved_parent_digest: hash(b"approved"),
        depth: 1,
        status: ObservedStatus::RepairableFailure {
            episode_id: episode.into(),
            failure_kind: FailureKind::Compile,
            repair_template_digest: hash(b"repair"),
            environment_reset: true,
            dispatched_repairs: 0,
        },
        best_valid_ancestor_micros: Some(500_000),
        recent_valid_gains_micros: vec![],
        repair_failures_dispatched: 0,
    }
}

fn prefix(nodes: Vec<PrefixNodeV2>) -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: hash(b"context"),
        approved_parent_digest: hash(b"approved"),
        initial_baseline_quality_micros: 500_000,
        nodes_used: nodes.len() as u8,
        nodes,
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decisions_completed: 5,
        waits: vec![],
        recovery_dispatches_used: 0,
    }
}

fn recover(seq: u32, target: u32, episode: &str) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("recover-{seq}"),
        action_seq: seq,
        branch_seq: target,
        target_depth: 2,
        kind: ActionKindV1::Recover {
            failed_node_seq: target,
            episode_id: episode.into(),
        },
        estimated_cost_upper_micros: Some(10),
    }
}

fn budget() -> BudgetViewV1 {
    BudgetViewV1 {
        remaining_nodes: 8,
        remaining_recovery_dispatches: 3,
        remaining_root_micros: 100,
    }
}

fn decide(
    p: &PrefixViewV2,
    actions: Vec<LegalActionV1>,
    b: &BudgetViewV1,
    caps: &ExplorationCapsV1,
    width: u8,
) -> BatchActionV1 {
    decide_elastic(
        &ElasticPolicyV1::default(),
        p,
        &LegalActionsV1 {
            schema_version: "rsia.legal_actions.v1".into(),
            actions,
        },
        b,
        caps,
        SimulationContext::Offline {
            w_sim: width,
            fixed_seed: 17,
        },
    )
    .unwrap()
}

fn dispatched(decision: BatchActionV1) -> (Vec<u32>, u64) {
    match decision {
        BatchActionV1::Dispatch {
            action_seqs,
            estimated_cost_upper_micros,
            ..
        } => (action_seqs, estimated_cost_upper_micros),
        other => panic!("expected dispatch, got {other:?}"),
    }
}

#[test]
fn failed_repair_line_is_closed_even_when_its_episode_is_renamed() {
    for episode in ["same", "renamed"] {
        let mut root = failure(1, "same");
        if let ObservedStatus::RepairableFailure {
            dispatched_repairs, ..
        } = &mut root.status
        {
            *dispatched_repairs = 1;
        }
        let mut child = failure(2, episode);
        child.branch_seq = 1;
        child.search_parent_seq = Some(1);
        child.depth = 2;
        child.repair_failures_dispatched = 1;
        let mut p = prefix(vec![root, child]);
        p.recovery_dispatches_used = 1;
        p.validate().unwrap();
        let mut action = recover(3, 2, episode);
        action.branch_seq = 1;
        action.target_depth = 3;
        assert!(matches!(
            decide(&p, vec![action], &budget(), &ExplorationCapsV1::online(), 1),
            BatchActionV1::Stop { .. }
        ));
    }
}

#[test]
fn batch_keeps_the_fair_first_repair_and_skips_its_duplicate_episode() {
    let mut p = prefix(vec![
        failure(1, "same"),
        failure(2, "same"),
        failure(3, "other"),
    ]);
    p.waits = vec![OpportunityWait {
        action_seq: 12,
        waited_rounds: 5,
    }];
    let before = fingerprint(&p).unwrap();
    let mut ordinary = recover(14, 4, "unused");
    ordinary.kind = ActionKindV1::Widen { root_slot: 4 };
    ordinary.target_depth = 1;
    ordinary.estimated_cost_upper_micros = Some(20);
    let actions = vec![
        recover(11, 1, "same"),
        recover(12, 2, "same"),
        recover(13, 3, "other"),
        ordinary,
    ];
    for (width, expected, cost) in [
        (1, vec![12], 10),
        (2, vec![12, 13], 20),
        (4, vec![12, 13, 14], 40),
    ] {
        let (seqs, total) = dispatched(decide(
            &p,
            actions.clone(),
            &budget(),
            &ExplorationCapsV1::online(),
            width,
        ));
        assert_eq!(seqs, expected);
        assert_eq!(total, cost);
        assert_eq!(fingerprint(&p).unwrap(), before);
    }
    let mut renamed = actions.clone();
    for (index, action) in renamed.iter_mut().enumerate() {
        action.action_id = format!("opaque-renamed-{}", 100 - index);
    }
    assert_eq!(
        dispatched(decide(
            &p,
            renamed,
            &budget(),
            &ExplorationCapsV1::online(),
            4
        ))
        .0,
        vec![12, 13, 14]
    );
}

#[test]
fn batch_preserves_total_cost_node_and_global_recovery_limits() {
    let p = prefix(vec![
        failure(1, "same"),
        failure(2, "same"),
        failure(3, "other"),
    ]);
    let actions = vec![
        recover(11, 1, "same"),
        recover(12, 2, "same"),
        recover(13, 3, "other"),
    ];
    for (nodes, money, global, expected) in [
        (1, 100, 3, vec![11]),
        (8, 10, 3, vec![11]),
        (8, 100, 1, vec![11]),
        (8, 100, 2, vec![11, 13]),
    ] {
        let b = BudgetViewV1 {
            remaining_nodes: nodes,
            remaining_root_micros: money,
            remaining_recovery_dispatches: global,
        };
        let (seqs, cost) = dispatched(decide(
            &p,
            actions.clone(),
            &b,
            &ExplorationCapsV1::online(),
            4,
        ));
        assert_eq!(seqs, expected);
        assert_eq!(cost, seqs.len() as u64 * 10);
    }
    let caps = ExplorationCapsV1 {
        max_nodes: 4,
        ..ExplorationCapsV1::online()
    };
    assert_eq!(
        dispatched(decide(&p, actions, &budget(), &caps, 4)).0.len(),
        1
    );
}

#[test]
fn repair_legality_still_precedes_value_and_fairness() {
    let base = prefix(vec![failure(1, "same")]);
    for case in 0..8 {
        let mut p = base.clone();
        p.waits = vec![OpportunityWait {
            action_seq: 11,
            waited_rounds: 9,
        }];
        let mut a = recover(11, 1, "same");
        let mut b = budget();
        let mut caps = ExplorationCapsV1::online();
        match case {
            0 => b.remaining_recovery_dispatches = 0,
            1 => caps.max_repair_dispatches_per_episode = 0,
            2 => b.remaining_nodes = 0,
            3 => b.remaining_root_micros = 9,
            4 => a.estimated_cost_upper_micros = None,
            5 => a.target_depth = 5,
            6 => {
                if let ObservedStatus::RepairableFailure {
                    environment_reset, ..
                } = &mut p.nodes[0].status
                {
                    *environment_reset = false;
                }
            }
            _ => {
                if let ObservedStatus::RepairableFailure {
                    dispatched_repairs, ..
                } = &mut p.nodes[0].status
                {
                    *dispatched_repairs = 1;
                }
            }
        }
        assert!(
            matches!(
                decide(&p, vec![a], &b, &caps, 4),
                BatchActionV1::Stop { .. }
            ),
            "case {case}"
        );
    }
}
