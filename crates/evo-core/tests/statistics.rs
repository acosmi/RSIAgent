//! Golden and partition tests for E01 (V010–V013).
use evo_core::evaluation::{
    ClusterObservation, ExperimentPlan, ProfileKind, Verdict, decide, empirical_bernstein,
    q_from_micros,
};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/statistics/golden")
        .join(name)
}

#[derive(Deserialize)]
struct Case {
    n: usize,
    d: f64,
    alpha_i: f64,
    #[serde(default)]
    expect_verdict: Option<String>,
    #[serde(default)]
    expect_verdict_not: Option<String>,
}

fn rows(n: usize, d: f64) -> Vec<ClusterObservation> {
    (0..n)
        .map(|i| ClusterObservation {
            cluster_id: format!("c{i}"),
            d,
            weight: 1.0,
        })
        .collect()
}

fn load(name: &str) -> Case {
    serde_json::from_str(&fs::read_to_string(golden(name)).unwrap()).unwrap()
}

#[test]
fn golden_zero_effect_is_not_improved() {
    let case = load("zero_effect.json");
    let mut plan = ExperimentPlan::first_low_risk("g1").unwrap();
    plan.freeze(1).unwrap();
    plan.bind_candidate("cand").unwrap();
    let report = empirical_bernstein(&rows(case.n, case.d), case.alpha_i).unwrap();
    let decided = decide(&plan, report, 1.0, 1.0, true).unwrap();
    assert_ne!(
        format!("{:?}", decided.verdict).to_lowercase(),
        case.expect_verdict_not.unwrap()
    );
    assert_ne!(decided.verdict, Verdict::Improved);
}

#[test]
fn golden_n_lt_2_invalid() {
    let case = load("n_lt_2.json");
    let report = empirical_bernstein(&rows(case.n, case.d), case.alpha_i).unwrap();
    assert_eq!(report.verdict, Verdict::Invalid);
    assert_eq!(case.expect_verdict.as_deref(), Some("invalid"));
}

#[test]
fn v1_evaluate_still_rejects_unchanged_pairs() {
    use evo_core::{AcceptancePolicy, Pair, evaluate};
    let pairs: Vec<Pair> = (0..60)
        .map(|i| Pair {
            task_hash: format!("t{i}"),
            baseline: 0.5,
            candidate: 0.5,
            baseline_units: 10,
            candidate_units: 10,
            critical: false,
        })
        .collect();
    assert!(
        !evaluate(&pairs, &AcceptancePolicy::default())
            .unwrap()
            .passed
    );
}

#[test]
fn boundary_scores_map_from_micros() {
    assert_eq!(q_from_micros(0).unwrap(), 0.0);
    assert_eq!(q_from_micros(1_000_000).unwrap(), 1.0);
}

#[test]
fn noninferior_profile_needs_savings() {
    let mut plan = ExperimentPlan::first_low_risk("g2").unwrap();
    plan.profile = ProfileKind::NoninferiorSavings;
    plan.freeze(1).unwrap();
    plan.bind_candidate("cand").unwrap();
    let report = empirical_bernstein(&rows(60, 0.0), 0.05).unwrap();
    let decided = decide(&plan, report, 1.0, 1.0, true).unwrap();
    assert_ne!(decided.verdict, Verdict::Noninferior);
}
