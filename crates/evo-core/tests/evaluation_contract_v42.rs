//! Constructor inputs and legacy wire evidence; no real experiment is authorized here.
use evo_core::evaluation::{
    ExperimentPlan, OBJECTIVE_V1, PLAN_SCHEMA, SCORE_MICROS_MAX, STATS_BERNSTEIN_V2,
    STATS_HOEFFDING_V1, q_from_micros,
};
use evo_core::holdout::{ANCHOR_COVERAGE_SCHEMA, HOLDOUT_SCHEMA};
use evo_core::sequential::{
    AlphaAllocation, CS_VERSION, EARLY_STOP_VERSION, EarlyStopPlan, FormalClaimKind,
    ResearchFamilyAlphaPlan,
};

// Captured by running the one-argument helper on ba01a2e before changing it.
// The id, freeze timestamp, and candidate are synthetic structural fixtures.
const LEGACY_HELPER_JSON: &str = r#"{"schema_version":"rsia.experiment_plan.v1","id":"ag057-legacy","objective_version":"rsia.attainment_auc.v1","stats_version":"rsia.empirical_bernstein.v2","profile":"quality_gain","conditions":["frozen_a","static_harness_b1","fixed_improver_c"],"first_round":true,"min_effect":0.02,"noninferior_bound":-0.01,"savings_ratio":0.1,"max_cost_ratio":1.1,"max_p95_latency_ratio":1.2,"alpha_total":0.05,"n_planned":60,"query_limit":3,"monetary_budget":{"currency":"USD","pricing_version":"unset","amount":"0"},"stop_on_harm":true,"frozen":false,"frozen_at":null,"candidate_digest":null,"dataset_epoch":"dev_pilot_unfunded"}"#;
const LEGACY_HELPER_DIGEST: &str =
    "4a37cbae65f9d31736d65be34ec29d8eaab33146be4245cb9c87bad300a4768c";
const LEGACY_FROZEN_60_JSON: &str = r#"{"schema_version":"rsia.experiment_plan.v1","id":"ag057-legacy","objective_version":"rsia.attainment_auc.v1","stats_version":"rsia.empirical_bernstein.v2","profile":"quality_gain","conditions":["frozen_a","static_harness_b1","fixed_improver_c"],"first_round":true,"min_effect":0.02,"noninferior_bound":-0.01,"savings_ratio":0.1,"max_cost_ratio":1.1,"max_p95_latency_ratio":1.2,"alpha_total":0.05,"n_planned":60,"query_limit":3,"monetary_budget":{"currency":"USD","pricing_version":"unset","amount":"0"},"stop_on_harm":true,"frozen":true,"frozen_at":1725000000,"candidate_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","dataset_epoch":"dev_pilot_unfunded"}"#;
const LEGACY_FROZEN_60_DIGEST: &str =
    "5fa4423314f01b8c793813dc5cb68f7d378d3cac8f7b4e3fb0fc44fb281ce077";
const LEGACY_FROZEN_137_JSON: &str = r#"{"schema_version":"rsia.experiment_plan.v1","id":"ag057-legacy","objective_version":"rsia.attainment_auc.v1","stats_version":"rsia.empirical_bernstein.v2","profile":"quality_gain","conditions":["frozen_a","static_harness_b1","fixed_improver_c"],"first_round":true,"min_effect":0.02,"noninferior_bound":-0.01,"savings_ratio":0.1,"max_cost_ratio":1.1,"max_p95_latency_ratio":1.2,"alpha_total":0.05,"n_planned":137,"query_limit":3,"monetary_budget":{"currency":"USD","pricing_version":"unset","amount":"0"},"stop_on_harm":true,"frozen":true,"frozen_at":1725000000,"candidate_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","dataset_epoch":"dev_pilot_unfunded"}"#;
const LEGACY_FROZEN_137_DIGEST: &str =
    "772af950b1431ab4ae840cffe75909c06a64cc04900a4e4f08caa8e5b6a9a869";

const DOC: &str = include_str!("../../../docs/evaluation.md");

fn doc_value(field: &str) -> &str {
    let values: Vec<_> = DOC
        .lines()
        .filter_map(|line| {
            let mut cells = line.split('|');
            cells.next()?;
            let name = cells.next()?.trim();
            let value = cells.next()?.trim().trim_matches('`');
            (name == field).then_some(value)
        })
        .collect();
    assert_eq!(
        values.len(),
        1,
        "missing or ambiguous documentation row: {field}"
    );
    values[0]
}

fn doc_number(field: &str) -> f64 {
    doc_value(field).parse().unwrap()
}

fn doc_micros(field: &str) -> u32 {
    let value = doc_number(field) * f64::from(SCORE_MICROS_MAX);
    assert_eq!(
        value.fract(),
        0.0,
        "threshold must be representable in score micros"
    );
    value as u32
}

#[test]
fn explicit_sample_size_is_preserved_at_nondefault_and_supported_bounds() {
    for n in [2, 137, 100_000] {
        let plan = ExperimentPlan::first_low_risk("explicit-n-fixture", n).unwrap();
        assert_eq!(plan.n_planned, n);
        plan.validate().unwrap();
    }
}

#[test]
fn unsupported_sample_size_is_rejected_without_clamping_or_fallback() {
    for n in [0, 1, 100_001] {
        let error = ExperimentPlan::first_low_risk("invalid-n-fixture", n).unwrap_err();
        assert!(
            matches!(error, evo_core::Error::Invalid(message) if message == "experiment plan bounds rejected")
        );
    }
}

#[test]
fn explicit_sixty_matches_the_old_helper_bytes_and_fingerprint() {
    let plan = ExperimentPlan::first_low_risk("ag057-legacy", 60).unwrap();
    assert_eq!(serde_json::to_string(&plan).unwrap(), LEGACY_HELPER_JSON);
    assert_eq!(plan.digest().unwrap(), LEGACY_HELPER_DIGEST);
    assert!(!plan.frozen);
    assert!(plan.monetary_budget.is_zero());
}

#[test]
fn existing_frozen_v1_plans_keep_their_size_bytes_and_fingerprint() {
    for (json, digest, n) in [
        (LEGACY_FROZEN_60_JSON, LEGACY_FROZEN_60_DIGEST, 60),
        (LEGACY_FROZEN_137_JSON, LEGACY_FROZEN_137_DIGEST, 137),
    ] {
        let restored: ExperimentPlan = serde_json::from_str(json).unwrap();
        restored.validate().unwrap();
        assert!(restored.frozen);
        assert_eq!(restored.n_planned, n);
        assert_eq!(restored.frozen_at, Some(1_725_000_000));
        assert_eq!(serde_json::to_string(&restored).unwrap(), json);
        assert_eq!(restored.digest().unwrap(), digest);

        let mut explicit = ExperimentPlan::first_low_risk("ag057-legacy", n).unwrap();
        explicit.freeze(1_725_000_000).unwrap();
        explicit
            .bind_candidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();
        assert_eq!(serde_json::to_string(&explicit).unwrap(), json);
        assert_eq!(explicit.digest().unwrap(), digest);
    }
}

#[test]
fn existing_wire_still_requires_n_planned() {
    for json in [
        LEGACY_HELPER_JSON,
        LEGACY_FROZEN_60_JSON,
        LEGACY_FROZEN_137_JSON,
    ] {
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value.as_object_mut().unwrap().remove("n_planned");
        let error = serde_json::from_value::<ExperimentPlan>(value).unwrap_err();
        assert!(error.to_string().contains("missing field `n_planned`"));
    }
}

#[test]
fn documented_versions_and_helper_starting_values_match_public_contracts() {
    let plan = ExperimentPlan::first_low_risk("documentation-fixture", 137).unwrap();
    for (field, actual) in [
        ("plan schema", PLAN_SCHEMA),
        ("objective_version", OBJECTIVE_V1),
        ("stats_version (new)", STATS_BERNSTEIN_V2),
        ("stats_version (kept)", STATS_HOEFFDING_V1),
        ("early_stop_version", EARLY_STOP_VERSION),
        ("cs_version", CS_VERSION),
    ] {
        assert_eq!(doc_value(field), actual);
    }
    for schema in [ANCHOR_COVERAGE_SCHEMA, HOLDOUT_SCHEMA] {
        assert!(
            DOC.contains(schema),
            "missing existing template schema {schema}"
        );
    }
    for (field, actual) in [
        ("min_effect", plan.min_effect),
        ("noninferior_bound", plan.noninferior_bound),
        ("savings_ratio", plan.savings_ratio),
        ("max_cost_ratio", plan.max_cost_ratio),
        ("max_p95_latency_ratio", plan.max_p95_latency_ratio),
        ("alpha_total", plan.alpha_total),
    ] {
        assert_eq!(doc_number(field), actual);
    }
    assert_eq!(
        doc_value("profile"),
        serde_json::to_value(plan.profile)
            .unwrap()
            .as_str()
            .unwrap()
    );
    assert_eq!(
        doc_value("query_limit").parse::<u32>().unwrap(),
        plan.query_limit
    );
    assert_eq!(doc_value("monetary_budget"), plan.monetary_budget.amount);
    let quality_range = q_from_micros(SCORE_MICROS_MAX).unwrap() - q_from_micros(0).unwrap();
    assert_eq!(doc_number("R"), 2.0 * quality_range);
}

#[test]
fn documented_sequential_values_fit_the_existing_explicit_contract() {
    let mut snapshot = ExperimentPlan::first_low_risk("sequence-documentation-fixture", 2).unwrap();
    snapshot.freeze(0).unwrap();
    let claim_alpha = (snapshot.alpha_total / 2.0).to_string();
    let alpha = ResearchFamilyAlphaPlan::new(
        "documentation-family-fixture",
        snapshot.alpha_total.to_string(),
        vec![
            AlphaAllocation {
                claim_id: "final-fixture".into(),
                attempt_id: "attempt-fixture".into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: claim_alpha.clone(),
            },
            AlphaAllocation {
                claim_id: "stop-fixture".into(),
                attempt_id: "attempt-fixture".into(),
                kind: FormalClaimKind::SequentialReject,
                alpha: claim_alpha.clone(),
            },
        ],
    )
    .unwrap();
    // Thresholds are explicit design choices, not undocumented constructor defaults.
    let early = EarlyStopPlan::new(
        snapshot.digest().unwrap(),
        &alpha,
        "stop-fixture",
        "attempt-fixture",
        snapshot.profile,
        snapshot.n_planned as u32,
        vec!["unit-fixture-0".into(), "unit-fixture-1".into()],
        evo_core::hash(b"assumptions-fixture"),
        evo_core::hash(b"sampling-fixture"),
        evo_core::hash(b"anchor-coverage-fixture"),
        doc_micros("min_gain"),
        doc_micros("max_regression"),
        doc_micros("noninferiority_margin"),
    )
    .unwrap();
    early.validate_against(&alpha).unwrap();
    assert_eq!(early.rho.parse::<f64>().unwrap(), doc_number("rho"));
    assert_eq!(early.alpha_stop_i, claim_alpha);
    assert_eq!(doc_number("min_gain"), snapshot.min_effect);
    assert_eq!(
        doc_number("noninferiority_margin"),
        -snapshot.noninferior_bound
    );
}

#[test]
fn complete_episode_cost_requires_all_observed_dimensions_and_declared_sources() {
    use evo_core::evaluation::CompleteEpisodeCostMeasurements;
    let complete = CompleteEpisodeCostMeasurements {
        episode_id: "episode".into(),
        cpu_nanoseconds: Some(0),
        storage_byte_seconds: Some(0),
        retained_history_bytes: Some(0),
        human_nanoseconds: Some(0),
        gray_release_nanoseconds: Some(0),
        measurement_sources: vec!["actual-meter-record".into()],
    };
    complete.validate_complete().unwrap();
    for dimension in 0..7 {
        let mut absent = complete.clone();
        match dimension {
            0 => absent.cpu_nanoseconds = None,
            1 => absent.storage_byte_seconds = None,
            2 => absent.retained_history_bytes = None,
            3 => absent.human_nanoseconds = None,
            4 => absent.gray_release_nanoseconds = None,
            5 => absent.measurement_sources.clear(),
            _ => absent
                .measurement_sources
                .push("actual-meter-record".into()),
        }
        assert!(absent.validate_complete().is_err(), "dimension {dimension}");
    }
    let mut wire = serde_json::to_value(&complete).unwrap();
    wire["ordinary_p_eligible"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CompleteEpisodeCostMeasurements>(wire).is_err());
}

mod experiment_plan_v2_content {
    use super::*;
    use evo_core::contract::{
        CompilePartsV3, EmptyRootContentV1, FrozenCompilerProfileContentV1, HostCapabilities,
        ImproverPatch, ParentContentV3, PatchOp, Profile, ProfileContentProofV3, ResolvedBundleV3,
        SkillPatch, SkillSnapshot, SystemSnapshot, compile_bundle_v3, replay_resolved_bundle_v3,
    };
    use evo_core::evaluation::{ExperimentPlanV2, PLAN_SCHEMA_V2};
    use evo_core::{Error, Strategy, fingerprint, hash};
    use serde::Serialize;
    use std::collections::BTreeSet;

    // Complete data-only wire shape of the engine candidate, including the
    // full typed bundle and all surrounding fields. Core cannot depend on
    // engine. This fixture grants no registered source/approval/Host authority;
    // the real same-Session ReadCandidate producer test remains an engine duty.
    #[derive(Debug, Clone, Serialize)]
    struct SourceContentFixture {
        kind: String,
        id: String,
        content_digest: String,
    }

    #[derive(Debug, Clone, Serialize)]
    struct CompleteCandidateContentFixture {
        id: String,
        schema_version: String,
        bundle: ResolvedBundleV3,
        bundle_digest: String,
        environment_digest: String,
        profile_id: String,
        parent_digest: String,
        proposer_actor: String,
        sources: Vec<SourceContentFixture>,
        revoke_watermark: u64,
    }

    fn candidate() -> CompleteCandidateContentFixture {
        let snapshot = SystemSnapshot {
            schema_version: "rsia.system_snapshot.v1".into(),
            profile_id: "content-profile".into(),
            host_id: "reference".into(),
            host_version: "0.1.0".into(),
            model_id: "development-model".into(),
            tools: vec![],
            mandatory_context_digest: hash(b"Complete context original for the data fixture."),
        };
        let baseline = FrozenCompilerProfileContentV1 {
            id: "content-profile-registration".into(),
            schema_version: "rsia.frozen_compiler_profile.v1".into(),
            namespace: "plan-content-fixture".into(),
            profile_id: "content-profile".into(),
            version: 1,
            host_registration_id: "content-host-registration".into(),
            provider_registration_id: "content-provider-registration".into(),
            resource_instrumentation_id: None,
            environment_digest: snapshot.digest().unwrap(),
            system_snapshot: snapshot,
            capabilities: HostCapabilities {
                available: ["cap-a".into()].into_iter().collect(),
                granted: ["cap-a".into()].into_iter().collect(),
            },
            baseline_skill: SkillSnapshot {
                content: "Complete baseline content.".into(),
                required_capabilities: vec!["cap-a".into()],
                ..SkillSnapshot::empty()
            },
            baseline_strategy: Strategy::default(),
            dependencies: vec![],
            purpose: evo_core::evidence::Purpose::Development,
            application_order: vec!["mandatory_context".into(), "skill".into(), "task".into()],
            authorized_by: "content-operator".into(),
        };
        let root = EmptyRootContentV1 {
            id: "content-empty-root".into(),
            schema_version: "rsia.empty_root_authorization.v1".into(),
            namespace: baseline.namespace.clone(),
            profile_registration_id: baseline.id.clone(),
            profile_digest: fingerprint(&baseline).unwrap(),
            host_registration_id: baseline.host_registration_id.clone(),
            skill: SkillSnapshot::empty(),
            strategy: Strategy {
                schema_version: "evo.strategy.v1".into(),
                instruction:
                    "Preserve the unchanged host baseline; no evolution instruction is applied."
                        .into(),
                max_candidates: 1,
                max_rounds: 1,
                require_counterexample: true,
            },
            authorized_by: "content-operator".into(),
        };
        let environment_digest = baseline.environment_digest.clone();
        let proof = ProfileContentProofV3::from_content(
            Profile {
                id: baseline.profile_id.clone(),
                evolution_enabled: true,
                parent_digest: fingerprint(&root).unwrap(),
                baseline_digest: fingerprint(&baseline).unwrap(),
            },
            ParentContentV3::EmptyRoot(root),
            baseline,
        )
        .unwrap();
        let skill_patch = SkillPatch {
            content: PatchOp::Set {
                value: "完整候选原像；保留UTF8与实际typed字段顺序。".into(),
            },
            required_capabilities: PatchOp::ResetToBaseline,
            ..SkillPatch::default()
        };
        let bundle = compile_bundle_v3(CompilePartsV3 {
            content: &proof,
            skill_patch: &skill_patch,
            improver_patch: &ImproverPatch::default(),
            revoked: &BTreeSet::new(),
        })
        .unwrap();
        replay_resolved_bundle_v3(&bundle, &proof, &BTreeSet::new()).unwrap();
        CompleteCandidateContentFixture {
            id: "candidate-content-fixture".into(),
            schema_version: "rsia.release_candidate.v2".into(),
            bundle_digest: bundle.digest.clone(),
            environment_digest,
            profile_id: bundle.profile_id.clone(),
            parent_digest: bundle.parent_digest.clone(),
            bundle,
            proposer_actor: "proposer-a".into(),
            sources: vec![],
            revoke_watermark: 0,
        }
    }

    fn frozen_configuration() -> ExperimentPlan {
        let mut configuration = ExperimentPlan::first_low_risk("v2-content-plan", 137).unwrap();
        configuration.monetary_budget.pricing_version = "content-pricing".into();
        configuration.freeze(1_725_000_000).unwrap();
        configuration
    }

    fn conflict<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
        assert!(
            matches!(result, Err(Error::Conflict(ref message)) if message == expected),
            "expected Conflict({expected:?}), got {result:?}"
        );
    }

    #[test]
    fn v2_keeps_complete_frozen_configuration_and_exact_typed_record_bytes() {
        let configuration = frozen_configuration();
        let original_configuration = serde_json::to_vec(&configuration).unwrap();
        let record = candidate();
        let typed_raw = serde_json::to_vec(&record).unwrap();
        let plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record).unwrap();
        plan.verify_candidate_content(&record.id, &record).unwrap();
        assert_eq!(plan.schema_version, PLAN_SCHEMA_V2);
        assert_eq!(PLAN_SCHEMA, "rsia.experiment_plan.v1");
        assert_eq!(plan.candidate_locator, record.id);
        assert_eq!(plan.candidate_record_utf8.as_bytes(), typed_raw);
        assert!(plan.candidate_record_utf8.contains("完整候选原像"));
        assert_eq!(plan.candidate_record_digest, fingerprint(&record).unwrap());
        assert_eq!(plan.candidate_record_digest, hash(&typed_raw));
        assert_eq!(
            serde_json::to_vec(&plan.frozen_configuration).unwrap(),
            original_configuration
        );
        assert!(plan.frozen_configuration.candidate_digest.is_none());
        assert_ne!(plan.id, configuration.id);
        assert_ne!(
            plan.digest_against(&record.id, &record).unwrap(),
            configuration.digest().unwrap()
        );
        assert_eq!(
            plan.digest_against(&record.id, &record).unwrap(),
            fingerprint(&plan).unwrap()
        );
        let restored: ExperimentPlanV2 =
            serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        restored
            .verify_candidate_content(&record.id, &record)
            .unwrap();
        assert!(
            serde_json::from_slice::<ExperimentPlan>(&serde_json::to_vec(&plan).unwrap()).is_err()
        );
        assert!(serde_json::from_slice::<ExperimentPlanV2>(&original_configuration).is_err());
        assert_eq!(
            serde_json::to_vec(&configuration).unwrap(),
            original_configuration
        );
    }

    #[test]
    fn v2_same_locator_and_bundle_cannot_hide_another_legal_record_field() {
        let configuration = frozen_configuration();
        let record = candidate();
        let plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record).unwrap();
        plan.verify_candidate_content(&record.id, &record).unwrap();
        let original = serde_json::to_vec(&plan).unwrap();
        let mut changed = record.clone();
        changed.proposer_actor = "proposer-b".into();
        evo_core::identifier(&changed.proposer_actor).unwrap();
        assert_eq!(changed.id, record.id);
        assert_eq!(
            fingerprint(&changed.bundle).unwrap(),
            fingerprint(&record.bundle).unwrap()
        );
        let changed_plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &changed.id, &changed).unwrap();
        changed_plan
            .verify_candidate_content(&changed.id, &changed)
            .unwrap();
        assert_ne!(
            plan.candidate_record_digest,
            changed_plan.candidate_record_digest
        );
        assert_ne!(plan.id, changed_plan.id);
        conflict(
            plan.verify_candidate_content(&changed.id, &changed),
            "experiment plan candidate content mismatch",
        );
        assert_eq!(serde_json::to_vec(&plan).unwrap(), original);
    }

    #[test]
    fn v2_another_valid_digest_and_locator_only_input_do_not_prove_the_original() {
        let configuration = frozen_configuration();
        let record = candidate();
        let plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record).unwrap();
        plan.verify_candidate_content(&record.id, &record).unwrap();
        let original = serde_json::to_vec(&plan).unwrap();
        let mut changed = record.clone();
        changed.revoke_watermark = 1;
        let changed_plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &changed.id, &changed).unwrap();
        changed_plan
            .verify_candidate_content(&changed.id, &changed)
            .unwrap();
        let mut wrong_digest = plan.clone();
        wrong_digest.candidate_record_digest = changed_plan.candidate_record_digest;
        assert_eq!(wrong_digest.candidate_record_digest.len(), 64);
        assert!(
            wrong_digest
                .candidate_record_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        conflict(
            wrong_digest.verify_candidate_content(&record.id, &record),
            "experiment plan stored candidate digest mismatch",
        );
        assert!(matches!(
            plan.verify_candidate_content(&record.id, &record.id),
            Err(Error::Invalid(message)) if message == "candidate record must serialize as a JSON object"
        ));
        conflict(
            plan.verify_candidate_content("another-locator", &record),
            "experiment plan candidate locator mismatch",
        );
        assert_eq!(serde_json::to_vec(&plan).unwrap(), original);
    }

    #[test]
    fn v2_uses_typed_field_order_and_raw_utf8_instead_of_value_canonicalization() {
        let configuration = frozen_configuration();
        let record = candidate();
        let plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record).unwrap();
        plan.verify_candidate_content(&record.id, &record).unwrap();
        let reordered = serde_json::to_value(&record).unwrap();
        assert_ne!(
            serde_json::to_vec(&record).unwrap(),
            serde_json::to_vec(&reordered).unwrap()
        );
        conflict(
            plan.verify_candidate_content(&record.id, &reordered),
            "experiment plan candidate content mismatch",
        );
        let mut altered_utf8 = plan.clone();
        altered_utf8.candidate_record_utf8 = altered_utf8
            .candidate_record_utf8
            .replace("完整候选原像", "另一候选原像");
        conflict(
            altered_utf8.verify_candidate_content(&record.id, &record),
            "experiment plan stored candidate digest mismatch",
        );
    }

    #[test]
    fn v2_does_not_upgrade_bound_legacy_or_unfrozen_configuration() {
        let configuration = frozen_configuration();
        let record = candidate();
        ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record)
            .unwrap()
            .verify_candidate_content(&record.id, &record)
            .unwrap();
        let mut bound = configuration.clone();
        bound.bind_candidate(&record.id).unwrap();
        let original_bound = serde_json::to_vec(&bound).unwrap();
        conflict(
            ExperimentPlanV2::from_typed_candidate(&bound, &record.id, &record),
            "v2 requires a frozen unbound configuration",
        );
        assert_eq!(serde_json::to_vec(&bound).unwrap(), original_bound);
        let unfrozen = ExperimentPlan::first_low_risk("unfrozen-config", 137).unwrap();
        let original_unfrozen = serde_json::to_vec(&unfrozen).unwrap();
        conflict(
            ExperimentPlanV2::from_typed_candidate(&unfrozen, &record.id, &record),
            "v2 requires a frozen unbound configuration",
        );
        assert_eq!(serde_json::to_vec(&unfrozen).unwrap(), original_unfrozen);
    }

    #[test]
    fn v2_decoded_schema_identity_and_frozen_configuration_require_revalidation() {
        let configuration = frozen_configuration();
        let record = candidate();
        let plan =
            ExperimentPlanV2::from_typed_candidate(&configuration, &record.id, &record).unwrap();
        plan.verify_candidate_content(&record.id, &record).unwrap();
        let mut unknown = plan.clone();
        unknown.schema_version = "rsia.experiment_plan.v99".into();
        assert!(matches!(
            unknown.verify_candidate_content(&record.id, &record),
            Err(Error::Invalid(message)) if message == "unsupported experiment plan v2 schema"
        ));
        let mut other_configuration = plan.clone();
        other_configuration.frozen_configuration.n_planned = 138;
        other_configuration.frozen_configuration.validate().unwrap();
        conflict(
            other_configuration.verify_candidate_content(&record.id, &record),
            "experiment plan v2 identity mismatch",
        );
        let mut rebound = plan.clone();
        rebound
            .frozen_configuration
            .bind_candidate(&record.id)
            .unwrap();
        conflict(
            rebound.verify_candidate_content(&record.id, &record),
            "v2 requires a frozen unbound configuration",
        );
    }
}

mod bound_pair_v2_data {
    use evo_core::evaluation::{DataUse, ExperimentPlan};
    use evo_core::{BoundPairV2, Error, Pair, fingerprint, hash};
    fn pair() -> BoundPairV2 {
        BoundPairV2::from_development_material(
            "task",
            "family",
            "实际材料\r\né".into(),
            (100_000, 900_000),
            (100, 80),
            "USD".into(),
            ("price-a".into(), "price-b".into()),
        )
        .unwrap()
    }
    #[test]
    fn old_pair_and_plan_keep_names_float_wire_and_v1_constants() {
        const WIRE: &str = r#"{"task_hash":"readable-task","baseline":0.1,"candidate":0.9,"baseline_units":100,"candidate_units":80,"critical":false}"#;
        let old: Pair = serde_json::from_str(WIRE).unwrap();
        assert_eq!(serde_json::to_string(&old).unwrap(), WIRE);
        let mut plan: ExperimentPlan = serde_json::from_str(super::LEGACY_HELPER_JSON).unwrap();
        assert_eq!(plan.digest().unwrap(), super::LEGACY_HELPER_DIGEST);
        plan.freeze(1_725_000_000).unwrap();
        plan.bind_candidate("readable-candidate").unwrap();
        plan.validate().unwrap();
        assert_eq!(plan.candidate_digest.as_deref(), Some("readable-candidate"));
        assert_eq!(super::PLAN_SCHEMA, "rsia.experiment_plan.v1");
    }
    #[test]
    fn actual_bound_pair_material_is_required_and_failed_checks_preserve_state() {
        let pair = pair();
        pair.validate_against_raw(pair.task_material_utf8.as_bytes())
            .unwrap();
        let before = serde_json::to_vec(&pair).unwrap();
        for raw in ["另一材料\r\né", "实际材料\né", "实际材料\r\ne\u{301}"] {
            assert!(matches!(
                pair.validate_against_raw(raw.as_bytes()),
                Err(Error::Conflict(_))
            ));
        }
        let mut other = pair.clone();
        other.task_hash = hash(b"another actual material");
        assert!(matches!(
            other.validate_against_raw(pair.task_material_utf8.as_bytes()),
            Err(Error::Conflict(_))
        ));
        assert_eq!(serde_json::to_vec(&pair).unwrap(), before);
        assert_eq!(pair.task.raw_hash, hash(pair.task_material_utf8.as_bytes()));
    }
    #[test]
    fn bound_pair_scores_accept_integer_endpoints_and_refuse_overflow() {
        for scores in [(0, 1_000_000), (1_000_000, 0)] {
            let mut pair = pair();
            pair.baseline_score_micros = scores.0;
            pair.candidate_score_micros = scores.1;
            pair.validate_against_raw(pair.task_material_utf8.as_bytes())
                .unwrap();
        }
        for baseline in [false, true] {
            let mut pair = pair();
            if baseline {
                pair.baseline_score_micros = 1_000_001;
            } else {
                pair.candidate_score_micros = u32::MAX;
            }
            assert!(
                matches!(pair.validate_against_raw(pair.task_material_utf8.as_bytes()),
                Err(Error::Invalid(message)) if message == "score_micros out of range")
            );
        }
    }
    #[test]
    fn bound_pair_domain_schema_data_use_and_cost_basis_are_fixed() {
        for fault in ["domain", "schema", "data_use", "cost_basis"] {
            let mut changed = pair();
            let original = fingerprint(&changed).unwrap();
            match fault {
                "domain" => changed.domain = "other.domain".into(),
                "schema" => changed.schema_version = "rsia.bound_pair.v1".into(),
                "data_use" => changed.task.data_use = DataUse::AcceptanceEpoch,
                "cost_basis" => changed.cost_basis = "estimated".into(),
                _ => unreachable!(),
            }
            assert_ne!(fingerprint(&changed).unwrap(), original);
            assert!(matches!(
                changed.validate_against_raw(changed.task_material_utf8.as_bytes()),
                Err(Error::Conflict(_))
            ));
        }
    }
    #[test]
    fn bound_pair_projection_is_arithmetic_and_zero_baseline_does_not_mint_savings() {
        let mut pair = pair();
        pair.baseline_metered_micros = 0;
        pair.candidate_metered_micros = 7;
        let projected = pair
            .to_legacy_pair(pair.task_material_utf8.as_bytes())
            .unwrap();
        assert_eq!(projected.baseline, 0.1);
        assert_eq!(projected.candidate, 0.9);
        assert_eq!(projected.baseline_units, 0);
        assert_eq!(projected.candidate_units, 7);
        assert_eq!(projected.task_hash, pair.task.raw_hash);
        assert!(!projected.critical);
        let policy = evo_core::AcceptancePolicy {
            min_tasks: 5,
            ..evo_core::AcceptancePolicy::default()
        };
        let decision = evo_core::evaluate(&[projected], &policy).unwrap();
        assert!(!decision.passed);
        assert!(
            decision
                .reasons
                .contains(&"insufficient_independent_tasks".to_string())
        );
        assert!(decision.reasons.contains(&"cost_regression".to_string()));
    }
}
