//! E14 increment 1 (plan §5.2, §9, §9.1): the restricted single-mechanism
//! ExplorationPolicy content, its bounded parameters, and the field contract
//! that registers its consumer. V086.c (a policy parameter alone changes the
//! allocation while the administrator caps stay 12/4/1 with W_online = 1),
//! V092.c (unknown parameters, other mechanisms and control-plane values are
//! refused) and V094.c (the policy is not part of a world's compatibility
//! signature) are covered at the structure/program level only.
use evo_core::contract::{FIELD_CONTRACTS, FieldOwner, assert_candidate_may_write, field_contract};
use evo_core::improver::{
    IMPROVER_CONTENT_MAX_BYTES, IMPROVER_CONTENT_V2, ImproverContentV2, ImproverMechanismV2,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ELASTIC_FAIRNESS_WAIT_ROUNDS_MAX,
    ELASTIC_FAIRNESS_WAIT_ROUNDS_MIN, ELASTIC_MAX_FOCUS_ACTIONS_MAX, ELASTIC_MAX_FOCUS_ACTIONS_MIN,
    ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS, ELASTIC_SIGNIFICANT_GAIN_MIN_MICROS,
    ELASTIC_STAGNATION_ABS_GAIN_MAX_MICROS, ELASTIC_STAGNATION_ABS_GAIN_MIN_MICROS,
    ELASTIC_STAGNATION_WINDOW_MAX, ELASTIC_STAGNATION_WINDOW_MIN, ElasticPolicyV1,
    ExplorationCapsV1, LegalActionV1, LegalActionsV1, MAX_DEPTH, MAX_NODES, MAX_REPAIR,
    ObservedStatus, OpportunityWait, PREFIX_RECENT_GAINS_MAX, PrefixNodeV2, PrefixViewV2,
    SimulationContext, decide_elastic,
};
use evo_core::{ArtifactKind, Error, fingerprint, hash};
use serde_json::{Value, json};

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn policy_with(change: impl FnOnce(&mut ElasticPolicyV1)) -> ElasticPolicyV1 {
    let mut policy = ElasticPolicyV1::default();
    change(&mut policy);
    policy
}

/// The error must be `Invalid` and name both the field and its interval.
fn assert_out_of_range(policy: &ElasticPolicyV1, field: &str, interval: &str) {
    match policy.validate() {
        Err(Error::Invalid(message)) => {
            assert!(message.contains(field), "{message} must name {field}");
            assert!(
                message.contains(interval),
                "{message} must name the interval {interval}"
            );
        }
        other => panic!("expected an out-of-range Invalid for {field}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1. Bounded validation
// ---------------------------------------------------------------------------

#[test]
fn default_policy_is_valid_and_bounds_reference_the_hard_limits() {
    ElasticPolicyV1::default().validate().unwrap();
    // Bounds that have a hard counterpart reference it instead of a literal.
    assert_eq!(ELASTIC_MAX_FOCUS_ACTIONS_MAX, MAX_DEPTH);
    assert_eq!(ELASTIC_FAIRNESS_WAIT_ROUNDS_MAX, MAX_NODES - 1);
    assert_eq!(ELASTIC_STAGNATION_WINDOW_MAX, PREFIX_RECENT_GAINS_MAX);
    assert_eq!(
        (MAX_NODES, MAX_DEPTH, MAX_REPAIR),
        (12, 4, 1),
        "the administrator caps are not a policy field"
    );
}

#[test]
fn stagnation_window_bound_is_what_the_prefix_actually_retains() {
    // `PrefixViewV2` keeps at most two recent gains, so a longer window could
    // never be satisfied (it would silently disable stagnation).
    let node = |gains: Vec<i32>| PrefixNodeV2 {
        node_seq: 1,
        branch_seq: 1,
        search_parent_seq: None,
        approved_parent_digest: d("approved"),
        depth: 1,
        status: ObservedStatus::Valid {
            quality_micros: 600_000,
        },
        best_valid_ancestor_micros: Some(600_000),
        recent_valid_gains_micros: gains,
        repair_failures_dispatched: 0,
    };
    let view = |gains: Vec<i32>| PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("context"),
        approved_parent_digest: d("approved"),
        initial_baseline_quality_micros: 500_000,
        nodes: vec![node(gains)],
        current_branch_seq: Some(1),
        current_branch_focus_actions: 1,
        decisions_completed: 1,
        waits: vec![],
        nodes_used: 1,
        recovery_dispatches_used: 0,
    };
    view(vec![1; usize::from(PREFIX_RECENT_GAINS_MAX)])
        .validate()
        .unwrap();
    assert!(
        view(vec![1; usize::from(PREFIX_RECENT_GAINS_MAX) + 1])
            .validate()
            .is_err()
    );
}

#[test]
fn every_field_accepts_both_bounds_and_rejects_one_step_outside() {
    // significant_gain_micros [1_000, 100_000]; the stagnation threshold must
    // stay below it, so the lower-bound case lowers that threshold as well.
    policy_with(|p| {
        p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MIN_MICROS;
        p.stagnation_abs_gain_micros = 0;
    })
    .validate()
    .unwrap();
    policy_with(|p| p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS)
        .validate()
        .unwrap();
    assert_out_of_range(
        &policy_with(|p| {
            p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MIN_MICROS - 1;
            p.stagnation_abs_gain_micros = 0;
        }),
        "significant_gain_micros",
        "[1000, 100000]",
    );
    assert_out_of_range(
        &policy_with(|p| p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS + 1),
        "significant_gain_micros",
        "[1000, 100000]",
    );

    // stagnation_abs_gain_micros [0, 50_000], below significant_gain_micros.
    policy_with(|p| p.stagnation_abs_gain_micros = ELASTIC_STAGNATION_ABS_GAIN_MIN_MICROS)
        .validate()
        .unwrap();
    policy_with(|p| {
        p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS;
        p.stagnation_abs_gain_micros = ELASTIC_STAGNATION_ABS_GAIN_MAX_MICROS;
    })
    .validate()
    .unwrap();
    assert_out_of_range(
        &policy_with(|p| p.stagnation_abs_gain_micros = ELASTIC_STAGNATION_ABS_GAIN_MIN_MICROS - 1),
        "stagnation_abs_gain_micros",
        "[0, 50000]",
    );
    assert_out_of_range(
        &policy_with(|p| {
            p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS;
            p.stagnation_abs_gain_micros = ELASTIC_STAGNATION_ABS_GAIN_MAX_MICROS + 1;
        }),
        "stagnation_abs_gain_micros",
        "[0, 50000]",
    );

    // stagnation_window [1, 2]: 0 is the degenerate "always stagnated" value.
    for window in [ELASTIC_STAGNATION_WINDOW_MIN, ELASTIC_STAGNATION_WINDOW_MAX] {
        policy_with(|p| p.stagnation_window = window)
            .validate()
            .unwrap();
    }
    assert_out_of_range(
        &policy_with(|p| p.stagnation_window = ELASTIC_STAGNATION_WINDOW_MIN - 1),
        "stagnation_window",
        "[1, 2]",
    );
    assert_out_of_range(
        &policy_with(|p| p.stagnation_window = ELASTIC_STAGNATION_WINDOW_MAX + 1),
        "stagnation_window",
        "[1, 2]",
    );

    // max_focus_actions [1, MAX_DEPTH = 4].
    for focus in [ELASTIC_MAX_FOCUS_ACTIONS_MIN, ELASTIC_MAX_FOCUS_ACTIONS_MAX] {
        policy_with(|p| p.max_focus_actions = focus)
            .validate()
            .unwrap();
    }
    assert_out_of_range(
        &policy_with(|p| p.max_focus_actions = ELASTIC_MAX_FOCUS_ACTIONS_MIN - 1),
        "max_focus_actions",
        "[1, 4]",
    );
    assert_out_of_range(
        &policy_with(|p| p.max_focus_actions = ELASTIC_MAX_FOCUS_ACTIONS_MAX + 1),
        "max_focus_actions",
        "[1, 4]",
    );

    // fairness_wait_rounds [1, MAX_NODES - 1 = 11].
    for rounds in [
        ELASTIC_FAIRNESS_WAIT_ROUNDS_MIN,
        ELASTIC_FAIRNESS_WAIT_ROUNDS_MAX,
    ] {
        policy_with(|p| p.fairness_wait_rounds = rounds)
            .validate()
            .unwrap();
    }
    assert_out_of_range(
        &policy_with(|p| p.fairness_wait_rounds = ELASTIC_FAIRNESS_WAIT_ROUNDS_MIN - 1),
        "fairness_wait_rounds",
        "[1, 11]",
    );
    assert_out_of_range(
        &policy_with(|p| p.fairness_wait_rounds = ELASTIC_FAIRNESS_WAIT_ROUNDS_MAX + 1),
        "fairness_wait_rounds",
        "[1, 11]",
    );
}

#[test]
fn stagnation_threshold_must_stay_below_the_significant_gain() {
    for (significant, stagnation) in [(10_000, 10_000), (20_000, 30_000), (1_000, 1_000)] {
        let policy = policy_with(|p| {
            p.significant_gain_micros = significant;
            p.stagnation_abs_gain_micros = stagnation;
        });
        match policy.validate() {
            Err(Error::Invalid(message)) => {
                assert!(message.contains("stagnation_abs_gain_micros"), "{message}");
                assert!(message.contains("significant_gain_micros"), "{message}");
            }
            other => panic!("expected Invalid for {significant}/{stagnation}, got {other:?}"),
        }
    }
    policy_with(|p| {
        p.significant_gain_micros = 10_000;
        p.stagnation_abs_gain_micros = 9_999;
    })
    .validate()
    .unwrap();
}

#[test]
fn wrong_schema_version_is_rejected() {
    let policy = policy_with(|p| p.schema_version = "rsia.elastic_priority.v2".into());
    assert!(matches!(policy.validate(), Err(Error::Invalid(_))));
}

#[test]
fn control_plane_keys_cannot_ride_in_a_policy_document() {
    let base = serde_json::to_value(ElasticPolicyV1::default()).unwrap();
    let parsed: ElasticPolicyV1 = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(
        parsed.digest().unwrap(),
        ElasticPolicyV1::default().digest().unwrap()
    );
    // The caps, the online width and the simulation context all belong to the
    // control plane; a policy document carrying any of them is not a policy.
    for (key, value) in [
        ("max_nodes", json!(12)),
        ("max_depth", json!(4)),
        ("max_repair_dispatches_per_episode", json!(1)),
        ("w_online", json!(1)),
        ("w_sim", json!(4)),
        ("simulation", json!({"online": {"fixed_seed": 7}})),
        ("caps", json!({"max_nodes": 12})),
    ] {
        let mut document = base.clone();
        document[key] = value;
        assert!(
            serde_json::from_value::<ElasticPolicyV1>(document).is_err(),
            "{key} must not deserialize into a policy"
        );
    }
}

#[test]
fn policy_and_caps_digests_follow_the_fingerprint_convention() {
    let policy = ElasticPolicyV1::default();
    assert_eq!(policy.digest().unwrap(), fingerprint(&policy).unwrap());
    let caps = ExplorationCapsV1::online();
    assert_eq!(caps.digest().unwrap(), fingerprint(&caps).unwrap());
    let changed = policy_with(|p| p.max_focus_actions = 1);
    assert_ne!(changed.digest().unwrap(), policy.digest().unwrap());
    // A serialization round trip keeps the digest.
    let round_trip: ElasticPolicyV1 =
        serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
    assert_eq!(round_trip.digest().unwrap(), changed.digest().unwrap());
}

// ---------------------------------------------------------------------------
// 2. V086.c: a single in-bound parameter changes the allocation; caps do not
// ---------------------------------------------------------------------------

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

fn prefix(nodes: Vec<PrefixNodeV2>, current: u32, focus: u8) -> PrefixViewV2 {
    PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: d("context-v42"),
        approved_parent_digest: d("approved"),
        initial_baseline_quality_micros: 500_000,
        nodes_used: nodes.len() as u8,
        nodes,
        current_branch_seq: Some(current),
        current_branch_focus_actions: focus,
        decisions_completed: 1,
        waits: vec![],
        recovery_dispatches_used: 0,
    }
}

fn action(seq: u32, branch: u32, kind: ActionKindV1, id: &str) -> LegalActionV1 {
    let target_depth = if matches!(kind, ActionKindV1::Widen { .. }) {
        1
    } else {
        2
    };
    LegalActionV1 {
        action_id: id.into(),
        action_seq: seq,
        branch_seq: branch,
        target_depth,
        kind,
        estimated_cost_upper_micros: Some(10),
    }
}

fn budget() -> BudgetViewV1 {
    BudgetViewV1 {
        remaining_nodes: 12,
        remaining_recovery_dispatches: 4,
        remaining_root_micros: 10_000,
    }
}

fn decide_with(
    policy: &ElasticPolicyV1,
    prefix: &PrefixViewV2,
    actions: &[LegalActionV1],
) -> BatchActionV1 {
    decide_elastic(
        policy,
        prefix,
        &LegalActionsV1 {
            schema_version: "rsia.legal_actions.v1".into(),
            actions: actions.to_vec(),
        },
        &budget(),
        &ExplorationCapsV1::online(),
        SimulationContext::Online { fixed_seed: 7 },
    )
    .unwrap()
}

fn selected(decision: &BatchActionV1) -> u32 {
    match decision {
        BatchActionV1::Dispatch {
            action_seqs,
            action_ids,
            ..
        } => {
            assert_eq!(
                action_seqs.len(),
                1,
                "W_online = 1: one action per decision"
            );
            assert_eq!(action_ids.len(), 1);
            action_seqs[0]
        }
        BatchActionV1::Stop { reason } => panic!("unexpected stop: {reason}"),
    }
}

/// Changing exactly one parameter, with the same caps and the same revealed
/// trajectory, must change the selected action. Renaming the opaque action ids
/// must not change the selection under either policy.
fn assert_single_parameter_changes_the_allocation(
    label: &str,
    changed: &ElasticPolicyV1,
    prefix: &PrefixViewV2,
    actions: Vec<LegalActionV1>,
    expected_default: u32,
    expected_changed: u32,
) {
    let default = ElasticPolicyV1::default();
    changed.validate().unwrap();
    let caps_before = ExplorationCapsV1::online().digest().unwrap();
    let under_default = selected(&decide_with(&default, prefix, &actions));
    let under_changed = selected(&decide_with(changed, prefix, &actions));
    assert_eq!(under_default, expected_default, "{label}: default policy");
    assert_eq!(under_changed, expected_changed, "{label}: changed policy");
    assert_ne!(under_default, under_changed, "{label}: the decision moved");
    // Only the policy differs: same caps value and digest, still 12/4/1 with
    // W_online = 1.
    let caps = ExplorationCapsV1::online();
    assert_eq!(caps.digest().unwrap(), caps_before);
    assert_eq!(
        (
            caps.max_nodes,
            caps.max_depth,
            caps.max_repair_dispatches_per_episode,
            caps.w_online
        ),
        (12, 4, 1, 1),
        "{label}"
    );
    assert_ne!(
        changed.digest().unwrap(),
        default.digest().unwrap(),
        "{label}: the two policies are different mechanisms"
    );
    // Opaque identity never participates in the choice.
    let renamed: Vec<_> = actions
        .iter()
        .enumerate()
        .map(|(index, action)| LegalActionV1 {
            action_id: format!("renamed-zzz-{}", 99 - index),
            ..action.clone()
        })
        .collect();
    assert_eq!(
        selected(&decide_with(&default, prefix, &renamed)),
        expected_default
    );
    assert_eq!(
        selected(&decide_with(changed, prefix, &renamed)),
        expected_changed
    );
}

#[test]
fn max_focus_actions_alone_changes_focus_versus_switch() {
    // One focused action already spent on branch 1, whose deepening is the
    // top-ranked action. The default allows one more; max_focus_actions = 1
    // forces the switch to the alternative root.
    let revealed = prefix(vec![node(1, 1, 800_000, vec![0])], 1, 1);
    assert_single_parameter_changes_the_allocation(
        "max_focus_actions 2 -> 1",
        &policy_with(|p| p.max_focus_actions = 1),
        &revealed,
        vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, "a"),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, "b"),
        ],
        1,
        2,
    );
}

#[test]
fn significant_gain_alone_changes_the_harvest_deepening_preference() {
    // Branch 1 just gained 25_000 but is ranked below branch 2's deepening.
    // A 20_000 threshold harvests branch 1; a 30_000 one does not.
    let revealed = prefix(
        vec![
            node(1, 1, 700_000, vec![25_000]),
            node(2, 2, 740_000, vec![0]),
        ],
        1,
        1,
    );
    assert_single_parameter_changes_the_allocation(
        "significant_gain_micros 20_000 -> 30_000",
        &policy_with(|p| p.significant_gain_micros = 30_000),
        &revealed,
        vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, "a"),
            action(2, 2, ActionKindV1::Deepen { parent_node_seq: 2 }, "b"),
        ],
        1,
        2,
    );
}

#[test]
fn stagnation_window_alone_changes_when_a_stalled_branch_is_left() {
    // A single recent gain within the stagnation threshold: not stagnated for a
    // window of two observations, stagnated for a window of one.
    let revealed = prefix(vec![node(1, 1, 800_000, vec![5_000])], 1, 1);
    assert_single_parameter_changes_the_allocation(
        "stagnation_window 2 -> 1",
        &policy_with(|p| p.stagnation_window = 1),
        &revealed,
        vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, "a"),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, "b"),
        ],
        1,
        2,
    );
}

#[test]
fn stagnation_abs_gain_alone_changes_what_counts_as_stalled() {
    // Two gains of magnitude 5_000: stalled at a 5_000 threshold, not at 4_000.
    let revealed = prefix(vec![node(1, 1, 800_000, vec![5_000, -5_000])], 1, 1);
    assert_single_parameter_changes_the_allocation(
        "stagnation_abs_gain_micros 5_000 -> 4_000",
        &policy_with(|p| p.stagnation_abs_gain_micros = 4_000),
        &revealed,
        vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, "a"),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, "b"),
        ],
        2,
        1,
    );
}

#[test]
fn fairness_wait_rounds_alone_changes_when_a_waiting_action_is_protected() {
    // The waiting root has waited three rounds: below the default four,
    // protected once the threshold is two.
    let mut revealed = prefix(vec![node(1, 1, 900_000, vec![30_000])], 1, 1);
    revealed.waits = vec![OpportunityWait {
        action_seq: 2,
        waited_rounds: 3,
    }];
    assert_single_parameter_changes_the_allocation(
        "fairness_wait_rounds 4 -> 2",
        &policy_with(|p| p.fairness_wait_rounds = 2),
        &revealed,
        vec![
            action(1, 1, ActionKindV1::Deepen { parent_node_seq: 1 }, "a"),
            action(2, 2, ActionKindV1::Widen { root_slot: 2 }, "b"),
        ],
        1,
        2,
    );
}

#[test]
fn no_in_bound_policy_can_exceed_the_administrator_caps() {
    let extremes = [
        policy_with(|p| {
            p.max_focus_actions = ELASTIC_MAX_FOCUS_ACTIONS_MAX;
            p.fairness_wait_rounds = ELASTIC_FAIRNESS_WAIT_ROUNDS_MAX;
            p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MIN_MICROS;
            p.stagnation_abs_gain_micros = 0;
            p.stagnation_window = ELASTIC_STAGNATION_WINDOW_MAX;
        }),
        policy_with(|p| {
            p.max_focus_actions = ELASTIC_MAX_FOCUS_ACTIONS_MIN;
            p.fairness_wait_rounds = ELASTIC_FAIRNESS_WAIT_ROUNDS_MIN;
            p.significant_gain_micros = ELASTIC_SIGNIFICANT_GAIN_MAX_MICROS;
            p.stagnation_abs_gain_micros = ELASTIC_STAGNATION_ABS_GAIN_MAX_MICROS;
            p.stagnation_window = ELASTIC_STAGNATION_WINDOW_MIN;
        }),
    ];
    // Twelve revealed nodes: the node cap is reached, every policy must stop.
    let full = prefix(
        (1..=12)
            .map(|seq| node(seq, seq, 600_000 + seq * 1_000, vec![0]))
            .collect(),
        12,
        1,
    );
    // A chain at the depth cap: deepening would exceed it.
    let mut chain = Vec::new();
    for depth in 1..=4u8 {
        let mut chain_node = node(
            u32::from(depth),
            1,
            600_000 + u32::from(depth) * 10_000,
            vec![0],
        );
        chain_node.depth = depth;
        chain_node.search_parent_seq = (depth > 1).then(|| u32::from(depth) - 1);
        chain.push(chain_node);
    }
    let deep = prefix(chain, 1, 1);
    let mut too_deep = action(1, 1, ActionKindV1::Deepen { parent_node_seq: 4 }, "deep");
    too_deep.target_depth = 5;
    for policy in &extremes {
        policy.validate().unwrap();
        let widen = action(13, 13, ActionKindV1::Widen { root_slot: 13 }, "root");
        assert!(matches!(
            decide_with(policy, &full, &[widen]),
            BatchActionV1::Stop { .. }
        ));
        assert!(matches!(
            decide_with(policy, &deep, std::slice::from_ref(&too_deep)),
            BatchActionV1::Stop { .. }
        ));
    }
}

// ---------------------------------------------------------------------------
// 5. ImproverContentV2
// ---------------------------------------------------------------------------

const CANONICAL_DEFAULT: &str = r#"{"schema_version":"rsia.improver_content.v2","mechanism":{"class":"exploration_policy","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4}}}"#;

fn content_json(class: &str, extra: Value) -> Vec<u8> {
    let mut mechanism = json!({"class": class});
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            mechanism[key] = value.clone();
        }
    }
    serde_json::to_vec(&json!({
        "schema_version": IMPROVER_CONTENT_V2,
        "mechanism": mechanism,
    }))
    .unwrap()
}

fn exploration_content(policy: Value) -> Vec<u8> {
    content_json("exploration_policy", json!({"policy": policy}))
}

#[test]
fn builtin_default_is_valid_and_its_digest_is_the_canonical_serialization() {
    let builtin = ImproverContentV2::builtin_default();
    builtin.validate().unwrap();
    assert_eq!(builtin.schema_version, IMPROVER_CONTENT_V2);
    assert_eq!(
        builtin.exploration_policy().digest().unwrap(),
        ElasticPolicyV1::default().digest().unwrap()
    );
    // The digest is the sha256 of the canonical typed serialization, so it is
    // stable across runs and independent of how the bytes were written.
    assert_eq!(
        builtin.content_digest().unwrap(),
        hash(CANONICAL_DEFAULT.as_bytes())
    );
    assert_eq!(serde_json::to_string(&builtin).unwrap(), CANONICAL_DEFAULT);
    let parsed = ImproverContentV2::parse(CANONICAL_DEFAULT.as_bytes()).unwrap();
    assert_eq!(
        parsed.content_digest().unwrap(),
        builtin.content_digest().unwrap()
    );
}

#[test]
fn valid_content_is_accepted_and_the_digest_ignores_key_order() {
    let forward = br#"{"schema_version":"rsia.improver_content.v2","mechanism":{"class":"exploration_policy","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":30000,"stagnation_abs_gain_micros":4000,"stagnation_window":1,"max_focus_actions":1,"fairness_wait_rounds":2}}}"#;
    let reordered = br#"{"mechanism":{"policy":{"fairness_wait_rounds":2,"max_focus_actions":1,"stagnation_window":1,"stagnation_abs_gain_micros":4000,"significant_gain_micros":30000,"schema_version":"rsia.elastic_priority.v1"},"class":"exploration_policy"},"schema_version":"rsia.improver_content.v2"}"#;
    let spaced = b"  {\n \"schema_version\" : \"rsia.improver_content.v2\" ,\n \"mechanism\" : { \"class\":\"exploration_policy\", \"policy\":{\"schema_version\":\"rsia.elastic_priority.v1\",\"significant_gain_micros\":30000,\"stagnation_abs_gain_micros\":4000,\"stagnation_window\":1,\"max_focus_actions\":1,\"fairness_wait_rounds\":2} } }\n";
    let first = ImproverContentV2::parse(forward).unwrap();
    let second = ImproverContentV2::parse(reordered).unwrap();
    let third = ImproverContentV2::parse(spaced).unwrap();
    let digest = first.content_digest().unwrap();
    assert_eq!(digest, second.content_digest().unwrap());
    assert_eq!(digest, third.content_digest().unwrap());
    // Same value, same digest; a different value, a different digest.
    assert_ne!(
        digest,
        ImproverContentV2::builtin_default()
            .content_digest()
            .unwrap()
    );
    let ImproverMechanismV2::ExplorationPolicy { policy } = &first.mechanism;
    assert_eq!(policy.max_focus_actions, 1);
    assert_eq!(policy.significant_gain_micros, 30_000);
    assert_eq!(
        first.exploration_policy().digest().unwrap(),
        policy.digest().unwrap()
    );
}

#[test]
fn classes_that_are_not_open_have_no_consumer_or_are_protected_are_refused_by_name() {
    // Not open in this round (plan §9.1): only ExplorationPolicy is.
    for class in ["generation_strategy", "optimizer_guidance"] {
        match ImproverContentV2::parse(&content_json(class, json!({}))) {
            Err(Error::Invalid(message)) => assert_eq!(
                message, "mechanism class not open in this round (plan §9.1)",
                "{class}"
            ),
            other => panic!("{class}: expected Invalid, got {other:?}"),
        }
    }
    // AcquisitionPolicy has no consumer.
    match ImproverContentV2::parse(&content_json("acquisition_policy", json!({}))) {
        Err(Error::Invalid(message)) => assert!(message.contains("no consumer"), "{message}"),
        other => panic!("acquisition_policy: expected Invalid, got {other:?}"),
    }
    // Protected control plane: Forbidden, whatever else the document carries.
    for class in [
        "caps",
        "exploration_caps",
        "simulation_profile",
        "holdout",
        "oracle",
        "grader",
        "budget",
        "approval",
        "goal",
    ] {
        assert!(
            matches!(
                ImproverContentV2::parse(&content_json(class, json!({}))),
                Err(Error::Forbidden)
            ),
            "{class}"
        );
        assert!(
            matches!(
                ImproverContentV2::parse(&content_json(
                    class,
                    json!({"max_nodes": 99, "w_online": 2})
                )),
                Err(Error::Forbidden)
            ),
            "{class} with a payload"
        );
    }
    // Anything else is an unknown class.
    for class in ["", "Exploration_Policy", "exploration", "policy", "meta"] {
        assert!(
            matches!(
                ImproverContentV2::parse(&content_json(class, json!({}))),
                Err(Error::Invalid(_))
            ),
            "unknown class {class:?}"
        );
    }
}

#[test]
fn unknown_fields_and_control_plane_values_inside_the_policy_are_refused() {
    let default_policy = serde_json::to_value(ElasticPolicyV1::default()).unwrap();
    for (key, value) in [
        ("max_nodes", json!(12)),
        ("w_online", json!(1)),
        ("simulation", json!({"online": {"fixed_seed": 1}})),
        ("unknown_knob", json!(1)),
    ] {
        let mut policy = default_policy.clone();
        policy[key] = value;
        assert!(
            matches!(
                ImproverContentV2::parse(&exploration_content(policy)),
                Err(Error::Invalid(_))
            ),
            "policy key {key}"
        );
    }
    // Unknown fields at the other two levels.
    let mut top: Value =
        serde_json::from_slice(&exploration_content(default_policy.clone())).unwrap();
    top["extra"] = json!(1);
    assert!(matches!(
        ImproverContentV2::parse(&serde_json::to_vec(&top).unwrap()),
        Err(Error::Invalid(_))
    ));
    let mut nested: Value = serde_json::from_slice(&exploration_content(default_policy)).unwrap();
    nested["mechanism"]["extra"] = json!(1);
    assert!(matches!(
        ImproverContentV2::parse(&serde_json::to_vec(&nested).unwrap()),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn duplicate_keys_are_refused_at_every_level_and_cannot_split_the_class() {
    let good_policy = r#"{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4}"#;
    let documents = [
        // Top level.
        format!(
            r#"{{"schema_version":"rsia.improver_content.v2","schema_version":"rsia.improver_content.v2","mechanism":{{"class":"exploration_policy","policy":{good_policy}}}}}"#
        ),
        // Policy level: the second value would be silently kept by a Value.
        r#"{"schema_version":"rsia.improver_content.v2","mechanism":{"class":"exploration_policy","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"significant_gain_micros":30000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4}}}"#.to_string(),
        // The class shown to the classifier must be the class that is parsed,
        // so a duplicated class is refused as Invalid in either order.
        format!(
            r#"{{"schema_version":"rsia.improver_content.v2","mechanism":{{"class":"exploration_policy","class":"caps","policy":{good_policy}}}}}"#
        ),
        format!(
            r#"{{"schema_version":"rsia.improver_content.v2","mechanism":{{"class":"caps","class":"exploration_policy","policy":{good_policy}}}}}"#
        ),
        // A duplicated unknown key is still a duplicate.
        format!(
            r#"{{"schema_version":"rsia.improver_content.v2","mechanism":{{"class":"exploration_policy","policy":{good_policy}}},"x":1,"x":2}}"#
        ),
    ];
    for document in &documents {
        assert!(
            matches!(
                ImproverContentV2::parse(document.as_bytes()),
                Err(Error::Invalid(_))
            ),
            "{document}"
        );
    }
}

#[test]
fn out_of_bound_or_malformed_content_is_refused() {
    for policy in [
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 100_001, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 3, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 0, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 5, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 12}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 10_000, "stagnation_abs_gain_micros": 10_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        // Type mismatches: overflow, float and text where an integer belongs.
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 3_000_000_000u64, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000.0, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": "20000", "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
        json!({"schema_version": "rsia.elastic_priority.v1", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2}),
        json!({"schema_version": "rsia.elastic_priority.v9", "significant_gain_micros": 20_000, "stagnation_abs_gain_micros": 5_000, "stagnation_window": 2, "max_focus_actions": 2, "fairness_wait_rounds": 4}),
    ] {
        assert!(
            matches!(
                ImproverContentV2::parse(&exploration_content(policy.clone())),
                Err(Error::Invalid(_))
            ),
            "{policy}"
        );
    }
    let good_policy = serde_json::to_value(ElasticPolicyV1::default()).unwrap();
    let wrong_schema = serde_json::to_vec(&json!({
        "schema_version": "rsia.improver_content.v1",
        "mechanism": {"class": "exploration_policy", "policy": good_policy},
    }))
    .unwrap();
    assert!(matches!(
        ImproverContentV2::parse(&wrong_schema),
        Err(Error::Invalid(_))
    ));
    for raw in [
        &b""[..],
        b"null",
        b"[]",
        b"\"exploration_policy\"",
        b"{}",
        b"{\"mechanism\":null}",
        b"{\"mechanism\":\"exploration_policy\"}",
        b"{\"mechanism\":{}}",
        b"{\"mechanism\":{\"class\":7}}",
        b"{\"schema_version\":\"rsia.improver_content.v2\",\"mechanism\":{\"class\":\"exploration_policy\"}}",
        &[0xff, 0xfe, b'{'],
    ] {
        assert!(
            matches!(ImproverContentV2::parse(raw), Err(Error::Invalid(_))),
            "{raw:?}"
        );
    }
    let mut trailing = CANONICAL_DEFAULT.as_bytes().to_vec();
    trailing.extend_from_slice(b" {}");
    assert!(matches!(
        ImproverContentV2::parse(&trailing),
        Err(Error::Invalid(_))
    ));
    let mut oversized = CANONICAL_DEFAULT.as_bytes().to_vec();
    oversized.extend(std::iter::repeat_n(b' ', IMPROVER_CONTENT_MAX_BYTES));
    assert!(matches!(
        ImproverContentV2::parse(&oversized),
        Err(Error::Invalid(_))
    ));
    let deeply_nested = format!("{}{}", "[".repeat(2_000), "]".repeat(2_000));
    assert!(matches!(
        ImproverContentV2::parse(deeply_nested.as_bytes()),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn a_content_value_built_in_code_is_validated_like_a_parsed_one() {
    let mut content = ImproverContentV2::builtin_default();
    content.validate().unwrap();
    content.schema_version = "rsia.improver_content.v1".into();
    assert!(matches!(content.validate(), Err(Error::Invalid(_))));
    let mut content = ImproverContentV2::builtin_default();
    let ImproverMechanismV2::ExplorationPolicy { policy } = &mut content.mechanism;
    policy.max_focus_actions = 0;
    assert!(matches!(content.validate(), Err(Error::Invalid(_))));
}

// ---------------------------------------------------------------------------
// 9. FIELD_CONTRACTS registers the consumer of exactly the five policy fields
// ---------------------------------------------------------------------------

const POLICY_FIELDS: [&str; 5] = [
    "significant_gain_micros",
    "stagnation_abs_gain_micros",
    "stagnation_window",
    "max_focus_actions",
    "fairness_wait_rounds",
];

#[test]
fn improver_may_write_the_five_policy_fields_and_nobody_else_may() {
    for name in POLICY_FIELDS {
        let path = format!("improver.exploration_policy.{name}");
        let contract = field_contract(&path).unwrap();
        assert_eq!(contract.owner, FieldOwner::ImproverCandidate, "{path}");
        assert_eq!(contract.consumer, "exploration.decide_elastic", "{path}");
        assert_candidate_may_write(ArtifactKind::Improver, &path).unwrap();
        assert!(
            matches!(
                assert_candidate_may_write(ArtifactKind::Skill, &path),
                Err(Error::Forbidden)
            ),
            "a Skill candidate must not write {path}"
        );
    }
}

#[test]
fn unregistered_policy_paths_have_no_consumer() {
    // Control-plane values are not fields of the policy: nothing consumes
    // them as an improver-written field, so they stay refused.
    for name in [
        "max_nodes",
        "max_depth",
        "max_repair_dispatches_per_episode",
        "w_online",
        "w_sim",
        "simulation",
        "caps",
        "schema_version",
    ] {
        let path = format!("improver.exploration_policy.{name}");
        for kind in [ArtifactKind::Improver, ArtifactKind::Skill] {
            match assert_candidate_may_write(kind, &path) {
                Err(Error::Invalid(message)) => {
                    assert!(message.contains("no consumer"), "{message}");
                }
                other => panic!("{path}: expected Invalid(no consumer), got {other:?}"),
            }
        }
    }
}

#[test]
fn registered_policy_fields_are_exactly_the_struct_fields_and_old_entries_are_kept() {
    let value = serde_json::to_value(ElasticPolicyV1::default()).unwrap();
    let mut struct_fields: Vec<String> = value
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.as_str() != "schema_version")
        .cloned()
        .collect();
    struct_fields.sort();
    let mut registered: Vec<String> = FIELD_CONTRACTS
        .iter()
        .filter_map(|contract| {
            contract
                .path
                .strip_prefix("improver.exploration_policy.")
                .map(str::to_owned)
        })
        .collect();
    registered.sort();
    let mut expected: Vec<String> = POLICY_FIELDS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    expected.sort();
    assert_eq!(registered, expected);
    assert_eq!(struct_fields, expected);
    // The historical improver and protected entries are untouched.
    for path in [
        "improver.instruction",
        "improver.max_candidates",
        "improver.max_rounds",
    ] {
        let contract = field_contract(path).unwrap();
        assert_eq!(contract.owner, FieldOwner::ImproverCandidate);
        assert_eq!(contract.consumer, "worker");
    }
    for path in ["namespace", "budget", "approval"] {
        assert_eq!(
            field_contract(path).unwrap().owner,
            FieldOwner::ProtectedCore
        );
        assert!(matches!(
            assert_candidate_may_write(ArtifactKind::Improver, path),
            Err(Error::Forbidden)
        ));
    }
}
