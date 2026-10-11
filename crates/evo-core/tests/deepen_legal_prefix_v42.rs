//! Normal Deepen legality over observed prefixes, through the original public API.
use evo_core::hash;
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind,
    LegalActionV1, LegalActionsV1, ObservedStatus, OpportunityWait, PrefixNodeV2, PrefixViewV2,
    SimulationContext, decide_elastic,
};

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn valid(quality_micros: u32) -> ObservedStatus {
    ObservedStatus::Valid { quality_micros }
}

fn node(
    seq: u32,
    branch: u32,
    parent: Option<u32>,
    depth: u8,
    status: ObservedStatus,
) -> PrefixNodeV2 {
    let best = match status {
        ObservedStatus::Valid { quality_micros } => quality_micros.max(800_000),
        _ => 800_000,
    };
    let mut gains = Vec::new();
    if parent.is_some() {
        gains.push(0);
    }
    if let ObservedStatus::Valid { quality_micros } = status {
        gains.push(quality_micros as i32 - 800_000);
    }
    PrefixNodeV2 {
        node_seq: seq,
        branch_seq: branch,
        search_parent_seq: parent,
        approved_parent_digest: d("approved"),
        depth,
        status,
        best_valid_ancestor_micros: Some(best),
        recent_valid_gains_micros: gains,
        repair_failures_dispatched: 0,
    }
}

fn prefix(nodes: Vec<PrefixNodeV2>) -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("context"),
        approved_parent_digest: d("approved"),
        initial_baseline_quality_micros: 800_000,
        nodes_used: nodes.len().try_into().unwrap(),
        nodes,
        current_branch_seq: Some(1),
        current_branch_focus_actions: 0,
        decisions_completed: 2,
        waits: Vec::new(),
        recovery_dispatches_used: 0,
    }
}

fn deepen(seq: u32, parent: u32, branch: u32, depth: u8) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("opaque-{seq}"),
        action_seq: seq,
        branch_seq: branch,
        target_depth: depth,
        kind: ActionKindV1::Deepen {
            parent_node_seq: parent,
        },
        estimated_cost_upper_micros: Some(10),
    }
}

fn decide(prefix: &PrefixViewV2, actions: Vec<LegalActionV1>, width: u8) -> BatchActionV1 {
    prefix.validate().unwrap();
    decide_elastic(
        &ElasticPolicyV1::default(),
        prefix,
        &LegalActionsV1 {
            schema_version: "rsia.legal_actions.v1".into(),
            actions,
        },
        &BudgetViewV1 {
            remaining_nodes: 12 - prefix.nodes_used,
            remaining_recovery_dispatches: 1,
            remaining_root_micros: 1000,
        },
        &ExplorationCapsV1::online(),
        SimulationContext::Offline {
            w_sim: width,
            fixed_seed: 17,
        },
    )
    .unwrap()
}

#[test]
fn a_valid_observed_child_consumes_its_parents_normal_deepen_opportunity() {
    let p = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, valid(900_000)),
    ]);
    let decision = decide(&p, vec![deepen(3, 1, 1, 2)], 1);
    println!("validated two-round prefix; same parent under a new action sequence: {decision:?}");
    assert!(matches!(decision, BatchActionV1::Stop { .. }));
}

#[test]
fn a_hard_failed_observed_child_does_not_return_its_valid_parents_opportunity() {
    let p = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, ObservedStatus::HardFailure),
    ]);
    let decision = decide(&p, vec![deepen(30, 1, 1, 2)], 1);
    println!(
        "validated failed-child prefix; valid parent with another action sequence: {decision:?}"
    );
    assert!(matches!(decision, BatchActionV1::Stop { .. }));
}

#[test]
fn every_observed_child_status_consumes_the_normal_parent_opportunity() {
    for status in [
        valid(900_000),
        ObservedStatus::RepairableFailure {
            episode_id: "episode".into(),
            failure_kind: FailureKind::Compile,
            repair_template_digest: d("repair"),
            environment_reset: true,
            dispatched_repairs: 0,
        },
        ObservedStatus::EnvironmentFailure,
        ObservedStatus::HardFailure,
        ObservedStatus::SafetyRejected,
        ObservedStatus::Cancelled,
        ObservedStatus::UsageUncertain,
    ] {
        let p = prefix(vec![
            node(1, 1, None, 1, valid(800_000)),
            node(2, 1, Some(1), 2, status),
        ]);
        for width in [1, 2, 4] {
            assert!(matches!(
                decide(&p, vec![deepen(3, 1, 1, 2)], width),
                BatchActionV1::Stop { .. }
            ));
        }
    }
}

fn selected(decision: BatchActionV1) -> Vec<u32> {
    match decision {
        BatchActionV1::Dispatch { action_seqs, .. } => action_seqs,
        other => panic!("expected a legal dispatch: {other:?}"),
    }
}

#[test]
fn fairness_cannot_reopen_an_expanded_parent_while_another_branch_remains_legal() {
    let mut p = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, valid(900_000)),
        node(3, 2, None, 1, valid(800_000)),
    ]);
    p.waits.push(OpportunityWait {
        action_seq: 40,
        waited_rounds: 100,
    });
    let before = serde_json::to_vec(&p).unwrap();
    for width in [1, 2, 4] {
        assert_eq!(
            selected(decide(
                &p,
                vec![deepen(40, 1, 1, 2), deepen(41, 3, 2, 2)],
                width
            )),
            vec![41]
        );
    }
    assert_eq!(serde_json::to_vec(&p).unwrap(), before);
}

#[test]
fn a_valid_child_and_an_unexpanded_root_keep_their_own_opportunities() {
    let child_prefix = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, valid(900_000)),
    ]);
    assert_eq!(
        selected(decide(
            &child_prefix,
            vec![deepen(3, 1, 1, 2), deepen(4, 2, 1, 3)],
            1
        )),
        vec![4]
    );
    let no_child = prefix(vec![node(1, 1, None, 1, valid(800_000))]);
    assert_eq!(
        selected(decide(&no_child, vec![deepen(3, 1, 1, 2)], 1)),
        vec![3]
    );
}

#[test]
fn widths_preserve_branch_deduplication_and_the_observed_parent_barrier() {
    let p = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, valid(900_000)),
        node(3, 2, None, 1, valid(800_000)),
        node(4, 3, None, 1, valid(800_000)),
        node(5, 4, None, 1, valid(800_000)),
    ]);
    let actions = vec![
        deepen(10, 1, 1, 2), // Already expanded, even though the parent is still valid.
        deepen(11, 2, 1, 3),
        deepen(12, 2, 1, 3), // Same branch cannot fill another slot in this batch.
        deepen(13, 3, 2, 2),
        deepen(14, 4, 3, 2),
        deepen(15, 5, 4, 2),
        deepen(16, 11, 1, 4), // A selected action is not yet an observed parent.
    ];
    let before = serde_json::to_vec(&p).unwrap();
    for (width, expected) in [(1, vec![11]), (2, vec![11, 13]), (4, vec![11, 13, 14, 15])] {
        assert_eq!(selected(decide(&p, actions.clone(), width)), expected);
        assert_eq!(serde_json::to_vec(&p).unwrap(), before);
    }
}

#[test]
fn action_order_and_opaque_identity_do_not_change_observed_prefix_selection() {
    let p = prefix(vec![
        node(1, 1, None, 1, valid(800_000)),
        node(2, 1, Some(1), 2, valid(900_000)),
        node(3, 2, None, 1, valid(800_000)),
    ]);
    let actions = vec![
        deepen(10, 1, 1, 2),
        deepen(11, 2, 1, 3),
        deepen(12, 3, 2, 2),
    ];
    let mut renamed = actions.clone();
    renamed.reverse();
    for action in &mut renamed {
        action.action_id = format!("unrelated-audit-id-{}", action.action_seq);
    }
    for width in [1, 2, 4] {
        assert_eq!(
            selected(decide(&p, actions.clone(), width)),
            selected(decide(&p, renamed.clone(), width))
        );
    }
}
