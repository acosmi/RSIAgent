//! The standalone compiler requires P/B labels without treating them as content anchors.
use evo_core::contract::{
    ImproverPatch, OriginLayer, PatchOp, Profile, SkillPatch, SkillSnapshot, ValueOrigin,
    compile_improver, compile_skill,
};
use evo_core::{Error, Strategy, Validate, fingerprint};
use std::collections::BTreeSet;

const REQUIRED: &str = "parent and baseline digests are required";
const UNBOUNDED: &str = "unsupported or unbounded strategy";
const INSTRUCTION_INVALID: &str = "instruction: expected nonempty text <= 8192 bytes";
const EXPLICIT_INSTRUCTION: &str = "显式 café\r\n只改已开放叶 e\u{301}";

fn profile(parent_digest: &str, baseline_digest: &str) -> Profile {
    Profile {
        id: "profile-fixture".into(),
        evolution_enabled: true,
        parent_digest: parent_digest.into(),
        baseline_digest: baseline_digest.into(),
    }
}

fn strategies() -> (Strategy, Strategy) {
    (
        Strategy {
            schema_version: "evo.strategy.v1".into(),
            instruction: "父 café\r\n保留 e\u{301}".into(),
            max_candidates: 4,
            max_rounds: 3,
            require_counterexample: false,
        },
        Strategy {
            schema_version: "evo.strategy.v1".into(),
            instruction: "安全 baseline\n保留原字节".into(),
            max_candidates: 1,
            max_rounds: 1,
            require_counterexample: true,
        },
    )
}

fn op<T>(choice: usize, explicit: T) -> PatchOp<T> {
    match choice {
        0 => PatchOp::Inherit,
        1 => PatchOp::ResetToBaseline,
        2 => PatchOp::Set { value: explicit },
        _ => unreachable!(),
    }
}

fn patch(instruction: usize, candidates: usize, rounds: usize) -> ImproverPatch {
    ImproverPatch {
        instruction: op(instruction, EXPLICIT_INSTRUCTION.to_owned()),
        max_candidates: op(candidates, 2),
        max_rounds: op(rounds, 2),
    }
}

fn assert_invalid<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    match result {
        Err(Error::Invalid(message)) => assert_eq!(message, expected),
        other => panic!("expected Invalid({expected:?}), got {other:?}"),
    }
}

fn assert_missing_matrix(parent_digest: &str, baseline_digest: &str) {
    let profile = profile(parent_digest, baseline_digest);
    let (parent, baseline) = strategies();
    let mut unexpected = Vec::new();
    for instruction in 0..3 {
        for candidates in 0..3 {
            for rounds in 0..3 {
                let patch = patch(instruction, candidates, rounds);
                let result = compile_improver(&profile, &parent, &baseline, &patch);
                match result {
                    Err(Error::Invalid(message)) if message == REQUIRED => {}
                    other => unexpected.push(format!(
                        "P={parent_digest:?} B={baseline_digest:?} \
                         instruction={instruction} candidates={candidates} rounds={rounds}: {other:?}"
                    )),
                }
            }
        }
    }
    // Accumulate every outcome so a baseline or a mutant proves the entire matrix.
    for outcome in &unexpected {
        eprintln!("MISSING_LABEL_UNEXPECTED {outcome}");
    }
    assert!(
        unexpected.is_empty(),
        "{} of 27 public-API cases did not reject missing P/B labels",
        unexpected.len()
    );
}

#[test]
fn missing_parent_label_rejects_every_patch_combination() {
    assert_missing_matrix("", "baseline-fixture");
}

#[test]
fn missing_baseline_label_rejects_every_patch_combination() {
    assert_missing_matrix("parent-fixture", "");
}

#[test]
fn missing_both_labels_rejects_every_patch_combination() {
    assert_missing_matrix("", "");
}

#[test]
fn skill_compiler_keeps_the_same_missing_label_error() {
    for (p, b) in [("", "baseline-fixture"), ("parent-fixture", ""), ("", "")] {
        assert_invalid(
            compile_skill(
                &profile(p, b),
                &SkillSnapshot::empty(),
                &SkillSnapshot::empty(),
                &SkillPatch::default(),
                &BTreeSet::new(),
            ),
            REQUIRED,
        );
    }
}

// Captured from the unmodified public compiler at 9b08c4b2fd882abab65f48ea0af8f23b13a3b50d.
struct FrozenOutput {
    name: &'static str,
    json: &'static str,
    strategy_digest: &'static str,
    origins_digest: &'static str,
    output_digest: &'static str,
}

const FROZEN_OUTPUTS: [FrozenOutput; 4] = [
    FrozenOutput {
        name: "inherit",
        json: r#"[{"schema_version":"evo.strategy.v1","instruction":"父 café\r\n保留 é","max_candidates":4,"max_rounds":3,"require_counterexample":false},[{"field":"improver.instruction","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"09faaafacb5078842e9e0ce461d239a64434158b3fd41ba9f2bcaca6618d7cc4"},{"field":"improver.max_candidates","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"4b227777d4dd1fc61c6f884f48641d02b4d121d3fd328cb08b5531fcacdabf8a"},{"field":"improver.max_rounds","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"4e07408562bedb8b60ce05c1decfe3ad16b72230967de01f640b7e4729b49fce"}]]"#,
        strategy_digest: "62110e7267c038dfec45cca86d42737595973746c9e7f8eff25815f59a6ed3dd",
        origins_digest: "8447046871a49ab995e31fb6b158591276bbb0359e9fbf953c22a0c638c976e7",
        output_digest: "7f54f7da524985902f81343256dd6323c0f033dd0c3971597c3ec6a3ccfd54f3",
    },
    FrozenOutput {
        name: "reset",
        json: r#"[{"schema_version":"evo.strategy.v1","instruction":"安全 baseline\n保留原字节","max_candidates":1,"max_rounds":1,"require_counterexample":false},[{"field":"improver.instruction","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"9f049afb12915f02ade153709cb2b8801f3efb2e2b8ddaa8456c0e455ef4c4e0"},{"field":"improver.max_candidates","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"},{"field":"improver.max_rounds","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"}]]"#,
        strategy_digest: "e4208414d522696034496067ae403ec2e171a0418f7e0518d774805e633a083a",
        origins_digest: "452118c5ea71822fda259b7223124389ad39e97f93800d0c6de1978069940e46",
        output_digest: "7fff9f2c4bf11ca95200c0752fb9b2d270fb6e84d05e50263ebfcc55f0309878",
    },
    FrozenOutput {
        name: "set",
        json: r#"[{"schema_version":"evo.strategy.v1","instruction":"显式 café\r\n只改已开放叶 é","max_candidates":2,"max_rounds":2,"require_counterexample":false},[{"field":"improver.instruction","layer":"explicit","source_digest":"set","op":"set","value_digest":"4427d8c3a10a0438dd0d793fa96d9b00543743c1c29a84b89b1dab7f6f616049"},{"field":"improver.max_candidates","layer":"explicit","source_digest":"set","op":"set","value_digest":"d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666eec13ab35"},{"field":"improver.max_rounds","layer":"explicit","source_digest":"set","op":"set","value_digest":"d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666eec13ab35"}]]"#,
        strategy_digest: "4721173666df828203f9c2de268bb7eb1e6415244d1823883e56ba41242b0906",
        origins_digest: "10231ccc98bf7cf0ddb38e1eb14991c5aa2cc100ab90b93551f1e5b80b29039b",
        output_digest: "aa61ab94a74f6c6a582e2cafcc940d7ef0aa5e35fba12785b963bab43c2b272a",
    },
    FrozenOutput {
        name: "mixed",
        json: r#"[{"schema_version":"evo.strategy.v1","instruction":"父 café\r\n保留 é","max_candidates":1,"max_rounds":2,"require_counterexample":false},[{"field":"improver.instruction","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"09faaafacb5078842e9e0ce461d239a64434158b3fd41ba9f2bcaca6618d7cc4"},{"field":"improver.max_candidates","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"},{"field":"improver.max_rounds","layer":"explicit","source_digest":"set","op":"set","value_digest":"d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666eec13ab35"}]]"#,
        strategy_digest: "24fc2787566117f0238e16245020b8da3e28f8c4f9bf8afd213257b245eecb4e",
        origins_digest: "8e8c49061c20b7b078d92c713d069876aa37f1e8f16bc619704202b258c5c185",
        output_digest: "26f8d9191afefc5f06bbe89afab911a85f96ab78209227a3e479e17c40cbbe46",
    },
];

#[test]
fn legal_outputs_match_frozen_json_and_fingerprints() {
    let profile = profile("parent-fixture", "baseline-fixture");
    let (parent, baseline) = strategies();
    for (name, patch) in [
        ("inherit", ImproverPatch::default()),
        ("reset", patch(1, 1, 1)),
        ("set", patch(2, 2, 2)),
        ("mixed", patch(0, 1, 2)),
    ] {
        let output = compile_improver(&profile, &parent, &baseline, &patch).unwrap();
        let json = serde_json::to_string(&output).unwrap();
        let strategy_digest = fingerprint(&output.0).unwrap();
        let origins_digest = fingerprint(&output.1).unwrap();
        let output_digest = fingerprint(&output).unwrap();
        println!("LEGAL_SNAPSHOT {name} json={json}");
        println!(
            "LEGAL_DIGEST {name} strategy={strategy_digest} \
             origins={origins_digest} output={output_digest}"
        );
        let frozen = FROZEN_OUTPUTS
            .iter()
            .find(|sample| sample.name == name)
            .unwrap();
        assert_eq!(
            json.as_bytes(),
            frozen.json.as_bytes(),
            "{name}: complete output bytes"
        );
        assert_eq!(
            strategy_digest, frozen.strategy_digest,
            "{name}: Strategy fingerprint"
        );
        assert_eq!(
            origins_digest, frozen.origins_digest,
            "{name}: ValueOrigin fingerprint"
        );
        assert_eq!(
            output_digest, frozen.output_digest,
            "{name}: complete output fingerprint"
        );
        output.0.validate().unwrap();
        assert_eq!(output.1.len(), 3);
    }
}

#[test]
fn all_legal_patch_combinations_preserve_values_and_origin_order() {
    let profile = profile("parent-fixture", "baseline-fixture");
    let (parent, baseline) = strategies();
    let instructions = [
        parent.instruction.as_str(),
        baseline.instruction.as_str(),
        EXPLICIT_INSTRUCTION,
    ];
    let candidates = [4, 1, 2];
    let rounds = [3, 1, 2];
    for (i, &instruction) in instructions.iter().enumerate() {
        for (c, &max_candidates) in candidates.iter().enumerate() {
            for (r, &max_rounds) in rounds.iter().enumerate() {
                let (out, origins) =
                    compile_improver(&profile, &parent, &baseline, &patch(i, c, r)).unwrap();
                assert_eq!(out.schema_version, parent.schema_version);
                assert_eq!(out.instruction, instruction);
                assert_eq!(out.max_candidates, max_candidates);
                assert_eq!(out.max_rounds, max_rounds);
                assert_eq!(out.require_counterexample, parent.require_counterexample);
                let expected_fields = [
                    "improver.instruction",
                    "improver.max_candidates",
                    "improver.max_rounds",
                ];
                let value_digests = [
                    fingerprint(&out.instruction).unwrap(),
                    fingerprint(&out.max_candidates).unwrap(),
                    fingerprint(&out.max_rounds).unwrap(),
                ];
                assert_eq!(origins.len(), 3);
                for (index, choice) in [i, c, r].into_iter().enumerate() {
                    let (layer, source, operation) = match choice {
                        0 => (OriginLayer::Parent, "parent-fixture", "inherit"),
                        1 => (
                            OriginLayer::Baseline,
                            "baseline-fixture",
                            "reset_to_baseline",
                        ),
                        2 => (OriginLayer::Explicit, "set", "set"),
                        _ => unreachable!(),
                    };
                    let origin = &origins[index];
                    assert_eq!(origin.field, expected_fields[index]);
                    assert_eq!(origin.layer, layer);
                    assert_eq!(origin.source_digest, source);
                    assert_eq!(origin.op, operation);
                    assert_eq!(origin.value_digest, value_digests[index]);
                }
            }
        }
    }
}

fn invalid_strategies() -> Vec<(Strategy, &'static str)> {
    let (valid, _) = strategies();
    let mut cases = Vec::new();
    let mut invalid = valid.clone();
    invalid.schema_version = "evo.strategy.v2".into();
    cases.push((invalid, UNBOUNDED));
    for max_candidates in [0, 5] {
        let mut invalid = valid.clone();
        invalid.max_candidates = max_candidates;
        cases.push((invalid, UNBOUNDED));
    }
    for max_rounds in [0, 4] {
        let mut invalid = valid.clone();
        invalid.max_rounds = max_rounds;
        cases.push((invalid, UNBOUNDED));
    }
    for instruction in [
        String::new(),
        " \t\r\n".into(),
        "bad\0text".into(),
        "é".repeat(4097),
    ] {
        let mut invalid = valid.clone();
        invalid.instruction = instruction;
        cases.push((invalid, INSTRUCTION_INVALID));
    }
    cases
}

fn label_cases() -> [Profile; 4] {
    [
        profile("", "baseline-fixture"),
        profile("parent-fixture", ""),
        profile("", ""),
        profile("parent-fixture", "baseline-fixture"),
    ]
}

#[test]
fn parent_validation_keeps_priority_over_baseline_and_missing_labels() {
    for (invalid_parent, expected) in invalid_strategies() {
        let (_, mut invalid_baseline) = strategies();
        if expected == UNBOUNDED {
            invalid_baseline.instruction.clear();
        } else {
            invalid_baseline.schema_version = "evo.strategy.v2".into();
        }
        for profile in label_cases() {
            assert_invalid(
                compile_improver(
                    &profile,
                    &invalid_parent,
                    &invalid_baseline,
                    &ImproverPatch::default(),
                ),
                expected,
            );
        }
    }
}

#[test]
fn baseline_validation_keeps_priority_over_missing_labels() {
    let (parent, _) = strategies();
    for (invalid_baseline, expected) in invalid_strategies() {
        for profile in label_cases() {
            assert_invalid(
                compile_improver(
                    &profile,
                    &parent,
                    &invalid_baseline,
                    &ImproverPatch::default(),
                ),
                expected,
            );
        }
    }
}

#[test]
fn explicit_empty_instruction_and_invalid_numeric_sets_keep_old_rejections() {
    let profile = profile("parent-fixture", "baseline-fixture");
    let (parent, baseline) = strategies();
    for (invalid, expected) in invalid_strategies() {
        if invalid.schema_version != parent.schema_version {
            continue; // schema_version is not an ImproverPatch leaf.
        }
        let patch = ImproverPatch {
            instruction: PatchOp::Set {
                value: invalid.instruction,
            },
            max_candidates: PatchOp::Set {
                value: invalid.max_candidates,
            },
            max_rounds: PatchOp::Set {
                value: invalid.max_rounds,
            },
        };
        let result = compile_improver(&profile, &parent, &baseline, &patch);
        println!("INVALID_SET expected={expected:?} actual={result:?}");
        assert_invalid(result, expected);
    }
}

#[test]
fn numeric_bounds_and_both_counterexample_values_keep_parent_semantics() {
    let profile = profile("parent-fixture", "baseline-fixture");
    let (mut parent, mut baseline) = strategies();
    for require_counterexample in [false, true] {
        parent.require_counterexample = require_counterexample;
        baseline.require_counterexample = !require_counterexample;
        for candidates in [1, 4] {
            for rounds in [1, 3] {
                let mut patch = patch(2, 2, 2);
                patch.max_candidates = PatchOp::Set { value: candidates };
                patch.max_rounds = PatchOp::Set { value: rounds };
                let (out, _) = compile_improver(&profile, &parent, &baseline, &patch).unwrap();
                assert_eq!(out.max_candidates, candidates);
                assert_eq!(out.max_rounds, rounds);
                assert_eq!(out.require_counterexample, require_counterexample);
            }
        }
    }
}

#[test]
fn omission_and_explicit_inherit_keep_identical_outputs() {
    let profile = profile("parent-fixture", "baseline-fixture");
    let (parent, baseline) = strategies();
    let omitted: ImproverPatch = serde_json::from_str("{}").unwrap();
    let explicit: ImproverPatch = serde_json::from_str(
        r#"{"instruction":{"op":"inherit"},"max_candidates":{"op":"inherit"},"max_rounds":{"op":"inherit"}}"#,
    )
    .unwrap();
    let omitted = compile_improver(&profile, &parent, &baseline, &omitted).unwrap();
    let explicit = compile_improver(&profile, &parent, &baseline, &explicit).unwrap();
    assert_eq!(
        serde_json::to_vec(&omitted).unwrap(),
        serde_json::to_vec(&explicit).unwrap()
    );
    assert_eq!(
        fingerprint(&omitted).unwrap(),
        fingerprint(&explicit).unwrap()
    );
}

#[test]
fn equal_labels_and_different_labels_for_equal_content_remain_allowed() {
    let (parent, _) = strategies();
    for profile in [profile("same", "same"), profile("P", "B")] {
        for patch in [ImproverPatch::default(), patch(1, 1, 1)] {
            let (out, origins) = compile_improver(&profile, &parent, &parent, &patch).unwrap();
            assert_eq!(
                serde_json::to_vec(&out).unwrap(),
                serde_json::to_vec(&parent).unwrap()
            );
            let source = if origins[0].layer == OriginLayer::Parent {
                &profile.parent_digest
            } else {
                &profile.baseline_digest
            };
            assert!(origins.iter().all(|origin| &origin.source_digest == source));
        }
    }
}

#[test]
fn nonempty_labels_keep_the_standalone_entrys_existing_boundary() {
    let (parent, baseline) = strategies();
    for (p, b) in [
        (" ", "\t"),
        ("parent/任意标签", "baseline?标签"),
        (&"p".repeat(129), &"b".repeat(129)),
    ] {
        let mut profile = profile(p, b);
        profile.id.clear();
        profile.evolution_enabled = false;
        let (out, origins): (Strategy, Vec<ValueOrigin>) =
            compile_improver(&profile, &parent, &baseline, &patch(0, 1, 2)).unwrap();
        assert_eq!(out.instruction, parent.instruction);
        assert_eq!(out.max_candidates, baseline.max_candidates);
        assert_eq!(out.max_rounds, 2);
        assert_eq!(origins[0].source_digest, p);
        assert_eq!(origins[1].source_digest, b);
        assert_eq!(origins[2].source_digest, "set");
    }
}
