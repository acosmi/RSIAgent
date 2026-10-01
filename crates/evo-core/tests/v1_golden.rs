//! Frozen baseline typed v1 bytes, not a new canonical JSON or HTTP host contract.
use evo_core::{Application, Evaluation, Feedback, Inspect, Prepare, Proposal, Strategy, Validate};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const PREPARE: &str = r#"{"request_key":"prepare-1","goal":"检查 café\n保留原始换行\r\ne\u0301","capabilities":["tool-z","tool-a"]}"#;
const FEEDBACK: &str = r#"{"request_key":"feedback-1","run_id":"run-1","outcome":"failure","details":"工具失败\n凭据未提供","failure_class":"tool_use"}"#;
const PROPOSAL: &str = r#"{"request_key":"proposal-1","run_id":"run-1","kind":"skill","parent_snapshot":"snapshot-1","hypothesis":"先检查配置","applicability":"café 项目","counterexample":"无配置的任务","content":"第一行\r\n第二行\ne\u0301","evidence_refs":["feedback-z","feedback-a"],"required_capabilities":["tool-z","tool-a"],"dependencies":["skill-z","skill-a"]}"#;
const INSPECT: &str = r#"{"kind":"candidate","id":"candidate-1"}"#;
const APPLICATION: &str = r#"{"request_key":"application-1","run_id":"run-1","candidate_id":"candidate-1","stage":"attached","evidence_ref":"evidence-1"}"#;
const STRATEGY: &str = r#"{"schema_version":"evo.strategy.v1","instruction":"保留 café\r\n下一行\ne\u0301","max_candidates":4,"max_rounds":3,"require_counterexample":false}"#;
const EVALUATION: &str = r#"{"id":"evaluation-1","owner":"evaluator-1","input":{"request_key":"evaluate-1","candidate_id":"candidate-1","candidate_digest":"digest-1","dataset_id":"dataset-1","baseline_snapshot":"snapshot-1","model_identity":"fixture-model","environment_hash":"environment-1","pairs":[{"task_hash":"task-z","baseline":0.125,"candidate":0.875,"baseline_units":12,"candidate_units":10,"critical":false},{"task_hash":"task-a","baseline":1.0,"candidate":0.5,"baseline_units":2,"candidate_units":3,"critical":true}],"origin":"fixture café\r\n原始\ne\u0301","safety_passed":false,"meta":null},"decision":{"passed":false,"n":2,"mean_gain":0.125,"lower_bound":-1.0,"upper_bound":1.0,"reasons":["reason-z","reason-a"]},"created_at":1700000000}"#;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/contracts/v1")
        .join(format!("{name}.json"));
    fs::read(&path)
        .unwrap_or_else(|error| panic!("missing frozen v1 golden {}: {error}", path.display()))
}

fn sample<T: DeserializeOwned>(input: &str) -> T {
    serde_json::from_str(input).expect("complete baseline input must deserialize")
}

fn assert_golden<T: Serialize + DeserializeOwned>(name: &str, value: &T) {
    let bytes = serde_json::to_vec(value).unwrap();
    assert_eq!(
        bytes,
        fixture(name),
        "{name}: frozen typed v1 bytes changed"
    );
    let round_trip: T = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&round_trip).unwrap(), bytes);
}

#[test]
fn five_complete_requests_match_typed_wire_goldens() {
    assert_eq!(
        evo_core::contract::V1_REQUEST_KINDS,
        ["prepare", "feedback", "propose", "inspect", "application"]
    );
    let prepare: Prepare = sample(PREPARE);
    prepare.validate().unwrap();
    assert_golden("prepare", &prepare);
    let feedback: Feedback = sample(FEEDBACK);
    feedback.validate().unwrap();
    assert_golden("feedback", &feedback);
    let proposal: Proposal = sample(PROPOSAL);
    proposal.validate().unwrap();
    assert_golden("proposal", &proposal);
    // These two legacy types have no Validate implementation.
    assert_golden("inspect", &sample::<Inspect>(INSPECT));
    assert_golden("application", &sample::<Application>(APPLICATION));
}

fn rejects_injected_fields<T: DeserializeOwned>(input: &str) {
    let _: T = sample(input); // Prove the complete base is accepted before injecting one field.
    for (field, value) in [
        ("unknown", json!(true)),
        ("actor", json!("admin")),
        ("role", json!("admin")),
        ("namespace", json!("other-tenant")),
    ] {
        let mut injected: Value = sample(input);
        injected
            .as_object_mut()
            .unwrap()
            .insert(field.into(), value);
        let error = match serde_json::from_value::<T>(injected) {
            Ok(_) => panic!("v1 request accepted {field}"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("unknown field") && error.contains(field),
            "{error}"
        );
    }
}

#[test]
fn all_five_complete_requests_reject_unknown_and_forged_identity_fields() {
    rejects_injected_fields::<Prepare>(PREPARE);
    rejects_injected_fields::<Feedback>(FEEDBACK);
    rejects_injected_fields::<Proposal>(PROPOSAL);
    rejects_injected_fields::<Inspect>(INSPECT);
    rejects_injected_fields::<Application>(APPLICATION);
}

#[test]
fn omission_uses_existing_defaults_without_conflating_null_or_empty() {
    let prepare: Prepare = sample(r#"{"request_key":"prepare-1","goal":"default capabilities"}"#);
    prepare.validate().unwrap();
    assert!(prepare.capabilities.is_empty());
    assert_golden("prepare_defaults", &prepare);
    let explicit_empty: Prepare =
        sample(r#"{"request_key":"prepare-1","goal":"default capabilities","capabilities":[]}"#);
    assert_eq!(
        serde_json::to_vec(&prepare).unwrap(),
        serde_json::to_vec(&explicit_empty).unwrap()
    );
    let feedback: Feedback = sample(
        r#"{"request_key":"feedback-1","run_id":"run-1","outcome":"failure","details":"default class"}"#,
    );
    feedback.validate().unwrap();
    assert_eq!(feedback.failure_class, evo_core::FailureClass::Unknown);
    assert_golden("feedback_defaults", &feedback);
    let explicit_unknown: Feedback = sample(
        r#"{"request_key":"feedback-1","run_id":"run-1","outcome":"failure","details":"default class","failure_class":"unknown"}"#,
    );
    assert_eq!(
        serde_json::to_vec(&feedback).unwrap(),
        serde_json::to_vec(&explicit_unknown).unwrap()
    );
    let mut proposal: Value = sample(PROPOSAL);
    for field in ["evidence_refs", "required_capabilities", "dependencies"] {
        proposal.as_object_mut().unwrap().remove(field);
    }
    let proposal: Proposal = serde_json::from_value(proposal).unwrap();
    proposal.validate().unwrap();
    assert!(proposal.evidence_refs.is_empty());
    assert!(proposal.required_capabilities.is_empty());
    assert!(proposal.dependencies.is_empty());
    assert_golden("proposal_defaults", &proposal);
    for field in ["capabilities", "goal"] {
        let mut value: Value = sample(PREPARE);
        value[field] = Value::Null;
        assert!(serde_json::from_value::<Prepare>(value).is_err());
    }
    let mut value: Value = sample(FEEDBACK);
    value["failure_class"] = Value::Null;
    assert!(serde_json::from_value::<Feedback>(value).is_err());
    for field in ["evidence_refs", "required_capabilities", "dependencies"] {
        let mut value: Value = sample(PROPOSAL);
        value[field] = Value::Null;
        assert!(serde_json::from_value::<Proposal>(value).is_err());
    }
    let mut empty: Prepare = sample(PREPARE);
    empty.goal.clear();
    assert!(empty.validate().is_err());
}

#[test]
fn strategy_preserves_legacy_schema_bytes_and_fingerprint() {
    let strategy: Strategy = sample(STRATEGY);
    strategy.validate().unwrap();
    assert_golden("strategy", &strategy);
    assert_eq!(
        evo_core::fingerprint(&strategy).unwrap(),
        "ca7d1c3465d5f3cc330c9b61b6da9e1e3521c3d93a61e41d967df121e7fffe79"
    );
    assert!(strategy.instruction.contains("\r\n"));
    assert!(strategy.instruction.ends_with("e\u{301}"));
    let mut changed = strategy.clone();
    changed.instruction = changed.instruction.replace("\r\n", "\n");
    assert_ne!(
        evo_core::fingerprint(&changed).unwrap(),
        evo_core::fingerprint(&strategy).unwrap()
    );
    changed = strategy.clone();
    changed.instruction = changed.instruction.replace("e\u{301}", "é");
    assert_ne!(
        evo_core::fingerprint(&changed).unwrap(),
        evo_core::fingerprint(&strategy).unwrap()
    );
}

#[test]
fn strategy_checks_existing_numeric_schema_and_utf8_byte_boundaries() {
    for candidates in [1, 4] {
        for rounds in [1, 3] {
            let mut strategy: Strategy = sample(STRATEGY);
            strategy.max_candidates = candidates;
            strategy.max_rounds = rounds;
            strategy.validate().unwrap();
        }
    }
    for candidates in [0, 5] {
        let mut strategy: Strategy = sample(STRATEGY);
        strategy.max_candidates = candidates;
        assert!(strategy.validate().is_err());
    }
    for rounds in [0, 4] {
        let mut strategy: Strategy = sample(STRATEGY);
        strategy.max_rounds = rounds;
        assert!(strategy.validate().is_err());
    }
    let mut strategy: Strategy = sample(STRATEGY);
    strategy.schema_version = "evo.strategy.v2".into();
    assert!(strategy.validate().is_err());
    strategy = sample(STRATEGY);
    strategy.instruction = "é".repeat(4096); // 8192 UTF-8 bytes, not characters.
    strategy.validate().unwrap();
    strategy.instruction.push('a');
    assert!(strategy.validate().is_err());
    strategy.instruction.clear();
    assert!(strategy.validate().is_err());
}

#[test]
fn evaluation_preserves_legacy_floats_options_and_collection_order() {
    let none: Evaluation = sample(EVALUATION);
    assert!(none.input.meta.is_none());
    assert_golden("evaluation_meta_none", &none);
    let mut omitted_meta: Value = sample(EVALUATION);
    omitted_meta["input"]
        .as_object_mut()
        .unwrap()
        .remove("meta");
    let omitted: Evaluation = serde_json::from_value(omitted_meta).unwrap();
    // This Option already treats omission and null alike; no new reset semantics.
    assert_eq!(
        serde_json::to_vec(&omitted).unwrap(),
        serde_json::to_vec(&none).unwrap()
    );
    let mut value: Value = sample(EVALUATION);
    value["input"]["meta"] = json!({
        "old_improver":"improver-old","new_improver":"improver-new","start_snapshot":"snapshot-1",
        "old_descendant_hash":"descendant-old","new_descendant_hash":"descendant-new",
        "old_total_units":10,"new_total_units":10,"streams":2,"control_groups":["group-z","group-a"]
    });
    let some: Evaluation = serde_json::from_value(value).unwrap();
    assert_golden("evaluation_meta_some", &some);
    let groups: Vec<_> = some
        .input
        .meta
        .unwrap()
        .control_groups
        .into_iter()
        .collect();
    assert_eq!(groups, ["group-a", "group-z"]); // Existing BTreeSet behavior only.
    assert_eq!(none.input.pairs[0].task_hash, "task-z");
    assert_eq!(none.decision.reasons, ["reason-z", "reason-a"]);
    assert!(none.input.origin.contains("\r\n"));
    // Legacy Evaluation has no top-level schema or deny_unknown_fields.
    let mut extended: Value = sample(EVALUATION);
    extended["legacy_extra"] = json!(true);
    let accepted: Evaluation = serde_json::from_value(extended).unwrap();
    assert_eq!(
        serde_json::to_vec(&accepted).unwrap(),
        serde_json::to_vec(&none).unwrap()
    );
}
