use evo_core::evaluation::config_precedence_generator::*;
use evo_core::evaluation::config_precedence_oracle::{grade_exact, oracle_answer};
use evo_engine::hosts::run_owned_config_reference;
use evo_storage::{OwnedConfigWorkspace, OwnedWorkspaceError};
use serde_json::{Value, json};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn task(
    template: Template,
    layers: Layers,
    key_type: KeyType,
    placement: Placement,
    seed: Seed,
) -> ClosedCaseSelector {
    ClosedCaseSelector::Task {
        template,
        layer_count: layers,
        key_type,
        placement,
        seed,
    }
}
#[test]
fn owned_reference_executes_generator_issuer_parser() {
    let _serial = SERIAL.lock().unwrap();
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            for key_type in KEY_TYPES {
                for placement in PLACEMENTS {
                    for seed in SEEDS {
                        let selector = task(template, layers, key_type, placement, seed);
                        let case = generate_case(selector).unwrap();
                        let expected = oracle_answer(&case).unwrap();
                        let actual = run_owned_config_reference(selector).unwrap();
                        assert_eq!(
                            grade_exact(&expected, &serde_json::to_value(actual).unwrap()),
                            1,
                            "{}",
                            case.task_id()
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn priority_and_missing_key_use_actual_files() {
    let _serial = SERIAL.lock().unwrap();
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            let priority = run_owned_config_reference(task(
                template,
                layers,
                KeyType::S,
                Placement::E,
                Seed::One,
            ))
            .unwrap();
            assert_eq!(priority.value, json!(""));
            assert_eq!(priority.source_layer, json!("override"));
            assert_eq!(priority.source_path, json!("override/config.json"));
            let inherited = run_owned_config_reference(task(
                template,
                layers,
                KeyType::S,
                Placement::M,
                Seed::One,
            ))
            .unwrap();
            assert_eq!(inherited.value, json!("base-1"));
            assert_eq!(inherited.source_layer, json!("defaults"));
            assert_eq!(inherited.source_path, json!("defaults/base.json"));
        }
    }
}
#[test]
fn false_zero_empty_survive_real_reads() {
    let _serial = SERIAL.lock().unwrap();
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            for (key, value) in [
                (KeyType::B, json!(false)),
                (KeyType::I, json!(0)),
                (KeyType::S, json!("")),
            ] {
                let answer = run_owned_config_reference(task(
                    template,
                    layers,
                    key,
                    Placement::E,
                    Seed::Zero,
                ))
                .unwrap();
                assert!(answer.present);
                assert_eq!(answer.value, value);
                assert_eq!(answer.source_layer, json!("override"));
                assert_eq!(answer.source_path, json!("override/config.json"));
            }
        }
    }
}
#[test]
fn nullable_and_inherited_null_have_real_sources() {
    let _serial = SERIAL.lock().unwrap();
    for template in TEMPLATES {
        for layers in LAYER_COUNTS {
            let null = run_owned_config_reference(task(
                template,
                layers,
                KeyType::N0,
                Placement::E,
                Seed::One,
            ))
            .unwrap();
            assert!(null.present && null.value.is_null());
            assert_eq!(null.source_layer, json!("override"));
            assert_eq!(null.source_path, json!("override/config.json"));
            let inherited = run_owned_config_reference(task(
                template,
                layers,
                KeyType::H,
                Placement::E,
                Seed::One,
            ))
            .unwrap();
            let prefix = if layers == Layers::L3 {
                "project"
            } else {
                "base"
            };
            assert_eq!(inherited.value, json!(format!("{prefix}-theme-1")));
            assert_eq!(
                inherited.source_layer,
                json!(if layers == Layers::L3 {
                    "project"
                } else {
                    "defaults"
                })
            );
            assert_eq!(inherited.json_pointer, inherited.key);
        }
    }
}
#[test]
fn not_found_is_distinct_from_io_failure() {
    let _serial = SERIAL.lock().unwrap();
    let answer = run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::NotFound)).unwrap();
    assert!(!answer.present);
    assert_eq!(answer.key, "/title");
    assert!(
        answer.value.is_null()
            && answer.source_layer.is_null()
            && answer.source_path.is_null()
            && answer.json_pointer.is_null()
    );
    // A real active issuer failure propagates rather than becoming an absent answer.
    // Actual IO/truncation failures are injected into the private storage read boundary.
    let workspace = OwnedConfigWorkspace::create(
        generate_case(ClosedCaseSelector::Anchor(Anchor::False)).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::NotFound)),
        Err(OwnedWorkspaceError::Active)
    ));
    workspace.finish().unwrap();
    assert!(matches!(
        run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::UnknownFormat)),
        Err(OwnedWorkspaceError::UnsupportedFormat("toml"))
    ));
}
#[test]
fn write_and_outside_requests_cannot_expand_cap() {
    let _serial = SERIAL.lock().unwrap();
    assert!(matches!(
        run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::NoWrite)),
        Err(OwnedWorkspaceError::WriteForbidden)
    ));
    assert!(matches!(
        run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::WorkspaceBoundary)),
        Err(OwnedWorkspaceError::WorkspaceBoundary)
    ));
    let answer = run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::False)).unwrap();
    assert_eq!(answer.value, json!(false));
    assert!(parse_case_selector(&json!({"anchor":"no_write","bytes":"replace"})).is_err());
    assert!(
        parse_case_selector(&json!({"anchor":"workspace_boundary","path":"../sentinel"})).is_err()
    );
}
#[test]
fn unsupported_format_has_real_owned_material() {
    let _serial = SERIAL.lock().unwrap();
    let generated = generate_case(ClosedCaseSelector::Anchor(Anchor::UnknownFormat)).unwrap();
    assert_eq!(
        generated.files()[0].source_path(),
        "config/unsupported.toml"
    );
    let mut workspace = OwnedConfigWorkspace::create(generated).unwrap();
    let snapshot = workspace.read_once().unwrap();
    assert_eq!(snapshot.files()[0].format(), "toml");
    assert_eq!(snapshot.files()[0].bytes(), b"x = 1\n");
    workspace.finish().unwrap();
    assert!(matches!(
        run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::UnknownFormat)),
        Err(OwnedWorkspaceError::UnsupportedFormat("toml"))
    ));
}
#[test]
fn every_answer_or_error_finishes_before_return() {
    let _serial = SERIAL.lock().unwrap();
    for _ in 0..2 {
        for anchor in ANCHORS {
            let result = run_owned_config_reference(ClosedCaseSelector::Anchor(anchor));
            match anchor {
                Anchor::NoWrite => {
                    assert!(matches!(result, Err(OwnedWorkspaceError::WriteForbidden)))
                }
                Anchor::WorkspaceBoundary => assert!(matches!(
                    result,
                    Err(OwnedWorkspaceError::WorkspaceBoundary)
                )),
                Anchor::UnknownFormat => assert!(matches!(
                    result,
                    Err(OwnedWorkspaceError::UnsupportedFormat("toml"))
                )),
                _ => {
                    let answer = result.unwrap();
                    assert_eq!(
                        serde_json::to_value(answer)
                            .unwrap()
                            .as_object()
                            .unwrap()
                            .len(),
                        6
                    );
                }
            }
            let mut next = OwnedConfigWorkspace::create(
                generate_case(ClosedCaseSelector::Anchor(Anchor::False)).unwrap(),
            )
            .unwrap();
            let snapshot = next.read_once().unwrap();
            assert_eq!(snapshot.files()[0].bytes(), b"{\"enabled\":false}");
            next.finish().unwrap();
        }
    }
    assert!(serde_json::from_value::<evo_engine::hosts::VerifiedHostUse>(Value::Null).is_err());
}
