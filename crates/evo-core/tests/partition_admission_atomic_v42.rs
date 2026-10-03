//! Public-API checks for one rejected in-memory admission, not source provenance.
//! Exposure facts and successful history remain authoritative across every retry.
use evo_core::evaluation::{DataUse, PartitionRegistry, TaskIdentity, normalize_for_near_dup};
use evo_core::{Error, Result, fingerprint};
use serde::Serialize;

const ROOT_RAW: &[u8] = b"Root Development Source";
const NEAR_ROOT_RAW: &[u8] = b"  root\tdevelopment\nSOURCE ";
const EXPOSED_RAW: &[u8] = b"Previously dispatched protected fixture";
const USES: [DataUse; 5] = [
    DataUse::Development,
    DataUse::ReplayTrain,
    DataUse::ReplaySelect,
    DataUse::AcceptanceEpoch,
    DataUse::OperationalMonitoring,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
enum Observed {
    Ok,
    Invalid(String),
    Conflict(String),
}

fn observe(result: Result<()>) -> Observed {
    match result {
        Ok(()) => Observed::Ok,
        Err(Error::Invalid(message)) => Observed::Invalid(message),
        Err(Error::Conflict(message)) => Observed::Conflict(message),
        Err(other) => panic!("unexpected admission error: {other:?}"),
    }
}

fn conflict(message: &str) -> Observed {
    Observed::Conflict(message.into())
}

fn invalid_empty() -> Observed {
    Observed::Invalid("identifier: expected nonempty text <= 128 bytes".into())
}

fn invalid_char() -> Observed {
    Observed::Invalid("invalid identifier".into())
}

fn task(raw: &[u8], family: &str, data_use: DataUse) -> TaskIdentity {
    TaskIdentity::from_bytes("fixture-task", raw, family, data_use).unwrap()
}

fn parent(mut task: TaskIdentity, parent_id: &str) -> TaskIdentity {
    task.parent_id = Some(parent_id.into());
    task
}

fn seeded() -> PartitionRegistry {
    let mut registry = PartitionRegistry::default();
    for task in [
        task(ROOT_RAW, "dev-family", DataUse::Development),
        task(
            b"Independent acceptance root",
            "accept-family",
            DataUse::AcceptanceEpoch,
        ),
        task(
            b"Independent monitoring root",
            "monitor-family",
            DataUse::OperationalMonitoring,
        ),
    ] {
        registry.admit(&task).unwrap();
    }
    registry.mark_exposed(evo_core::hash(EXPOSED_RAW));
    registry
}

struct Probe {
    task: TaskIdentity,
    expected: Observed,
}

fn probe(task: TaskIdentity, expected: Observed) -> Probe {
    Probe { task, expected }
}

// Both registries have the same genuine successful history and exposure facts.
// Only one receives the rejected candidate. Compare future public results, not Debug.
fn compare_after_rejection(rejected: &TaskIdentity, expected: Observed, probes: &[Probe]) {
    let mut reference = seeded();
    let mut attempted = seeded();
    assert_eq!(observe(attempted.admit(rejected)), expected);
    let expected: Vec<_> = probes.iter().map(|probe| probe.expected.clone()).collect();
    let reference_results: Vec<_> = probes
        .iter()
        .map(|probe| observe(reference.admit(&probe.task)))
        .collect();
    assert_eq!(reference_results, expected, "successful-history control");
    let attempted_results: Vec<_> = probes
        .iter()
        .map(|probe| observe(attempted.admit(&probe.task)))
        .collect();
    assert_eq!(
        attempted_results, reference_results,
        "Err changed future admission behavior"
    );
}

fn history_controls() -> Vec<Probe> {
    vec![
        probe(
            task(ROOT_RAW, "new-root-family", DataUse::AcceptanceEpoch),
            conflict("duplicate_raw_hash"),
        ),
        probe(
            task(NEAR_ROOT_RAW, "near-family", DataUse::Development),
            conflict("near_duplicate_normalized_digest"),
        ),
        probe(
            task(
                b"Existing family purpose probe",
                "dev-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("family_crosses_train_test_or_data_use"),
        ),
        probe(
            parent(
                task(
                    b"Existing parent purpose probe",
                    "parent-probe-family",
                    DataUse::AcceptanceEpoch,
                ),
                "dev-family",
            ),
            conflict("parent_family_data_use_mismatch"),
        ),
        probe(
            task(
                EXPOSED_RAW,
                "exposure-probe-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("holdout_previously_exposed"),
        ),
        probe(
            task(
                b"Fresh development history control",
                "dev-family",
                DataUse::Development,
            ),
            Observed::Ok,
        ),
    ]
}

fn corrected_retry(rejected: &TaskIdentity, expected: Observed, corrected: TaskIdentity) {
    let probes = [
        probe(corrected.clone(), Observed::Ok),
        probe(corrected, conflict("duplicate_raw_hash")),
    ];
    compare_after_rejection(rejected, expected.clone(), &probes);
    compare_after_rejection(rejected, expected, &history_controls());
}

#[test]
fn invalid_task_id_adds_no_members() {
    let corrected = task(
        b"Invalid task id candidate",
        "task-id-child",
        DataUse::Development,
    );
    let mut rejected = corrected.clone();
    rejected.task_id.clear();
    corrected_retry(&rejected, invalid_empty(), corrected);
    compare_after_rejection(
        &rejected,
        invalid_empty(),
        &[probe(
            task(
                b"Different task id sibling",
                "task-id-child",
                DataUse::ReplayTrain,
            ),
            Observed::Ok,
        )],
    );
}

#[test]
fn invalid_family_id_adds_no_members() {
    let corrected = task(
        b"Invalid family id candidate",
        "family-id-child",
        DataUse::Development,
    );
    let mut rejected = corrected.clone();
    rejected.family_id = "bad/".into();
    corrected_retry(&rejected, invalid_char(), corrected);
}

#[test]
fn duplicate_raw_rejection_keeps_successful_members_and_adds_no_family() {
    let rejected = task(ROOT_RAW, "raw-rejected-family", DataUse::AcceptanceEpoch);
    compare_after_rejection(
        &rejected,
        conflict("duplicate_raw_hash"),
        &[probe(
            task(
                b"Different raw after duplicate",
                "raw-rejected-family",
                DataUse::ReplaySelect,
            ),
            Observed::Ok,
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("duplicate_raw_hash"),
        &history_controls(),
    );
}

#[test]
fn normalized_duplicate_rejection_does_not_add_raw_or_family() {
    let rejected = task(NEAR_ROOT_RAW, "near-rejected-family", DataUse::Development);
    let root = task(ROOT_RAW, "dev-family", DataUse::Development);
    assert_ne!(rejected.raw_hash, root.raw_hash);
    assert_eq!(rejected.normalized_digest, root.normalized_digest);
    compare_after_rejection(
        &rejected,
        conflict("near_duplicate_normalized_digest"),
        &[probe(
            rejected.clone(),
            conflict("near_duplicate_normalized_digest"),
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("near_duplicate_normalized_digest"),
        &[probe(
            task(
                b"Distinct sibling after near rejection",
                "near-rejected-family",
                DataUse::AcceptanceEpoch,
            ),
            Observed::Ok,
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("near_duplicate_normalized_digest"),
        &history_controls(),
    );
}

#[test]
fn wrong_family_use_can_be_corrected_to_genuine_development() {
    let mut corrected = task(
        b"Development candidate with wrong purpose",
        "dev-family",
        DataUse::AcceptanceEpoch,
    );
    let rejected = corrected.clone();
    corrected.data_use = DataUse::Development;
    corrected_retry(
        &rejected,
        conflict("family_crosses_train_test_or_data_use"),
        corrected,
    );
}

#[test]
fn invalid_parent_can_be_corrected_and_does_not_leak_family() {
    let rejected = parent(
        task(
            b"Invalid parent development candidate",
            "invalid-parent-child",
            DataUse::Development,
        ),
        "bad/",
    );
    let corrected = parent(rejected.clone(), "dev-family");
    corrected_retry(&rejected, invalid_char(), corrected);
    compare_after_rejection(
        &rejected,
        invalid_char(),
        &[probe(
            task(
                b"Different raw after invalid parent",
                "invalid-parent-child",
                DataUse::ReplaySelect,
            ),
            Observed::Ok,
        )],
    );
}

#[test]
fn parent_use_rejection_adds_no_members_and_does_not_leak_family() {
    let rejected = parent(
        task(
            b"Parent purpose mismatch candidate",
            "mismatched-parent-child",
            DataUse::AcceptanceEpoch,
        ),
        "dev-family",
    );
    let mut corrected = rejected.clone();
    corrected.data_use = DataUse::Development;
    corrected_retry(
        &rejected,
        conflict("parent_family_data_use_mismatch"),
        corrected,
    );
    compare_after_rejection(
        &rejected,
        conflict("parent_family_data_use_mismatch"),
        &[probe(
            task(
                b"Different raw after parent mismatch",
                "mismatched-parent-child",
                DataUse::ReplayTrain,
            ),
            Observed::Ok,
        )],
    );
}

#[test]
fn exposed_holdout_rejection_adds_no_members_but_keeps_exposure() {
    let rejected = task(EXPOSED_RAW, "exposed-child", DataUse::AcceptanceEpoch);
    compare_after_rejection(
        &rejected,
        conflict("holdout_previously_exposed"),
        &[probe(
            task(
                EXPOSED_RAW,
                "another-exposed-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("holdout_previously_exposed"),
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("holdout_previously_exposed"),
        &[probe(
            task(
                EXPOSED_RAW,
                "development-only-exposed",
                DataUse::Development,
            ),
            Observed::Ok,
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("holdout_previously_exposed"),
        &[probe(
            task(
                b"Different raw after exposure rejection",
                "exposed-child",
                DataUse::Development,
            ),
            Observed::Ok,
        )],
    );
    compare_after_rejection(
        &rejected,
        conflict("holdout_previously_exposed"),
        &history_controls(),
    );
}

#[test]
fn multiple_rejections_match_a_reference_with_only_successful_history() {
    let mut attempted = seeded();
    let failures = [
        (
            task(NEAR_ROOT_RAW, "many-near-family", DataUse::Development),
            conflict("near_duplicate_normalized_digest"),
        ),
        (
            task(
                b"Many wrong family candidate",
                "dev-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("family_crosses_train_test_or_data_use"),
        ),
        (
            parent(
                task(
                    b"Many invalid parent candidate",
                    "many-invalid-parent-family",
                    DataUse::Development,
                ),
                "bad/",
            ),
            invalid_char(),
        ),
        (
            parent(
                task(
                    b"Many wrong parent candidate",
                    "many-parent-family",
                    DataUse::AcceptanceEpoch,
                ),
                "dev-family",
            ),
            conflict("parent_family_data_use_mismatch"),
        ),
        (
            task(EXPOSED_RAW, "many-exposed-family", DataUse::AcceptanceEpoch),
            conflict("holdout_previously_exposed"),
        ),
    ];
    for (task, expected) in &failures {
        assert_eq!(observe(attempted.admit(task)), *expected);
    }
    let mut reference = seeded();
    let probes = [
        probe(
            task(
                b"Many invalid-parent sibling",
                "many-invalid-parent-family",
                DataUse::ReplayTrain,
            ),
            Observed::Ok,
        ),
        probe(
            task(
                b"Many parent sibling",
                "many-parent-family",
                DataUse::ReplaySelect,
            ),
            Observed::Ok,
        ),
        probe(
            task(
                b"Many exposed sibling",
                "many-exposed-family",
                DataUse::OperationalMonitoring,
            ),
            Observed::Ok,
        ),
    ];
    for probe in probes.into_iter().chain(history_controls()) {
        let control = observe(reference.admit(&probe.task));
        assert_eq!(control, probe.expected);
        assert_eq!(observe(attempted.admit(&probe.task)), control);
    }
}

// Each precedence case starts from a separate, identical successful history.
fn precedence_case(index: usize) -> (PartitionRegistry, TaskIdentity, Observed) {
    let mut registry = seeded();
    let (mut candidate, expected) = match index {
        0 => {
            let mut candidate = task(ROOT_RAW, "valid-family", DataUse::AcceptanceEpoch);
            candidate.task_id.clear();
            candidate.family_id = "bad/".into();
            (candidate, invalid_empty())
        }
        1 => {
            let mut candidate = task(ROOT_RAW, "valid-family", DataUse::AcceptanceEpoch);
            candidate.family_id.clear();
            (candidate, invalid_empty())
        }
        2 => (
            task(ROOT_RAW, "dev-family", DataUse::AcceptanceEpoch),
            conflict("duplicate_raw_hash"),
        ),
        3 => (
            task(NEAR_ROOT_RAW, "dev-family", DataUse::AcceptanceEpoch),
            conflict("near_duplicate_normalized_digest"),
        ),
        4 => (
            task(
                b"Precedence family candidate",
                "dev-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("family_crosses_train_test_or_data_use"),
        ),
        5 => (
            task(
                b"Precedence invalid parent candidate",
                "priority-invalid-parent-family",
                DataUse::AcceptanceEpoch,
            ),
            invalid_empty(),
        ),
        6 => (
            task(
                b"Precedence parent purpose candidate",
                "priority-parent-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("parent_family_data_use_mismatch"),
        ),
        7 => (
            task(
                EXPOSED_RAW,
                "priority-exposed-family",
                DataUse::AcceptanceEpoch,
            ),
            conflict("holdout_previously_exposed"),
        ),
        _ => panic!("unknown precedence case"),
    };
    candidate.parent_id = match index {
        5 => Some("".into()),
        6 => Some("dev-family".into()),
        7 => None,
        _ => Some("bad/".into()),
    };
    registry.mark_exposed(candidate.raw_hash.clone());
    (registry, candidate, expected)
}

fn assert_precedence(index: usize) {
    let (mut registry, candidate, expected) = precedence_case(index);
    assert_eq!(observe(registry.admit(&candidate)), expected);
}

#[test]
fn task_id_error_precedes_family_duplicate_parent_and_exposure_errors() {
    assert_precedence(0);
}

#[test]
fn family_id_error_precedes_duplicate_parent_and_exposure_errors() {
    assert_precedence(1);
}

#[test]
fn raw_duplicate_precedes_normalized_family_parent_and_exposure_errors() {
    assert_precedence(2);
}

#[test]
fn normalized_duplicate_precedes_family_parent_and_exposure_errors() {
    assert_precedence(3);
}

#[test]
fn existing_family_use_error_precedes_parent_and_exposure_errors() {
    assert_precedence(4);
}

#[test]
fn parent_identifier_error_precedes_exposure_error() {
    assert_precedence(5);
}

#[test]
fn parent_family_use_error_precedes_exposure_error() {
    assert_precedence(6);
}

#[test]
fn successful_raw_duplicate_remains_rejected() {
    let mut registry = PartitionRegistry::default();
    let first = task(
        b"Successful raw fixture",
        "raw-success-family",
        DataUse::Development,
    );
    registry.admit(&first).unwrap();
    let same_raw = task(
        b"Successful raw fixture",
        "another-success-family",
        DataUse::ReplayTrain,
    );
    assert_eq!(
        observe(registry.admit(&same_raw)),
        conflict("duplicate_raw_hash")
    );
}

#[test]
fn genuine_case_and_whitespace_near_duplicate_remains_rejected() {
    let mut registry = PartitionRegistry::default();
    let first = task(ROOT_RAW, "dev-family", DataUse::Development);
    let near = task(NEAR_ROOT_RAW, "different-family", DataUse::Development);
    assert_ne!(first.raw_hash, near.raw_hash);
    assert_eq!(first.normalized_digest, near.normalized_digest);
    registry.admit(&first).unwrap();
    assert_eq!(
        observe(registry.admit(&near)),
        conflict("near_duplicate_normalized_digest")
    );
}

#[test]
fn existing_family_purpose_is_preserved_for_all_five_data_uses() {
    for (index, data_use) in USES.into_iter().enumerate() {
        let mut registry = PartitionRegistry::default();
        registry
            .admit(&task(
                format!("Family root {index}").as_bytes(),
                "five-use-family",
                data_use,
            ))
            .unwrap();
        for (other_index, other_use) in USES.into_iter().enumerate() {
            if other_use != data_use {
                let candidate = task(
                    format!("Wrong family use {index} {other_index}").as_bytes(),
                    "five-use-family",
                    other_use,
                );
                assert_eq!(
                    observe(registry.admit(&candidate)),
                    conflict("family_crosses_train_test_or_data_use")
                );
            }
        }
        registry
            .admit(&task(
                format!("Same family purpose after rejects {index}").as_bytes(),
                "five-use-family",
                data_use,
            ))
            .unwrap();
    }
}

#[test]
fn same_purpose_parent_is_permitted_for_all_five_data_uses() {
    for (index, data_use) in USES.into_iter().enumerate() {
        let mut registry = PartitionRegistry::default();
        registry
            .admit(&task(
                format!("Same purpose parent {index}").as_bytes(),
                "parent-family",
                data_use,
            ))
            .unwrap();
        let child = parent(
            task(
                format!("Same purpose child {index}").as_bytes(),
                "child-family",
                data_use,
            ),
            "parent-family",
        );
        registry.admit(&child).unwrap();
    }
}

#[test]
fn different_purpose_parent_is_rejected_for_all_five_data_uses() {
    for (index, parent_use) in USES.into_iter().enumerate() {
        for (other_index, child_use) in USES.into_iter().enumerate() {
            if child_use != parent_use {
                let mut registry = PartitionRegistry::default();
                registry
                    .admit(&task(
                        format!("Different purpose root {index} {other_index}").as_bytes(),
                        "parent-family",
                        parent_use,
                    ))
                    .unwrap();
                let child = parent(
                    task(
                        format!("Different purpose child {index} {other_index}").as_bytes(),
                        "child-family",
                        child_use,
                    ),
                    "parent-family",
                );
                assert_eq!(
                    observe(registry.admit(&child)),
                    conflict("parent_family_data_use_mismatch")
                );
            }
        }
    }
}

#[test]
fn unknown_parent_remains_permitted() {
    let mut registry = seeded();
    let child = parent(
        task(
            b"Unknown parent compatibility fixture",
            "unknown-parent-family",
            DataUse::AcceptanceEpoch,
        ),
        "not-registered-family",
    );
    registry.admit(&child).unwrap();
}

#[test]
fn self_parent_without_preexisting_family_remains_permitted() {
    for (index, data_use) in USES.into_iter().enumerate() {
        let mut registry = seeded();
        let child = parent(
            task(
                format!("Self parent compatibility {index}").as_bytes(),
                "self-parent-family",
                data_use,
            ),
            "self-parent-family",
        );
        registry.admit(&child).unwrap();
    }
}

#[test]
fn parent_lookup_remains_by_family_key_rather_than_task_id() {
    let mut registry = PartitionRegistry::default();
    let mut root = task(
        b"Parent lookup root",
        "actual-parent-family",
        DataUse::Development,
    );
    root.task_id = "root-task-id".into();
    registry.admit(&root).unwrap();
    let by_task_id = parent(
        task(
            b"Task id is unknown parent key",
            "by-task-id-family",
            DataUse::AcceptanceEpoch,
        ),
        "root-task-id",
    );
    registry.admit(&by_task_id).unwrap();
    let by_family_id = parent(
        task(
            b"Family id remains known parent key",
            "by-family-id-family",
            DataUse::AcceptanceEpoch,
        ),
        "actual-parent-family",
    );
    assert_eq!(
        observe(registry.admit(&by_family_id)),
        conflict("parent_family_data_use_mismatch")
    );
}

#[test]
fn same_task_id_with_distinct_raw_remains_permitted() {
    let mut registry = PartitionRegistry::default();
    for (index, data_use) in USES.into_iter().enumerate() {
        let candidate = task(
            format!("Same identifier different genuine bytes {index}").as_bytes(),
            &format!("same-id-family-{index}"),
            data_use,
        );
        registry.admit(&candidate).unwrap();
    }
}

#[test]
fn source_repo_remains_unvalidated_metadata() {
    let mut registry = PartitionRegistry::default();
    let mut candidate = task(
        b"Source metadata compatibility fixture",
        "source-metadata-family",
        DataUse::AcceptanceEpoch,
    );
    candidate.source_repo = Some("/unknown repository\0metadata".into());
    registry.admit(&candidate).unwrap();
}

#[test]
fn exposure_is_sticky_across_failed_attempts_and_new_family() {
    let mut registry = seeded();
    let bad_parent = parent(
        task(
            EXPOSED_RAW,
            "exposed-invalid-parent",
            DataUse::AcceptanceEpoch,
        ),
        "bad/",
    );
    assert_eq!(observe(registry.admit(&bad_parent)), invalid_char());
    for family in [
        "exposed-invalid-parent",
        "exposed-new-family",
        "exposed-third-family",
    ] {
        let candidate = task(EXPOSED_RAW, family, DataUse::AcceptanceEpoch);
        assert_eq!(
            observe(registry.admit(&candidate)),
            conflict("holdout_previously_exposed")
        );
    }
    let sibling = task(
        b"Distinct development after exposed attempts",
        "exposed-invalid-parent",
        DataUse::Development,
    );
    registry.admit(&sibling).unwrap();
}

#[test]
fn mark_exposed_does_not_revoke_existing_successful_records() {
    let mut registry = seeded();
    registry.mark_exposed(evo_core::hash(ROOT_RAW));
    registry.mark_exposed(evo_core::hash(ROOT_RAW));
    let duplicate = task(
        ROOT_RAW,
        "new-exposed-root-family",
        DataUse::AcceptanceEpoch,
    );
    assert_eq!(
        observe(registry.admit(&duplicate)),
        conflict("duplicate_raw_hash")
    );
    let wrong_use = task(
        b"Previously successful family still authoritative",
        "dev-family",
        DataUse::ReplayTrain,
    );
    assert_eq!(
        observe(registry.admit(&wrong_use)),
        conflict("family_crosses_train_test_or_data_use")
    );
    registry
        .admit(&task(
            b"Genuine development after exposure mark",
            "dev-family",
            DataUse::Development,
        ))
        .unwrap();
}

#[test]
fn exposure_restriction_remains_acceptance_epoch_only() {
    for (index, data_use) in USES.into_iter().enumerate() {
        let mut registry = PartitionRegistry::default();
        let candidate = task(
            format!("Exposed use compatibility {index}").as_bytes(),
            "exposed-use-family",
            data_use,
        );
        registry.mark_exposed(candidate.raw_hash.clone());
        let expected = if data_use == DataUse::AcceptanceEpoch {
            conflict("holdout_previously_exposed")
        } else {
            Observed::Ok
        };
        assert_eq!(observe(registry.admit(&candidate)), expected);
    }
}

#[test]
fn lowercase_whitespace_and_lossy_normalization_remain_unchanged() {
    for (raw, expected) in [
        (NEAR_ROOT_RAW, "root development source"),
        (" \u{0130}\u{2003}ABC\r\n ".as_bytes(), "i\u{0307} abc"),
        (&b"A\t\xff\nB"[..], "a \u{fffd} b"),
    ] {
        assert_eq!(normalize_for_near_dup(raw), expected);
        let identity = task(raw, "normalization-family", DataUse::Development);
        assert_eq!(identity.raw_hash, evo_core::hash(raw));
        assert_eq!(
            identity.normalized_digest,
            evo_core::hash(expected.as_bytes())
        );
    }
}

#[derive(Serialize)]
struct WireRecord {
    label: String,
    task_json: String,
    task_fingerprint: String,
    result: Observed,
}

fn capture(
    registry: &mut PartitionRegistry,
    label: String,
    task: TaskIdentity,
    expected: Observed,
) -> WireRecord {
    let result = observe(registry.admit(&task));
    assert_eq!(result, expected, "{label}");
    WireRecord {
        label,
        task_json: serde_json::to_string(&task).unwrap(),
        task_fingerprint: fingerprint(&task).unwrap(),
        result,
    }
}

#[derive(Serialize)]
struct Golden {
    schema: &'static str,
    valid_sequence: Vec<WireRecord>,
    independent_first_errors: Vec<WireRecord>,
}

// Run this exact test with --nocapture on the real parent before product changes.
// Retain its emitted bytes/digest, then independently compare real head output.
// No hand-written fingerprint or failed-call half-write sequence is the oracle.
#[test]
fn valid_sequences_and_first_errors_have_stable_wire_evidence() {
    let mut registry = PartitionRegistry::default();
    let mut valid_sequence = Vec::new();
    for (index, data_use) in USES.into_iter().enumerate() {
        let family = format!("wire-family-{index}");
        valid_sequence.push(capture(
            &mut registry,
            format!("root-{index}"),
            task(format!("Wire root {index}").as_bytes(), &family, data_use),
            Observed::Ok,
        ));
        valid_sequence.push(capture(
            &mut registry,
            format!("same-purpose-parent-{index}"),
            parent(
                task(
                    format!("Wire child {index}").as_bytes(),
                    &format!("wire-child-family-{index}"),
                    data_use,
                ),
                &family,
            ),
            Observed::Ok,
        ));
        let self_family = format!("wire-self-family-{index}");
        valid_sequence.push(capture(
            &mut registry,
            format!("self-parent-{index}"),
            parent(
                task(
                    format!("Wire self parent {index}").as_bytes(),
                    &self_family,
                    data_use,
                ),
                &self_family,
            ),
            Observed::Ok,
        ));
        valid_sequence.push(capture(
            &mut registry,
            format!("unknown-parent-{index}"),
            parent(
                task(
                    format!("Wire unknown parent {index}").as_bytes(),
                    &format!("wire-unknown-family-{index}"),
                    data_use,
                ),
                "wire-unregistered-parent",
            ),
            Observed::Ok,
        ));
    }
    let mut metadata = task(
        b"Wire source metadata",
        "wire-source-family",
        DataUse::AcceptanceEpoch,
    );
    metadata.source_repo = Some("/unknown repository\0metadata".into());
    valid_sequence.push(capture(
        &mut registry,
        "source-metadata".into(),
        metadata,
        Observed::Ok,
    ));
    for (index, raw) in [
        NEAR_ROOT_RAW,
        " \u{0130}\u{2003}ABC\r\n ".as_bytes(),
        &b"A\t\xff\nB"[..],
    ]
    .into_iter()
    .enumerate()
    {
        valid_sequence.push(capture(
            &mut registry,
            format!("normalization-{index}"),
            task(
                raw,
                &format!("wire-normalization-family-{index}"),
                DataUse::Development,
            ),
            Observed::Ok,
        ));
    }
    let independent_first_errors = (0..8)
        .map(|index| {
            let (mut registry, task, expected) = precedence_case(index);
            capture(
                &mut registry,
                format!("first-error-{index}"),
                task,
                expected,
            )
        })
        .collect();
    let golden = Golden {
        schema: "ag083.valid_and_first_error.v1",
        valid_sequence,
        independent_first_errors,
    };
    assert_eq!(golden.valid_sequence.len(), 24);
    assert_eq!(golden.independent_first_errors.len(), 8);
    println!(
        "AG083_GOLDEN_JSON={}",
        serde_json::to_string(&golden).unwrap()
    );
    println!("AG083_GOLDEN_DIGEST={}", fingerprint(&golden).unwrap());
}
