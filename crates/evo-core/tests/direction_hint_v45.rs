//! AG-179 (plan §9.1.1, V118.b): the direction hint control-plane record and its descriptive on/off
//! ablation, library layer only. Nothing here is persisted, registered or consumed by a run.
use evo_core::contract::{FIELD_CONTRACTS, FieldOwner, assert_candidate_may_write, field_contract};
use evo_core::direction_hint::{
    AblationBudgetV1, AblationClusterV1, DIRECTION_HINT_ABLATION_SCHEMA,
    DIRECTION_HINT_BODY_MAX_BYTES, DIRECTION_HINT_SCHEMA, DirectionHintAblationV1, DirectionHintV1,
    direction_hint_ablation_is_not_formal, generation_signature_with_hint,
};
use evo_core::evaluation::{DataUse, SCORE_MICROS_MAX};
use evo_core::improver::{IMPROVER_CONTENT_V2, ImproverContentV2};
use evo_core::{ArtifactKind, Error, Result, hash};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn hint_body(body: &str) -> DirectionHintV1 {
    DirectionHintV1 {
        schema_version: DIRECTION_HINT_SCHEMA.into(),
        id: "hint-1".into(),
        world_id: "world-1".into(),
        source_node_ids: vec!["node-a".into(), "node-b".into()],
        generation_rule_version: "rule-v1".into(),
        body: body.into(),
        digest: d(body),
        episode_ids: vec!["ep-1".into(), "ep-2".into()],
    }
}

fn hint() -> DirectionHintV1 {
    hint_body("Prefer deepening the branch that gained last round.")
}

fn hint_with(change: impl FnOnce(&mut DirectionHintV1)) -> DirectionHintV1 {
    let mut hint = hint();
    change(&mut hint);
    hint
}

fn cluster(id: &str, off: u32, on: u32) -> AblationClusterV1 {
    AblationClusterV1 {
        cluster_id: id.into(),
        off_score_micros: off,
        on_score_micros: on,
    }
}

/// Differences on - off are +100_000, -50_000 and 0, so the mean is floor(50_000 / 3). The spend is
/// 120_000 + 180_000 = 300_000 inside the 400_000 cap.
fn ablation() -> DirectionHintAblationV1 {
    DirectionHintAblationV1 {
        schema_version: DIRECTION_HINT_ABLATION_SCHEMA.into(),
        id: "ablation-1".into(),
        data_use: DataUse::Development,
        hint: hint(),
        base_generation_signature: d("base generation signature"),
        budget: AblationBudgetV1 {
            scope_id: "dev-scope-1".into(),
            cap_micros: 400_000,
        },
        clusters: vec![
            cluster("c1", 400_000, 500_000),
            cluster("c2", 600_000, 550_000),
            cluster("c3", 300_000, 300_000),
        ],
        off_cost_micros: 120_000,
        on_dev_cost_micros: 150_000,
        hint_generation_cost_micros: 30_000,
        on_total_cost_micros: 180_000,
        mean_delta_micros: 16_666,
    }
}

fn ablation_with(change: impl FnOnce(&mut DirectionHintAblationV1)) -> DirectionHintAblationV1 {
    let mut ablation = ablation();
    change(&mut ablation);
    ablation
}

#[track_caller]
fn assert_invalid<T: std::fmt::Debug>(result: Result<T>, what: &str) {
    assert!(
        matches!(result, Err(Error::Invalid(_))),
        "{what}: {result:?}"
    );
}

#[track_caller]
fn assert_conflict<T: std::fmt::Debug>(result: Result<T>, what: &str) {
    assert!(
        matches!(result, Err(Error::Conflict(_))),
        "{what}: {result:?}"
    );
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn healthy_hint_validates_and_digest_is_recomputed_from_body_bytes() {
    hint().validate().unwrap();
    // The digest is the plain sha256 of the raw bytes (the known vector for "abc").
    let abc = hint_body("abc");
    assert_eq!(
        abc.digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    abc.validate().unwrap();
    // No newline or Unicode normalization (plan §5.1): each spelling has its own digest.
    for (left, right) in [("a\r\nb", "a\nb"), ("caf\u{e9}", "cafe\u{301}")] {
        let (left, right) = (hint_body(left), hint_body(right));
        left.validate().unwrap();
        right.validate().unwrap();
        assert_ne!(left.digest, right.digest);
        let mut crossed = left.clone();
        crossed.digest = right.digest.clone();
        assert_conflict(crossed.validate(), "digest of the other spelling");
    }
    // The bound is in UTF-8 bytes: 8192 pass, one more fails, and so do 4097 two-byte characters.
    hint_body(&"a".repeat(DIRECTION_HINT_BODY_MAX_BYTES))
        .validate()
        .unwrap();
    assert_invalid(
        hint_body(&"a".repeat(DIRECTION_HINT_BODY_MAX_BYTES + 1)).validate(),
        "8193 bytes",
    );
    assert_invalid(
        hint_body(&"\u{e9}".repeat(DIRECTION_HINT_BODY_MAX_BYTES / 2 + 1)).validate(),
        "4097 two-byte characters",
    );
    assert_invalid(hint_body("").validate(), "empty body");
}

#[test]
fn hint_digest_must_be_recomputable_not_a_name_or_stale() {
    let healthy = hint();
    healthy.validate().unwrap();
    // Not a digest at all (Invalid): a readable name, a wrong length, uppercase, a prefix.
    let right = healthy.digest.clone();
    for (label, bad) in [
        ("name", healthy.id.clone()),
        ("short", right[..63].to_string()),
        ("long", format!("{right}0")),
        ("uppercase", right.to_uppercase()),
        ("prefixed", format!("sha256:{right}")),
        ("not hex", "g".repeat(64)),
    ] {
        assert_invalid(hint_with(|h| h.digest = bad).validate(), label);
    }
    // Well formed but not the sha256 of the body (Conflict): shape alone never counts, and a stale
    // hint cannot be bound into a signature either.
    for (label, stale) in [
        ("body edited, digest stale", hint_with(|h| h.body.push('!'))),
        ("zero digest", hint_with(|h| h.digest = "0".repeat(64))),
    ] {
        assert_conflict(stale.validate(), label);
        assert_conflict(
            generation_signature_with_hint(&d("base"), Some(&stale)),
            label,
        );
    }
}

#[test]
fn hint_closure_rule_version_and_episodes_are_required_unique_sorted() {
    let owned = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    // Order is lexicographic, not numeric.
    hint_with(|h| h.source_node_ids = owned(&["node-10", "node-2"]))
        .validate()
        .unwrap();
    let bad_sets: [(&str, &[&str]); 5] = [
        ("empty", &[]),
        ("duplicate", &["a", "a"]),
        ("unsorted", &["b", "a"]),
        ("numeric order", &["node-2", "node-10"]),
        ("blank id", &["", "a"]),
    ];
    for (label, ids) in bad_sets {
        assert_invalid(
            hint_with(|h| h.source_node_ids = owned(ids)).validate(),
            &format!("source_node_ids {label}"),
        );
        assert_invalid(
            hint_with(|h| h.episode_ids = owned(ids)).validate(),
            &format!("episode_ids {label}"),
        );
    }
    let long = "r".repeat(129);
    for (label, version) in [
        ("empty", ""),
        ("space", "rule v1"),
        ("too long", long.as_str()),
    ] {
        assert_invalid(
            hint_with(|h| h.generation_rule_version = version.into()).validate(),
            &format!("rule version {label}"),
        );
    }
}

#[test]
fn hint_and_ablation_reject_unknown_and_control_plane_keys() {
    let hint_doc = serde_json::to_value(hint()).unwrap();
    let ablation_doc = serde_json::to_value(ablation()).unwrap();
    // Health control: the untouched documents read back and validate.
    serde_json::from_value::<DirectionHintV1>(hint_doc.clone())
        .unwrap()
        .validate()
        .unwrap();
    serde_json::from_value::<DirectionHintAblationV1>(ablation_doc.clone())
        .unwrap()
        .validate()
        .unwrap();
    // No level of either record takes an unknown key, an enable switch, a control-plane value or
    // a result field.
    for key in [
        "extra",
        "enabled",
        "default_on",
        "caps",
        "approval",
        "holdout",
        "grader",
        "verdict",
        "lcb_micros",
        "alpha_i",
        "family_id",
        "ticket_id",
        "release_id",
    ] {
        let mut doc = hint_doc.clone();
        doc[key] = json!(true);
        assert!(
            serde_json::from_value::<DirectionHintV1>(doc).is_err(),
            "hint took {key}"
        );
        for level in ["", "/hint", "/budget", "/clusters/0"] {
            let mut doc = ablation_doc.clone();
            doc.pointer_mut(level).unwrap()[key] = json!(true);
            assert!(
                serde_json::from_value::<DirectionHintAblationV1>(doc).is_err(),
                "ablation{level} took {key}"
            );
        }
    }
}

/// CPP1: on the parent the class is unknown (Invalid), on head it is control plane (Forbidden).
#[test]
fn ctrl_probe_ag179_direction_hint_class_is_control_plane_forbidden() {
    // Health control: the one open class still parses.
    let open = serde_json::to_vec(&ImproverContentV2::builtin_default()).unwrap();
    ImproverContentV2::parse(&open).unwrap();
    // The direction hint is refused by name, whatever else the document carries.
    for mechanism in [
        json!({"class": "direction_hint"}),
        json!({"class": "direction_hint", "body": "x", "enabled": true}),
    ] {
        let document = serde_json::to_vec(
            &json!({"schema_version": IMPROVER_CONTENT_V2, "mechanism": mechanism}),
        )
        .unwrap();
        assert!(
            matches!(ImproverContentV2::parse(&document), Err(Error::Forbidden)),
            "{mechanism}"
        );
    }
}

#[test]
fn hint_off_is_default_and_keeps_every_signature() {
    // No hint means off: every base, well formed or not, passes through byte for byte.
    for base in [
        d("world-a"),
        d("world-b"),
        String::new(),
        "not a digest".into(),
    ] {
        assert_eq!(generation_signature_with_hint(&base, None).unwrap(), base);
    }
    // The off arm of a recorded ablation is its base signature; the on arm is another signature.
    let a = ablation();
    let off = generation_signature_with_hint(&a.base_generation_signature, None).unwrap();
    let on = generation_signature_with_hint(&a.base_generation_signature, Some(&a.hint)).unwrap();
    assert_eq!(off, a.base_generation_signature);
    assert_ne!(on, off);
}

#[test]
fn hint_on_changes_the_signature_and_each_hint_field_changes_it() {
    let base = d("world generation signature");
    let sign = |hint: &DirectionHintV1, base: &str| {
        generation_signature_with_hint(base, Some(hint)).unwrap()
    };
    let on = sign(&hint(), &base);
    assert!(on.len() == 64 && on.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    assert_eq!(on, sign(&hint(), &base), "the signature is deterministic");
    // The inputs are the base, the digest (through the body), the rule version, the node closure
    // and the episodes; each one moves the signature to a value of its own.
    let moved = [
        ("base", sign(&hint(), &d("another base"))),
        (
            "body and digest",
            sign(&hint_body("A different direction."), &base),
        ),
        (
            "rule version",
            sign(
                &hint_with(|h| h.generation_rule_version = "rule-v2".into()),
                &base,
            ),
        ),
        (
            "node closure",
            sign(
                &hint_with(|h| h.source_node_ids.push("node-c".into())),
                &base,
            ),
        ),
        (
            "episodes",
            sign(&hint_with(|h| h.episode_ids.push("ep-3".into())), &base),
        ),
    ];
    let mut distinct: BTreeSet<&str> = moved.iter().map(|(_, s)| s.as_str()).collect();
    distinct.extend([on.as_str(), base.as_str()]);
    assert_eq!(distinct.len(), moved.len() + 2, "{moved:?}");
    // The id and the world id are names, not inputs.
    for same in [
        hint_with(|h| h.id = "hint-renamed".into()),
        hint_with(|h| h.world_id = "world-renamed".into()),
    ] {
        assert_eq!(sign(&same, &base), on);
    }
}

#[test]
fn ablation_runs_only_on_development_data() {
    for data_use in [
        DataUse::ReplayTrain,
        DataUse::ReplaySelect,
        DataUse::AcceptanceEpoch,
        DataUse::OperationalMonitoring,
    ] {
        let mut a = ablation();
        a.validate().unwrap();
        a.data_use = data_use;
        assert_invalid(a.validate(), &format!("{data_use:?}"));
    }
}

#[test]
fn ablation_has_one_pair_per_cluster_and_recomputes_the_mean() {
    let healthy = ablation();
    healthy.validate().unwrap();
    assert_eq!(healthy.mean_delta_micros, 16_666);
    assert_conflict(
        ablation_with(|a| a.mean_delta_micros += 1).validate(),
        "mean off by one",
    );
    // Rounding is toward negative infinity (div_euclid): differences -1, -1, 0 average to -1.
    let floored = |mean| {
        ablation_with(|a| {
            a.clusters = vec![
                cluster("c1", 10, 9),
                cluster("c2", 10, 9),
                cluster("c3", 10, 10),
            ];
            a.mean_delta_micros = mean;
        })
    };
    floored(-1).validate().unwrap();
    assert_conflict(floored(0).validate(), "mean truncated toward zero");
    // At least one cluster, and one pair per cluster id (the mean follows the extra pair).
    assert_invalid(
        ablation_with(|a| a.clusters.clear()).validate(),
        "no clusters",
    );
    let pushed = |id| {
        ablation_with(|a| {
            a.clusters.push(cluster(id, 5, 5));
            a.mean_delta_micros = 12_500;
        })
    };
    pushed("c4").validate().unwrap();
    assert_conflict(pushed("c1").validate(), "second pair for c1");
    // Scores stay inside [0, SCORE_MICROS_MAX]; the mean is consistent in every case.
    let single = |off, on, mean| {
        ablation_with(|a| {
            a.clusters = vec![cluster("c1", off, on)];
            a.mean_delta_micros = mean;
        })
    };
    single(0, SCORE_MICROS_MAX, 1_000_000).validate().unwrap();
    single(SCORE_MICROS_MAX, 0, -1_000_000).validate().unwrap();
    assert_invalid(
        single(SCORE_MICROS_MAX + 1, SCORE_MICROS_MAX, -1).validate(),
        "off score above the maximum",
    );
    assert_invalid(
        single(0, SCORE_MICROS_MAX + 1, 1_000_001).validate(),
        "on score above the maximum",
    );
}

#[test]
fn ablation_spend_must_stay_inside_the_registered_budget() {
    let healthy = ablation();
    healthy.validate().unwrap();
    // The spend is the off cost plus the on-arm total, and it may equal the cap.
    let spend = healthy.off_cost_micros + healthy.on_total_cost_micros;
    assert_eq!(spend, 300_000);
    ablation_with(|a| a.budget.cap_micros = spend)
        .validate()
        .unwrap();
    for cap in [spend - 1, 0] {
        assert!(
            matches!(
                ablation_with(|a| a.budget.cap_micros = cap).validate(),
                Err(Error::Budget)
            ),
            "cap {cap}"
        );
    }
    // An overflowing sum is an overspend, not a wrap.
    assert!(matches!(
        ablation_with(|a| a.off_cost_micros = u64::MAX).validate(),
        Err(Error::Budget)
    ));
    for (label, scope_id) in [("empty", ""), ("blank", " "), ("not an id", "dev scope")] {
        assert_invalid(
            ablation_with(|a| a.budget.scope_id = scope_id.into()).validate(),
            &format!("scope_id {label}"),
        );
    }
}

#[test]
fn hint_generation_cost_is_part_of_the_on_arm_total() {
    let healthy = ablation();
    healthy.validate().unwrap();
    assert_eq!(
        healthy.on_total_cost_micros,
        healthy.on_dev_cost_micros + healthy.hint_generation_cost_micros
    );
    // A total that leaves the hint out, or is any other number, is not the total.
    assert_conflict(
        ablation_with(|a| a.on_total_cost_micros = a.on_dev_cost_micros).validate(),
        "total without the hint cost",
    );
    assert_conflict(
        ablation_with(|a| a.on_total_cost_micros += 1).validate(),
        "total off by one",
    );
    assert_conflict(
        ablation_with(|a| {
            a.on_dev_cost_micros = u64::MAX;
            a.hint_generation_cost_micros = 1;
        })
        .validate(),
        "overflowing parts",
    );
    // The hint cost spends the budget: a cap that fits development alone rejects the full total,
    // and the same cap passes when the hint cost is zero.
    let dev_only_cap = healthy.off_cost_micros + healthy.on_dev_cost_micros;
    assert!(matches!(
        ablation_with(|a| a.budget.cap_micros = dev_only_cap).validate(),
        Err(Error::Budget)
    ));
    ablation_with(|a| {
        a.budget.cap_micros = dev_only_cap;
        a.hint_generation_cost_micros = 0;
        a.on_total_cost_micros = a.on_dev_cost_micros;
    })
    .validate()
    .unwrap();
}

#[test]
fn ablation_binds_the_hint_digest_and_base_signature() {
    let healthy = ablation();
    healthy.validate().unwrap();
    // The nested hint is judged by its own rules, including the recomputed digest.
    assert_conflict(
        ablation_with(|a| a.hint.body.push('!')).validate(),
        "hint body edited, digest stale",
    );
    assert_invalid(
        ablation_with(|a| a.hint.digest = a.hint.id.clone()).validate(),
        "hint name as digest",
    );
    assert_invalid(
        ablation_with(|a| a.hint.source_node_ids.clear()).validate(),
        "hint without nodes",
    );
    // The base signature is a lowercase sha256.
    let base = healthy.base_generation_signature.clone();
    for (label, bad) in [
        ("name", "base-signature".to_string()),
        ("short", base[..63].to_string()),
        ("uppercase", base.to_uppercase()),
        ("prefixed", format!("sha256:{base}")),
    ] {
        assert_invalid(
            ablation_with(|a| a.base_generation_signature = bad).validate(),
            label,
        );
    }
    // Both arms are recomputed from the record: off is the base, on moves with the hint digest and
    // with the base.
    let off = generation_signature_with_hint(&base, None).unwrap();
    let on = generation_signature_with_hint(&base, Some(&healthy.hint)).unwrap();
    assert_eq!(off, base);
    for (label, other) in [
        (
            "hint digest",
            generation_signature_with_hint(&base, Some(&hint_body("another hint body"))).unwrap(),
        ),
        (
            "base",
            generation_signature_with_hint(&d("another base"), Some(&healthy.hint)).unwrap(),
        ),
    ] {
        assert_ne!(other, on, "{label}");
    }
}

#[test]
fn ablation_result_has_no_verdict_family_alpha_or_release_surface() {
    let healthy = ablation();
    let doc = serde_json::to_value(&healthy).unwrap();
    // The serialized keys are exactly the whitelist: no bound, verdict, alpha, family, ticket,
    // release or default switch exists to be set.
    assert_eq!(
        keys(&doc),
        BTreeSet::from([
            "schema_version",
            "id",
            "data_use",
            "hint",
            "base_generation_signature",
            "budget",
            "clusters",
            "off_cost_micros",
            "on_dev_cost_micros",
            "hint_generation_cost_micros",
            "on_total_cost_micros",
            "mean_delta_micros",
        ])
    );
    assert_eq!(
        keys(&doc["hint"]),
        BTreeSet::from([
            "schema_version",
            "id",
            "world_id",
            "source_node_ids",
            "generation_rule_version",
            "body",
            "digest",
            "episode_ids",
        ])
    );
    assert_eq!(
        keys(&doc["budget"]),
        BTreeSet::from(["scope_id", "cap_micros"])
    );
    assert_eq!(
        keys(&doc["clusters"][0]),
        BTreeSet::from(["cluster_id", "off_score_micros", "on_score_micros"])
    );
    // Whatever the record says, it is never a formal result.
    for a in [
        healthy,
        ablation_with(|a| a.data_use = DataUse::ReplayTrain),
    ] {
        assert_invalid(direction_hint_ablation_is_not_formal(&a), "formal");
    }
}

#[test]
fn mechanism_classes_and_registry_rows_stay_unchanged() {
    let document = |class: &str| {
        serde_json::to_vec(&json!({
            "schema_version": IMPROVER_CONTENT_V2,
            "mechanism": {"class": class},
        }))
        .unwrap()
    };
    // Only the exploration policy is open.
    let open = serde_json::to_vec(&ImproverContentV2::builtin_default()).unwrap();
    ImproverContentV2::parse(&open).unwrap();
    for class in ["generation_strategy", "optimizer_guidance"] {
        match ImproverContentV2::parse(&document(class)) {
            Err(Error::Invalid(message)) => assert_eq!(
                message, "mechanism class not open in this round (plan §9.1)",
                "{class}"
            ),
            other => panic!("{class}: expected Invalid, got {other:?}"),
        }
    }
    match ImproverContentV2::parse(&document("acquisition_policy")) {
        Err(Error::Invalid(message)) => assert!(message.contains("no consumer"), "{message}"),
        other => panic!("acquisition_policy: expected Invalid, got {other:?}"),
    }
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
                ImproverContentV2::parse(&document(class)),
                Err(Error::Forbidden)
            ),
            "{class}"
        );
    }
    // Near misses of the new name stay unknown classes.
    for class in ["direction", "DirectionHint", "direction_hints", "meta"] {
        match ImproverContentV2::parse(&document(class)) {
            Err(Error::Invalid(message)) => {
                assert_eq!(message, "unknown improver mechanism class", "{class}");
            }
            other => panic!("{class}: expected Invalid, got {other:?}"),
        }
    }
    // The improver and protected rows keep their owner and consumer, and no row registers the
    // hint, so a candidate has no path to it.
    for (path, consumer) in [
        ("improver.instruction", "worker"),
        ("improver.max_candidates", "worker"),
        ("improver.max_rounds", "worker"),
        (
            "improver.exploration_policy.significant_gain_micros",
            "exploration.decide_elastic",
        ),
        (
            "improver.exploration_policy.stagnation_abs_gain_micros",
            "exploration.decide_elastic",
        ),
        (
            "improver.exploration_policy.stagnation_window",
            "exploration.decide_elastic",
        ),
        (
            "improver.exploration_policy.max_focus_actions",
            "exploration.decide_elastic",
        ),
        (
            "improver.exploration_policy.fairness_wait_rounds",
            "exploration.decide_elastic",
        ),
    ] {
        let row = field_contract(path).unwrap();
        assert_eq!(
            (row.owner, row.consumer),
            (FieldOwner::ImproverCandidate, consumer),
            "{path}"
        );
    }
    for path in ["namespace", "budget", "approval"] {
        assert_eq!(
            field_contract(path).unwrap().owner,
            FieldOwner::ProtectedCore,
            "{path}"
        );
    }
    assert!(
        FIELD_CONTRACTS
            .iter()
            .all(|row| !row.path.contains("direction_hint"))
    );
    for path in [
        "direction_hint",
        "direction_hint.body",
        "improver.direction_hint",
    ] {
        for kind in [ArtifactKind::Improver, ArtifactKind::Skill] {
            match assert_candidate_may_write(kind, path) {
                Err(Error::Invalid(message)) => assert!(message.contains("no consumer"), "{path}"),
                other => panic!("{path}: expected Invalid(no consumer), got {other:?}"),
            }
        }
    }
}
