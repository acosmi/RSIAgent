use evo_core::Error;
use evo_core::evaluation::config_precedence_generator::*;
use evo_core::evaluation::config_precedence_oracle::{grade_exact, oracle_answer};
use evo_core::evaluation::{ControlledTaskAnchorExpectedV1, DataUse};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn selector(
    template: Template,
    layer_count: Layers,
    key_type: KeyType,
    placement: Placement,
    seed: Seed,
) -> ClosedCaseSelector {
    ClosedCaseSelector::Task {
        template,
        layer_count,
        key_type,
        placement,
        seed,
    }
}
#[test]
fn catalog_generates_real_documents() {
    let (profile, materials) = build_descriptive_catalog().unwrap();
    assert_eq!(profile.task_commitments.len(), 180);
    assert_eq!(materials.task_materials.len(), 180);
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            for key in KEY_TYPES {
                for placement in PLACEMENTS {
                    for seed in SEEDS {
                        let input = selector(template, layers, key, placement, seed);
                        let a = generate_case(input).unwrap();
                        let b = generate_case(input).unwrap();
                        assert_eq!(a.material(), b.material());
                        assert_eq!(materials.task_materials[a.task_id()], a.material());
                        assert_eq!(a.files().len(), layers.names().len());
                        assert!(!a.material().as_object().unwrap().contains_key("expected"));
                        for (file, layer) in a.files().iter().zip(layers.names()) {
                            assert_eq!(file.logical_id(), *layer);
                            assert_eq!(file.components().join("/"), file.source_path());
                            assert_eq!(file.format(), "json");
                            assert!(
                                file.raw_utf8().len() <= 512 && !file.raw_utf8().ends_with('\n')
                            );
                            assert_eq!(
                                serde_json::from_str::<Value>(file.raw_utf8()).unwrap(),
                                decode_config_document(template, file.raw_utf8().as_bytes())
                                    .unwrap()
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn closed_parameters_are_rejected() {
    let healthy = json!({"template":"F","layer_count":2,"key_type":"B","placement":"E","seed":0});
    assert!(parse_case_selector(&healthy).is_ok());
    for (field, values) in [
        (
            "seed",
            vec![json!(2), json!(-1), json!("0"), json!(0.0), Value::Null],
        ),
        ("template", vec![json!("f"), json!("script")]),
        ("placement", vec![json!("X")]),
        ("layer_count", vec![json!(1), json!("2"), json!(2.0)]),
        ("key_type", vec![json!("unknown")]),
    ] {
        for value in values {
            let mut bad = healthy.clone();
            bad[field] = value;
            assert!(parse_case_selector(&bad).is_err());
        }
    }
    let mut extra = healthy.clone();
    extra["root"] = json!("outside");
    assert!(parse_case_selector(&extra).is_err());
    for field in ["template", "layer_count", "key_type", "placement", "seed"] {
        let mut bad = healthy.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(parse_case_selector(&bad).is_err());
    }
    assert!(parse_case_selector(&json!({"anchor":"no_write","root":"outside"})).is_err());
    for anchor in ANCHORS {
        assert_eq!(
            parse_case_selector(&json!({"anchor":anchor.id()})).unwrap(),
            ClosedCaseSelector::Anchor(anchor)
        );
    }
}
#[test]
fn partition_table_precedes_generation() {
    use DataUse::{AcceptanceEpoch as A, Development as D, ReplaySelect as S, ReplayTrain as T};
    let expected = [
        [D, T, S, A, D],
        [T, S, A, D, T],
        [T, S, A, D, T],
        [S, A, D, T, S],
        [S, A, D, T, S],
        [A, D, T, S, A],
    ];
    let mut counts = [0usize; 4];
    for (t, template) in TEMPLATES.into_iter().enumerate() {
        for (l, layers) in LAYER_COUNTS.into_iter().enumerate() {
            for (k, key) in KEY_TYPES.into_iter().enumerate() {
                for placement in PLACEMENTS {
                    for seed in SEEDS {
                        let case = generate_case(selector(template, layers, key, placement, seed))
                            .unwrap();
                        assert_eq!(case.data_use(), expected[2 * t + l][k]);
                        counts[match case.data_use() {
                            D => 0,
                            T => 1,
                            S => 2,
                            A => 3,
                            _ => panic!("unexpected monitoring partition"),
                        }] += 1;
                    }
                }
            }
        }
    }
    assert_eq!(counts, [42, 48, 48, 42]);
}
#[test]
fn seed_and_placement_do_not_mint_clusters() {
    let (profile, materials) = build_descriptive_catalog().unwrap();
    assert_eq!(
        profile
            .registered_cluster_count_against(&materials)
            .unwrap(),
        30
    );
    let mut triples = BTreeMap::<String, BTreeSet<String>>::new();
    for task in &profile.task_commitments {
        let cluster = profile
            .registered_cluster_id_against(&materials, &task.task_id)
            .unwrap();
        let triple = format!(
            "{}:{:?}:{}",
            task.template_id, task.layer_combination, task.key_type
        );
        triples.entry(triple).or_default().insert(cluster);
    }
    assert_eq!(triples.len(), 30);
    assert!(triples.values().all(|ids| ids.len() == 1));
}
#[test]
fn profile_material_bindings_are_recomputable() {
    let (profile, materials) = build_descriptive_catalog().unwrap();
    let digest = profile.digest_against(&materials).unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(profile.generator.version, GENERATOR_VERSION);
    assert_eq!(profile.resolver.version, RESOLVER_VERSION);
    assert_eq!(
        materials.environment["components"]
            .as_array()
            .unwrap()
            .len(),
        17
    );
    for role in ["generator", "resolver"] {
        let frame = if role == "generator" {
            &materials.generator_source
        } else {
            &materials.resolver_source
        };
        let prefix = format!("{SOURCE_FRAME_VERSION}\n{role}\n17\n");
        let mut rest = frame.strip_prefix(&prefix).unwrap();
        let mut paths = Vec::new();
        for _ in 0..17 {
            let (count, after) = rest.split_once(':').unwrap();
            let n: usize = count.parse().unwrap();
            assert_eq!(count, n.to_string());
            let (path, after) = after.split_at(n);
            let (count, after) = after.split_once(':').unwrap();
            let n: usize = count.parse().unwrap();
            assert_eq!(count, n.to_string());
            let (raw, after) = after.split_at(n);
            let registered = materials.environment["components"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["path"] == path)
                .unwrap();
            assert_eq!(registered["sha256"], evo_core::hash(raw.as_bytes()));
            assert_eq!(registered["bytes"], raw.len());
            paths.push(path);
            rest = after.strip_prefix('\n').unwrap();
        }
        assert!(rest.is_empty());
        assert!(paths.windows(2).all(|p| p[0] < p[1]));
        assert!(paths.contains(&"crates/evo-core/src/holdout.rs"));
    }
    let mut tampered = materials.clone();
    tampered.generator_source.push('x');
    assert!(
        matches!(profile.digest_against(&tampered),Err(Error::Conflict(message)) if message=="controlled task generator source digest differs from material")
    );
    let mut tampered = materials.clone();
    tampered.task_materials.get_mut("F-L2-B-E-s0").unwrap()["files"][0]["utf8"] = json!("{}");
    assert!(
        matches!(profile.digest_against(&tampered),Err(Error::Conflict(message)) if message=="controlled task commitment digest differs from material")
    );
    assert_eq!(profile.digest_against(&materials).unwrap(), digest);
}
#[test]
fn oracle_matches_frozen_six_field_cases() {
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            for key in KEY_TYPES {
                for placement in PLACEMENTS {
                    for seed in SEEDS {
                        let case = generate_case(selector(template, layers, key, placement, seed))
                            .unwrap();
                        let answer = oracle_answer(&case).unwrap();
                        let value = serde_json::to_value(&answer).unwrap();
                        assert_eq!(value.as_object().unwrap().len(), 6);
                        assert_eq!(answer.present, placement != Placement::Z);
                        if placement == Placement::Z {
                            assert!(
                                answer.value.is_null()
                                    && answer.source_layer.is_null()
                                    && answer.source_path.is_null()
                                    && answer.json_pointer.is_null()
                            );
                        } else {
                            let layer = if placement == Placement::M {
                                "defaults"
                            } else if key == KeyType::H {
                                if layers == Layers::L3 {
                                    "project"
                                } else {
                                    "defaults"
                                }
                            } else {
                                "override"
                            };
                            assert_eq!(answer.source_layer, json!(layer));
                            let source = case
                                .files()
                                .iter()
                                .find(|f| f.logical_id() == layer)
                                .unwrap();
                            assert_eq!(answer.source_path, json!(source.source_path()));
                            let doc: Value = serde_json::from_str(source.raw_utf8()).unwrap();
                            assert_eq!(
                                doc.pointer(answer.json_pointer.as_str().unwrap()),
                                Some(&answer.value)
                            );
                            if placement == Placement::E {
                                match key {
                                    KeyType::B => assert_eq!(answer.value, json!(false)),
                                    KeyType::I => assert_eq!(answer.value, json!(0)),
                                    KeyType::S => assert_eq!(answer.value, json!("")),
                                    KeyType::N0 => assert!(answer.value.is_null()),
                                    KeyType::H => assert!(answer.value.is_string()),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn all_nine_anchor_endpoints_are_real() {
    let (profile, materials) = build_descriptive_catalog().unwrap();
    profile.validate_against(&materials).unwrap();
    assert_eq!(profile.anchor_coverage.critical_capabilities.len(), 9);
    for anchor in ANCHORS {
        let case = generate_case(ClosedCaseSelector::Anchor(anchor)).unwrap();
        let material = materials
            .anchor_materials
            .iter()
            .find(|m| m.anchor_id == anchor.id())
            .unwrap();
        assert_eq!(material.input, case.anchor_input().unwrap());
        match &material.expected {
            ControlledTaskAnchorExpectedV1::Answer { output } => assert_eq!(
                grade_exact(
                    output,
                    &serde_json::to_value(oracle_answer(&case).unwrap()).unwrap()
                ),
                1
            ),
            ControlledTaskAnchorExpectedV1::Rejected { .. } => match anchor {
                Anchor::NoWrite => assert_eq!(case.intent(), ConfigIntent::WriteProbe),
                Anchor::WorkspaceBoundary => assert_eq!(case.intent(), ConfigIntent::OutsideProbe),
                Anchor::UnknownFormat => {
                    assert_eq!(case.files()[0].source_path(), "config/unsupported.toml");
                    assert_eq!(case.files()[0].raw_utf8().as_bytes(), b"x = 1\n");
                }
                _ => panic!("unexpected rejected anchor"),
            },
        }
    }
}
#[test]
fn grading_requires_exact_six_fields() {
    let case = generate_case(ClosedCaseSelector::Anchor(Anchor::False)).unwrap();
    let expected = oracle_answer(&case).unwrap();
    let actual = serde_json::to_value(&expected).unwrap();
    assert_eq!(grade_exact(&expected, &actual), 1);
    for field in [
        "key",
        "present",
        "value",
        "source_layer",
        "source_path",
        "json_pointer",
    ] {
        let mut missing = actual.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(grade_exact(&expected, &missing), 0);
        let mut different = actual.clone();
        different[field] = json!("different");
        assert_eq!(grade_exact(&expected, &different), 0);
    }
    let mut extra = actual.clone();
    extra["score"] = json!(1);
    assert_eq!(grade_exact(&expected, &extra), 0);
    assert_eq!(grade_exact(&expected, &json!("false")), 0);
}
