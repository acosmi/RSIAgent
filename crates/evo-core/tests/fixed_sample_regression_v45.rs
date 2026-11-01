//! Pure configuration/content and numerical contracts; no source or execution authority.
use evo_core::evaluation::{
    BernsteinReport, ClusterObservation, ExperimentPlan, ExperimentPlanV2, ExperimentPlanV3,
    FixedRegressionThreshold, FrozenFixedSampleConfiguration, ProfileKind, STATS_BERNSTEIN_V2,
    Verdict, decide_fixed_sample_v45, empirical_bernstein,
};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Error, fingerprint, hash};
use serde::Serialize;

#[derive(Serialize)]
struct Candidate<'a> {
    id: &'a str,
    body: &'a str,
}
fn configuration(profile: ProfileKind, m: f64) -> FrozenFixedSampleConfiguration {
    let mut plan = ExperimentPlan::first_low_risk("fixed-regression", 64).unwrap();
    plan.profile = profile;
    plan.noninferior_bound = -0.875;
    plan.freeze(1).unwrap();
    FrozenFixedSampleConfiguration::new(plan, FixedRegressionThreshold::new(m).unwrap()).unwrap()
}
fn plan(profile: ProfileKind, m: f64) -> ExperimentPlanV3 {
    ExperimentPlanV3::from_typed_candidate(
        &configuration(profile, m),
        "candidate",
        &Candidate {
            id: "candidate",
            body: "complete α original",
        },
    )
    .unwrap()
}
fn report(mean: f64, radius: f64) -> BernsteinReport {
    BernsteinReport {
        stats_version: STATS_BERNSTEIN_V2.into(),
        verdict: Verdict::Inconclusive,
        n: 64,
        mean,
        variance: 0.0,
        radius,
        lcb: (mean - radius).max(-1.0),
        alpha_i: 0.05,
        reasons: vec![],
    }
}
fn early(configuration: &FrozenFixedSampleConfiguration, micros: u32) -> EarlyStopPlan {
    let alpha = ResearchFamilyAlphaPlan::new(
        "family",
        "0.05",
        vec![AlphaAllocation {
            claim_id: "stop".into(),
            attempt_id: "attempt".into(),
            kind: FormalClaimKind::SequentialReject,
            alpha: "0.01".into(),
        }],
    )
    .unwrap();
    EarlyStopPlan::new(
        configuration.plan.digest().unwrap(),
        &alpha,
        "stop",
        "attempt",
        configuration.plan.profile,
        64,
        (0..64).map(|i| format!("c{i}")).collect(),
        hash(b"assumptions"),
        hash(b"sampling"),
        hash(b"anchors"),
        20_000,
        micros,
        875_000,
    )
    .unwrap()
}
#[test]
fn frozen_regression_threshold_is_explicit_and_positive() {
    for m in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            FixedRegressionThreshold::new(m),
            Err(Error::Invalid(_))
        ));
    }
    for json in ["0", "-0.0", "-1", "null", "\"0.125\"", "1e400"] {
        assert!(
            serde_json::from_str::<FixedRegressionThreshold>(json).is_err(),
            "{json}"
        );
    }
    for m in [f64::from_bits(1), f64::MIN_POSITIVE, 0.125, 2.0, f64::MAX] {
        let value = FixedRegressionThreshold::new(m).unwrap();
        assert_eq!(value.value().to_bits(), m.to_bits());
        assert_eq!(
            serde_json::from_str::<FixedRegressionThreshold>(
                &serde_json::to_string(&value).unwrap()
            )
            .unwrap(),
            value
        );
    }
    let mut missing = serde_json::to_value(configuration(ProfileKind::QualityGain, 0.125)).unwrap();
    missing.as_object_mut().unwrap().remove("max_regression");
    assert!(serde_json::from_value::<FrozenFixedSampleConfiguration>(missing).is_err());
}
#[test]
fn threshold_is_frozen_before_candidate_binding() {
    let candidate = Candidate {
        id: "candidate",
        body: "complete α original",
    };
    let config = configuration(ProfileKind::QualityGain, 0.125);
    let original = serde_json::to_vec(&config).unwrap();
    let current = ExperimentPlanV3::from_typed_candidate(&config, "candidate", &candidate).unwrap();
    assert_eq!(serde_json::to_vec(&config).unwrap(), original);
    assert!(config.plan.candidate_digest.is_none());
    current
        .verify_candidate_content("candidate", &candidate)
        .unwrap();
    let mut bound = config.clone();
    bound.plan.bind_candidate("candidate").unwrap();
    assert!(matches!(
        ExperimentPlanV3::from_typed_candidate(&bound, "candidate", &candidate),
        Err(Error::Conflict(_))
    ));
    let mut unfrozen = config.clone();
    unfrozen.plan.frozen = false;
    unfrozen.plan.frozen_at = None;
    assert!(ExperimentPlanV3::from_typed_candidate(&unfrozen, "candidate", &candidate).is_err());
    let mut tampered = current.clone();
    tampered.frozen_configuration.max_regression = FixedRegressionThreshold::new(0.25).unwrap();
    assert!(
        matches!(tampered.verify_candidate_content("candidate",&candidate),Err(Error::Conflict(message)) if message=="experiment plan v3 identity mismatch")
    );
}
#[test]
fn threshold_changes_current_identity_only() {
    let candidate = Candidate {
        id: "candidate",
        body: "complete α original",
    };
    let a = configuration(ProfileKind::QualityGain, 0.125);
    let b = configuration(ProfileKind::QualityGain, 0.25);
    let legacy_a =
        ExperimentPlanV2::from_typed_candidate(&a.plan, "candidate", &candidate).unwrap();
    let legacy_b =
        ExperimentPlanV2::from_typed_candidate(&b.plan, "candidate", &candidate).unwrap();
    assert_eq!(
        fingerprint(&legacy_a).unwrap(),
        fingerprint(&legacy_b).unwrap()
    );
    assert_eq!(a.plan.digest().unwrap(), b.plan.digest().unwrap());
    assert_ne!(a.digest().unwrap(), b.digest().unwrap());
    let ca = ExperimentPlanV3::from_typed_candidate(&a, "candidate", &candidate).unwrap();
    let cb = ExperimentPlanV3::from_typed_candidate(&b, "candidate", &candidate).unwrap();
    assert_ne!(ca.id, cb.id);
    assert_ne!(ca.id, legacy_a.id);
    assert!(ca.id.starts_with("experiment-plan-v3-"));
    assert_eq!(
        ca.candidate_record_utf8,
        serde_json::to_string(&candidate).unwrap()
    );
    assert_eq!(
        ca.candidate_record_digest,
        hash(ca.candidate_record_utf8.as_bytes())
    );
    assert!(
        serde_json::to_string(&ca)
            .unwrap()
            .starts_with("{\"schema_version\":\"rsia.experiment_plan.v3\",\"id\":")
    );
}
#[test]
fn regression_boundary_is_strict_in_both_profiles() {
    for profile in [ProfileKind::QualityGain, ProfileKind::NoninferiorSavings] {
        let plan = plan(profile, 0.125);
        for (mean, radius, regressed) in [
            (-0.5, 0.125, true),
            (-0.25, 0.125, false),
            (-0.1875, 0.125, false),
            (-0.125, 0.25, false),
        ] {
            let result =
                decide_fixed_sample_v45(&plan, report(mean, radius), 0.8, 1.0, true).unwrap();
            assert_eq!(
                result.verdict == Verdict::Regressed,
                regressed,
                "{profile:?} {mean} {radius}"
            );
            assert_eq!(
                result
                    .reasons
                    .iter()
                    .any(|r| r == "fixed_sample_regression_ucb_below_negative_max_regression"),
                regressed
            );
            assert!(!result.reasons.iter().any(|r| r == "negative_lcb"));
        }
    }
}
#[test]
fn negative_lcb_does_not_prove_regression() {
    for profile in [ProfileKind::QualityGain, ProfileKind::NoninferiorSavings] {
        let result =
            decide_fixed_sample_v45(&plan(profile, 0.125), report(-0.05, 0.45), 1.0, 1.0, true)
                .unwrap();
        assert_eq!(result.verdict, Verdict::Inconclusive);
        assert!(
            !result
                .reasons
                .iter()
                .any(|r| r.contains("regression") || r == "negative_lcb")
        );
    }
}
#[test]
fn raw_radius_and_fixed_alpha_are_used() {
    for profile in [ProfileKind::QualityGain, ProfileKind::NoninferiorSavings] {
        let raw = report(-0.95, 1.1);
        assert_eq!(raw.lcb, -1.0);
        assert!(raw.mean - raw.lcb < raw.radius);
        let result =
            decide_fixed_sample_v45(&plan(profile, 0.125), raw.clone(), 1.0, 1.0, true).unwrap();
        assert_ne!(result.verdict, Verdict::Regressed);
        assert_eq!(result.radius.to_bits(), raw.radius.to_bits());
        assert_eq!(result.alpha_i, 0.05);
        let rows: Vec<_> = (0..64)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: -0.5,
                weight: 1.0,
            })
            .collect();
        let fixed = empirical_bernstein(&rows, 0.05).unwrap();
        let stop = empirical_bernstein(&rows, 0.01).unwrap();
        assert_ne!(fixed.radius.to_bits(), stop.radius.to_bits());
        let result =
            decide_fixed_sample_v45(&plan(profile, 0.125), fixed.clone(), 0.8, 1.0, true).unwrap();
        assert_eq!(result.verdict, Verdict::Regressed);
        assert_eq!(result.alpha_i.to_bits(), fixed.alpha_i.to_bits());
    }
}
#[test]
fn disabled_early_stop_still_requires_threshold() {
    let config = configuration(ProfileKind::QualityGain, 0.125);
    config.validate_early_stop(None).unwrap();
    let wire = serde_json::to_string(&config).unwrap();
    assert!(wire.ends_with(",\"max_regression\":0.125}"));
    let mut missing = serde_json::to_value(&config).unwrap();
    missing.as_object_mut().unwrap().remove("max_regression");
    assert!(serde_json::from_value::<FrozenFixedSampleConfiguration>(missing).is_err());
}
#[test]
fn enabled_early_stop_requires_identical_threshold() {
    for m in [0.125, 0.01, 0.000001] {
        let config = configuration(ProfileKind::QualityGain, m);
        let micros = if m == 0.125 {
            125_000
        } else if m == 0.01 {
            10_000
        } else {
            1
        };
        let early = early(&config, micros);
        config.validate_early_stop(Some(&early)).unwrap();
        let mut other = config.clone();
        other.max_regression =
            FixedRegressionThreshold::new(f64::from(micros + 1) / 1_000_000.0).unwrap();
        assert!(
            matches!(other.validate_early_stop(Some(&early)),Err(Error::Invalid(message)) if message=="fixed and sequential max_regression differ")
        );
        let mut adjacent = config.clone();
        adjacent.max_regression =
            FixedRegressionThreshold::new(f64::from_bits(m.to_bits() + 1)).unwrap();
        assert!(adjacent.validate_early_stop(Some(&early)).is_err());
    }
}
