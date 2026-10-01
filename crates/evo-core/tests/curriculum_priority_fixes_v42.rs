//! E12 / E04 (plan v4.2 §1.4, AG-050): text validation never marks a proposal
//! oracle-verified.
//!
//! `proposal_from_text` checks the shape of the text and nothing else. A shape check
//! says nothing about whether the task has a sound oracle, and neither the model nor
//! the constructor may self-report that (V030, V083.a). A proposal built from text is
//! therefore born unverified: it is quarantined as `no_oracle` and `next_task` never
//! selects it. Only an independent ValidityReport with real check references can
//! verify a proposal; that path is not part of this change.

use evo_core::Error;
use evo_core::curriculum::{LearnerState, TaskProposal, next_task, proposal_from_text};
use evo_core::evaluation::DataUse;

const FAMILY: &str = "fam-a";

fn learner() -> LearnerState {
    LearnerState {
        checkpoint: "checkpoint-1".into(),
        failure_clusters: vec![FAMILY.into()],
    }
}

/// A proposal whose oracle flag was set by something other than text validation
/// (a legacy v1 fixture). It is the contrast case: it stays selectable.
fn compliant(id: &str) -> TaskProposal {
    TaskProposal {
        id: id.into(),
        parent_family: FAMILY.into(),
        data_use: DataUse::Development,
        difficulty: 0.5,
        learning_value: 0.5,
        correct: false,
        oracle_ok: true,
    }
}

fn from_text(id: &str) -> TaskProposal {
    proposal_from_text(id, FAMILY, "Return the sum of two integers.").unwrap()
}

#[test]
fn text_derived_proposal_is_unverified_and_quarantined() {
    let proposal = from_text("t-text");
    assert!(
        !proposal.oracle_ok,
        "text validation must not produce oracle_ok=true"
    );
    assert_eq!(proposal.quarantine_reason(), Some("no_oracle"));
    // Text-derived proposals stay development-only and are not self-declared correct.
    assert_eq!(proposal.data_use, DataUse::Development);
    assert!(!proposal.correct);
    // Still a structurally valid proposal; it is only unverified.
    proposal.validate().unwrap();
}

#[test]
fn next_task_never_selects_a_text_derived_proposal() {
    let only_text = [from_text("t-text")];
    assert!(matches!(
        next_task(&learner(), &only_text),
        Err(Error::NotFound)
    ));
}

#[test]
fn a_compliant_proposal_is_selected_over_a_text_derived_one() {
    // The text-derived proposal comes first in the pool, so a selection that trusted
    // it would pick it; the compliant proposal must win in both pool orders.
    let text_first = [from_text("t-text"), compliant("t-ok")];
    assert_eq!(next_task(&learner(), &text_first).unwrap(), "t-ok");
    let compliant_first = [compliant("t-ok"), from_text("t-text")];
    assert_eq!(next_task(&learner(), &compliant_first).unwrap(), "t-ok");
}

#[test]
fn text_validation_still_rejects_malformed_bodies() {
    for body in ["", "   \n\t", "has\0nul"] {
        assert!(
            matches!(
                proposal_from_text("t-text", FAMILY, body),
                Err(Error::Invalid(_))
            ),
            "body {body:?} must stay rejected"
        );
    }
    let longest = "a".repeat(4096);
    proposal_from_text("t-text", FAMILY, &longest).unwrap();
    let too_long = "a".repeat(4097);
    assert!(matches!(
        proposal_from_text("t-text", FAMILY, &too_long),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn proposal_serialization_shape_is_unchanged() {
    // Only the value of `oracle_ok` changes; the field set and the other defaults do not.
    let value = serde_json::to_value(from_text("t-text")).unwrap();
    let object = value.as_object().unwrap();
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "correct",
            "data_use",
            "difficulty",
            "id",
            "learning_value",
            "oracle_ok",
            "parent_family"
        ]
    );
    assert_eq!(object["oracle_ok"], serde_json::json!(false));
    assert_eq!(object["correct"], serde_json::json!(false));
    assert_eq!(object["difficulty"], serde_json::json!(0.5));
    assert_eq!(object["learning_value"], serde_json::json!(0.5));
    assert_eq!(object["data_use"], serde_json::json!("development"));
}
