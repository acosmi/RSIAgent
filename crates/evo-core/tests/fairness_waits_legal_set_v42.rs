//! AG-103 (E09/E10; plan §7.1.1): the legal opportunities of a decision round are one
//! core filter, `legal_candidates`, which the decision ranks and over which online
//! exploration and replay move their fairness waits. An action core excludes (past the
//! depth cap, with no node left under the budget or the node cap, without an authorized
//! cost bound or over the remaining root budget, a Recover that is not eligible, a
//! successor whose structure does not fit the prefix) is no opportunity: it has no say
//! in the decision, so moving the filter out of `decide_elastic` changed no decision.
//!
//! The decisions in `the_decision_is_what_it_was_before_the_filter_was_shared` are
//! pinned value for value. Synthetic prefixes and actions; no model, store or provider.
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, FailureKind,
    LegalActionV1, LegalActionsV1, ObservedStatus, OpportunityWait, PrefixNodeV2, PrefixViewV2,
    SimulationContext, decide_elastic, legal_candidates,
};
use evo_core::{Result, hash};
use serde_json::{Value, json};

const BASELINE: u32 = 500_000;
const COST: u64 = 10;
const STOP_REASON: &str = "no_legal_action_with_authorized_cost";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn valid(quality_micros: u32) -> ObservedStatus {
    ObservedStatus::Valid { quality_micros }
}

fn repairable(episode: &str, environment_reset: bool, dispatched_repairs: u8) -> ObservedStatus {
    ObservedStatus::RepairableFailure {
        episode_id: episode.into(),
        failure_kind: FailureKind::Compile,
        repair_template_digest: d("repair"),
        environment_reset,
        dispatched_repairs,
    }
}

/// A revealed node, with the best valid ancestor the prefix check derives: its parent's
/// (the baseline for a root), raised to its own quality when it is valid.
fn node(
    seq: u32,
    branch: u32,
    parent: Option<&PrefixNodeV2>,
    status: ObservedStatus,
    gains: Vec<i32>,
) -> PrefixNodeV2 {
    let inherited = parent
        .and_then(|parent| parent.best_valid_ancestor_micros)
        .unwrap_or(BASELINE);
    let best = match status {
        ObservedStatus::Valid { quality_micros } => inherited.max(quality_micros),
        _ => inherited,
    };
    PrefixNodeV2 {
        node_seq: seq,
        branch_seq: branch,
        search_parent_seq: parent.map(|parent| parent.node_seq),
        approved_parent_digest: d("approved"),
        depth: parent.map_or(1, |parent| parent.depth + 1),
        status,
        best_valid_ancestor_micros: Some(best),
        recent_valid_gains_micros: gains,
        repair_failures_dispatched: 0,
    }
}

fn prefix(nodes: Vec<PrefixNodeV2>, current_branch: Option<u32>) -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("context"),
        approved_parent_digest: d("approved"),
        initial_baseline_quality_micros: BASELINE,
        nodes_used: nodes.len() as u8,
        nodes,
        current_branch_seq: current_branch,
        current_branch_focus_actions: u8::from(current_branch.is_some()),
        decisions_completed: 0,
        waits: Vec::new(),
        recovery_dispatches_used: 0,
    }
}

fn widen(seq: u32, branch: u32, cost: Option<u64>) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("widen-{seq}"),
        action_seq: seq,
        branch_seq: branch,
        target_depth: 1,
        kind: ActionKindV1::Widen { root_slot: branch },
        estimated_cost_upper_micros: cost,
    }
}

fn deepen(seq: u32, parent: &PrefixNodeV2) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("deepen-{seq}"),
        action_seq: seq,
        branch_seq: parent.branch_seq,
        target_depth: parent.depth + 1,
        kind: ActionKindV1::Deepen {
            parent_node_seq: parent.node_seq,
        },
        estimated_cost_upper_micros: Some(COST),
    }
}

fn recover(seq: u32, failed: &PrefixNodeV2, episode: &str) -> LegalActionV1 {
    LegalActionV1 {
        action_id: format!("recover-{seq}"),
        action_seq: seq,
        branch_seq: failed.branch_seq,
        target_depth: failed.depth + 1,
        kind: ActionKindV1::Recover {
            failed_node_seq: failed.node_seq,
            episode_id: episode.into(),
        },
        estimated_cost_upper_micros: Some(COST),
    }
}

fn offer(actions: Vec<LegalActionV1>) -> LegalActionsV1 {
    LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions,
    }
}

fn budget() -> BudgetViewV1 {
    BudgetViewV1 {
        remaining_nodes: 8,
        remaining_recovery_dispatches: 1,
        remaining_root_micros: 1_000,
    }
}

fn caps() -> ExplorationCapsV1 {
    ExplorationCapsV1::online()
}

/// One valid root (600000, a gain of 100000 over the baseline), the current branch.
fn one_valid_root() -> (PrefixViewV2, PrefixNodeV2) {
    let root = node(1, 1, None, valid(600_000), vec![100_000]);
    (prefix(vec![root.clone()], Some(1)), root)
}

/// The valid root above and a second root on branch 2 that ended in `status`.
fn valid_and_failed_roots(status: ObservedStatus) -> (PrefixViewV2, PrefixNodeV2, PrefixNodeV2) {
    let good = node(1, 1, None, valid(600_000), vec![100_000]);
    let failed = node(2, 2, None, status, vec![]);
    (
        prefix(vec![good.clone(), failed.clone()], Some(1)),
        good,
        failed,
    )
}

/// One decision round: what is offered on which prefix, under which budget and caps, and
/// the actions core leaves legal, by `action_seq` in the order offered.
struct Round {
    name: &'static str,
    prefix: PrefixViewV2,
    actions: Vec<LegalActionV1>,
    budget: BudgetViewV1,
    caps: ExplorationCapsV1,
    legal: Vec<u32>,
}

fn round(
    name: &'static str,
    prefix: PrefixViewV2,
    actions: Vec<LegalActionV1>,
    legal: &[u32],
) -> Round {
    Round {
        name,
        prefix,
        actions,
        budget: budget(),
        caps: caps(),
        legal: legal.to_vec(),
    }
}

/// Every reason core has to exclude an action, each next to an action it keeps, and the
/// controls that keep everything.
fn rounds() -> Vec<Round> {
    let mut rounds = Vec::new();

    let (p, root) = one_valid_root();
    rounds.push(round(
        "every_offered_action_is_legal",
        p,
        vec![deepen(3, &root), widen(2, 2, Some(COST))],
        &[3, 2],
    ));
    rounds.push(round(
        "an_unknown_cost_is_no_authorized_bound",
        prefix(vec![], None),
        vec![widen(1, 1, None), widen(2, 2, Some(COST))],
        &[2],
    ));
    rounds.push(round(
        "a_cost_over_the_remaining_root_budget",
        prefix(vec![], None),
        vec![widen(1, 1, Some(1_001)), widen(2, 2, Some(1_000))],
        &[2],
    ));

    let (p, root) = one_valid_root();
    let mut shallow = round(
        "past_the_depth_cap",
        p,
        vec![deepen(3, &root), widen(2, 2, Some(COST))],
        &[2],
    );
    shallow.caps.max_depth = 1;
    rounds.push(shallow);

    let (p, root) = one_valid_root();
    let mut spent = round(
        "no_node_left_in_the_budget",
        p,
        vec![deepen(3, &root), widen(2, 2, Some(COST))],
        &[],
    );
    spent.budget.remaining_nodes = 0;
    rounds.push(spent);

    let (p, root) = one_valid_root();
    let mut full = round(
        "the_node_cap_is_reached",
        p,
        vec![deepen(3, &root), widen(2, 2, Some(COST))],
        &[],
    );
    full.caps.max_nodes = 1;
    rounds.push(full);

    let (p, good, failed) = valid_and_failed_roots(repairable("episode-2", true, 0));
    rounds.push(round(
        "a_recover_with_a_recovery_dispatch_left",
        p.clone(),
        vec![recover(5, &failed, "episode-2"), deepen(3, &good)],
        &[5, 3],
    ));
    let mut no_dispatch = round(
        "a_recover_without_a_recovery_dispatch_left",
        p.clone(),
        vec![recover(5, &failed, "episode-2"), deepen(3, &good)],
        &[3],
    );
    no_dispatch.budget.remaining_recovery_dispatches = 0;
    rounds.push(no_dispatch);
    rounds.push(round(
        "a_recover_of_another_episode",
        p.clone(),
        vec![recover(5, &failed, "episode-3"), deepen(3, &good)],
        &[3],
    ));
    let mut other_branch = recover(5, &failed, "episode-2");
    other_branch.branch_seq = 1;
    let mut other_depth = recover(6, &failed, "episode-2");
    other_depth.target_depth = 3;
    let mut unrevealed = recover(7, &failed, "episode-2");
    unrevealed.kind = ActionKindV1::Recover {
        failed_node_seq: 9,
        episode_id: "episode-2".into(),
    };
    rounds.push(round(
        "a_recover_on_another_branch_depth_or_node",
        p,
        vec![other_branch, other_depth, unrevealed, deepen(3, &good)],
        &[3],
    ));

    let (p, good, failed) = valid_and_failed_roots(repairable("episode-2", false, 0));
    rounds.push(round(
        "a_recover_whose_environment_was_not_reset",
        p,
        vec![recover(5, &failed, "episode-2"), deepen(3, &good)],
        &[3],
    ));
    let (p, good, failed) = valid_and_failed_roots(repairable("episode-2", true, 1));
    rounds.push(round(
        "a_recover_of_an_episode_at_its_repair_cap",
        p,
        vec![recover(5, &failed, "episode-2"), deepen(3, &good)],
        &[3],
    ));

    // A repair that failed again closes its line, whatever its episode is called.
    let first = node(1, 1, None, repairable("episode-1", true, 1), vec![]);
    let mut repair = node(2, 1, Some(&first), repairable("episode-2", true, 0), vec![]);
    repair.repair_failures_dispatched = 1;
    rounds.push(round(
        "a_recover_of_a_line_whose_repair_failed",
        prefix(vec![first, repair.clone()], Some(1)),
        vec![recover(5, &repair, "episode-2"), widen(9, 9, Some(COST))],
        &[9],
    ));

    let (p, root) = one_valid_root();
    rounds.push(round(
        "a_recover_of_a_valid_node",
        p,
        vec![recover(5, &root, "episode-1"), widen(2, 2, Some(COST))],
        &[2],
    ));

    let (p, root) = one_valid_root();
    let mut low = widen(2, 2, Some(COST));
    low.target_depth = 2;
    rounds.push(round(
        "a_widen_of_an_opened_branch_or_below_the_root",
        p,
        vec![widen(4, 1, Some(COST)), low, deepen(3, &root)],
        &[3],
    ));

    let (p, good, failed) = valid_and_failed_roots(ObservedStatus::HardFailure);
    rounds.push(round(
        "a_deepen_of_a_failed_parent",
        p,
        vec![deepen(4, &failed), deepen(3, &good)],
        &[3],
    ));

    let root = node(1, 1, None, valid(600_000), vec![100_000]);
    let child = node(2, 1, Some(&root), valid(700_000), vec![100_000, 100_000]);
    rounds.push(round(
        "a_deepen_of_an_expanded_parent",
        prefix(vec![root.clone(), child.clone()], Some(1)),
        vec![deepen(3, &root), deepen(4, &child)],
        &[4],
    ));

    let (p, root) = one_valid_root();
    let mut other_branch = deepen(3, &root);
    other_branch.branch_seq = 2;
    let mut other_depth = deepen(4, &root);
    other_depth.target_depth = 3;
    let mut unrevealed = deepen(5, &root);
    unrevealed.kind = ActionKindV1::Deepen { parent_node_seq: 8 };
    rounds.push(round(
        "a_deepen_on_another_branch_depth_or_parent",
        p,
        vec![other_branch, other_depth, unrevealed, widen(2, 2, Some(COST))],
        &[2],
    ));

    rounds.push(round(
        "a_zero_sequence_or_branch",
        prefix(vec![], None),
        vec![
            widen(0, 1, Some(COST)),
            widen(2, 0, Some(COST)),
            widen(3, 3, Some(COST)),
        ],
        &[3],
    ));
    rounds
}

fn kept_seqs(round: &Round) -> Vec<u32> {
    legal_candidates(
        &round.prefix,
        &offer(round.actions.clone()),
        &round.budget,
        &round.caps,
    )
    .unwrap()
    .iter()
    .map(|action| action.action_seq)
    .collect()
}

fn decide_on(
    prefix: &PrefixViewV2,
    legal: &LegalActionsV1,
    budget: &BudgetViewV1,
    caps: &ExplorationCapsV1,
    simulation: SimulationContext,
) -> Result<BatchActionV1> {
    decide_elastic(
        &ElasticPolicyV1::default(),
        prefix,
        legal,
        budget,
        caps,
        simulation,
    )
}

const SIMULATIONS: [SimulationContext; 4] = [
    SimulationContext::Online { fixed_seed: 7 },
    SimulationContext::Offline {
        w_sim: 1,
        fixed_seed: 17,
    },
    SimulationContext::Offline {
        w_sim: 2,
        fixed_seed: 17,
    },
    SimulationContext::Offline {
        w_sim: 4,
        fixed_seed: 17,
    },
];

#[test]
fn the_shared_filter_keeps_exactly_the_actions_core_leaves_legal() {
    for round in rounds() {
        // Every prefix is one the decision accepts.
        round.prefix.validate().unwrap();
        assert_eq!(kept_seqs(&round), round.legal, "{}", round.name);
    }
}

#[test]
fn an_action_core_excludes_has_no_say_in_the_decision() {
    for round in rounds() {
        let offered = offer(round.actions.clone());
        let kept = offer(
            legal_candidates(&round.prefix, &offered, &round.budget, &round.caps)
                .unwrap()
                .into_iter()
                .cloned()
                .collect(),
        );
        for simulation in SIMULATIONS {
            let all = decide_on(
                &round.prefix,
                &offered,
                &round.budget,
                &round.caps,
                simulation,
            )
            .unwrap();
            let only_kept = decide_on(
                &round.prefix,
                &kept,
                &round.budget,
                &round.caps,
                simulation,
            )
            .unwrap();
            // Taking every excluded action out of the offer leaves the decision as it is.
            assert_eq!(
                serde_json::to_value(&all).unwrap(),
                serde_json::to_value(&only_kept).unwrap(),
                "{} {simulation:?}",
                round.name
            );
            match all {
                BatchActionV1::Stop { reason } => {
                    assert!(round.legal.is_empty(), "{}: stopped", round.name);
                    assert_eq!(reason, STOP_REASON, "{}", round.name);
                }
                BatchActionV1::Dispatch { action_seqs, .. } => assert!(
                    action_seqs.iter().all(|seq| round.legal.contains(seq)),
                    "{}: dispatched {action_seqs:?} outside {:?}",
                    round.name,
                    round.legal
                ),
            }
        }
    }
}

#[test]
fn an_excluded_action_never_waits_its_way_into_fair_scheduling() {
    // Whatever wait an excluded action carries, it is neither a candidate nor chosen;
    // the legal opportunity that waited the four rounds is.
    let root = node(1, 1, None, valid(900_000), vec![400_000]);
    let mut p = prefix(vec![root.clone()], Some(1));
    p.waits = vec![
        OpportunityWait {
            action_seq: 2,
            waited_rounds: 9,
        },
        OpportunityWait {
            action_seq: 4,
            waited_rounds: 4,
        },
    ];
    let offered = offer(vec![
        deepen(3, &root),
        widen(2, 2, None),
        widen(4, 4, Some(COST)),
    ]);
    let kept: Vec<u32> = legal_candidates(&p, &offered, &budget(), &caps())
        .unwrap()
        .iter()
        .map(|action| action.action_seq)
        .collect();
    assert_eq!(kept, vec![3, 4]);
    for simulation in SIMULATIONS {
        match decide_on(&p, &offered, &budget(), &caps(), simulation).unwrap() {
            BatchActionV1::Dispatch {
                action_seqs,
                reason,
                ..
            } => {
                assert_eq!(action_seqs[0], 4, "{simulation:?}");
                assert!(!action_seqs.contains(&2), "{simulation:?}");
                assert_eq!(reason, "fairness_wait_threshold", "{simulation:?}");
            }
            stop => panic!("expected a dispatch, got {stop:?}"),
        }
    }
}

#[test]
fn the_shared_filter_refuses_what_the_decision_refuses() {
    let (p, root) = one_valid_root();
    let mut wrong_schema = offer(vec![deepen(3, &root)]);
    wrong_schema.schema_version = "rsia.legal_actions.v0".into();
    let mut malformed = widen(5, 5, Some(COST));
    malformed.action_id = "not an identifier".into();
    let mut same_id = widen(4, 4, Some(COST));
    same_id.action_id = "deepen-3".into();
    let same_seq = widen(3, 4, Some(COST));
    let refused = [
        (wrong_schema, "invalid input: unsupported legal action schema"),
        (
            // Identities are checked on every offered action, an excluded one too.
            offer(vec![widen(2, 2, None), malformed]),
            "invalid input: invalid identifier",
        ),
        (
            offer(vec![deepen(3, &root), same_id]),
            "invalid input: duplicate legal action identity",
        ),
        (
            offer(vec![deepen(3, &root), same_seq]),
            "invalid input: duplicate legal action identity",
        ),
    ];
    for (offered, message) in refused {
        let filtered = legal_candidates(&p, &offered, &budget(), &caps())
            .err()
            .map(|error| error.to_string());
        let decided = decide_on(&p, &offered, &budget(), &caps(), SIMULATIONS[0])
            .err()
            .map(|error| error.to_string());
        assert_eq!(filtered.as_deref(), Some(message));
        assert_eq!(decided.as_deref(), Some(message));
    }
}

/// A decision of `decide_elastic` as it serializes.
fn decided(
    prefix: &PrefixViewV2,
    actions: Vec<LegalActionV1>,
    budget: &BudgetViewV1,
    caps: &ExplorationCapsV1,
    simulation: SimulationContext,
) -> Value {
    serde_json::to_value(decide_on(prefix, &offer(actions), budget, caps, simulation).unwrap())
        .unwrap()
}

fn dispatched(ids: &[&str], seqs: &[u32], reason: &str, cost: u64) -> Value {
    json!({
        "decision": "dispatch",
        "action_ids": ids,
        "action_seqs": seqs,
        "reason": reason,
        "estimated_cost_upper_micros": cost,
    })
}

#[test]
fn the_decision_is_what_it_was_before_the_filter_was_shared() {
    let online = SIMULATIONS[0];

    // The first root is the first legal one in the fixed reveal order: an unknown-cost
    // root ahead of it is skipped.
    assert_eq!(
        decided(
            &prefix(vec![], None),
            vec![widen(1, 1, None), widen(2, 2, Some(COST))],
            &budget(),
            &caps(),
            online,
        ),
        dispatched(&["widen-2"], &[2], "first_preregistered_root", COST)
    );

    // A root over the remaining budget leaves one legal action.
    let (p, root) = one_valid_root();
    assert_eq!(
        decided(
            &p,
            vec![deepen(3, &root), widen(2, 2, Some(1_001))],
            &budget(),
            &caps(),
            online,
        ),
        dispatched(&["deepen-3"], &[3], "only_legal_authorized_action", COST)
    );

    // Nothing legal: a deepening past the depth cap and an unknown-cost root.
    let mut shallow = caps();
    shallow.max_depth = 1;
    assert_eq!(
        decided(
            &p,
            vec![deepen(3, &root), widen(2, 2, None)],
            &budget(),
            &shallow,
            online,
        ),
        json!({"decision": "stop", "reason": STOP_REASON})
    );

    // An excluded action's long wait preempts nothing; the legal one that waited does.
    let strong = node(1, 1, None, valid(900_000), vec![400_000]);
    let mut waiting = prefix(vec![strong.clone()], Some(1));
    waiting.waits = vec![
        OpportunityWait {
            action_seq: 2,
            waited_rounds: 9,
        },
        OpportunityWait {
            action_seq: 4,
            waited_rounds: 4,
        },
    ];
    assert_eq!(
        decided(
            &waiting,
            vec![
                deepen(3, &strong),
                widen(2, 2, None),
                widen(4, 4, Some(COST))
            ],
            &budget(),
            &caps(),
            online,
        ),
        dispatched(&["widen-4"], &[4], "fairness_wait_threshold", COST)
    );

    // An offline batch fills its slots from the legal candidates only.
    assert_eq!(
        decided(
            &p,
            vec![
                deepen(3, &root),
                widen(2, 2, None),
                widen(4, 4, Some(COST))
            ],
            &budget(),
            &caps(),
            SIMULATIONS[3],
        ),
        dispatched(
            &["deepen-3", "widen-4"],
            &[3, 4],
            "elastic_priority",
            2 * COST
        )
    );

    // The Recover of a failure below a strong ancestor wins while a recovery dispatch is
    // left, and yields to the only other legal action when none is.
    let strong = node(1, 1, None, valid(900_000), vec![400_000]);
    let failed = node(
        2,
        1,
        Some(&strong),
        repairable("episode-2", true, 0),
        vec![400_000],
    );
    let p = prefix(vec![strong, failed.clone()], Some(1));
    let actions = vec![recover(5, &failed, "episode-2"), widen(2, 2, Some(COST))];
    assert_eq!(
        decided(&p, actions.clone(), &budget(), &caps(), online),
        dispatched(&["recover-5"], &[5], "elastic_priority", COST)
    );
    let mut no_recovery = budget();
    no_recovery.remaining_recovery_dispatches = 0;
    assert_eq!(
        decided(&p, actions, &no_recovery, &caps(), online),
        dispatched(&["widen-2"], &[2], "only_legal_authorized_action", COST)
    );
}
