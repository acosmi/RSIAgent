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
