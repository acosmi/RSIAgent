//! Known direct dependency revocation on supplied P/B snapshots.
//! These tests use public compiler APIs; no Store or trusted revocation view is implied.
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, PatchOp, Profile, ResolvedBundle, SkillPatch,
    SkillSnapshot, ValueOrigin, assert_candidate_may_write, compile_bundle, compile_improver,
    compile_skill,
};
use evo_core::{ArtifactKind, Error, Result, Strategy, fingerprint};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Debug;

#[derive(Clone)]
struct Fixture {
    profile: Profile,
    parent: SkillSnapshot,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    caps: HostCapabilities,
}

impl Fixture {
    fn new() -> Self {
        Self {
            profile: Profile {
                id: "ag092-profile".into(),
                evolution_enabled: true,
                parent_digest: "parent-fixture".into(),
                baseline_digest: "baseline-fixture".into(),
            },
            parent: SkillSnapshot {
                content: "父 café\r\n保留 e\u{301}".into(),
                applicability: "parent applicability".into(),
                counterexample: "parent counterexample".into(),
                required_capabilities: vec!["fs_read".into()],
                dependencies: vec!["dep-p".into()],
            },
            baseline: SkillSnapshot {
                content: "安全 baseline\n保留原字节".into(),
                applicability: "baseline applicability".into(),
                counterexample: "baseline counterexample".into(),
                required_capabilities: vec!["clock".into()],
                dependencies: vec!["dep-b".into()],
            },
            parent_strategy: Strategy {
                schema_version: "evo.strategy.v1".into(),
                instruction: "父 strategy".into(),
                max_candidates: 4,
                max_rounds: 3,
                require_counterexample: false,
            },
            baseline_strategy: Strategy {
                schema_version: "evo.strategy.v1".into(),
                instruction: "安全 baseline strategy".into(),
                max_candidates: 1,
                max_rounds: 1,
                require_counterexample: true,
            },
            caps: HostCapabilities {
                available: ids(&["clock", "fs_read"]),
                granted: ids(&["clock", "fs_read"]),
            },
        }
    }

    fn skill(
        &self,
        patch: &SkillPatch,
        revoked: &BTreeSet<String>,
    ) -> Result<(SkillSnapshot, Vec<ValueOrigin>)> {
        compile_skill(&self.profile, &self.parent, &self.baseline, patch, revoked)
    }

    fn bundle(
        &self,
        skill_patch: &SkillPatch,
        improver_patch: &ImproverPatch,
        revoked: &BTreeSet<String>,
    ) -> Result<ResolvedBundle> {
        compile_bundle(CompileParts {
            profile: &self.profile,
            parent: &self.parent,
            baseline: &self.baseline,
            parent_strategy: &self.parent_strategy,
            baseline_strategy: &self.baseline_strategy,
            skill_patch,
            improver_patch,
            caps: &self.caps,
            revoked,
        })
    }
}

fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn assert_revoked<T: Debug>(result: Result<T>) {
    match result {
        Err(Error::Conflict(message)) => assert_eq!(message, "dependency_revoked"),
        other => panic!("expected dependency_revoked, got {other:?}"),
    }
}

fn assert_invalid<T: Debug>(result: Result<T>, expected: &str) {
    match result {
        Err(Error::Invalid(message)) => assert_eq!(message, expected),
        other => panic!("expected Invalid({expected:?}), got {other:?}"),
    }
}

fn reset_skill() -> SkillPatch {
    SkillPatch {
        content: PatchOp::ResetToBaseline,
        applicability: PatchOp::ResetToBaseline,
        counterexample: PatchOp::ResetToBaseline,
        required_capabilities: PatchOp::ResetToBaseline,
        dependencies: PatchOp::ResetToBaseline,
    }
}

fn set_empty_skill() -> SkillPatch {
    SkillPatch {
        content: PatchOp::Set {
            value: String::new(),
        },
        applicability: PatchOp::Set {
            value: String::new(),
        },
        counterexample: PatchOp::Set {
            value: String::new(),
        },
        required_capabilities: PatchOp::Set { value: Vec::new() },
        dependencies: PatchOp::Set { value: Vec::new() },
    }
}

fn set_improver() -> ImproverPatch {
    ImproverPatch {
        instruction: PatchOp::Set {
            value: "explicit strategy".into(),
        },
        max_candidates: PatchOp::Set { value: 3 },
        max_rounds: PatchOp::Set { value: 2 },
    }
}

fn healthy_control(
    fixture: &Fixture,
    skill_patch: &SkillPatch,
    improver_patch: &ImproverPatch,
    content: &str,
    dependencies: &[&str],
) {
    let clean = BTreeSet::new();
    // Profile labels and capabilities are not dependency IDs in these inputs.
    let unrelated = ids(&[
        "unrelated-id",
        "parent-fixture",
        "baseline-fixture",
        "fs_read",
        "clock",
    ]);
    let (skill, origins) = fixture.skill(skill_patch, &clean).unwrap();
    assert_eq!(skill.content, content);
    assert_eq!(
        skill.dependencies,
        dependencies
            .iter()
            .map(|id| (*id).to_string())
            .collect::<Vec<_>>()
    );
    let clean_skill = serde_json::to_vec(&(&skill, &origins)).unwrap();
    let unrelated_skill = fixture.skill(skill_patch, &unrelated).unwrap();
    assert_eq!(clean_skill, serde_json::to_vec(&unrelated_skill).unwrap());
    let clean_bundle = fixture.bundle(skill_patch, improver_patch, &clean).unwrap();
    assert_eq!(
        serde_json::to_vec(&clean_bundle).unwrap(),
        serde_json::to_vec(
            &fixture
                .bundle(skill_patch, improver_patch, &unrelated)
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&clean_bundle.skill).unwrap(),
        serde_json::to_vec(&skill).unwrap()
    );
    assert_eq!(origins.len(), 5);
    assert_eq!(clean_bundle.origins.len(), 8);
}

#[test]
fn baseline_content_reset_cannot_hide_revoked_baseline_dependency() {
    let mut fixture = Fixture::new();
    fixture.parent.dependencies.clear();
    let patch = SkillPatch {
        content: PatchOp::ResetToBaseline,
        ..SkillPatch::default()
    };
    let improver = ImproverPatch::default();
    let revoked = ids(&["dep-b"]);

    assert_revoked(fixture.skill(&patch, &revoked));
    assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    healthy_control(&fixture, &patch, &improver, &fixture.baseline.content, &[]);
}

#[test]
fn inherited_parent_content_cannot_hide_dependency_with_explicit_empty_set() {
    let fixture = Fixture::new();
    let patch = SkillPatch {
        dependencies: PatchOp::Set { value: Vec::new() },
        ..SkillPatch::default()
    };
    let improver = ImproverPatch::default();
    let revoked = ids(&["dep-p"]);

    assert_revoked(fixture.skill(&patch, &revoked));
    assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    healthy_control(&fixture, &patch, &improver, &fixture.parent.content, &[]);
}

#[test]
fn reset_of_every_leaf_still_rejects_unselected_parent_dependency() {
    let fixture = Fixture::new();
    let patch = reset_skill();
    let improver = ImproverPatch::default();
    let revoked = ids(&["dep-p"]);

    assert_revoked(fixture.skill(&patch, &revoked));
    assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    healthy_control(
        &fixture,
        &patch,
        &improver,
        &fixture.baseline.content,
        &["dep-b"],
    );
}

#[test]
fn inherit_of_every_leaf_still_rejects_unselected_baseline_dependency() {
    let fixture = Fixture::new();
    let patch = SkillPatch::default();
    let improver = ImproverPatch::default();
    let revoked = ids(&["dep-b"]);

    assert_revoked(fixture.skill(&patch, &revoked));
    assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    healthy_control(
        &fixture,
        &patch,
        &improver,
        &fixture.parent.content,
        &["dep-p"],
    );
}

#[test]
fn all_set_skill_and_improver_cannot_bypass_either_input_gate() {
    let fixture = Fixture::new();
    let patch = set_empty_skill();
    let improver = set_improver();

    for dependency in ["dep-p", "dep-b"] {
        let revoked = ids(&[dependency]);
        assert_revoked(fixture.skill(&patch, &revoked));
        assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    }
    healthy_control(&fixture, &patch, &improver, "", &[]);
}

#[test]
fn newly_set_revoked_dependency_still_fails_final_output_check() {
    let mut fixture = Fixture::new();
    fixture.parent.dependencies.clear();
    fixture.baseline.dependencies.clear();
    let patch = SkillPatch {
        dependencies: PatchOp::Set {
            value: vec!["dep-new".into()],
        },
        ..set_empty_skill()
    };
    let improver = set_improver();
    let revoked = ids(&["dep-new"]);

    assert_revoked(fixture.skill(&patch, &revoked));
    assert_revoked(fixture.bundle(&patch, &improver, &revoked));
    healthy_control(&fixture, &patch, &improver, "", &["dep-new"]);
}

#[test]
fn input_revocation_precedes_invalid_patched_output() {
    let fixture = Fixture::new();
    let patch = SkillPatch {
        content: PatchOp::Set {
            value: "\0invalid output".into(),
        },
        ..set_empty_skill()
    };
    let improver = set_improver();

    assert_revoked(fixture.skill(&patch, &ids(&["dep-p"])));
    assert_revoked(fixture.bundle(&patch, &improver, &ids(&["dep-p"])));
    assert_invalid(
        fixture.skill(&patch, &BTreeSet::new()),
        "content: expected nonempty text <= 16384 bytes",
    );
}

#[test]
fn both_input_shapes_are_validated_before_direct_dependency_revocation() {
    let patch = set_empty_skill();
    let improver = set_improver();
    let revoked = ids(&["dep-p", "dep-b"]);

    for parent_is_invalid in [true, false] {
        let mut fixture = Fixture::new();
        let input = if parent_is_invalid {
            &mut fixture.parent
        } else {
            &mut fixture.baseline
        };
        input.dependencies.push("not an identifier".into());
        assert_invalid(fixture.skill(&patch, &revoked), "invalid identifier");
        assert_invalid(
            fixture.bundle(&patch, &improver, &revoked),
            "invalid identifier",
        );
    }

    let mut fixture = Fixture::new();
    fixture.parent.content = "\0invalid parent".into();
    fixture
        .baseline
        .dependencies
        .push("not an identifier".into());
    assert_invalid(
        fixture.skill(&patch, &revoked),
        "content: expected nonempty text <= 16384 bytes",
    );
    assert_invalid(
        fixture.bundle(&patch, &improver, &revoked),
        "content: expected nonempty text <= 16384 bytes",
    );
}

#[test]
fn missing_labels_and_bundle_identifier_errors_keep_original_precedence() {
    let patch = set_empty_skill();
    let improver = set_improver();
    let revoked = ids(&["dep-p", "dep-b"]);

    for parent_label_is_missing in [true, false] {
        let mut fixture = Fixture::new();
        if parent_label_is_missing {
            fixture.profile.parent_digest.clear();
        } else {
            fixture.profile.baseline_digest.clear();
        }
        assert_invalid(
            fixture.skill(&patch, &revoked),
            "parent and baseline digests are required",
        );
        assert_invalid(
            fixture.bundle(&patch, &improver, &revoked),
            "identifier: expected nonempty text <= 128 bytes",
        );
    }

    let mut fixture = Fixture::new();
    fixture.profile.id = "not an identifier".into();
    assert_invalid(
        fixture.bundle(&patch, &improver, &revoked),
        "invalid identifier",
    );
    fixture.profile.parent_digest.clear();
    fixture.baseline.content = "\0invalid baseline".into();
    assert_invalid(
        fixture.skill(&patch, &revoked),
        "content: expected nonempty text <= 16384 bytes",
    );
}

#[test]
fn dependency_id_equal_to_context_label_is_checked_as_an_actual_reference() {
    let patch = set_empty_skill();
    let improver = set_improver();

    for is_parent in [true, false] {
        let mut fixture = Fixture::new();
        fixture.parent.dependencies.clear();
        fixture.baseline.dependencies.clear();
        let dependency = if is_parent {
            fixture
                .parent
                .dependencies
                .push(fixture.profile.parent_digest.clone());
            fixture.profile.parent_digest.clone()
        } else {
            fixture
                .baseline
                .dependencies
                .push(fixture.profile.baseline_digest.clone());
            fixture.profile.baseline_digest.clone()
        };
        assert_revoked(fixture.skill(&patch, &ids(&[&dependency])));
        assert_revoked(fixture.bundle(&patch, &improver, &ids(&[&dependency])));
        let clean = BTreeSet::new();
        assert!(fixture.skill(&patch, &clean).is_ok());
        assert!(fixture.bundle(&patch, &improver, &clean).is_ok());
    }
}

// Static public-contract fixtures authored from the existing serde field order and
// documented leaf semantics. These are expected outputs, not a claimed Parent run.
const FROZEN_HEALTHY_BUNDLES: [&str; 4] = [
    r#"{"schema_version":"rsia.resolved_bundle.v2","profile_id":"ag092-profile","parent_digest":"parent-fixture","baseline_digest":"baseline-fixture","skill":{"content":"父 café\r\n保留 é","applicability":"parent applicability","counterexample":"parent counterexample","required_capabilities":["fs_read"],"dependencies":["dep-p"]},"improver":{"schema_version":"evo.strategy.v1","instruction":"父 strategy","max_candidates":4,"max_rounds":3,"require_counterexample":false},"origins":[{"field":"skill.content","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"09faaafacb5078842e9e0ce461d239a64434158b3fd41ba9f2bcaca6618d7cc4"},{"field":"skill.applicability","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"9b46718f217c4b2c7740eccd06abb7bb5b3d6d8dd1c37a439910e3609130e877"},{"field":"skill.counterexample","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"31d4d865b50dbe1c8f4cf7288e3c86bf61905329d59e610dcf1c141a24cda45b"},{"field":"skill.required_capabilities","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"2ef2b736bb91e50bef29723c9dcbc3beae3dd6764487a5045d66a6e38020095e"},{"field":"skill.dependencies","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"7e07fe2c19f2f0417dbb7d170641c38b6c09165c9a9213a0971d59e842d06882"},{"field":"improver.instruction","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"bd3888559be41df79fabbf59ff100d0953f6978589c58af66c264b4f27997d0f"},{"field":"improver.max_candidates","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"4b227777d4dd1fc61c6f884f48641d02b4d121d3fd328cb08b5531fcacdabf8a"},{"field":"improver.max_rounds","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"4e07408562bedb8b60ce05c1decfe3ad16b72230967de01f640b7e4729b49fce"}],"digest":"70832642add5620b077d7aebebcd0e61b5bedd15284d06fd9651f186ecee09f4"}"#,
    r#"{"schema_version":"rsia.resolved_bundle.v2","profile_id":"ag092-profile","parent_digest":"parent-fixture","baseline_digest":"baseline-fixture","skill":{"content":"安全 baseline\n保留原字节","applicability":"baseline applicability","counterexample":"baseline counterexample","required_capabilities":["clock"],"dependencies":["dep-b"]},"improver":{"schema_version":"evo.strategy.v1","instruction":"安全 baseline strategy","max_candidates":1,"max_rounds":1,"require_counterexample":false},"origins":[{"field":"skill.content","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"9f049afb12915f02ade153709cb2b8801f3efb2e2b8ddaa8456c0e455ef4c4e0"},{"field":"skill.applicability","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"91999d6f5d6266d3915a8fb3dd37837521064938d84faae78f94bc1aa6fd933d"},{"field":"skill.counterexample","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"a86346213e54edf119ccc92803674ec1662131c451a76a2d275a04d2a64946a7"},{"field":"skill.required_capabilities","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"cdadbd79d6234e2eedca9a8120b0b0ad4a4fa6f6cbb4f1ac6fb818d8e1b27b20"},{"field":"skill.dependencies","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"b02d520167042cd4c2f0191fd5b127dd986f84543d8b506b14725f0d88c4f098"},{"field":"improver.instruction","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"91d9b8f4785cc6435972cd0fe15a408e2650b502c405b9f169247adcc8ca450b"},{"field":"improver.max_candidates","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"},{"field":"improver.max_rounds","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"}],"digest":"07fc9e1c6c7c6eff58bf67b89e2150b08cfbbbfea6159ee239082678273c61f1"}"#,
    r#"{"schema_version":"rsia.resolved_bundle.v2","profile_id":"ag092-profile","parent_digest":"parent-fixture","baseline_digest":"baseline-fixture","skill":{"content":"","applicability":"","counterexample":"","required_capabilities":[],"dependencies":[]},"improver":{"schema_version":"evo.strategy.v1","instruction":"explicit strategy","max_candidates":3,"max_rounds":2,"require_counterexample":false},"origins":[{"field":"skill.content","layer":"explicit","source_digest":"set","op":"set","value_digest":"12ae32cb1ec02d01eda3581b127c1fee3b0dc53572ed6baf239721a03d82e126"},{"field":"skill.applicability","layer":"explicit","source_digest":"set","op":"set","value_digest":"12ae32cb1ec02d01eda3581b127c1fee3b0dc53572ed6baf239721a03d82e126"},{"field":"skill.counterexample","layer":"explicit","source_digest":"set","op":"set","value_digest":"12ae32cb1ec02d01eda3581b127c1fee3b0dc53572ed6baf239721a03d82e126"},{"field":"skill.required_capabilities","layer":"explicit","source_digest":"set","op":"set","value_digest":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"},{"field":"skill.dependencies","layer":"explicit","source_digest":"set","op":"set","value_digest":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"},{"field":"improver.instruction","layer":"explicit","source_digest":"set","op":"set","value_digest":"46592cc76834efd1ce8205a40ed88125eca14ef838be15687d12a0efb19ffb67"},{"field":"improver.max_candidates","layer":"explicit","source_digest":"set","op":"set","value_digest":"4e07408562bedb8b60ce05c1decfe3ad16b72230967de01f640b7e4729b49fce"},{"field":"improver.max_rounds","layer":"explicit","source_digest":"set","op":"set","value_digest":"d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666eec13ab35"}],"digest":"bef1d13d501fcf458a74b9d5ce57d4846c659eb4dc1a1688bfd67e895d51261e"}"#,
    r#"{"schema_version":"rsia.resolved_bundle.v2","profile_id":"ag092-profile","parent_digest":"parent-fixture","baseline_digest":"baseline-fixture","skill":{"content":"安全 baseline\n保留原字节","applicability":"parent applicability","counterexample":"","required_capabilities":[],"dependencies":["dep-p"]},"improver":{"schema_version":"evo.strategy.v1","instruction":"安全 baseline strategy","max_candidates":4,"max_rounds":2,"require_counterexample":false},"origins":[{"field":"skill.content","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"9f049afb12915f02ade153709cb2b8801f3efb2e2b8ddaa8456c0e455ef4c4e0"},{"field":"skill.applicability","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"9b46718f217c4b2c7740eccd06abb7bb5b3d6d8dd1c37a439910e3609130e877"},{"field":"skill.counterexample","layer":"explicit","source_digest":"set","op":"set","value_digest":"12ae32cb1ec02d01eda3581b127c1fee3b0dc53572ed6baf239721a03d82e126"},{"field":"skill.required_capabilities","layer":"explicit","source_digest":"set","op":"set","value_digest":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"},{"field":"skill.dependencies","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"7e07fe2c19f2f0417dbb7d170641c38b6c09165c9a9213a0971d59e842d06882"},{"field":"improver.instruction","layer":"baseline","source_digest":"baseline-fixture","op":"reset_to_baseline","value_digest":"91d9b8f4785cc6435972cd0fe15a408e2650b502c405b9f169247adcc8ca450b"},{"field":"improver.max_candidates","layer":"parent","source_digest":"parent-fixture","op":"inherit","value_digest":"4b227777d4dd1fc61c6f884f48641d02b4d121d3fd328cb08b5531fcacdabf8a"},{"field":"improver.max_rounds","layer":"explicit","source_digest":"set","op":"set","value_digest":"d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666eec13ab35"}],"digest":"5229a4d828bc10ea46c47625490983b6b7c17f0391ccfe93030c04a16ccc7097"}"#,
];

#[test]
fn healthy_full_api_outputs_preserve_bytes_origins_and_bundle_digests() {
    let fixture = Fixture::new();
    let omitted_skill: SkillPatch = serde_json::from_str("{}").unwrap();
    let omitted_improver: ImproverPatch = serde_json::from_str("{}").unwrap();
    let cases = [
        (omitted_skill, omitted_improver),
        (
            reset_skill(),
            ImproverPatch {
                instruction: PatchOp::ResetToBaseline,
                max_candidates: PatchOp::ResetToBaseline,
                max_rounds: PatchOp::ResetToBaseline,
            },
        ),
        (set_empty_skill(), set_improver()),
        (
            SkillPatch {
                content: PatchOp::ResetToBaseline,
                counterexample: PatchOp::Set {
                    value: String::new(),
                },
                required_capabilities: PatchOp::Set { value: Vec::new() },
                ..SkillPatch::default()
            },
            ImproverPatch {
                instruction: PatchOp::ResetToBaseline,
                max_rounds: PatchOp::Set { value: 2 },
                ..ImproverPatch::default()
            },
        ),
    ];

    for ((skill_patch, improver_patch), golden) in cases.iter().zip(FROZEN_HEALTHY_BUNDLES) {
        let clean = BTreeSet::new();
        let bundle = fixture.bundle(skill_patch, improver_patch, &clean).unwrap();
        assert_eq!(serde_json::to_string(&bundle).unwrap(), golden);
        let expected: Value = serde_json::from_str(golden).unwrap();
        let all_origins = expected["origins"].as_array().unwrap();
        let (skill, origins) = fixture.skill(skill_patch, &clean).unwrap();
        let (improver, improver_origins) = compile_improver(
            &fixture.profile,
            &fixture.parent_strategy,
            &fixture.baseline_strategy,
            improver_patch,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&skill).unwrap(), expected["skill"]);
        assert_eq!(
            serde_json::to_value(&origins).unwrap(),
            Value::Array(all_origins[..5].to_vec())
        );
        assert_eq!(
            serde_json::to_value(&improver).unwrap(),
            expected["improver"]
        );
        assert_eq!(
            serde_json::to_value(&improver_origins).unwrap(),
            Value::Array(all_origins[5..].to_vec())
        );
        let mut unsigned = bundle.clone();
        unsigned.digest.clear();
        assert_eq!(bundle.digest, fingerprint(&unsigned).unwrap());

        for revoked in [
            ids(&["unrelated-id"]),
            ids(&["parent-fixture", "baseline-fixture"]),
            ids(&["fs_read", "clock"]),
        ] {
            assert_eq!(
                serde_json::to_vec(&fixture.skill(skill_patch, &revoked).unwrap()).unwrap(),
                serde_json::to_vec(&(&skill, &origins)).unwrap()
            );
            assert_eq!(
                serde_json::to_string(
                    &fixture
                        .bundle(skill_patch, improver_patch, &revoked)
                        .unwrap()
                )
                .unwrap(),
                golden
            );
        }
    }
}

#[test]
fn omitted_and_explicit_inherit_keep_identical_complete_outputs() {
    let fixture = Fixture::new();
    let omitted: SkillPatch = serde_json::from_str("{}").unwrap();
    let explicit: SkillPatch = serde_json::from_str(
        r#"{"content":{"op":"inherit"},"applicability":{"op":"inherit"},"counterexample":{"op":"inherit"},"required_capabilities":{"op":"inherit"},"dependencies":{"op":"inherit"}}"#,
    )
    .unwrap();
    let improver = ImproverPatch::default();
    let revoked = BTreeSet::new();

    assert_eq!(
        serde_json::to_vec(&fixture.skill(&omitted, &revoked).unwrap()).unwrap(),
        serde_json::to_vec(&fixture.skill(&explicit, &revoked).unwrap()).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&fixture.bundle(&omitted, &improver, &revoked).unwrap()).unwrap(),
        serde_json::to_vec(&fixture.bundle(&explicit, &improver, &revoked).unwrap()).unwrap()
    );
}

#[test]
fn patch_null_unknown_and_protected_field_contracts_remain_closed() {
    for field in [
        "content",
        "applicability",
        "counterexample",
        "required_capabilities",
        "dependencies",
    ] {
        let mut object = serde_json::Map::new();
        object.insert(field.into(), Value::Null);
        assert!(serde_json::from_value::<SkillPatch>(Value::Object(object)).is_err());
    }
    for input in [
        r#"{"namespace":"other"}"#,
        r#"{"budget":100}"#,
        r#"{"approval":true}"#,
        r#"{"settings":{}}"#,
        r#"{"content":{"op":"unknown"}}"#,
        r#"{"dependencies":{"op":"set","value":null}}"#,
    ] {
        assert!(serde_json::from_str::<SkillPatch>(input).is_err());
    }
    for input in [
        r#"{"instruction":null}"#,
        r#"{"max_candidates":null}"#,
        r#"{"max_rounds":null}"#,
        r#"{"dependencies":{"op":"set","value":[]}}"#,
    ] {
        assert!(serde_json::from_str::<ImproverPatch>(input).is_err());
    }
    for field in ["namespace", "budget", "approval", "improver.instruction"] {
        assert!(matches!(
            assert_candidate_may_write(ArtifactKind::Skill, field),
            Err(Error::Forbidden)
        ));
    }
}
