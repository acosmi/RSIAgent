//! Explicit pure ProgramFixture evidence, not real model execution or approval.
use evo_core::contract::SkillSnapshot;
use evo_core::evidence::Purpose;
use evo_core::hash;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, compile_skill_edit_batch,
    parse_skill_edit_batch, skill_snapshot_digest,
};

fn d(s: &str) -> String {
    hash(s.as_bytes())
}
fn source(id: &str) -> EvidenceRef {
    EvidenceRef {
        id: id.into(),
        digest: d(id),
    }
}
fn parent() -> SkillSnapshot {
    SkillSnapshot {
        content: r#"{"schema_version":"rsia.import_fixture.program.v1","condition":{"max_length":32,"min_value":-1000000,"max_value":1000000},"steps":[]}"#.into(),
        applicability: "bounded integer list".into(),
        counterexample: "a duplicate after absolute value".into(),
        required_capabilities: Vec::new(),
        dependencies: vec!["existing-skill-dependency".into()],
    }
}

#[test]
fn old_request_accepts_multiple_bounded_parts_without_a_full_input_cap() {
    let parts = ["parent", "material"]
        .into_iter()
        .map(|label| ModelInputPart {
            role: ModelInputRole::Evidence,
            label: label.into(),
            content: "x".repeat(128 * 1024),
        })
        .collect();
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: "fixture-request".into(),
            namespace: "fixture".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: d("parent"),
            bundle_digest: d("bundle"),
            source_closure: vec![source("source")],
            model_digest: d("model"),
            tools_digest: d("tools"),
            rules_digest: d("rules"),
            sampling_digest: d("sampling"),
            revoke_watermark: 1,
            max_suggestions: 1,
        },
        parts,
    )
    .unwrap();
    assert!(
        request
            .input
            .iter()
            .all(|part| part.content.len() <= 128 * 1024)
    );
    assert!(serde_json::to_vec(&request).unwrap().len() > 256 * 1024);
    println!(
        "old ModelRequest accepts {} full serialized bytes",
        serde_json::to_vec(&request).unwrap().len()
    );
}

#[test]
fn old_compiler_accepts_both_operation_orders_without_executing_either() {
    let input = parent();
    let refs = vec![
        source("support"),
        source("counter-only"),
        source("unselected"),
    ];
    let context = TrustedEditContext::new(
        "fixture",
        "profile",
        "skill",
        "v1",
        d("P"),
        d("B"),
        &input,
        refs.clone(),
    )
    .unwrap();
    let mut outputs = Vec::new();
    for steps in [
        r#"["abs","deduplicate","sort_ascending"]"#,
        r#"["deduplicate","abs","sort_ascending"]"#,
    ] {
        let content = format!(
            r#"{{"schema_version":"rsia.import_fixture.program.v1","condition":{{"max_length":32,"min_value":-1000000,"max_value":1000000}},"steps":{steps}}}"#
        );
        let batch = SkillEditBatch {
            schema_version: SKILL_EDIT_SCHEMA.into(),
            compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
            namespace: "fixture".into(),
            profile_id: "profile".into(),
            skill_id: "skill".into(),
            skill_version: "v1".into(),
            input_digest: skill_snapshot_digest(&input).unwrap(),
            approved_parent_digest: d("P"),
            safe_baseline_digest: d("B"),
            evidence: EvidenceClosure {
                support: vec![refs[0].clone()],
                counterexamples: vec![refs[1].clone()],
                dependencies: refs.clone(),
            },
            edits: vec![SkillTextEdit {
                field: SkillTextField::Content,
                start: 0,
                end: input.content.len(),
                expected_text_digest: hash(input.content.as_bytes()),
                exact_anchor: None,
                operation: TextEditOperation::Replace {
                    text: content.clone(),
                },
            }],
        };
        let encoded = serde_json::to_vec(&batch).unwrap();
        let parsed = parse_skill_edit_batch(&encoded).unwrap();
        let compiled = compile_skill_edit_batch(&input, &context, &parsed, &[]).unwrap();
        assert_eq!(compiled.output.content, content);
        assert_eq!(compiled.output.dependencies, input.dependencies);
        outputs.push(compiled.output.content);
    }
    assert_ne!(outputs[0], outputs[1]);
    println!(
        "old parser/compiler accepts two distinct operation-order texts; no execution outcome is produced"
    );
}

use evo_core::evidence::{ExecutionAttestation, TaskOrigin};
use evo_core::import_fixture::{
    BYTE_CODEC, EncodedFixtureRequest, FIXTURE_MODEL, FixtureAssessment, FixtureBinding,
    FixtureCase, FixtureCaseRole, FixtureCondition, FixtureContext, FixtureControl,
    FixtureExecutionKind, FixtureLimits, FixtureMethod, FixtureOperation as Op, FixtureProgram,
    FixtureProtectedRange, FixtureSourceCategory as Category, FixtureSourceKind, FixtureSourceRef,
    MAX_CONTEXT_TOKENS, MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, METHOD_SCHEMA, PROGRAM_SCHEMA,
    encode_fixture_request, execute_program_fixture, parse_fixture_method, run_program_fixture,
};
use evo_core::skill_edit::{EditApplyStatus, ProtectedTextRange};

fn sources() -> Vec<FixtureSourceRef> {
    [
        ("selection", Category::SourceSelection),
        ("result", Category::ImportResult),
        ("support", Category::ImportSource),
        ("counter-only", Category::ImportSource),
        ("unselected", Category::ImportSource),
    ]
    .into_iter()
    .map(|(id, category)| FixtureSourceRef {
        kind: FixtureSourceKind::Artifact,
        id: id.into(),
        category,
        object_digest: d(id),
    })
    .collect()
}

fn case(source: &FixtureSourceRef, input: Vec<i64>) -> FixtureCase {
    FixtureCase {
        source: source.clone(),
        locator_digest: d("locator"),
        excerpt_digest: d("excerpt"),
        input,
    }
}

fn method(steps: Vec<Op>) -> FixtureMethod {
    let refs = sources();
    FixtureMethod {
        schema_version: METHOD_SCHEMA.into(),
        program: FixtureProgram {
            schema_version: PROGRAM_SCHEMA.into(),
            condition: FixtureCondition::default(),
            steps,
        },
        support: vec![case(&refs[2], vec![3, -1, 3])],
        counterexamples: vec![case(&refs[3], vec![-2, 2])],
        dependencies: refs,
    }
}

fn good() -> Vec<Op> {
    vec![Op::Abs, Op::Deduplicate, Op::SortAscending]
}
fn bad() -> Vec<Op> {
    vec![Op::Deduplicate, Op::Abs, Op::SortAscending]
}

fn binding() -> FixtureBinding {
    FixtureBinding {
        namespace: "fixture".into(),
        profile_id: "profile".into(),
        skill_id: "skill".into(),
        skill_version: "v1".into(),
        approved_parent_digest: d("P"),
        safe_baseline_digest: d("B"),
    }
}

fn control_with(
    parent: &SkillSnapshot,
    deps: Vec<FixtureSourceRef>,
    limits: FixtureLimits,
    context: FixtureContext,
    protected: Vec<FixtureProtectedRange>,
) -> FixtureControl {
    FixtureControl::new(
        binding(),
        parent,
        deps.clone(),
        deps,
        limits,
        context,
        protected,
    )
    .unwrap()
}

fn control(parent: &SkillSnapshot, m: &FixtureMethod) -> FixtureControl {
    control_with(
        parent,
        m.dependencies.clone(),
        FixtureLimits::default(),
        FixtureContext::default(),
        Vec::new(),
    )
}

fn run(m: &FixtureMethod) -> evo_core::import_fixture::FixtureRun {
    let p = parent();
    let c = control(&p, m);
    let encoded = encode_fixture_request(&c, &p, m).unwrap();
    run_program_fixture(&c, &encoded).unwrap()
}

fn matched(assessment: &FixtureAssessment) -> Option<bool> {
    match assessment {
        FixtureAssessment::Supported { matched, .. } => Some(*matched),
        FixtureAssessment::Unsupported { .. } => None,
    }
}

fn repackage(bytes: Vec<u8>) -> EncodedFixtureRequest {
    EncodedFixtureRequest {
        input_tokens: bytes.len() as u64,
        digest: hash(&bytes),
        bytes,
    }
}

fn tamper(
    encoded: &EncodedFixtureRequest,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> EncodedFixtureRequest {
    let mut value: serde_json::Value = serde_json::from_slice(&encoded.bytes).unwrap();
    mutate(&mut value);
    repackage(serde_json::to_vec(&value).unwrap())
}

fn reason<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    assert_eq!(
        result.unwrap_err().to_string(),
        format!("invalid input: {expected}")
    );
}

#[test]
fn operation_order_changes_actual_compiled_body_and_execution() {
    let a = run(&method(good()));
    let b = run(&method(bad()));
    assert_ne!(a.output.content, b.output.content);
    assert_eq!(a.cases.len(), b.cases.len());
    assert_eq!(matched(&a.cases[1].assessment), Some(true));
    assert_eq!(matched(&b.cases[1].assessment), Some(false));
    assert_eq!(
        a.cases[1].assessment,
        FixtureAssessment::Supported {
            output: vec![2],
            oracle: vec![2],
            matched: true,
            local_operations: 3
        }
    );
    assert_eq!(
        b.cases[1].assessment,
        FixtureAssessment::Supported {
            output: vec![2, 2],
            oracle: vec![2],
            matched: false,
            local_operations: 3
        }
    );
    assert_eq!(a.local_operations, 6);
    assert_eq!(a.real_provider_calls, 0);
    assert_eq!(a.historical_origin, TaskOrigin::ImportedHistory);
    assert_eq!(
        a.historical_attestation,
        ExecutionAttestation::UnverifiedImport
    );
    assert_eq!(a.historical_purpose, Purpose::Development);
    assert_eq!(a.execution_kind, FixtureExecutionKind::ProgramFixture);
    assert_eq!(a.execution_purpose, Purpose::Development);
    assert_eq!(a.edit_report.status, EditApplyStatus::Applied);
    assert!(!a.identity_program);
    let report = serde_json::to_string(&a).unwrap();
    for authority in [
        "trusted_host",
        "trusted_run",
        "applied_receipt",
        "formal_evaluation",
        "approval",
        "active",
    ] {
        assert!(!report.contains(authority));
    }
}

#[test]
fn changed_counterexample_changes_actual_matching_not_only_digest() {
    let mut m = method(bad());
    let distinguishing = run(&m);
    m.counterexamples[0].input = vec![1, 2];
    let nondistinguishing = run(&m);
    assert_eq!(
        distinguishing.output.content,
        nondistinguishing.output.content
    );
    assert_ne!(distinguishing.input_digest, nondistinguishing.input_digest);
    assert_eq!(matched(&distinguishing.cases[1].assessment), Some(false));
    assert_eq!(matched(&nondistinguishing.cases[1].assessment), Some(true));
}

#[test]
fn compiled_skill_body_is_reread_and_oracle_is_independent() {
    let mut result = run(&method(good()));
    let input = vec![-3, 1, 3, -1, 0];
    assert_eq!(
        execute_program_fixture(&result.output, &input).unwrap(),
        FixtureAssessment::Supported {
            output: vec![0, 1, 3],
            oracle: vec![0, 1, 3],
            matched: true,
            local_operations: 3
        }
    );
    result.output.content = serde_json::to_string(&method(bad()).program).unwrap();
    assert_eq!(
        execute_program_fixture(&result.output, &input).unwrap(),
        FixtureAssessment::Supported {
            output: vec![0, 1, 1, 3, 3],
            oracle: vec![0, 1, 3],
            matched: false,
            local_operations: 3
        }
    );
}

#[test]
fn identity_no_change_remains_no_change_and_does_not_match_a_learning_case() {
    let identity = run(&method(Vec::new()));
    assert_eq!(identity.edit_report.status, EditApplyStatus::NoChange);
    assert_eq!(identity.edit_report.changed_bytes, 0);
    assert_eq!(identity.output.content, parent().content);
    assert!(identity.identity_program);
    assert_eq!(identity.local_operations, 0);
    assert_eq!(
        identity.cases[1].assessment,
        FixtureAssessment::Supported {
            output: vec![-2, 2],
            oracle: vec![2],
            matched: false,
            local_operations: 0
        }
    );
    let learned_parent = run(&method(good())).output;
    let m = method(good());
    let c = control(&learned_parent, &m);
    let encoded = encode_fixture_request(&c, &learned_parent, &m).unwrap();
    assert_eq!(
        run_program_fixture(&c, &encoded)
            .unwrap()
            .edit_report
            .status,
        EditApplyStatus::NoChange
    );
}

#[test]
fn narrowing_conditions_return_unsupported_without_matching_or_operation_credit() {
    let mut m = method(bad());
    m.program.condition.min_value = 0;
    let result = run(&m);
    assert_eq!(
        result.cases[1].assessment,
        FixtureAssessment::Unsupported { oracle: vec![2] }
    );
    assert_eq!(matched(&result.cases[1].assessment), None);
    assert_eq!(result.local_operations, 0); // Both cases contain negative values.
    m.program.condition = FixtureCondition {
        max_length: 1,
        min_value: -1,
        max_value: 1,
    };
    let result = run(&m);
    assert_eq!(
        execute_program_fixture(&result.output, &[1]).unwrap(),
        FixtureAssessment::Supported {
            output: vec![1],
            oracle: vec![1],
            matched: true,
            local_operations: 3
        }
    );
    for input in [vec![1, 1], vec![-2], vec![2]] {
        assert!(matches!(
            execute_program_fixture(&result.output, &input).unwrap(),
            FixtureAssessment::Unsupported { .. }
        ));
    }
}

#[test]
fn fixed_domain_covers_empty_max_values_and_rejects_overflow_inputs() {
    let result = run(&method(good()));
    for (input, expected) in [
        (vec![], vec![]),
        (vec![-1_000_000, 1_000_000, 0], vec![0, 1_000_000]),
        (vec![-1; 32], vec![1]),
    ] {
        assert_eq!(
            execute_program_fixture(&result.output, &input).unwrap(),
            FixtureAssessment::Supported {
                output: expected.clone(),
                oracle: expected,
                matched: true,
                local_operations: 3
            }
        );
    }
    for input in [
        vec![0; 33],
        vec![-1_000_001],
        vec![1_000_001],
        vec![i64::MIN],
        vec![i64::MAX],
    ] {
        reason(
            execute_program_fixture(&result.output, &input),
            "fixture_input_out_of_domain",
        );
    }
    let mut m = method(good());
    m.program.condition = FixtureCondition {
        max_length: 0,
        min_value: 0,
        max_value: 0,
    };
    assert!(matches!(
        execute_program_fixture(&run(&m).output, &[]).unwrap(),
        FixtureAssessment::Supported { matched: true, .. }
    ));
}

#[test]
fn raw_method_rejects_unknown_duplicate_fields_commands_and_trailing_text_without_echo() {
    let m = method(good());
    let raw = serde_json::to_string(&m).unwrap();
    for bad_json in [
        raw.replacen("{", "{\"command\":\"SECRET-shell-rm\",", 1),
        raw.replacen(
            "{",
            &format!("{{\"schema_version\":\"{METHOD_SCHEMA}\","),
            1,
        ),
        raw.replace("\"abs\"", "\"SECRET-shell-rm\""),
        format!("{raw} SECRET trailing plan"),
        format!("[{raw},{raw}]"),
        raw.replace("\"max_length\":32", "\"max_length\":32,\"max_length\":1"),
        raw.replace("\"source\":{", "\"source\":{\"path\":\"SECRET-path\","),
        raw.replace("\"kind\":\"artifact\"", "\"kind\":\"run\""),
        raw.replace("\"category\":\"import_source\"", "\"category\":\"unknown\""),
    ] {
        reason(
            parse_fixture_method(bad_json.as_bytes()),
            "fixture_invalid_method_json",
        );
    }
    let mut unknown = m.clone();
    unknown.schema_version = "new.v2".into();
    reason(
        parse_fixture_method(&serde_json::to_vec(&unknown).unwrap()),
        "fixture_unknown_method_schema",
    );
    let mut unknown = m;
    unknown.program.schema_version = "new.v2".into();
    reason(
        parse_fixture_method(&serde_json::to_vec(&unknown).unwrap()),
        "fixture_unknown_program_schema",
    );
    reason(
        parse_fixture_method(&vec![b' '; MAX_INPUT_BYTES + 1]),
        "fixture_input_byte_cap",
    );
}

#[test]
fn invalid_program_or_parent_is_rejected_with_fixed_reasons() {
    for body in ["SECRET natural language", "{\"command\":\"SECRET\"}", "{}"] {
        let mut p = parent();
        p.content = body.into();
        reason(
            FixtureControl::new(
                binding(),
                &p,
                sources(),
                sources(),
                FixtureLimits::default(),
                FixtureContext::default(),
                Vec::new(),
            ),
            "fixture_invalid_program_json",
        );
        reason(
            execute_program_fixture(&p, &[1]),
            "fixture_invalid_program_json",
        );
    }
    let mut p = parent();
    p.content = p
        .content
        .replace("\"steps\":[]", "\"steps\":[],\"steps\":[]");
    reason(
        execute_program_fixture(&p, &[1]),
        "fixture_invalid_program_json",
    );
}

#[test]
fn method_conditions_and_typed_scale_are_checked_before_encoding() {
    let p = parent();
    let base = method(good());
    let c = control(&p, &base);
    for condition in [
        FixtureCondition {
            max_length: 33,
            ..FixtureCondition::default()
        },
        FixtureCondition {
            min_value: -1_000_001,
            ..FixtureCondition::default()
        },
        FixtureCondition {
            max_value: 1_000_001,
            ..FixtureCondition::default()
        },
        FixtureCondition {
            min_value: 3,
            max_value: 2,
            ..FixtureCondition::default()
        },
    ] {
        let mut m = base.clone();
        m.program.condition = condition;
        reason(
            encode_fixture_request(&c, &p, &m),
            "fixture_invalid_condition",
        );
    }
    let mut m = base.clone();
    m.program.steps = vec![Op::Abs; 5];
    reason(encode_fixture_request(&c, &p, &m), "fixture_too_many_steps");
    let mut m = base.clone();
    m.support = vec![m.support[0].clone(); 32];
    reason(encode_fixture_request(&c, &p, &m), "fixture_material_cap");
    let mut m = base.clone();
    m.dependencies = vec![m.dependencies[0].clone(); 203];
    reason(encode_fixture_request(&c, &p, &m), "fixture_material_cap");
    for input in [vec![0; 33], vec![1_000_001]] {
        let mut m = base.clone();
        m.support[0].input = input;
        reason(
            encode_fixture_request(&c, &p, &m),
            "fixture_input_out_of_domain",
        );
    }
    let mut m = base;
    m.support[0].locator_digest = "SECRET".repeat(100);
    reason(encode_fixture_request(&c, &p, &m), "fixture_invalid_digest");
}

#[test]
fn full_source_closure_includes_counter_only_unselected_and_multiple_locators() {
    let mut m = method(good());
    m.counterexamples.push(case(&m.dependencies[3], vec![1, 2]));
    m.counterexamples[1].locator_digest = d("second locator");
    m.counterexamples[1].excerpt_digest = d("second excerpt");
    let result = run(&m);
    assert_eq!(result.dependencies.len(), 5);
    assert!(result.dependencies.iter().any(|r| r.id == "unselected"));
    assert!(result.dependencies.iter().any(|r| r.id == "counter-only"));
    assert_eq!(result.cases.len(), 3);
    assert_eq!(result.cases[1].role, FixtureCaseRole::Counterexample);
    assert_eq!(result.cases[2].case.input, vec![1, 2]);
    assert_eq!(result.edit_report.evidence.counterexamples.len(), 1);
    assert_eq!(result.edit_report.evidence.dependencies.len(), 5);
    let old = result
        .edit_report
        .evidence
        .dependencies
        .iter()
        .find(|r| r.id == "counter-only")
        .unwrap();
    assert_eq!(old.digest, m.dependencies[3].object_digest);
    assert_ne!(old.digest, m.counterexamples[0].locator_digest);
    assert_ne!(old.digest, m.counterexamples[0].excerpt_digest);
    assert_eq!(result.output.dependencies, parent().dependencies);
}

#[test]
fn material_sets_are_canonical_but_operation_and_case_order_remain() {
    let p = parent();
    let mut m = method(good());
    let c = control(&p, &m);
    let first = encode_fixture_request(&c, &p, &m).unwrap();
    m.dependencies.reverse();
    m.dependencies.push(m.dependencies[0].clone());
    let second = encode_fixture_request(&c, &p, &m).unwrap();
    assert_eq!(first, second);
    m.support.push(m.support[0].clone());
    m.support[1].input = vec![99];
    let same_sources_two_cases = run(&m);
    assert_eq!(same_sources_two_cases.cases.len(), 3);
    m.support.swap(0, 1);
    let swapped = run(&m);
    assert_ne!(same_sources_two_cases.input_digest, swapped.input_digest);
    assert_eq!(same_sources_two_cases.cases[0].case.input, vec![3, -1, 3]);
    assert_eq!(swapped.cases[0].case.input, vec![99]);
    m.program.steps.swap(0, 1);
    assert_ne!(run(&m).output.content, swapped.output.content);
}

#[test]
fn omitted_unselected_sources_and_unallowed_sources_cannot_pass_old_subset_rules() {
    let p = parent();
    let m = method(good());
    let c = control(&p, &m);
    let mut missing = m.clone();
    missing.dependencies.retain(|r| r.id != "unselected");
    reason(
        encode_fixture_request(&c, &p, &missing),
        "fixture_required_material_mismatch",
    );
    let mut missing_counter = m.clone();
    missing_counter
        .dependencies
        .retain(|r| r.id != "counter-only");
    reason(
        encode_fixture_request(&c, &p, &missing_counter),
        "fixture_incomplete_material_closure",
    );
    let mut unallowed = m.clone();
    let mut extra = sources()[2].clone();
    extra.id = "other-source".into();
    unallowed.dependencies.push(extra);
    reason(
        encode_fixture_request(&c, &p, &unallowed),
        "fixture_required_material_mismatch",
    );
    let mut allowed = m.dependencies.clone();
    allowed.retain(|r| r.id != "unselected");
    reason(
        FixtureControl::new(
            binding(),
            &p,
            allowed,
            m.dependencies,
            FixtureLimits::default(),
            FixtureContext::default(),
            Vec::new(),
        ),
        "fixture_required_source_not_allowed",
    );
}

#[test]
fn same_physical_id_conflicting_category_or_object_digest_is_rejected() {
    let p = parent();
    let m = method(good());
    let c = control(&p, &m);
    for conflict in [
        FixtureSourceRef {
            object_digest: d("changed object"),
            ..m.dependencies[2].clone()
        },
        FixtureSourceRef {
            category: Category::ImportResult,
            ..m.dependencies[2].clone()
        },
    ] {
        let mut altered = m.clone();
        altered.dependencies.push(conflict.clone());
        reason(
            encode_fixture_request(&c, &p, &altered),
            "fixture_source_identity_conflict",
        );
        let mut required = m.dependencies.clone();
        required[2] = conflict;
        reason(
            FixtureControl::new(
                binding(),
                &p,
                m.dependencies.clone(),
                required,
                FixtureLimits::default(),
                FixtureContext::default(),
                Vec::new(),
            ),
            "fixture_source_identity_conflict",
        );
    }
}

#[test]
fn complete_encoded_request_and_same_edit_response_are_actually_consumed() {
    let mut p = parent();
    p.applicability = "中文\n\"quoted\" \\ path".into();
    p.counterexample = "ö\tcounterexample".into();
    p.required_capabilities = vec!["fixed-capability".into()];
    let mut ctx = FixtureContext::default();
    ctx.mandatory_context.push_str(" 中文 \"quoted\" \\ tail");
    let m = method(good());
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits::default(),
        ctx.clone(),
        Vec::new(),
    );
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    assert_eq!(encoded.input_tokens, encoded.bytes.len() as u64);
    assert_eq!(encoded.digest, hash(&encoded.bytes));
    let text = std::str::from_utf8(&encoded.bytes).unwrap();
    assert!(text.len() > text.chars().count());
    assert!(text.contains("\\n") && text.contains("\\\\") && text.contains("\\\""));
    let v: serde_json::Value = serde_json::from_slice(&encoded.bytes).unwrap();
    assert_eq!(v["codec"], BYTE_CODEC);
    assert_eq!(v["model"], FIXTURE_MODEL);
    assert_eq!(v["parent"], serde_json::to_value(&p).unwrap());
    assert_eq!(
        v["control"]["context"]["mandatory_context"],
        ctx.mandatory_context
    );
    for field in [
        "model_declaration",
        "policy",
        "tool_schema",
        "output_schema",
        "role_framing",
    ] {
        assert!(!v[field].as_str().unwrap().is_empty());
    }
    assert_eq!(v["method"]["dependencies"].as_array().unwrap().len(), 5);
    let result = run_program_fixture(&c, &encoded).unwrap();
    assert_eq!(result.input_tokens, encoded.bytes.len() as u64);
    assert_eq!(result.output_tokens, result.edit_response.len() as u64);
    assert_eq!(result.output_digest, hash(&result.edit_response));
    assert_eq!(result.output.required_capabilities, p.required_capabilities);
    assert_eq!(result.output.applicability, p.applicability);
    assert_eq!(result.output.counterexample, p.counterexample);
    let parsed = parse_skill_edit_batch(&result.edit_response).unwrap();
    let old_context = TrustedEditContext::new(
        "fixture",
        "profile",
        "skill",
        "v1",
        d("P"),
        d("B"),
        &p,
        m.dependencies.iter().map(|r| EvidenceRef {
            id: r.id.clone(),
            digest: r.object_digest.clone(),
        }),
    )
    .unwrap();
    let independently_compiled = compile_skill_edit_batch(&p, &old_context, &parsed, &[]).unwrap();
    assert_eq!(
        skill_snapshot_digest(&independently_compiled.output).unwrap(),
        result.edit_report.output_digest
    );
    assert_eq!(
        result.declaration_digests.model,
        d(v["model_declaration"].as_str().unwrap())
    );
    assert_eq!(
        result.declaration_digests.policy,
        d(v["policy"].as_str().unwrap())
    );
    assert_eq!(
        result.declaration_digests.tools,
        hash(
            &serde_json::to_vec(&(v["tool_schema"].as_str().unwrap(), ctx.tool_declarations))
                .unwrap()
        )
    );
    let again = run_program_fixture(&c, &encoded).unwrap();
    assert_eq!(
        serde_json::to_vec(&result).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );
}

#[test]
fn mandatory_parent_and_tools_cost_actual_bytes_and_tools_digest() {
    let m = method(good());
    let p = parent();
    let c = control(&p, &m);
    let original = encode_fixture_request(&c, &p, &m).unwrap();
    let baseline = run_program_fixture(&c, &original).unwrap();
    for field in [0, 1, 2] {
        let mut p = p.clone();
        let mut ctx = FixtureContext::default();
        match field {
            0 => ctx
                .mandatory_context
                .push_str("additional mandatory material"),
            1 => p.applicability.push_str("additional full parent field"),
            _ => ctx
                .tool_declarations
                .push_str("additional actual tool description"),
        }
        let c = control_with(
            &p,
            m.dependencies.clone(),
            FixtureLimits::default(),
            ctx,
            Vec::new(),
        );
        let encoded = encode_fixture_request(&c, &p, &m).unwrap();
        assert!(encoded.input_tokens > original.input_tokens);
        assert_ne!(encoded.digest, original.digest);
        let result = run_program_fixture(&c, &encoded).unwrap();
        if field == 2 {
            assert_ne!(
                result.declaration_digests.tools,
                baseline.declaration_digests.tools
            );
        }
    }
}

#[test]
fn trusted_input_limit_accepts_exact_count_and_rejects_one_less() {
    let p = parent();
    let m = method(good());
    let original = encode_fixture_request(&control(&p, &m), &p, &m).unwrap();
    let n = original.input_tokens;
    let exact = n - MAX_INPUT_BYTES.to_string().len() as u64 + n.to_string().len() as u64;
    let limits = FixtureLimits {
        input_token_limit: exact,
        ..FixtureLimits::default()
    };
    let c = control_with(
        &p,
        m.dependencies.clone(),
        limits,
        FixtureContext::default(),
        Vec::new(),
    );
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    assert_eq!(encoded.input_tokens, exact);
    assert!(run_program_fixture(&c, &encoded).is_ok());
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits {
            input_token_limit: exact - 1,
            ..limits
        },
        FixtureContext::default(),
        Vec::new(),
    );
    reason(
        encode_fixture_request(&c, &p, &m),
        "fixture_input_budget_exceeded",
    );
}

#[test]
fn total_context_includes_frozen_output_reserve_even_before_output_exists() {
    let p = parent();
    let m = method(good());
    let original = encode_fixture_request(&control(&p, &m), &p, &m).unwrap();
    let reserve = MAX_OUTPUT_BYTES as u64;
    let mut limit = original.input_tokens + reserve;
    loop {
        let actual_input = original.input_tokens - MAX_CONTEXT_TOKENS.to_string().len() as u64
            + limit.to_string().len() as u64;
        let next = actual_input + reserve;
        if next == limit {
            break;
        }
        limit = next;
    }
    let limits = FixtureLimits {
        total_context_limit: limit,
        ..FixtureLimits::default()
    };
    let c = control_with(
        &p,
        m.dependencies.clone(),
        limits,
        FixtureContext::default(),
        Vec::new(),
    );
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    assert_eq!(encoded.input_tokens + reserve, limit);
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits {
            total_context_limit: limit - 1,
            ..limits
        },
        FixtureContext::default(),
        Vec::new(),
    );
    reason(
        encode_fixture_request(&c, &p, &m),
        "fixture_input_budget_exceeded",
    );
}

#[test]
fn complete_edit_response_fits_exact_reserve_and_rejects_one_less() {
    let p = parent();
    let m = method(good());
    let size = run(&m).output_tokens;
    for reserve in [size, size - 1, 0] {
        let c = control_with(
            &p,
            m.dependencies.clone(),
            FixtureLimits {
                output_token_reserve: reserve,
                ..FixtureLimits::default()
            },
            FixtureContext::default(),
            Vec::new(),
        );
        let encoded = encode_fixture_request(&c, &p, &m).unwrap();
        let result = run_program_fixture(&c, &encoded);
        if reserve == size {
            assert_eq!(result.unwrap().output_tokens, size);
        } else {
            reason(result, "fixture_output_budget_exceeded");
        }
    }
}

#[test]
fn checked_arithmetic_and_absolute_limit_caps_reject_expansion() {
    let p = parent();
    for (limits, expected) in [
        (
            FixtureLimits {
                input_token_limit: u64::MAX,
                output_token_reserve: 1,
                total_context_limit: u64::MAX,
            },
            "fixture_budget_overflow",
        ),
        (
            FixtureLimits {
                input_token_limit: MAX_INPUT_BYTES as u64 + 1,
                ..FixtureLimits::default()
            },
            "fixture_budget_cap",
        ),
        (
            FixtureLimits {
                output_token_reserve: MAX_OUTPUT_BYTES as u64 + 1,
                ..FixtureLimits::default()
            },
            "fixture_budget_cap",
        ),
        (
            FixtureLimits {
                total_context_limit: MAX_CONTEXT_TOKENS + 1,
                ..FixtureLimits::default()
            },
            "fixture_budget_cap",
        ),
    ] {
        reason(
            FixtureControl::new(
                binding(),
                &p,
                sources(),
                sources(),
                limits,
                FixtureContext::default(),
                Vec::new(),
            ),
            expected,
        );
    }
}

#[test]
fn byte_count_digest_and_changed_encoded_bytes_cannot_use_a_side_channel() {
    let p = parent();
    let m = method(good());
    let c = control(&p, &m);
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    let mut changed = encoded.clone();
    changed.input_tokens -= 1;
    reason(
        run_program_fixture(&c, &changed),
        "fixture_input_meter_mismatch",
    );
    let mut changed = encoded.clone();
    changed.digest = d("claimed other digest");
    reason(
        run_program_fixture(&c, &changed),
        "fixture_input_meter_mismatch",
    );
    let mut changed = encoded.clone();
    changed.bytes[0] = b'[';
    reason(
        run_program_fixture(&c, &changed),
        "fixture_input_meter_mismatch",
    );
    let mut changed = encoded.clone();
    changed.bytes.push(b' ');
    changed = repackage(changed.bytes);
    reason(
        run_program_fixture(&c, &changed),
        "fixture_noncanonical_input",
    );
    reason(
        run_program_fixture(&c, &repackage(vec![b' '; MAX_INPUT_BYTES + 1])),
        "fixture_input_byte_cap",
    );
}

#[test]
fn unknown_codec_model_and_encoded_protocol_or_historical_attestation_are_rejected() {
    let p = parent();
    let m = method(good());
    let c = control(&p, &m);
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    for (field, bad) in [
        ("schema_version", "rsia.import_fixture.request.v2"),
        ("codec", "real-tokenizer"),
        ("model", "real-provider"),
        ("model_declaration", "claimed same digest cheaper model"),
        ("policy", "skip material"),
        ("tool_schema", "shell"),
        ("output_schema", "free prose"),
        ("role_framing", "omit evidence"),
        ("historical_origin", "trusted_run"),
        ("historical_attestation", "trusted_host"),
        ("historical_purpose", "generation"),
    ] {
        reason(
            run_program_fixture(&c, &tamper(&encoded, |v| v[field] = bad.into())),
            "fixture_unknown_protocol",
        );
    }
    let text = std::str::from_utf8(&encoded.bytes).unwrap();
    for changed in [
        text.replacen("{", "{\"secret\":\"SECRET\",", 1),
        text.replacen("{", "{\"codec\":\"duplicate\",", 1),
        format!("{text} SECRET trailing"),
    ] {
        reason(
            run_program_fixture(&c, &repackage(changed.into_bytes())),
            "fixture_invalid_request_json",
        );
    }
}

#[test]
fn json_echo_cannot_grant_scope_limits_or_source_authority() {
    let p = parent();
    let m = method(good());
    let c = control(&p, &m);
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    for change in 0..7 {
        let changed = tamper(&encoded, |v| match change {
            0 => v["control"]["binding"]["namespace"] = "other".into(),
            1 => v["control"]["binding"]["approved_parent_digest"] = d("other P").into(),
            2 => v["control"]["binding"]["safe_baseline_digest"] = d("other B").into(),
            3 => v["control"]["limits"]["input_token_limit"] = 1_000_000.into(),
            4 => {
                v["control"]["required_sources"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            5 => v["control"]["context"]["mandatory_context"] = "omitted mandatory".into(),
            _ => {
                v["control"]["allowed_sources"]
                    .as_array_mut()
                    .unwrap()
                    .clear();
            }
        });
        reason(
            run_program_fixture(&c, &changed),
            "fixture_control_mismatch",
        );
    }
    let changed = tamper(&encoded, |v| {
        v["parent"]["applicability"] = "different parent".into()
    });
    reason(run_program_fixture(&c, &changed), "fixture_parent_mismatch");
    let mut changed_parent = p;
    changed_parent.counterexample.push_str("changed");
    reason(
        encode_fixture_request(&c, &changed_parent, &m),
        "fixture_parent_mismatch",
    );
}

#[test]
fn whole_replace_keeps_real_old_protection_gate_even_when_protected_text_is_unchanged() {
    let p = parent();
    let m = method(good());
    let protected = FixtureProtectedRange {
        field: SkillTextField::Content,
        start: 0,
        end: 1,
        expected_text_digest: d("{"),
    };
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits::default(),
        FixtureContext::default(),
        vec![protected.clone()],
    );
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    reason(run_program_fixture(&c, &encoded), "fixture_edit_rejected");
    // Changing the echo cannot remove a trusted protected range.
    let dropped = tamper(&encoded, |v| {
        v["control"]["protected"] = serde_json::json!([])
    });
    reason(
        run_program_fixture(&c, &dropped),
        "fixture_control_mismatch",
    );
    let identity = method(Vec::new());
    let encoded = encode_fixture_request(&c, &p, &identity).unwrap();
    assert_eq!(
        run_program_fixture(&c, &encoded)
            .unwrap()
            .edit_report
            .status,
        EditApplyStatus::NoChange
    );
    let mut bad_preimage = protected;
    bad_preimage.expected_text_digest = d("wrong");
    reason(
        FixtureControl::new(
            binding(),
            &p,
            sources(),
            sources(),
            FixtureLimits::default(),
            FixtureContext::default(),
            vec![bad_preimage],
        ),
        "fixture_invalid_protected_preimage",
    );
}

#[test]
fn old_changed_bytes_gate_accepts_4096_and_rejects_4097_without_splitting() {
    let m = method(good());
    let new_length = serde_json::to_vec(&m.program).unwrap().len();
    for total in [4096, 4097] {
        let mut p = parent();
        p.content
            .push_str(&" ".repeat(total - new_length - p.content.len()));
        let c = control(&p, &m);
        let encoded = encode_fixture_request(&c, &p, &m).unwrap();
        let result = run_program_fixture(&c, &encoded);
        if total == 4096 {
            let result = result.unwrap();
            assert_eq!(result.edit_report.changed_bytes, total);
            assert_eq!(result.edit_report.edits.len(), 1);
        } else {
            reason(result, "fixture_edit_rejected");
        }
    }
}

#[test]
fn actual_old_compiler_rejects_wrong_scope_pb_preimage_and_protected_range() {
    let p = parent();
    let m = method(good());
    let result = run(&m);
    let batch = parse_skill_edit_batch(&result.edit_response).unwrap();
    let context = TrustedEditContext::new(
        "fixture",
        "profile",
        "skill",
        "v1",
        d("P"),
        d("B"),
        &p,
        m.dependencies.iter().map(|r| EvidenceRef {
            id: r.id.clone(),
            digest: r.object_digest.clone(),
        }),
    )
    .unwrap();
    for change in 0..5 {
        let mut bad = batch.clone();
        match change {
            0 => bad.namespace = "other".into(),
            1 => bad.approved_parent_digest = d("wrong P"),
            2 => bad.safe_baseline_digest = d("wrong B"),
            3 => bad.input_digest = d("wrong parent"),
            _ => bad.edits[0].expected_text_digest = d("wrong preimage"),
        }
        assert!(compile_skill_edit_batch(&p, &context, &bad, &[]).is_err());
    }
    assert!(
        compile_skill_edit_batch(
            &p,
            &context,
            &batch,
            &[ProtectedTextRange::new(
                SkillTextField::Content,
                0,
                1,
                d("{")
            )]
        )
        .is_err()
    );
}

#[test]
fn typed_control_and_parent_string_and_array_caps_reject_before_cloning() {
    let p = parent();
    for field in 0..2 {
        let mut ctx = FixtureContext::default();
        if field == 0 {
            ctx.mandatory_context = "x".repeat(16_385);
        } else {
            ctx.tool_declarations = "x".repeat(16_385);
        }
        reason(
            FixtureControl::new(
                binding(),
                &p,
                sources(),
                sources(),
                FixtureLimits::default(),
                ctx,
                Vec::new(),
            ),
            "fixture_context_text_cap",
        );
    }
    for field in 0..5 {
        let mut p = p.clone();
        match field {
            0 => p.content = "x".repeat(16_385),
            1 => p.applicability = "x".repeat(4097),
            2 => p.counterexample = "x".repeat(4097),
            3 => p.dependencies = vec!["s".into(); 33],
            _ => p.required_capabilities = vec!["s".into(); 33],
        }
        reason(
            FixtureControl::new(
                binding(),
                &p,
                sources(),
                sources(),
                FixtureLimits::default(),
                FixtureContext::default(),
                Vec::new(),
            ),
            "fixture_invalid_parent",
        );
    }
    let mut refs = sources();
    refs[0].id = "x".repeat(129);
    reason(
        FixtureControl::new(
            binding(),
            &p,
            refs.clone(),
            refs,
            FixtureLimits::default(),
            FixtureContext::default(),
            Vec::new(),
        ),
        "fixture_invalid_identifier",
    );
    reason(
        FixtureControl::new(
            binding(),
            &p,
            vec![sources()[0].clone(); 203],
            sources(),
            FixtureLimits::default(),
            FixtureContext::default(),
            Vec::new(),
        ),
        "fixture_control_cap",
    );
    let r = FixtureProtectedRange {
        field: SkillTextField::Content,
        start: 0,
        end: 1,
        expected_text_digest: d("{"),
    };
    reason(
        FixtureControl::new(
            binding(),
            &p,
            sources(),
            sources(),
            FixtureLimits::default(),
            FixtureContext::default(),
            vec![r; 2000],
        ),
        "fixture_control_cap",
    );
}

#[test]
fn complete_byte_cap_accepts_131072_and_rejects_one_extra_without_pruning() {
    let mut found = false;
    for count in 150..=202 {
        let mut p = parent();
        p.applicability = "a".repeat(4096);
        p.counterexample = "c".repeat(4096);
        let refs = (0..count)
            .map(|i| FixtureSourceRef {
                kind: FixtureSourceKind::Artifact,
                id: format!("source-{i:03}"),
                category: Category::ImportSource,
                object_digest: d(&format!("object-{i}")),
            })
            .collect::<Vec<_>>();
        let mut m = method(good());
        m.support = vec![case(&refs[0], vec![-1])];
        m.counterexamples = vec![case(&refs[1], vec![-2, 2])];
        m.dependencies = refs;
        let ctx = FixtureContext {
            mandatory_context: "m".repeat(16_384),
            tool_declarations: "t".repeat(16_384),
        };
        let c = control_with(
            &p,
            m.dependencies.clone(),
            FixtureLimits::default(),
            ctx.clone(),
            Vec::new(),
        );
        let base = match encode_fixture_request(&c, &p, &m) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let padding = MAX_INPUT_BYTES - base.bytes.len();
        if p.content.len() + padding + 1 > 16_384 {
            continue;
        }
        p.content.push_str(&" ".repeat(padding));
        let c = control_with(
            &p,
            m.dependencies.clone(),
            FixtureLimits::default(),
            ctx.clone(),
            Vec::new(),
        );
        let exact = encode_fixture_request(&c, &p, &m).unwrap();
        assert_eq!(exact.bytes.len(), MAX_INPUT_BYTES);
        assert_eq!(
            exact.input_tokens + MAX_OUTPUT_BYTES as u64,
            MAX_CONTEXT_TOKENS
        );
        p.content.push(' ');
        let c = control_with(
            &p,
            m.dependencies.clone(),
            FixtureLimits::default(),
            ctx,
            Vec::new(),
        );
        reason(encode_fixture_request(&c, &p, &m), "fixture_input_byte_cap");
        found = true;
        break;
    }
    assert!(
        found,
        "the frozen domain admits a complete 128KiB encoded boundary request"
    );
}

#[test]
fn bounded_writer_stops_on_json_expansion_even_when_each_typed_text_is_bounded() {
    let mut p = parent();
    p.applicability = "\"".repeat(4096);
    p.counterexample = "\"".repeat(4096);
    let mut m = method(good());
    m.dependencies = (0..100)
        .map(|i| FixtureSourceRef {
            kind: FixtureSourceKind::Artifact,
            id: format!("source-{i}-{}", "x".repeat(100)),
            category: Category::ImportSource,
            object_digest: d(&i.to_string()),
        })
        .collect();
    m.support = vec![case(&m.dependencies[0], vec![-1])];
    m.counterexamples = vec![case(&m.dependencies[1], vec![-2, 2])];
    let ctx = FixtureContext {
        mandatory_context: "\"".repeat(16_384),
        tool_declarations: "\"".repeat(16_384),
    };
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits::default(),
        ctx,
        Vec::new(),
    );
    reason(encode_fixture_request(&c, &p, &m), "fixture_input_byte_cap");
}

#[test]
fn each_closed_operation_preserves_its_actual_semantics_and_four_steps_are_allowed() {
    for (steps, input, output, target) in [
        (vec![Op::Abs], vec![-2, 2, -1], vec![2, 2, 1], vec![1, 2]),
        (
            vec![Op::Deduplicate],
            vec![3, 1, 3, 2, 1],
            vec![3, 1, 2],
            vec![1, 2, 3],
        ),
        (
            vec![Op::SortAscending],
            vec![-3, 1, -3],
            vec![-3, -3, 1],
            vec![1, 3],
        ),
    ] {
        let result = run(&method(steps));
        assert_eq!(
            execute_program_fixture(&result.output, &input).unwrap(),
            FixtureAssessment::Supported {
                output,
                oracle: target,
                matched: false,
                local_operations: 1,
            }
        );
    }
    let four = method(vec![
        Op::Abs,
        Op::SortAscending,
        Op::Deduplicate,
        Op::SortAscending,
    ]);
    let result = run(&four);
    assert_eq!(result.local_operations, 8);
    assert_eq!(matched(&result.cases[1].assessment), Some(true));
}

#[test]
fn caller_allow_set_can_be_larger_but_required_set_must_remain_exact() {
    let p = parent();
    let m = method(good());
    let mut allowed = m.dependencies.clone();
    let extra = FixtureSourceRef {
        id: "allowed-extra".into(),
        ..allowed[2].clone()
    };
    allowed.push(extra.clone());
    allowed.push(allowed[0].clone());
    let mut required = m.dependencies.clone();
    required.push(required[0].clone());
    let c = FixtureControl::new(
        binding(),
        &p,
        allowed,
        required,
        FixtureLimits::default(),
        FixtureContext::default(),
        Vec::new(),
    )
    .unwrap();
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    let result = run_program_fixture(&c, &encoded).unwrap();
    assert_eq!(result.dependencies.len(), 5);
    assert!(!result.dependencies.contains(&extra));
    let mut expanded = m;
    expanded.dependencies.push(extra);
    reason(
        encode_fixture_request(&c, &p, &expanded),
        "fixture_required_material_mismatch",
    );
    reason(
        FixtureControl::new(
            binding(),
            &p,
            sources(),
            Vec::new(),
            FixtureLimits::default(),
            FixtureContext::default(),
            Vec::new(),
        ),
        "fixture_required_source_not_allowed",
    );
}

#[test]
fn noncontent_protection_is_preserved_and_empty_or_nul_context_is_rejected() {
    let p = parent();
    let m = method(good());
    let c = control_with(
        &p,
        m.dependencies.clone(),
        FixtureLimits::default(),
        FixtureContext::default(),
        vec![FixtureProtectedRange {
            field: SkillTextField::Applicability,
            start: 0,
            end: p.applicability.len(),
            expected_text_digest: hash(p.applicability.as_bytes()),
        }],
    );
    let encoded = encode_fixture_request(&c, &p, &m).unwrap();
    assert_eq!(
        run_program_fixture(&c, &encoded)
            .unwrap()
            .output
            .applicability,
        p.applicability
    );
    for value in ["", " \n\t", "secret\0context"] {
        for field in 0..2 {
            let mut ctx = FixtureContext::default();
            if field == 0 {
                ctx.mandatory_context = value.into();
            } else {
                ctx.tool_declarations = value.into();
            }
            reason(
                FixtureControl::new(
                    binding(),
                    &p,
                    sources(),
                    sources(),
                    FixtureLimits::default(),
                    ctx,
                    Vec::new(),
                ),
                "fixture_context_text_cap",
            );
        }
    }
}

#[test]
fn maximum_source_and_case_counts_work_when_the_complete_context_fits() {
    let refs = (0..202)
        .map(|i| FixtureSourceRef {
            kind: FixtureSourceKind::Artifact,
            id: format!("source-{i:03}"),
            category: match i {
                0 => Category::SourceSelection,
                1 => Category::ImportResult,
                _ => Category::ImportSource,
            },
            object_digest: d(&format!("object-{i}")),
        })
        .collect::<Vec<_>>();
    let mut m = method(good());
    m.support = refs[2..18].iter().map(|r| case(r, vec![-1; 32])).collect();
    m.counterexamples = refs[18..34].iter().map(|r| case(r, vec![-2, 2])).collect();
    m.dependencies = refs;
    let result = run(&m);
    assert_eq!(result.dependencies.len(), 202);
    assert_eq!(result.edit_report.evidence.dependencies.len(), 202);
    assert_eq!(result.cases.len(), 32);
    assert_eq!(result.local_operations, 96);
    assert!(
        result
            .cases
            .iter()
            .all(|r| matched(&r.assessment) == Some(true))
    );
    assert!(result.input_tokens <= MAX_INPUT_BYTES as u64);
    assert!(result.output_tokens <= MAX_OUTPUT_BYTES as u64);
}
