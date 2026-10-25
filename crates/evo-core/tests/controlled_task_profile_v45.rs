use std::collections::BTreeMap;

use evo_core::evaluation::{
    ControlledTaskAlphaRegistrationV1, ControlledTaskAnchorExpectedV1,
    ControlledTaskAnchorMaterialV1, ControlledTaskAnchorRejectionV1, ControlledTaskAnswerV1,
    ControlledTaskProfileMaterialsV1, ControlledTaskProfileV1, DataUse,
};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::{Error, Result, canonical_fingerprint, hash};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const CAPABILITIES: [&str; 9] = [
    "no_write",
    "workspace_boundary",
    "explicit_false",
    "explicit_zero",
    "explicit_empty_string",
    "default_inheritance",
    "layer_precedence",
    "not_found",
    "unknown_format_rejection",
];

fn answer(value: Value, present: bool) -> ControlledTaskAnchorExpectedV1 {
    ControlledTaskAnchorExpectedV1::Answer {
        output: ControlledTaskAnswerV1 {
            key: "setting".into(),
            present,
            value,
            source_layer: if present {
                json!("defaults")
            } else {
                Value::Null
            },
            source_path: if present {
                json!("config/defaults.json")
            } else {
                Value::Null
            },
            json_pointer: if present {
                json!("/setting~0~1")
            } else {
                Value::Null
            },
        },
    }
}

fn complete_fixture() -> (ControlledTaskProfileV1, ControlledTaskProfileMaterialsV1) {
    let mut materials: ControlledTaskProfileMaterialsV1 = serde_json::from_value(json!({
        "generator_source": "// fixture generator v1\nfn generate() {}\n",
        "resolver_source": "// fixture resolver v1\nfn resolve() {}\n",
        "template_catalog": {"templates": [
            {"template_id":"template-a", "layer_combinations":[["workspace","user","defaults"],["defaults"]],
             "key_types":["string","number"], "parameter_sets":[{"seed":1,"style":"plain"},{"seed":2,"style":"rewrite"}]},
            {"template_id":"template-b", "layer_combinations":[["defaults"],["user","defaults"]],
             "key_types":["number","string"], "parameter_sets":[{"seed":3},{"seed":4}]}
        ]},
        "task_materials": {
            "task-1":{"seed":1,"rewrite":"base","sequence":[1,2]},
            "task-2":{"seed":2,"rewrite":"paraphrase","sequence":[1,2]},
            "task-3":{"seed":3,"rewrite":"holdout","sequence":[2,1]}
        },
        "anchor_materials": [],
        "environment":{"os":"fixture-only","limits":{"network":false,"write":false}}
    })).unwrap();
    let mut coverages = Vec::new();
    for capability in CAPABILITIES {
        let expected = match capability {
            "no_write" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WriteForbidden,
            },
            "workspace_boundary" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WorkspaceBoundary,
            },
            "unknown_format_rejection" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::UnsupportedFormat,
            },
            "explicit_false" => answer(json!(false), true),
            "explicit_zero" => answer(json!(0), true),
            "explicit_empty_string" => answer(json!(""), true),
            "not_found" => answer(Value::Null, false),
            "default_inheritance" => answer(Value::Null, true),
            "layer_precedence" => answer(json!("workspace value"), true),
            _ => unreachable!(),
        };
        let id = format!("anchor-{capability}");
        let mut anchor_ids = vec![id.clone()];
        materials
            .anchor_materials
            .push(ControlledTaskAnchorMaterialV1 {
                anchor_id: id,
                input: json!({"query":"setting","scenario":capability}),
                expected: expected.clone(),
            });
        if capability == "explicit_false" {
            let second_id = "anchor-explicit_false-2".to_string();
            anchor_ids.push(second_id.clone());
            materials
                .anchor_materials
                .push(ControlledTaskAnchorMaterialV1 {
                    anchor_id: second_id,
                    input: json!({"query":"other-false"}),
                    expected,
                });
        }
        coverages.push(json!({"capability_id":capability,"anchor_ids":anchor_ids}));
    }
    let mut profile: ControlledTaskProfileV1 = serde_json::from_value(json!({
        "schema_version":"rsia.controlled_task_profile.v1", "profile_id":"config_precedence.v1",
        "risk":"read_only_no_network_no_write_no_code_execution",
        "layer_order":["workspace","user","defaults"], "config_formats":["json","toml"],
        "generator":{"version":"generator-v1","source_digest":hash(materials.generator_source.as_bytes())},
        "resolver":{"version":"resolver-v1","source_digest":hash(materials.resolver_source.as_bytes())},
        "template_catalog_digest":hash(b"pending"),
        "task_commitments":[
            {"task_id":"task-1","template_id":"template-a","layer_combination":["workspace","user","defaults"],"key_type":"string","data_use":"development","commitment_digest":hash(b"pending")},
            {"task_id":"task-2","template_id":"template-a","layer_combination":["workspace","user","defaults"],"key_type":"string","data_use":"development","commitment_digest":hash(b"pending")},
            {"task_id":"task-3","template_id":"template-b","layer_combination":["defaults"],"key_type":"number","data_use":"acceptance_epoch","commitment_digest":hash(b"pending")}
        ],
        "anchor_coverage":{"schema_version":"rsia.anchor_coverage_matrix.v1","profile_id":"config_precedence.v1","version":"coverage-v1","critical_capabilities":coverages},
        "anchor_material_digest":hash(b"pending"), "n_planned":8,
        "alpha_registration":{"mode":"descriptive"},
        "budget":{"currency":"USD","pricing_version":"fixture-v1","per_call_cap_micros":"0","experiment_total_micros":"0","payment_subject":"fixture-payer"},
        "environment_digest":hash(b"pending")
    })).unwrap();
    bind_materials(&mut profile, &materials);
    (profile, materials)
}

// These fixture projections spell out the frozen domains independently of the validator.
fn bind_materials(
    profile: &mut ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
) {
    let mut catalog = materials.template_catalog.clone();
    catalog
        .templates
        .sort_by(|a, b| a.template_id.cmp(&b.template_id));
    for template in &mut catalog.templates {
        template.key_types.sort();
        template
            .layer_combinations
            .sort_by_cached_key(|value| serde_json::to_vec(value).unwrap());
        template
            .parameter_sets
            .sort_by_cached_key(|value| serde_json::to_vec(value).unwrap());
    }
    profile.template_catalog_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_catalog.v1","profile_id":profile.profile_id,"catalog":catalog
    }))
    .unwrap();
    for task in &mut profile.task_commitments {
        task.commitment_digest = canonical_fingerprint(&json!({
            "domain":"rsia.controlled_task_commitment.v1","profile_id":profile.profile_id,
            "task_id":task.task_id,"template_id":task.template_id,"layer_combination":task.layer_combination,
            "key_type":task.key_type,"data_use":task.data_use,"material":materials.task_materials[&task.task_id]
        })).unwrap();
    }
    let mut anchors = materials.anchor_materials.clone();
    anchors.sort_by(|a, b| a.anchor_id.cmp(&b.anchor_id));
    profile.anchor_material_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_anchors.v1","profile_id":profile.profile_id,"anchors":anchors
    }))
    .unwrap();
    profile.environment_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_environment.v1","profile_id":profile.profile_id,"environment":materials.environment
    })).unwrap();
}

fn healthy(
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
) -> String {
    profile.validate_against(materials).unwrap();
    profile.digest_against(materials).unwrap()
}

fn exact_invalid<T>(result: Result<T>, reason: &str) {
    match result {
        Err(Error::Invalid(actual)) => assert_eq!(actual, reason),
        Err(other) => panic!("expected Invalid({reason}), got {other:?}"),
        Ok(_) => panic!("expected Invalid({reason}), got success"),
    }
}

fn exact_conflict<T>(result: Result<T>, reason: &str) {
    match result {
        Err(Error::Conflict(actual)) => assert_eq!(actual, reason),
        Err(other) => panic!("expected Conflict({reason}), got {other:?}"),
        Ok(_) => panic!("expected Conflict({reason}), got success"),
    }
}

fn invalid(
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
    reason: &str,
) {
    exact_invalid(profile.validate_against(materials), reason);
    exact_invalid(profile.digest_against(materials), reason);
    exact_invalid(profile.registered_cluster_count_against(materials), reason);
    exact_invalid(
        profile.registered_cluster_id_against(materials, "task-1"),
        reason,
    );
}

fn conflict(
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
    reason: &str,
) {
    exact_conflict(profile.validate_against(materials), reason);
    exact_conflict(profile.digest_against(materials), reason);
    exact_conflict(profile.registered_cluster_count_against(materials), reason);
    exact_conflict(
        profile.registered_cluster_id_against(materials, "task-1"),
        reason,
    );
}

fn serde_reason<T: DeserializeOwned>(value: Value, reason: &str) {
    match serde_json::from_value::<T>(value) {
        Err(error) => {
            assert_eq!(error.classify(), serde_json::error::Category::Data);
            assert_eq!(error.to_string().split(", ").next().unwrap(), reason);
        }
        Ok(_) => panic!("expected serde data rejection: {reason}"),
    }
}

fn output_mut<'a>(
    materials: &'a mut ControlledTaskProfileMaterialsV1,
    capability: &str,
) -> &'a mut ControlledTaskAnswerV1 {
    let id = format!("anchor-{capability}");
    match &mut materials
        .anchor_materials
        .iter_mut()
        .find(|anchor| anchor.anchor_id == id)
        .unwrap()
        .expected
    {
        ControlledTaskAnchorExpectedV1::Answer { output } => output,
        _ => panic!("fixture capability must use an answer"),
    }
}

fn formal(profile: &mut ControlledTaskProfileV1) {
    profile.alpha_registration = ControlledTaskAlphaRegistrationV1::Formal {
        plan: ResearchFamilyAlphaPlan::new(
            "family",
            "0.05",
            vec![
                AlphaAllocation {
                    claim_id: "claim-fixed".into(),
                    attempt_id: "attempt-1".into(),
                    kind: FormalClaimKind::FixedSampleGain,
                    alpha: "0.02".into(),
                },
                AlphaAllocation {
                    claim_id: "claim-other".into(),
                    attempt_id: "attempt-2".into(),
                    kind: FormalClaimKind::OtherFormalMetric,
                    alpha: "0.01".into(),
                },
            ],
        )
        .unwrap(),
        fixed_claim_id: "claim-fixed".into(),
        attempt_id: "attempt-1".into(),
    };
}

#[test]
fn complete_profile_validates_and_has_material_bound_digest() {
    let (profile, materials) = complete_fixture();
    let original_profile = serde_json::to_value(&profile).unwrap();
    let original_materials = serde_json::to_value(&materials).unwrap();
    let digest = healthy(&profile, &materials);
    assert_eq!(digest.len(), 64);
    assert_eq!(
        profile
            .registered_cluster_count_against(&materials)
            .unwrap(),
        2
    );
    let cluster = profile
        .registered_cluster_id_against(&materials, "task-1")
        .unwrap();
    assert_eq!(cluster, canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_cluster.v1","profile_id":"config_precedence.v1",
        "template_id":"template-a","layer_combination":["workspace","user","defaults"],"key_type":"string"
    })).unwrap());
    let decoded: ControlledTaskProfileV1 =
        serde_json::from_value(original_profile.clone()).unwrap();
    let decoded_materials = serde_json::from_value(original_materials.clone()).unwrap();
    assert_eq!(healthy(&decoded, &decoded_materials), digest);
    assert_eq!(serde_json::to_value(&profile).unwrap(), original_profile);
    assert_eq!(
        serde_json::to_value(&materials).unwrap(),
        original_materials
    );
    exact_invalid(
        profile.registered_cluster_id_against(&materials, "unknown"),
        "unknown controlled task task_id",
    );
}

#[test]
fn required_fields_unknown_schema_and_unknown_fields_are_rejected() {
    let (profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    let wire = serde_json::to_value(&profile).unwrap();
    type Mutation = fn(&mut ControlledTaskProfileV1, &mut ControlledTaskProfileMaterialsV1);
    let required: &[(Mutation, &str)] = &[
        (
            |p, _| p.layer_order.clear(),
            "controlled task layer_order must be nonempty",
        ),
        (
            |p, _| p.config_formats.clear(),
            "controlled task config_formats must be nonempty",
        ),
        (
            |p, _| p.generator.version.clear(),
            "controlled task generator version must be nonempty",
        ),
        (
            |p, _| p.resolver.version.clear(),
            "controlled task resolver version must be nonempty",
        ),
        (
            |_, m| m.generator_source.clear(),
            "controlled task generator source must be nonempty",
        ),
        (
            |_, m| m.resolver_source.clear(),
            "controlled task resolver source must be nonempty",
        ),
        (
            |_, m| m.environment = json!({}),
            "controlled task environment must be a nonempty JSON object",
        ),
        (
            |_, m| m.template_catalog.templates.clear(),
            "controlled task templates must be nonempty",
        ),
        (
            |_, m| m.template_catalog.templates[0].key_types.clear(),
            "controlled task key_types must be nonempty",
        ),
        (
            |_, m| m.template_catalog.templates[0].layer_combinations.clear(),
            "controlled task layer_combinations must be nonempty",
        ),
        (
            |_, m| m.template_catalog.templates[0].parameter_sets.clear(),
            "controlled task parameter_sets must be nonempty",
        ),
        (
            |_, m| m.template_catalog.templates[0].parameter_sets[0] = json!({}),
            "controlled task parameter set must be a nonempty JSON object",
        ),
        (
            |p, _| p.task_commitments.clear(),
            "controlled task task_commitments must be nonempty",
        ),
        (
            |p, _| p.task_commitments[0].task_id.clear(),
            "controlled task task_id must be nonempty",
        ),
        (
            |p, _| p.task_commitments[0].template_id.clear(),
            "controlled task task template_id must be nonempty",
        ),
        (
            |p, _| p.task_commitments[0].key_type.clear(),
            "controlled task task key_type must be nonempty",
        ),
        (
            |p, _| p.task_commitments[0].layer_combination.clear(),
            "controlled task layer_combination must be nonempty",
        ),
        (
            |_, m| *m.task_materials.get_mut("task-1").unwrap() = json!({}),
            "controlled task task material must be a nonempty JSON object",
        ),
        (
            |_, m| m.anchor_materials[0].input = json!({}),
            "controlled task anchor input must be a nonempty JSON object",
        ),
        (
            |_, m| output_mut(m, "explicit_false").key.clear(),
            "controlled task anchor answer key must be nonempty",
        ),
        (
            |p, _| p.budget.currency.clear(),
            "controlled task budget currency must be nonempty",
        ),
        (
            |p, _| p.budget.pricing_version.clear(),
            "controlled task budget pricing_version must be nonempty",
        ),
        (
            |p, _| p.budget.payment_subject.clear(),
            "controlled task budget payment_subject must be nonempty",
        ),
    ];
    for (mutate, reason) in required {
        let (mut changed, mut changed_materials) = complete_fixture();
        healthy(&changed, &changed_materials);
        mutate(&mut changed, &mut changed_materials);
        invalid(&changed, &changed_materials, reason);
    }
    for field in wire.as_object().unwrap().keys() {
        let mut changed = wire.clone();
        changed.as_object_mut().unwrap().remove(field);
        serde_reason::<ControlledTaskProfileV1>(changed, &format!("missing field `{field}`"));
    }
    let materials_wire = serde_json::to_value(&materials).unwrap();
    for field in materials_wire.as_object().unwrap().keys() {
        let mut changed = materials_wire.clone();
        changed.as_object_mut().unwrap().remove(field);
        serde_reason::<ControlledTaskProfileMaterialsV1>(
            changed,
            &format!("missing field `{field}`"),
        );
    }
    for pointer in [
        "",
        "/generator",
        "/resolver",
        "/budget",
        "/task_commitments/0",
        "/anchor_coverage",
        "/alpha_registration",
    ] {
        let mut changed = wire.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unregistered".into(), json!(true));
        serde_reason::<ControlledTaskProfileV1>(changed, "unknown field `unregistered`");
    }
    for pointer in [
        "",
        "/template_catalog",
        "/template_catalog/templates/0",
        "/anchor_materials/0",
        "/anchor_materials/0/expected",
    ] {
        let mut changed = materials_wire.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("passed".into(), json!(true));
        serde_reason::<ControlledTaskProfileMaterialsV1>(changed, "unknown field `passed`");
    }
    let answer_index = materials
        .anchor_materials
        .iter()
        .position(|anchor| anchor.anchor_id == "anchor-explicit_false")
        .unwrap();
    for field in [
        "key",
        "present",
        "value",
        "source_layer",
        "source_path",
        "json_pointer",
    ] {
        let mut changed = materials_wire.clone();
        changed["anchor_materials"][answer_index]["expected"]["output"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        serde_reason::<ControlledTaskProfileMaterialsV1>(
            changed,
            &format!("missing field `{field}`"),
        );
    }
    let mut changed = profile.clone();
    changed.schema_version = "rsia.controlled_task_profile.v2".into();
    invalid(
        &changed,
        &materials,
        "unsupported controlled task profile schema",
    );
    changed = profile.clone();
    changed.profile_id = "other.v1".into();
    invalid(
        &changed,
        &materials,
        "unsupported controlled task profile id",
    );
    let mut changed = wire.clone();
    changed["risk"] = json!("can_write");
    serde_reason::<ControlledTaskProfileV1>(changed, "unknown variant `can_write`");
    let mut changed = wire;
    changed["alpha_registration"]["mode"] = json!("automatic");
    serde_reason::<ControlledTaskProfileV1>(changed, "unknown variant `automatic`");
    let mut changed = materials_wire.clone();
    changed["anchor_materials"][0]["expected"]["kind"] = json!("passed");
    serde_reason::<ControlledTaskProfileMaterialsV1>(changed, "unknown variant `passed`");
    let mut changed = materials_wire;
    changed["anchor_materials"][0]["expected"]["reason"] =
        json!("write_forbidden|workspace_boundary|unsupported_format");
    serde_reason::<ControlledTaskProfileMaterialsV1>(
        changed,
        "unknown variant `write_forbidden|workspace_boundary|unsupported_format`",
    );
}

#[test]
fn every_atomic_anchor_capability_is_required() {
    for capability in CAPABILITIES {
        let (mut profile, materials) = complete_fixture();
        healthy(&profile, &materials);
        profile
            .anchor_coverage
            .critical_capabilities
            .retain(|row| row.capability_id != capability);
        invalid(
            &profile,
            &materials,
            &format!("controlled task anchor capability missing: {capability}"),
        );
    }
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.anchor_coverage.critical_capabilities.push(
        evo_core::holdout::CriticalCapabilityCoverage {
            capability_id: "invented".into(),
            anchor_ids: vec!["invented-anchor".into()],
        },
    );
    invalid(
        &profile,
        &materials,
        "unsupported controlled task anchor capability: invented",
    );
}

#[test]
fn source_bytes_and_registration_versions_have_distinct_bindings() {
    for generator in [true, false] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        if generator {
            materials.generator_source.push('\n');
        } else {
            materials.resolver_source.push('\n');
        }
        conflict(
            &profile,
            &materials,
            &format!(
                "controlled task {} source digest differs from material",
                if generator { "generator" } else { "resolver" }
            ),
        );
        let (mut profile, materials) = complete_fixture();
        let original = healthy(&profile, &materials);
        if generator {
            profile.generator.version = "generator-v2".into();
        } else {
            profile.resolver.version = "resolver-v2".into();
        }
        assert_ne!(healthy(&profile, &materials), original);
    }
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.generator.source_digest = hash(profile.generator.version.as_bytes());
    conflict(
        &profile,
        &materials,
        "controlled task generator source digest differs from material",
    );
}

#[test]
fn catalog_and_task_commitments_reject_tampering_and_extra_material() {
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    materials.template_catalog.templates[0].parameter_sets[0]["seed"] = json!(100);
    conflict(
        &profile,
        &materials,
        "controlled task template catalog digest differs from material",
    );
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    materials.task_materials.get_mut("task-1").unwrap()["rewrite"] = json!("altered");
    conflict(
        &profile,
        &materials,
        "controlled task commitment digest differs from material",
    );
    for missing in [true, false] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        if missing {
            materials.task_materials.remove("task-1");
        } else {
            materials
                .task_materials
                .insert("extra".into(), json!({"seed":99}));
        }
        conflict(
            &profile,
            &materials,
            "controlled task task material ids differ from commitments",
        );
    }
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].commitment_digest = hash(b"fabricated");
    conflict(
        &profile,
        &materials,
        "controlled task commitment digest differs from material",
    );
}

#[test]
fn semantic_cluster_count_ignores_seed_rewrite_and_task_id() {
    let (mut profile, mut materials) = complete_fixture();
    let original = healthy(&profile, &materials);
    assert_eq!(
        profile
            .registered_cluster_id_against(&materials, "task-1")
            .unwrap(),
        profile
            .registered_cluster_id_against(&materials, "task-2")
            .unwrap()
    );
    let mut extra = profile.task_commitments[0].clone();
    extra.task_id = "renamed-with-new-seed".into();
    materials.task_materials.insert(
        extra.task_id.clone(),
        json!({"seed":999,"rewrite":"new opaque label"}),
    );
    profile.task_commitments.push(extra);
    bind_materials(&mut profile, &materials);
    assert_ne!(healthy(&profile, &materials), original);
    assert_eq!(
        profile
            .registered_cluster_count_against(&materials)
            .unwrap(),
        2
    );
    assert_eq!(
        profile
            .registered_cluster_id_against(&materials, "task-1")
            .unwrap(),
        profile
            .registered_cluster_id_against(&materials, "renamed-with-new-seed")
            .unwrap()
    );
}

#[test]
fn same_cluster_cannot_cross_development_and_holdout() {
    let allowed = [
        DataUse::Development,
        DataUse::ReplayTrain,
        DataUse::ReplaySelect,
        DataUse::AcceptanceEpoch,
    ];
    for first in allowed {
        for second in allowed {
            let (mut profile, materials) = complete_fixture();
            profile.task_commitments[0].data_use = first;
            profile.task_commitments[1].data_use = first;
            bind_materials(&mut profile, &materials);
            healthy(&profile, &materials);
            profile.task_commitments[1].data_use = second;
            bind_materials(&mut profile, &materials);
            if first == second {
                healthy(&profile, &materials);
            } else {
                conflict(
                    &profile,
                    &materials,
                    "controlled task semantic cluster crosses data_use partitions",
                );
            }
        }
    }
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].data_use = DataUse::OperationalMonitoring;
    invalid(
        &profile,
        &materials,
        "controlled task data_use is not a development or holdout use",
    );
}

#[test]
fn unknown_templates_layers_and_key_types_are_rejected() {
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].template_id = "unknown".into();
    invalid(
        &profile,
        &materials,
        "controlled task references an unknown template",
    );
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].key_type = "unknown".into();
    invalid(
        &profile,
        &materials,
        "controlled task key_type is not in the template catalog",
    );
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].layer_combination = vec!["unknown".into()];
    invalid(
        &profile,
        &materials,
        "controlled task layer_combination has an unknown layer",
    );
    let (mut profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    profile.task_commitments[0].layer_combination = vec!["workspace".into()];
    invalid(
        &profile,
        &materials,
        "controlled task layer_combination is not in the template catalog",
    );
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    materials.template_catalog.templates[0].layer_combinations[0].swap(0, 1);
    invalid(
        &profile,
        &materials,
        "controlled task layer_combination must follow layer_order",
    );
}

#[test]
fn anchor_answers_distinguish_false_zero_empty_null_and_absent() {
    let (profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    let wire = serde_json::to_value(&materials).unwrap();
    let values: Vec<_> = [
        "explicit_false",
        "explicit_zero",
        "explicit_empty_string",
        "default_inheritance",
        "not_found",
    ]
    .into_iter()
    .map(|capability| {
        wire["anchor_materials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|anchor| anchor["anchor_id"] == format!("anchor-{capability}"))
            .unwrap()["expected"]
            .clone()
    })
    .collect();
    assert_eq!(values[0]["output"]["value"], json!(false));
    assert_eq!(values[1]["output"]["value"], json!(0));
    assert_eq!(values[2]["output"]["value"], json!(""));
    assert_eq!(values[3]["output"]["value"], Value::Null);
    assert_eq!(values[3]["output"]["present"], json!(true));
    assert_eq!(values[4]["output"]["present"], json!(false));
    for capability in ["explicit_false", "explicit_zero", "explicit_empty_string"] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        *output_mut(&mut materials, capability) = match answer(Value::Null, false) {
            ControlledTaskAnchorExpectedV1::Answer { output } => output,
            _ => unreachable!(),
        };
        invalid(
            &profile,
            &materials,
            &format!("controlled task anchor endpoint does not match capability: {capability}"),
        );
    }
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    output_mut(&mut materials, "explicit_zero").value = json!(false);
    invalid(
        &profile,
        &materials,
        "controlled task anchor endpoint does not match capability: explicit_zero",
    );
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    output_mut(&mut materials, "not_found").value = json!(0);
    invalid(
        &profile,
        &materials,
        "controlled task absent answer must have null value and sources",
    );
}

#[test]
fn wrong_anchor_endpoint_capability_or_source_pointer_is_rejected() {
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    materials.anchor_materials[0].expected = ControlledTaskAnchorExpectedV1::Rejected {
        reason: ControlledTaskAnchorRejectionV1::UnsupportedFormat,
    };
    invalid(
        &profile,
        &materials,
        "controlled task anchor endpoint does not match capability: no_write",
    );
    for path in [
        "",
        "/absolute",
        "../escape",
        "a//b",
        "a/./b",
        "a/../b",
        "a\\b",
        "C:/absolute",
    ] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        output_mut(&mut materials, "explicit_false").source_path = json!(path);
        invalid(
            &profile,
            &materials,
            "controlled task anchor answer source_path is invalid",
        );
    }
    for pointer in ["setting", "/bad~", "/bad~2", "#/fragment"] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        output_mut(&mut materials, "explicit_false").json_pointer = json!(pointer);
        invalid(
            &profile,
            &materials,
            "controlled task anchor answer json_pointer is invalid",
        );
    }
    let (profile, mut materials) = complete_fixture();
    healthy(&profile, &materials);
    output_mut(&mut materials, "explicit_false").source_layer = json!("unknown");
    invalid(
        &profile,
        &materials,
        "controlled task anchor answer source_layer is unknown",
    );
    for input in [true, false] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        if input {
            materials.anchor_materials[0].input["query"] = json!("altered");
        } else {
            output_mut(&mut materials, "layer_precedence").value = json!("altered");
        }
        conflict(
            &profile,
            &materials,
            "controlled task anchor material digest differs from material",
        );
    }
    for missing in [true, false] {
        let (profile, mut materials) = complete_fixture();
        healthy(&profile, &materials);
        if missing {
            materials.anchor_materials.pop();
        } else {
            let mut extra = materials.anchor_materials[0].clone();
            extra.anchor_id = "unreferenced".into();
            materials.anchor_materials.push(extra);
        }
        conflict(
            &profile,
            &materials,
            "controlled task anchor material ids differ from coverage",
        );
    }
}

#[test]
fn formal_alpha_requires_same_claim_attempt_and_fixed_scope() {
    let (mut profile, materials) = complete_fixture();
    formal(&mut profile);
    let original = healthy(&profile, &materials);
    for field in ["claim", "attempt", "scope"] {
        let mut changed = profile.clone();
        if let ControlledTaskAlphaRegistrationV1::Formal {
            plan,
            fixed_claim_id,
            attempt_id,
        } = &mut changed.alpha_registration
        {
            match field {
                "claim" => *fixed_claim_id = "missing".into(),
                "attempt" => *attempt_id = "missing".into(),
                "scope" => plan.allocations[0].kind = FormalClaimKind::SequentialReject,
                _ => unreachable!(),
            }
            plan.validate().unwrap();
        }
        invalid(
            &changed,
            &materials,
            "alpha claim is absent or has the wrong scope",
        );
    }
    if let ControlledTaskAlphaRegistrationV1::Formal { plan, .. } = &mut profile.alpha_registration
    {
        plan.allocations.reverse();
    }
    assert_eq!(healthy(&profile, &materials), original);
    let wire = serde_json::to_value(&profile).unwrap();
    for field in ["plan", "fixed_claim_id", "attempt_id"] {
        let mut changed = wire.clone();
        changed["alpha_registration"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        serde_reason::<ControlledTaskProfileV1>(changed, &format!("missing field `{field}`"));
    }
}

#[test]
fn descriptive_zero_budget_remains_registration_only() {
    let (profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    let wire = serde_json::to_value(&profile).unwrap();
    assert_eq!(wire["alpha_registration"], json!({"mode":"descriptive"}));
    assert_eq!(wire["budget"]["per_call_cap_micros"], json!("0"));
    assert_eq!(wire["budget"]["experiment_total_micros"], json!("0"));
    assert_eq!(
        profile
            .registered_cluster_count_against(&materials)
            .unwrap(),
        2
    );
    assert_ne!(profile.n_planned, 2);
    for extra in [
        "plan",
        "fixed_claim_id",
        "attempt_id",
        "authorized",
        "formal_eligible",
    ] {
        let mut changed = wire.clone();
        changed["alpha_registration"][extra] = json!(true);
        serde_reason::<ControlledTaskProfileV1>(changed, &format!("unknown field `{extra}`"));
    }
    let mut changed = wire.clone();
    changed["budget"]["authorized"] = json!(true);
    serde_reason::<ControlledTaskProfileV1>(changed, "unknown field `authorized`");
    let mut changed = profile.clone();
    changed.n_planned = 0;
    invalid(
        &changed,
        &materials,
        "controlled task n_planned must be nonzero",
    );
}

#[test]
fn declared_sets_are_order_invariant_and_reject_duplicates() {
    let (mut profile, mut materials) = complete_fixture();
    formal(&mut profile);
    let original = healthy(&profile, &materials);
    type Permutation = fn(&mut ControlledTaskProfileV1, &mut ControlledTaskProfileMaterialsV1);
    let permutations: &[(&str, Permutation)] = &[
        ("config_formats", |p, _| p.config_formats.reverse()),
        ("templates", |_, m| m.template_catalog.templates.reverse()),
        ("key_types", |_, m| {
            for t in &mut m.template_catalog.templates {
                t.key_types.reverse();
            }
        }),
        ("layer_combinations", |_, m| {
            for t in &mut m.template_catalog.templates {
                t.layer_combinations.reverse();
            }
        }),
        ("parameter_sets", |_, m| {
            for t in &mut m.template_catalog.templates {
                t.parameter_sets.reverse();
            }
        }),
        ("task_commitments", |p, _| p.task_commitments.reverse()),
        ("task_materials", |_, m| {
            m.task_materials = std::mem::take(&mut m.task_materials)
                .into_iter()
                .rev()
                .collect()
        }),
        ("critical_capabilities", |p, _| {
            p.anchor_coverage.critical_capabilities.reverse()
        }),
        ("anchor_ids", |p, _| {
            for c in &mut p.anchor_coverage.critical_capabilities {
                c.anchor_ids.reverse();
            }
        }),
        ("anchor_materials", |_, m| m.anchor_materials.reverse()),
        ("allocations", |p, _| {
            if let ControlledTaskAlphaRegistrationV1::Formal { plan, .. } =
                &mut p.alpha_registration
            {
                plan.allocations.reverse();
            }
        }),
    ];
    for (name, permute) in permutations {
        let (mut permuted, mut permuted_materials) = complete_fixture();
        formal(&mut permuted);
        assert_eq!(healthy(&permuted, &permuted_materials), original);
        permute(&mut permuted, &mut permuted_materials);
        let snapshot = (
            serde_json::to_value(&permuted).unwrap(),
            serde_json::to_value(&permuted_materials).unwrap(),
        );
        assert_eq!(
            healthy(&permuted, &permuted_materials),
            original,
            "permuted {name}"
        );
        assert_eq!(
            (
                serde_json::to_value(&permuted).unwrap(),
                serde_json::to_value(&permuted_materials).unwrap()
            ),
            snapshot
        );
    }
    let before_profile = serde_json::to_value(&profile).unwrap();
    let before_materials = serde_json::to_value(&materials).unwrap();
    profile.config_formats.reverse();
    profile.task_commitments.reverse();
    profile.anchor_coverage.critical_capabilities.reverse();
    for coverage in &mut profile.anchor_coverage.critical_capabilities {
        coverage.anchor_ids.reverse();
    }
    if let ControlledTaskAlphaRegistrationV1::Formal { plan, .. } = &mut profile.alpha_registration
    {
        plan.allocations.reverse();
    }
    materials.template_catalog.templates.reverse();
    for template in &mut materials.template_catalog.templates {
        template.key_types.reverse();
        template.layer_combinations.reverse();
        template.parameter_sets.reverse();
    }
    materials.anchor_materials.reverse();
    materials.task_materials = materials
        .task_materials
        .into_iter()
        .rev()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(healthy(&profile, &materials), original);
    let permuted_profile = serde_json::to_value(&profile).unwrap();
    let permuted_materials = serde_json::to_value(&materials).unwrap();
    assert_ne!(permuted_profile, before_profile);
    assert_ne!(permuted_materials, before_materials);
    healthy(&profile, &materials);
    assert_eq!(serde_json::to_value(&profile).unwrap(), permuted_profile);
    assert_eq!(
        serde_json::to_value(&materials).unwrap(),
        permuted_materials
    );
    for duplicate in [
        "layer_order",
        "config_formats",
        "templates",
        "key_types",
        "layer_combinations",
        "layer_combination_member",
        "parameter_sets",
        "task_id",
        "capability",
        "coverage_anchor",
        "anchor_id",
        "allocation",
    ] {
        let (mut profile, mut materials) = complete_fixture();
        formal(&mut profile);
        healthy(&profile, &materials);
        let reason = match duplicate {
            "layer_order" => {
                profile.layer_order.push(profile.layer_order[0].clone());
                "duplicate controlled task layer_order"
            }
            "config_formats" => {
                profile
                    .config_formats
                    .push(profile.config_formats[0].clone());
                "duplicate controlled task config_formats"
            }
            "templates" => {
                materials
                    .template_catalog
                    .templates
                    .push(materials.template_catalog.templates[0].clone());
                "duplicate controlled task template_id"
            }
            "key_types" => {
                let t = &mut materials.template_catalog.templates[0];
                t.key_types.push(t.key_types[0].clone());
                "duplicate controlled task key_types"
            }
            "layer_combinations" => {
                let t = &mut materials.template_catalog.templates[0];
                t.layer_combinations.push(t.layer_combinations[0].clone());
                "duplicate controlled task layer_combinations"
            }
            "layer_combination_member" => {
                let t = &mut materials.template_catalog.templates[0];
                let duplicate = t.layer_combinations[0][0].clone();
                t.layer_combinations[0].push(duplicate);
                "duplicate controlled task layer_combination"
            }
            "parameter_sets" => {
                let t = &mut materials.template_catalog.templates[0];
                t.parameter_sets.push(t.parameter_sets[0].clone());
                "duplicate controlled task parameter_sets"
            }
            "task_id" => {
                profile
                    .task_commitments
                    .push(profile.task_commitments[0].clone());
                "duplicate controlled task task_id"
            }
            "capability" => {
                profile
                    .anchor_coverage
                    .critical_capabilities
                    .push(profile.anchor_coverage.critical_capabilities[0].clone());
                "duplicate critical capability"
            }
            "coverage_anchor" => {
                let c = &mut profile.anchor_coverage.critical_capabilities[0];
                c.anchor_ids.push(c.anchor_ids[0].clone());
                "anchor assigned more than once"
            }
            "anchor_id" => {
                materials
                    .anchor_materials
                    .push(materials.anchor_materials[0].clone());
                "duplicate controlled task anchor_id"
            }
            "allocation" => {
                if let ControlledTaskAlphaRegistrationV1::Formal { plan, .. } =
                    &mut profile.alpha_registration
                {
                    plan.allocations.push(plan.allocations[0].clone());
                }
                "duplicate alpha allocation identity"
            }
            _ => unreachable!(),
        };
        invalid(&profile, &materials, reason);
    }
    let (profile, materials) = complete_fixture();
    healthy(&profile, &materials);
    let raw = serde_json::to_string(&materials).unwrap();
    serde_json::from_str::<ControlledTaskProfileMaterialsV1>(&raw).unwrap();
    let duplicate = raw.replacen(
        "\"task_materials\":{",
        &format!(
            "\"task_materials\":{{\"task-1\":{},",
            materials.task_materials["task-1"]
        ),
        1,
    );
    let error = serde_json::from_str::<ControlledTaskProfileMaterialsV1>(&duplicate).unwrap_err();
    assert_eq!(error.classify(), serde_json::error::Category::Data);
    assert_eq!(
        error.to_string().split(" at line ").next().unwrap(),
        "duplicate controlled task material id"
    );
}

#[test]
fn ordered_layers_payload_arrays_budget_strings_and_environment_are_bound() {
    let (profile, materials) = complete_fixture();
    let original = healthy(&profile, &materials);
    let mut changed = profile.clone();
    let mut changed_materials = materials.clone();
    changed.layer_order.reverse();
    for template in &mut changed_materials.template_catalog.templates {
        for layers in &mut template.layer_combinations {
            layers.reverse();
        }
    }
    for task in &mut changed.task_commitments {
        task.layer_combination.reverse();
    }
    bind_materials(&mut changed, &changed_materials);
    assert_ne!(healthy(&changed, &changed_materials), original);
    let mut changed_materials = materials.clone();
    changed_materials.task_materials.get_mut("task-1").unwrap()["sequence"] = json!([2, 1]);
    conflict(
        &profile,
        &changed_materials,
        "controlled task commitment digest differs from material",
    );
    let mut changed = profile.clone();
    bind_materials(&mut changed, &changed_materials);
    assert_ne!(healthy(&changed, &changed_materials), original);
    let mut first = materials.clone();
    let mut second = materials.clone();
    first.task_materials.insert(
        "task-1".into(),
        serde_json::from_str(r#"{"rewrite":"base","seed":1,"sequence":[1,2]}"#).unwrap(),
    );
    second.task_materials.insert(
        "task-1".into(),
        serde_json::from_str(r#"{"sequence":[1,2],"seed":1,"rewrite":"base"}"#).unwrap(),
    );
    assert_eq!(healthy(&profile, &first), healthy(&profile, &second));
    let mut changed_materials = materials.clone();
    changed_materials.environment["os"] = json!("changed");
    conflict(
        &profile,
        &changed_materials,
        "controlled task environment digest differs from material",
    );
    let mut changed = profile.clone();
    bind_materials(&mut changed, &changed_materials);
    assert_ne!(healthy(&changed, &changed_materials), original);
    for field in ["per_call_cap_micros", "experiment_total_micros"] {
        for raw in [
            "",
            "00",
            "01",
            "-1",
            "1.0",
            "1e3",
            " 1",
            "1 ",
            "+1",
            "NaN",
            "Infinity",
            "18446744073709551616",
        ] {
            let (mut changed, materials) = complete_fixture();
            healthy(&changed, &materials);
            if field == "per_call_cap_micros" {
                changed.budget.per_call_cap_micros = raw.into();
            } else {
                changed.budget.experiment_total_micros = raw.into();
            }
            let reason = if raw == "18446744073709551616" {
                format!("controlled task {field} exceeds u64")
            } else {
                format!("controlled task {field} must be a canonical unsigned micros string")
            };
            invalid(&changed, &materials, &reason);
        }
        let mut wire = serde_json::to_value(&profile).unwrap();
        wire["budget"][field] = json!(0);
        serde_reason::<ControlledTaskProfileV1>(wire, "invalid type: integer `0`");
    }
    let mut changed = profile.clone();
    changed.budget.per_call_cap_micros = "1".into();
    invalid(
        &changed,
        &materials,
        "controlled task per-call budget exceeds experiment total",
    );
    changed.budget.experiment_total_micros = "18446744073709551615".into();
    assert_ne!(healthy(&changed, &materials), original);
    changed.budget.per_call_cap_micros = "18446744073709551615".into();
    healthy(&changed, &materials);
}
