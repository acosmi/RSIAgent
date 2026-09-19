use evo_core::contract::{PatchOp, Profile, SkillSnapshot, compile_skill};
use evo_core::hash;
use evo_core::skill_edit::{
    EditApplyStatus, EvidenceClosure, EvidenceRef, ExactAnchor, ProtectedTextRange,
    SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch, SkillTextEdit, SkillTextField,
    TextEditOperation, TrustedEditContext, compile_skill_edit_batch, parse_skill_edit_batch,
    skill_snapshot_digest,
};
use std::collections::BTreeSet;

const PARENT_DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const BASELINE_DIGEST: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn snapshot(content: &str) -> SkillSnapshot {
    SkillSnapshot {
        content: content.into(),
        applicability: "applies here".into(),
        counterexample: "not there".into(),
        required_capabilities: vec!["read".into()],
        dependencies: vec!["base-skill".into()],
    }
}

fn source(id: &str) -> EvidenceRef {
    EvidenceRef {
        id: id.into(),
        digest: hash(id.as_bytes()),
    }
}

fn context(input: &SkillSnapshot) -> TrustedEditContext {
    TrustedEditContext::new(
        "tenant",
        "profile-v1",
        "skill-a",
        "v3",
        PARENT_DIGEST,
        BASELINE_DIGEST,
        input,
        [source("support-1"), source("counter-1"), source("read-1")],
    )
    .unwrap()
}

fn batch(input: &SkillSnapshot, edits: Vec<SkillTextEdit>) -> SkillEditBatch {
    SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: "tenant".into(),
        profile_id: "profile-v1".into(),
        skill_id: "skill-a".into(),
        skill_version: "v3".into(),
        input_digest: skill_snapshot_digest(input).unwrap(),
        approved_parent_digest: PARENT_DIGEST.into(),
        safe_baseline_digest: BASELINE_DIGEST.into(),
        evidence: EvidenceClosure {
            support: vec![source("support-1")],
            counterexamples: vec![source("counter-1")],
            dependencies: vec![source("support-1"), source("counter-1"), source("read-1")],
        },
        edits,
    }
}

fn edit(
    field: SkillTextField,
    source: &str,
    range: std::ops::Range<usize>,
    anchor: &str,
    operation: TextEditOperation,
) -> SkillTextEdit {
    SkillTextEdit {
        field,
        start: range.start,
        end: range.end,
        expected_text_digest: hash(source[range].as_bytes()),
        exact_anchor: Some(ExactAnchor {
            text: anchor.into(),
        }),
        operation,
    }
}

fn assert_snapshot_eq(left: &SkillSnapshot, right: &SkillSnapshot) {
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

#[test]
fn utf8_ranges_are_bytes_and_must_be_character_boundaries() {
    let input = snapshot("甲乙丙");
    let start = "甲".len();
    let end = start + "乙".len();
    let valid = edit(
        SkillTextField::Content,
        &input.content,
        start..end,
        "甲乙丙",
        TextEditOperation::Replace { text: "丁".into() },
    );
    let compiled =
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, vec![valid]), &[])
            .unwrap();
    assert_eq!(compiled.output.content, "甲丁丙");

    let mut invalid = edit(
        SkillTextField::Content,
        &input.content,
        0.."甲".len(),
        "甲乙丙",
        TextEditOperation::Delete,
    );
    invalid.start = 1;
    let error =
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, vec![invalid]), &[])
            .unwrap_err();
    assert!(error.to_string().contains("not_utf8_boundary"));
}

#[test]
fn wrong_input_and_range_preimages_are_rejected() {
    let input = snapshot("alpha beta");
    let proposal = edit(
        SkillTextField::Content,
        &input.content,
        0..5,
        "alpha",
        TextEditOperation::Replace {
            text: "gamma".into(),
        },
    );
    let mut wrong_input = batch(&input, vec![proposal.clone()]);
    wrong_input.input_digest = hash(b"old snapshot");
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &wrong_input, &[])
            .unwrap_err()
            .to_string()
            .contains("edit_scope_mismatch:input_digest")
    );

    let mut wrong_range = batch(&input, vec![proposal]);
    wrong_range.edits[0].expected_text_digest = hash(b"wrong");
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &wrong_range, &[])
            .unwrap_err()
            .to_string()
            .contains("preimage_digest_mismatch")
    );
}

#[test]
fn anchors_must_exist_exactly_once_and_cover_the_edit() {
    let repeated = snapshot("same same");
    let repeated_edit = edit(
        SkillTextField::Content,
        &repeated.content,
        0..4,
        "same",
        TextEditOperation::Delete,
    );
    assert!(
        compile_skill_edit_batch(
            &repeated,
            &context(&repeated),
            &batch(&repeated, vec![repeated_edit]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("anchor_not_unique")
    );

    let input = snapshot("alpha beta");
    let missing = edit(
        SkillTextField::Content,
        &input.content,
        0..5,
        "absent",
        TextEditOperation::Delete,
    );
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, vec![missing]), &[],)
            .unwrap_err()
            .to_string()
            .contains("anchor_missing")
    );
}

#[test]
fn exact_anchor_is_optional_but_a_provided_anchor_never_falls_back_to_append() {
    let mut input = snapshot("placeholder");
    input.content.clear();
    let direct_insert = SkillTextEdit {
        field: SkillTextField::Content,
        start: 0,
        end: 0,
        expected_text_digest: hash(b""),
        exact_anchor: None,
        operation: TextEditOperation::Insert {
            text: "first rule".into(),
        },
    };
    let compiled = compile_skill_edit_batch(
        &input,
        &context(&input),
        &batch(&input, vec![direct_insert]),
        &[],
    )
    .unwrap();
    assert_eq!(compiled.output.content, "first rule");

    let nonempty = snapshot("alpha");
    let missing_anchor_insert = edit(
        SkillTextField::Content,
        &nonempty.content,
        nonempty.content.len()..nonempty.content.len(),
        "missing",
        TextEditOperation::Insert {
            text: " beta".into(),
        },
    );
    assert!(
        compile_skill_edit_batch(
            &nonempty,
            &context(&nonempty),
            &batch(&nonempty, vec![missing_anchor_insert]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("anchor_missing")
    );
}

#[test]
fn change_budget_is_global_across_fields_and_edit_count_is_bounded() {
    let input = SkillSnapshot {
        content: "a".repeat(1024),
        applicability: "b".repeat(1024),
        counterexample: "c".repeat(1024),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let exactly = vec![
        edit(
            SkillTextField::Content,
            &input.content,
            0..1024,
            &input.content,
            TextEditOperation::Replace {
                text: "d".repeat(1024),
            },
        ),
        edit(
            SkillTextField::Applicability,
            &input.applicability,
            0..1024,
            &input.applicability,
            TextEditOperation::Replace {
                text: "e".repeat(1024),
            },
        ),
    ];
    let compiled =
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, exactly), &[]).unwrap();
    assert_eq!(compiled.report.changed_bytes, 4096);

    let over = vec![edit(
        SkillTextField::Content,
        &input.content,
        0..1024,
        &input.content,
        TextEditOperation::Replace {
            text: "d".repeat(3073),
        },
    )];
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, over), &[])
            .unwrap_err()
            .to_string()
            .contains("changed_bytes_exceeded")
    );

    let tiny = snapshot("unique");
    let insert = || {
        edit(
            SkillTextField::Content,
            &tiny.content,
            1..1,
            "unique",
            TextEditOperation::Insert { text: "x".into() },
        )
    };
    let five = vec![insert(), insert(), insert(), insert(), insert()];
    assert!(
        compile_skill_edit_batch(&tiny, &context(&tiny), &batch(&tiny, five), &[])
            .unwrap_err()
            .to_string()
            .contains("too_many_edits")
    );

    let four = vec![
        edit(
            SkillTextField::Content,
            &tiny.content,
            0..0,
            "unique",
            TextEditOperation::Insert { text: "a".into() },
        ),
        edit(
            SkillTextField::Content,
            &tiny.content,
            1..1,
            "unique",
            TextEditOperation::Insert { text: "b".into() },
        ),
        edit(
            SkillTextField::Content,
            &tiny.content,
            2..2,
            "unique",
            TextEditOperation::Insert { text: "c".into() },
        ),
        edit(
            SkillTextField::Content,
            &tiny.content,
            3..3,
            "unique",
            TextEditOperation::Insert { text: "d".into() },
        ),
    ];
    let four_compiled =
        compile_skill_edit_batch(&tiny, &context(&tiny), &batch(&tiny, four), &[]).unwrap();
    assert_eq!(four_compiled.report.changed_bytes, 4);
    assert_eq!(
        four_compiled
            .report
            .edits
            .iter()
            .map(|edit| edit.inserted_text.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c", "d"]
    );

    let huge = vec![edit(
        SkillTextField::Content,
        &tiny.content,
        0..6,
        "unique",
        TextEditOperation::Replace {
            text: "z".repeat(100_000),
        },
    )];
    assert!(
        compile_skill_edit_batch(&tiny, &context(&tiny), &batch(&tiny, huge), &[])
            .unwrap_err()
            .to_string()
            .contains("changed_bytes_exceeded")
    );
}

#[test]
fn original_field_limits_still_apply_after_budget_validation() {
    let input = snapshot(&"a".repeat(16_380));
    let proposal = edit(
        SkillTextField::Content,
        &input.content,
        input.content.len()..input.content.len(),
        &input.content,
        TextEditOperation::Insert {
            text: "12345".into(),
        },
    );
    assert!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![proposal]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("content: expected nonempty text <= 16384 bytes")
    );
}

#[test]
fn trusted_protection_checks_the_whole_range_and_ignores_model_markers() {
    let input = snapshot("0123456789");
    let protected = ProtectedTextRange::new(
        SkillTextField::Content,
        3,
        7,
        hash(&input.content.as_bytes()[3..7]),
    );
    let crossing = edit(
        SkillTextField::Content,
        &input.content,
        1..8,
        "0123456789",
        TextEditOperation::Delete,
    );
    assert!(matches!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![crossing]),
            &[protected],
        ),
        Err(evo_core::Error::Forbidden)
    ));

    let fully_protected = ProtectedTextRange::new(
        SkillTextField::Content,
        0,
        input.content.len(),
        hash(input.content.as_bytes()),
    );
    let insert_marker = edit(
        SkillTextField::Content,
        &input.content,
        5..5,
        "0123456789",
        TextEditOperation::Insert {
            text: "<protected>fake</protected>".into(),
        },
    );
    assert!(matches!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![insert_marker]),
            &[fully_protected],
        ),
        Err(evo_core::Error::Forbidden)
    ));
}

#[test]
fn conflicts_and_one_bad_edit_roll_back_the_entire_batch() {
    let input = snapshot("abcdefghij");
    let first = edit(
        SkillTextField::Content,
        &input.content,
        2..5,
        "abcdefghij",
        TextEditOperation::Replace { text: "X".into() },
    );
    let overlap = edit(
        SkillTextField::Content,
        &input.content,
        4..7,
        "abcdefghij",
        TextEditOperation::Delete,
    );
    assert!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![first.clone(), overlap]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("edit_range_conflict")
    );

    let boundary_insert = edit(
        SkillTextField::Content,
        &input.content,
        5..5,
        "abcdefghij",
        TextEditOperation::Insert { text: "Y".into() },
    );
    assert!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![first.clone(), boundary_insert]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("edit_range_conflict")
    );

    let same_insert = edit(
        SkillTextField::Content,
        &input.content,
        8..8,
        "abcdefghij",
        TextEditOperation::Insert { text: "Q".into() },
    );
    assert!(
        compile_skill_edit_batch(
            &input,
            &context(&input),
            &batch(&input, vec![same_insert.clone(), same_insert]),
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("edit_range_conflict")
    );

    let mut bad = first;
    bad.exact_anchor.as_mut().unwrap().text = "missing".into();
    let before = serde_json::to_vec(&input).unwrap();
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, vec![bad]), &[]).is_err()
    );
    assert_eq!(serde_json::to_vec(&input).unwrap(), before);
}

#[test]
fn no_change_is_explicit_and_does_not_create_set_operations() {
    let input = snapshot("alpha");
    let proposal = edit(
        SkillTextField::Content,
        &input.content,
        0..5,
        "alpha",
        TextEditOperation::Replace {
            text: "alpha".into(),
        },
    );
    let compiled = compile_skill_edit_batch(
        &input,
        &context(&input),
        &batch(&input, vec![proposal]),
        &[],
    )
    .unwrap();
    assert_eq!(compiled.report.status, EditApplyStatus::NoChange);
    assert!(matches!(compiled.patch.content, PatchOp::Inherit));
    assert!(matches!(compiled.patch.applicability, PatchOp::Inherit));
    assert!(matches!(compiled.patch.counterexample, PatchOp::Inherit));
    assert_snapshot_eq(&compiled.output, &input);
}

#[test]
fn identical_inputs_compile_deterministically_and_preserve_the_evidence_closure() {
    let input = snapshot("alpha beta");
    let proposal = edit(
        SkillTextField::Content,
        &input.content,
        6..10,
        "alpha beta",
        TextEditOperation::Replace {
            text: "gamma".into(),
        },
    );
    let batch = batch(&input, vec![proposal]);
    let first = compile_skill_edit_batch(&input, &context(&input), &batch, &[]).unwrap();
    let second = compile_skill_edit_batch(&input, &context(&input), &batch, &[]).unwrap();
    assert_eq!(
        serde_json::to_vec(&first.report).unwrap(),
        serde_json::to_vec(&second.report).unwrap()
    );
    assert_snapshot_eq(&first.output, &second.output);
    assert_eq!(first.report.evidence.support, batch.evidence.support);
    assert_eq!(
        first.report.evidence.counterexamples,
        batch.evidence.counterexamples
    );
    assert_eq!(first.report.evidence.dependencies.len(), 3);
    assert_ne!(first.report.input_digest, first.report.output_digest);
}

#[test]
fn strict_json_rejects_unknown_and_duplicate_fields() {
    let input = snapshot("alpha");
    let value = batch(&input, vec![]);
    let mut unknown = serde_json::to_value(&value).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("settings".into(), serde_json::json!({}));
    assert!(parse_skill_edit_batch(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let encoded = serde_json::to_string(&value).unwrap();
    let duplicate = encoded.replacen(
        "\"schema_version\":",
        "\"schema_version\":\"duplicate\",\"schema_version\":",
        1,
    );
    assert!(parse_skill_edit_batch(duplicate.as_bytes()).is_err());

    let invalid_operation = encoded.replace("\"edits\":[]", "\"edits\":[{\"field\":\"content\",\"start\":0,\"end\":0,\"expected_text_digest\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\",\"exact_anchor\":{\"text\":\"alpha\"},\"operation\":{\"op\":\"append\",\"text\":\"x\"}}]");
    assert!(parse_skill_edit_batch(invalid_operation.as_bytes()).is_err());
}

#[test]
fn evidence_must_be_complete_and_authorized() {
    let input = snapshot("alpha");
    let mut incomplete = batch(&input, vec![]);
    incomplete
        .evidence
        .dependencies
        .retain(|item| item.id != "counter-1");
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &incomplete, &[])
            .unwrap_err()
            .to_string()
            .contains("incomplete_evidence_closure")
    );

    let mut unauthorized = batch(&input, vec![]);
    let rogue = source("rogue");
    unauthorized.evidence.dependencies.push(rogue);
    assert!(matches!(
        compile_skill_edit_batch(&input, &context(&input), &unauthorized, &[]),
        Err(evo_core::Error::Forbidden)
    ));
}

#[test]
fn evidence_source_ids_cannot_alias_different_digests_and_reports_sort_sets() {
    let input = snapshot("alpha");
    let first = source("same-id");
    let mut second = first.clone();
    second.digest = hash(b"different body");
    assert!(
        TrustedEditContext::new(
            "tenant",
            "profile-v1",
            "skill-a",
            "v3",
            PARENT_DIGEST,
            BASELINE_DIGEST,
            &input,
            [first.clone(), second.clone()],
        )
        .unwrap_err()
        .to_string()
        .contains("source_identity_conflict")
    );

    let mut proposal = batch(&input, vec![]);
    proposal.evidence.dependencies.push(second);
    proposal.evidence.dependencies.push(first);
    assert!(
        compile_skill_edit_batch(&input, &context(&input), &proposal, &[])
            .unwrap_err()
            .to_string()
            .contains("source_identity_conflict")
    );

    let compiled =
        compile_skill_edit_batch(&input, &context(&input), &batch(&input, vec![]), &[]).unwrap();
    let dependency_ids = compiled
        .report
        .evidence
        .dependencies
        .iter()
        .map(|source| source.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(dependency_ids, vec!["counter-1", "read-1", "support-1"]);
}

#[test]
fn compiled_output_is_the_existing_skill_patch_and_keeps_non_text_fields() {
    let input = snapshot("alpha");
    let proposal = edit(
        SkillTextField::Content,
        &input.content,
        0..5,
        "alpha",
        TextEditOperation::Replace {
            text: "beta".into(),
        },
    );
    let compiled = compile_skill_edit_batch(
        &input,
        &context(&input),
        &batch(&input, vec![proposal]),
        &[],
    )
    .unwrap();
    assert!(matches!(&compiled.patch.content, PatchOp::Set { .. }));
    assert!(matches!(
        &compiled.patch.required_capabilities,
        PatchOp::Inherit
    ));
    assert!(matches!(&compiled.patch.dependencies, PatchOp::Inherit));
    assert_eq!(
        &compiled.output.required_capabilities,
        &input.required_capabilities
    );
    assert_eq!(&compiled.output.dependencies, &input.dependencies);

    let profile = Profile {
        id: "profile-v1".into(),
        evolution_enabled: true,
        parent_digest: PARENT_DIGEST.into(),
        baseline_digest: BASELINE_DIGEST.into(),
    };
    let (consumed, _) = compile_skill(
        &profile,
        &input,
        &SkillSnapshot::empty(),
        &compiled.patch,
        &BTreeSet::new(),
    )
    .unwrap();
    assert_snapshot_eq(&consumed, &compiled.output);
}
