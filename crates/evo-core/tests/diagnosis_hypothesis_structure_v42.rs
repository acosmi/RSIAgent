//! Pure-helper coverage: these synthetic inputs do not attest a real application.

use evo_core::contract::{AppliedReceipt, CapabilityLevel};
use evo_core::evidence::ExecutionAttestation;
use evo_core::optimization::{
    ApplicationObservation, RuleHypothesis, RuleHypothesisKind, SelectionEvidence,
    SkillFailureDiagnosis, SkillFailureKind, diagnose_application,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Error, Result, hash};
use serde::Serialize;

const SECRET: &str = "AG079_SECRET";

fn evidence_ref(id: &str) -> EvidenceRef {
    EvidenceRef {
        id: id.into(),
        digest: hash(id.as_bytes()),
    }
}

fn hypothesis(kind: RuleHypothesisKind) -> RuleHypothesis {
    RuleHypothesis {
        kind,
        rule_id: "rule-a".into(),
        behavior_evidence_digest: hash(b"behavior"),
        support: vec![evidence_ref("run-failure")],
        counterexamples: vec![evidence_ref("run-success")],
    }
}

fn attached_receipt() -> AppliedReceipt {
    AppliedReceipt {
        offered: vec!["skill-a".into()],
        attached: vec!["skill-a".into()],
        used: vec![],
        verified_benefit: vec![],
        bundle_digest: hash(b"bundle-a"),
        request_digest: hash(b"request-a"),
        capability_level: CapabilityLevel::Attached,
        truncated: false,
        attested_by: "trusted-host".into(),
    }
}

#[derive(Clone)]
struct Case {
    name: &'static str,
    skill_id: String,
    bundle_digest: String,
    request_digest: String,
    selection: SelectionEvidence,
    receipt: Option<AppliedReceipt>,
    attestation: Option<ExecutionAttestation>,
    context_matches: bool,
    environment_failure: bool,
    expected_kind: SkillFailureKind,
    expected_reason: &'static str,
}

impl Case {
    fn full() -> Self {
        Self {
            name: "fully_attached",
            skill_id: "skill-a".into(),
            bundle_digest: hash(b"bundle-a"),
            request_digest: hash(b"request-a"),
            selection: SelectionEvidence::Selected,
            receipt: Some(attached_receipt()),
            attestation: Some(ExecutionAttestation::TrustedHost),
            context_matches: true,
            environment_failure: false,
            expected_kind: SkillFailureKind::SkillDefect,
            expected_reason: "localized skill defect hypothesis",
        }
    }

    fn observe(&self, hypothesis: Option<&RuleHypothesis>) -> Result<SkillFailureDiagnosis> {
        diagnose_application(ApplicationObservation {
            skill_id: &self.skill_id,
            expected_bundle_digest: &self.bundle_digest,
            expected_request_digest: &self.request_digest,
            selection: self.selection,
            receipt: self.receipt.as_ref(),
            request_attestation: self.attestation,
            context_matches: self.context_matches,
            environment_or_capability_failure: self.environment_failure,
            hypothesis,
        })
    }
}

fn early_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |name, kind, reason, change: fn(&mut Case)| {
        let mut case = Case::full();
        case.name = name;
        case.expected_kind = kind;
        case.expected_reason = reason;
        change(&mut case);
        cases.push(case);
    };
    push(
        "not_selected",
        SkillFailureKind::NotSelected,
        "skill was not selected",
        |case| case.selection = SelectionEvidence::NotSelected,
    );
    push(
        "receipt_missing",
        SkillFailureKind::Uncertain,
        "trusted application receipt missing",
        |case| case.receipt = None,
    );
    push(
        "bundle_mismatch",
        SkillFailureKind::ContextMismatch,
        "request or context does not match",
        |case| case.receipt.as_mut().unwrap().bundle_digest = hash(b"other-bundle"),
    );
    push(
        "request_mismatch",
        SkillFailureKind::ContextMismatch,
        "request or context does not match",
        |case| case.receipt.as_mut().unwrap().request_digest = hash(b"other-request"),
    );
    push(
        "context_false",
        SkillFailureKind::ContextMismatch,
        "request or context does not match",
        |case| case.context_matches = false,
    );
    push(
        "truncated",
        SkillFailureKind::ContextMismatch,
        "request or context does not match",
        |case| case.receipt.as_mut().unwrap().truncated = true,
    );
    push(
        "tool_only",
        SkillFailureKind::NotAttached,
        "selected skill was not attached",
        |case| {
            let receipt = case.receipt.as_mut().unwrap();
            receipt.attached.clear();
            receipt.capability_level = CapabilityLevel::ToolOnly;
        },
    );
    push(
        "not_attached",
        SkillFailureKind::NotAttached,
        "selected skill was not attached",
        |case| case.receipt.as_mut().unwrap().attached.clear(),
    );
    push(
        "environment_failure",
        SkillFailureKind::EnvironmentOrCapability,
        "environment or capability failure",
        |case| case.environment_failure = true,
    );
    cases
}

const HYPOTHESIS_KINDS: [RuleHypothesisKind; 4] = [
    RuleHypothesisKind::Missing,
    RuleHypothesisKind::Incorrect,
    RuleHypothesisKind::Ambiguous,
    RuleHypothesisKind::NotFollowed,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedError {
    Invalid(&'static str),
    Conflict(&'static str),
}

impl ExpectedError {
    fn matches(self, error: &Error) -> bool {
        match (self, error) {
            (Self::Invalid(expected), Error::Invalid(actual))
            | (Self::Conflict(expected), Error::Conflict(actual)) => expected == actual,
            _ => false,
        }
    }
}

struct InvalidHypothesis {
    name: &'static str,
    value: RuleHypothesis,
    error: ExpectedError,
}

fn invalid_hypotheses(kind: RuleHypothesisKind) -> Vec<InvalidHypothesis> {
    let mut cases = Vec::new();
    let mut push = |name, error, change: fn(&mut RuleHypothesis)| {
        let mut value = hypothesis(kind);
        change(&mut value);
        cases.push(InvalidHypothesis { name, value, error });
    };
    let empty_or_long = ExpectedError::Invalid("identifier: expected nonempty text <= 128 bytes");
    let bad_id = ExpectedError::Invalid("invalid identifier");
    let behavior = ExpectedError::Invalid("behavior evidence digest must be a lowercase sha256");
    let evidence = ExpectedError::Invalid("evidence digest must be a lowercase sha256");
    let conflict = ExpectedError::Conflict("evidence id has conflicting digests");
    push("empty_rule", empty_or_long, |value| value.rule_id.clear());
    push("bad_rule", bad_id, |value| {
        value.rule_id = format!("{SECRET}/rule")
    });
    push("unicode_rule", bad_id, |value| {
        value.rule_id = "条款".into()
    });
    push("long_rule", empty_or_long, |value| {
        value.rule_id = "r".repeat(129)
    });
    push("empty_behavior", behavior, |value| {
        value.behavior_evidence_digest.clear()
    });
    push("bad_behavior", behavior, |value| {
        value.behavior_evidence_digest = SECRET.into()
    });
    push("uppercase_behavior", behavior, |value| {
        value.behavior_evidence_digest = "A".repeat(64)
    });
    push("short_behavior", behavior, |value| {
        value.behavior_evidence_digest = "a".repeat(63)
    });
    push("long_behavior", behavior, |value| {
        value.behavior_evidence_digest = "a".repeat(65)
    });
    push(
        "empty_support",
        ExpectedError::Invalid("rule hypothesis lacks supporting evidence"),
        |value| value.support.clear(),
    );
    push("empty_support_id", empty_or_long, |value| {
        value.support[0].id.clear()
    });
    push("bad_support_id", bad_id, |value| {
        value.support[0].id = format!("{SECRET}/support")
    });
    push("long_support_id", empty_or_long, |value| {
        value.support[0].id = "s".repeat(129)
    });
    push("bad_support_digest", evidence, |value| {
        value.support[0].digest = SECRET.into()
    });
    push("uppercase_support_digest", evidence, |value| {
        value.support[0].digest = "B".repeat(64)
    });
    push("short_support_digest", evidence, |value| {
        value.support[0].digest = "b".repeat(63)
    });
    push("empty_counter_id", empty_or_long, |value| {
        value.counterexamples[0].id.clear()
    });
    push("bad_counter_id", bad_id, |value| {
        value.counterexamples[0].id = format!("{SECRET}/counter")
    });
    push("bad_counter_digest", evidence, |value| {
        value.counterexamples[0].digest = SECRET.into()
    });
    push("uppercase_counter_digest", evidence, |value| {
        value.counterexamples[0].digest = "C".repeat(64)
    });
    push("short_counter_digest", evidence, |value| {
        value.counterexamples[0].digest = "c".repeat(63)
    });
    push("support_internal_conflict", conflict, |value| {
        let mut other = value.support[0].clone();
        other.digest = hash(b"different-support-content");
        value.support.push(other);
    });
    push("counter_internal_conflict", conflict, |value| {
        let mut other = value.counterexamples[0].clone();
        other.digest = hash(b"different-counter-content");
        value.counterexamples.push(other);
    });
    push("support_counter_conflict", conflict, |value| {
        let mut other = value.support[0].clone();
        other.digest = hash(b"different-cross-content");
        value.counterexamples.push(other);
    });
    cases
}

fn assert_invalid_matrix(case: &Case) {
    let receipt_before = serde_json::to_vec(&case.receipt).unwrap();
    let mut failures = Vec::new();
    let mut cells = 0;
    for kind in HYPOTHESIS_KINDS {
        for invalid in invalid_hypotheses(kind) {
            cells += 1;
            let hypothesis_before = serde_json::to_vec(&invalid.value).unwrap();
            let first = case.observe(Some(&invalid.value));
            let second = case.observe(Some(&invalid.value));
            match (first, second) {
                (Err(first), Err(second)) => {
                    if !invalid.error.matches(&first) || !invalid.error.matches(&second) {
                        failures.push(format!(
                            "{:?}/{}: expected {:?}, got {first:?}/{second:?}",
                            kind, invalid.name, invalid.error
                        ));
                    }
                    assert!(!first.to_string().contains(SECRET));
                    assert!(!second.to_string().contains(SECRET));
                }
                (first, second) => failures.push(format!(
                    "{:?}/{}: expected {:?}, got {first:?}/{second:?}",
                    kind, invalid.name, invalid.error
                )),
            }
            assert_eq!(
                serde_json::to_vec(&invalid.value).unwrap(),
                hypothesis_before
            );
        }
    }
    assert_eq!(cells, 96, "the full matrix must run");
    assert_eq!(serde_json::to_vec(&case.receipt).unwrap(), receipt_before);
    assert!(
        failures.is_empty(),
        "{}: {}/96 matrix cells failed:\n{}",
        case.name,
        failures.len(),
        failures.join("\n")
    );
}

macro_rules! early_matrix {
    ($test:ident, $case:literal) => {
        #[test]
        fn $test() {
            let case = early_cases()
                .into_iter()
                .find(|case| case.name == $case)
                .unwrap();
            assert_invalid_matrix(&case);
        }
    };
}

early_matrix!(not_selected_rejects_all_invalid_hypotheses, "not_selected");
early_matrix!(
    missing_receipt_rejects_all_invalid_hypotheses,
    "receipt_missing"
);
early_matrix!(
    bundle_mismatch_rejects_all_invalid_hypotheses,
    "bundle_mismatch"
);
early_matrix!(
    request_mismatch_rejects_all_invalid_hypotheses,
    "request_mismatch"
);
early_matrix!(
    context_false_rejects_all_invalid_hypotheses,
    "context_false"
);
early_matrix!(truncated_rejects_all_invalid_hypotheses, "truncated");
early_matrix!(tool_only_rejects_all_invalid_hypotheses, "tool_only");
early_matrix!(not_attached_rejects_all_invalid_hypotheses, "not_attached");
early_matrix!(
    environment_failure_rejects_all_invalid_hypotheses,
    "environment_failure"
);

#[test]
fn final_paths_preserve_existing_structure_errors() {
    for attestation in [
        None,
        Some(ExecutionAttestation::UnverifiedImport),
        Some(ExecutionAttestation::TrustedHost),
    ] {
        let mut case = Case::full();
        case.attestation = attestation;
        assert_invalid_matrix(&case);
    }
}

fn assert_copied(
    case: &Case,
    value: Option<&RuleHypothesis>,
    expected_kind: SkillFailureKind,
    reason: &str,
) {
    let before = serde_json::to_vec(&(value, &case.receipt)).unwrap();
    let diagnosed = case.observe(value).unwrap();
    assert_eq!(diagnosed.kind, expected_kind, "{}", case.name);
    assert_eq!(diagnosed.reason, reason, "{}", case.name);
    assert_eq!(diagnosed.skill_id, case.skill_id);
    assert_eq!(diagnosed.bundle_digest, case.bundle_digest);
    assert_eq!(diagnosed.request_digest, case.request_digest);
    if let Some(value) = value {
        assert_eq!(diagnosed.rule_id.as_ref(), Some(&value.rule_id));
        assert_eq!(diagnosed.support, value.support);
        assert_eq!(diagnosed.counterexamples, value.counterexamples);
    } else {
        assert!(diagnosed.rule_id.is_none());
        assert!(diagnosed.support.is_empty());
        assert!(diagnosed.counterexamples.is_empty());
    }
    assert_eq!(serde_json::to_vec(&(value, &case.receipt)).unwrap(), before);
}

#[test]
fn valid_hypotheses_preserve_all_seven_kinds_and_reasons() {
    for case in early_cases() {
        for kind in HYPOTHESIS_KINDS {
            assert_copied(
                &case,
                Some(&hypothesis(kind)),
                case.expected_kind,
                case.expected_reason,
            );
        }
    }
    for attestation in [
        None,
        Some(ExecutionAttestation::UnverifiedImport),
        Some(ExecutionAttestation::TrustedHost),
    ] {
        let mut case = Case::full();
        case.attestation = attestation;
        for kind in HYPOTHESIS_KINDS {
            let (expected, reason) = match kind {
                RuleHypothesisKind::NotFollowed
                    if attestation == Some(ExecutionAttestation::TrustedHost) =>
                {
                    (
                        SkillFailureKind::ExecutionLapse,
                        "localized rule was not followed",
                    )
                }
                RuleHypothesisKind::NotFollowed => (
                    SkillFailureKind::Uncertain,
                    "execution lapse lacks a trusted request attestation",
                ),
                _ => (
                    SkillFailureKind::SkillDefect,
                    "localized skill defect hypothesis",
                ),
            };
            assert_copied(&case, Some(&hypothesis(kind)), expected, reason);
        }
    }
}

#[test]
fn no_hypothesis_keeps_all_existing_returns() {
    for case in early_cases() {
        assert_copied(&case, None, case.expected_kind, case.expected_reason);
    }
    assert_copied(
        &Case::full(),
        None,
        SkillFailureKind::Uncertain,
        "no localized rule hypothesis",
    );
}

#[test]
fn equal_digest_duplicates_keep_order_and_are_not_dropped() {
    let mut value = hypothesis(RuleHypothesisKind::Incorrect);
    value.support.push(value.support[0].clone());
    value.counterexamples.push(value.counterexamples[0].clone());
    value.counterexamples.push(value.support[0].clone());
    for case in early_cases().into_iter().chain([Case::full()]) {
        assert_copied(
            &case,
            Some(&value),
            case.expected_kind,
            case.expected_reason,
        );
    }
}

#[test]
fn no_new_reference_cap_or_identifier_restriction() {
    let mut value = hypothesis(RuleHypothesisKind::Incorrect);
    value.rule_id = format!(".:_-{}", "r".repeat(124));
    value.support = vec![evidence_ref(".:_-"); 33];
    value.counterexamples = value.support.clone();
    for case in early_cases().into_iter().chain([Case::full()]) {
        assert_copied(
            &case,
            Some(&value),
            case.expected_kind,
            case.expected_reason,
        );
    }
}

#[test]
fn prior_input_errors_keep_their_original_precedence() {
    let mut cases = Vec::new();
    let mut push = |expected, change: fn(&mut Case)| {
        let mut case = Case::full();
        change(&mut case);
        cases.push((case, expected));
    };
    push("identifier: expected nonempty text <= 128 bytes", |case| {
        case.skill_id.clear()
    });
    push("invalid identifier", |case| {
        case.skill_id = format!("{SECRET}/skill")
    });
    push("bundle digest must be a lowercase sha256", |case| {
        case.bundle_digest = SECRET.into()
    });
    push("request digest must be a lowercase sha256", |case| {
        case.request_digest = SECRET.into()
    });
    push("invalid identifier", |case| {
        case.receipt.as_mut().unwrap().bundle_digest = format!("{SECRET}/bundle")
    });
    push("invalid identifier", |case| {
        case.receipt.as_mut().unwrap().attested_by = format!("{SECRET}/actor")
    });
    push("attached must be subset of offered", |case| {
        case.receipt.as_mut().unwrap().offered.clear()
    });
    push("used must be subset of attached", |case| {
        case.receipt.as_mut().unwrap().used = vec![SECRET.into()]
    });
    push("verified_benefit must be subset of used", |case| {
        case.receipt.as_mut().unwrap().verified_benefit = vec!["skill-a".into()]
    });
    push("tool-only receipts may only claim offered", |case| {
        case.receipt.as_mut().unwrap().capability_level = CapabilityLevel::ToolOnly
    });
    push("truncated projection cannot claim used", |case| {
        let receipt = case.receipt.as_mut().unwrap();
        receipt.truncated = true;
        receipt.used = vec!["skill-a".into()];
    });
    let mut invalid = hypothesis(RuleHypothesisKind::NotFollowed);
    invalid.rule_id.clear();
    for (case, expected) in cases {
        let error = case.observe(Some(&invalid)).unwrap_err();
        assert!(
            ExpectedError::Invalid(expected).matches(&error),
            "{error:?}"
        );
        assert!(!error.to_string().contains(SECRET));
    }
    // NotSelected still ignores an unused invalid receipt, just as the parent did.
    let mut not_selected = early_cases().remove(0);
    not_selected.receipt.as_mut().unwrap().attested_by = format!("{SECRET}/actor");
    assert_copied(
        &not_selected,
        Some(&hypothesis(RuleHypothesisKind::Incorrect)),
        SkillFailureKind::NotSelected,
        "skill was not selected",
    );
}

#[test]
fn repeated_calls_and_serde_roundtrips_have_identical_outputs() {
    for case in early_cases().into_iter().chain([Case::full()]) {
        for kind in HYPOTHESIS_KINDS {
            let value = hypothesis(kind);
            let decoded: RuleHypothesis =
                serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
            let mut decoded_case = case.clone();
            decoded_case.receipt =
                serde_json::from_slice(&serde_json::to_vec(&case.receipt).unwrap()).unwrap();
            let first = serde_json::to_vec(&case.observe(Some(&value)).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_vec(&case.observe(Some(&value)).unwrap()).unwrap(),
                first
            );
            assert_eq!(
                serde_json::to_vec(&decoded_case.observe(Some(&decoded)).unwrap()).unwrap(),
                first
            );
            let diagnosis: SkillFailureDiagnosis = serde_json::from_slice(&first).unwrap();
            assert_eq!(serde_json::to_vec(&diagnosis).unwrap(), first);
        }
    }
}

#[derive(Serialize)]
struct WireEntry {
    branch: &'static str,
    hypothesis_kind: Option<RuleHypothesisKind>,
    attestation: Option<ExecutionAttestation>,
    duplicates: bool,
    diagnosis: SkillFailureDiagnosis,
}

fn valid_wire() -> Vec<u8> {
    let mut entries = Vec::new();
    for case in early_cases().into_iter().chain([Case::full()]) {
        for kind in HYPOTHESIS_KINDS {
            for duplicates in [false, true] {
                let mut value = hypothesis(kind);
                if duplicates {
                    value.support.push(value.support[0].clone());
                    value.counterexamples.push(value.support[0].clone());
                }
                entries.push(WireEntry {
                    branch: case.name,
                    hypothesis_kind: Some(kind),
                    attestation: case.attestation,
                    duplicates,
                    diagnosis: case.observe(Some(&value)).unwrap(),
                });
            }
        }
        entries.push(WireEntry {
            branch: case.name,
            hypothesis_kind: None,
            attestation: case.attestation,
            duplicates: false,
            diagnosis: case.observe(None).unwrap(),
        });
    }
    for attestation in [None, Some(ExecutionAttestation::UnverifiedImport)] {
        let mut case = Case::full();
        case.attestation = attestation;
        for kind in HYPOTHESIS_KINDS {
            entries.push(WireEntry {
                branch: case.name,
                hypothesis_kind: Some(kind),
                attestation,
                duplicates: false,
                diagnosis: case.observe(Some(&hypothesis(kind))).unwrap(),
            });
        }
    }
    assert_eq!(entries.len(), 98);
    serde_json::to_vec(&entries).unwrap()
}

// These 98 helper outputs were captured by executing the unchanged parent
// 212c78488b403397eaebaa65a6d280b9c6be3899. They do not prove real application.
#[test]
fn valid_wire_matches_real_parent_golden() {
    let wire = valid_wire();
    assert_eq!(
        hash(&wire),
        "c322250c1ccf9587186791beb3c477ef0736e677df91799a8539b4dad3102801"
    );
    println!("AG079_WIRE_BEGIN");
    println!("{}", String::from_utf8(wire).unwrap());
    println!("AG079_WIRE_END");
}
