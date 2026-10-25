use evo_core::Error;
use evo_core::evaluation::{
    BudgetAuthorizationModel, ClusterObservation, CostPlanningInput, ExperimentPlan,
    FeasibilityReason, FormalStatisticalUnit, MoneyPlanningInput, PlanFeasibilityAssessment,
    PlanFeasibilityDisposition, PlanFeasibilityInput, PlanningPremise, PlanningSource, ProfileKind,
    ResourcePlanningInput, SequentialFeasibilityProjection, SequentialPlanning, UnitPlanningInput,
    Verdict, assess_plan_feasibility_v1, decide, empirical_bernstein, estimate_n,
};
use evo_core::sequential::{
    AlphaAllocation, CompletePairedUnit, EarlyStopDecision, EarlyStopPlan, FormalClaimKind,
    ResearchFamilyAlphaPlan, SequentialRejectOnly,
};

use PlanningPremise::{DeclaredApplicable, Unavailable as MissingPremise};
use PlanningSource::{Declared, Unavailable};

#[derive(Clone)]
struct Fixture {
    snapshot: ExperimentPlan,
    alpha: ResearchFamilyAlphaPlan,
    total: u64,
}

impl Fixture {
    fn fixed(n: usize) -> Self {
        let mut snapshot = ExperimentPlan::first_low_risk("planning_fixture", n).unwrap();
        snapshot.monetary_budget.pricing_version = "fixture-price".into();
        snapshot.monetary_budget.amount = "3".into();
        Self {
            snapshot,
            alpha: ResearchFamilyAlphaPlan::new(
                "family",
                "0.05",
                vec![AlphaAllocation {
                    claim_id: "gain".into(),
                    attempt_id: "attempt".into(),
                    kind: FormalClaimKind::FixedSampleGain,
                    alpha: "0.05".into(),
                }],
            )
            .unwrap(),
            total: 3_000_000,
        }
    }

    fn with_sequence(n: usize) -> Self {
        let mut fixture = Self::fixed(n);
        fixture.alpha = ResearchFamilyAlphaPlan::new(
            "family",
            "0.05",
            vec![
                AlphaAllocation {
                    claim_id: "gain".into(),
                    attempt_id: "attempt".into(),
                    kind: FormalClaimKind::FixedSampleGain,
                    alpha: "0.04".into(),
                },
                AlphaAllocation {
                    claim_id: "stop".into(),
                    attempt_id: "attempt".into(),
                    kind: FormalClaimKind::SequentialReject,
                    alpha: "0.01".into(),
                },
            ],
        )
        .unwrap();
        fixture
    }

    fn input(&self) -> PlanFeasibilityInput<'_> {
        PlanFeasibilityInput {
            snapshot: &self.snapshot,
            effect_hypothesis: 0.10,
            effect_source: DeclaredApplicable,
            variance_upper: Declared(0.0),
            max_n: 100_000,
            alpha_plan: &self.alpha,
            fixed_claim_id: "gain",
            attempt_id: "attempt",
            units: UnitPlanningInput {
                kind: FormalStatisticalUnit::IndependentTaskCluster,
                available_units: Declared(self.snapshot.n_planned as u32),
                independence: DeclaredApplicable,
                common_mean: DeclaredApplicable,
            },
            money: MoneyPlanningInput {
                currency: Declared("USD"),
                pricing_version: Declared("fixture-price"),
                per_call_cap_micros: Declared(1000),
                experiment_total_micros: Declared(self.total),
                payment_subject: Declared("payer"),
                authorization: BudgetAuthorizationModel::DeclaredLimitMicros(self.total),
            },
            costs: CostPlanningInput {
                fixed_episode_upper_micros: Declared(1000),
                paired_unit_upper_micros: Declared(2000),
                largest_call_upper_micros: Declared(500),
            },
            resources: ResourcePlanningInput {
                expected_cost_ratio: Declared(1.0),
                expected_p95_ratio: Declared(1.0),
                baseline_cost_positive: DeclaredApplicable,
                bounded_reconcilable_cost: DeclaredApplicable,
                anchors_safety: DeclaredApplicable,
            },
            sequential: SequentialPlanning::Disabled,
        }
    }

    fn early(&self) -> EarlyStopPlan {
        EarlyStopPlan::new(
            self.snapshot.digest().unwrap(),
            &self.alpha,
            "stop",
            "attempt",
            self.snapshot.profile,
            self.snapshot.n_planned as u32,
            (0..self.snapshot.n_planned)
                .map(|i| format!("c{i}"))
                .collect(),
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            20_000,
            10_000,
            10_000,
        )
        .unwrap()
    }
}

fn pairs(n: usize, baseline: u32, candidate: u32) -> Vec<CompletePairedUnit> {
    (0..n)
        .map(|i| CompletePairedUnit {
            cluster_id: format!("c{i}"),
            baseline_score_micros: baseline,
            candidate_score_micros: candidate,
        })
        .collect()
}

fn descriptive(assessment: &PlanFeasibilityAssessment, reason: FeasibilityReason) {
    assert_eq!(
        assessment.disposition,
        PlanFeasibilityDisposition::DescriptiveOnly
    );
    assert!(
        assessment.reasons.contains(&reason),
        "{:?}",
        assessment.reasons
    );
    assert!(assessment.reasons.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn fixed_planning_uses_the_hypothesized_lcb_and_the_whole_plan_cost() {
    let fixture = Fixture::fixed(1000);
    let result = assess_plan_feasibility_v1(&fixture.input()).unwrap();
    assert_eq!(
        result.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible
    );
    assert!(result.reasons.is_empty());
    assert_eq!(result.planned_cost_upper_micros, Some(2_001_000));
    let fixed = result.fixed.unwrap();
    assert_eq!(fixed.assessed_n, 1000);
    assert!(fixed.statistical_pass);
    assert!(fixed.required_fixed_n.unwrap() <= 1000);
    assert_eq!(fixed.hypothesized_lcb, (0.10 - fixed.radius).max(-1.0));
    assert_eq!(fixed.mde_infimum, 0.02 + fixed.radius);

    let small = Fixture::fixed(64);
    let result = assess_plan_feasibility_v1(&small.input()).unwrap();
    descriptive(&result, FeasibilityReason::PlannedNTooSmall);
    assert!(result.fixed.as_ref().unwrap().required_fixed_n.unwrap() > 64);
    for effect in [-1.0, 0.0, 0.02] {
        let mut input = fixture.input();
        input.effect_hypothesis = effect;
        let result = assess_plan_feasibility_v1(&input).unwrap();
        descriptive(&result, FeasibilityReason::RequiredFixedNNotFound);
        assert!(!result.fixed.unwrap().statistical_pass);
    }
}

#[test]
fn planning_cap_is_an_input_bound_and_required_n_is_not_clamped() {
    let fixture = Fixture::fixed(1000);
    let mut input = fixture.input();
    input.max_n = 999;
    assert!(matches!(
        assess_plan_feasibility_v1(&input),
        Err(Error::Invalid(reason)) if reason == "planned n exceeds planning search cap"
    ));
    let small = Fixture::fixed(2);
    let mut input = small.input();
    input.max_n = 2;
    let result = assess_plan_feasibility_v1(&input).unwrap();
    descriptive(&result, FeasibilityReason::RequiredFixedNNotFound);
    assert_eq!(result.fixed.unwrap().required_fixed_n, None);
}

#[test]
fn noninferiority_has_healthy_and_infeasible_sides_with_a_pending_equality() {
    let mut fixture = Fixture::fixed(2000);
    fixture.snapshot.profile = ProfileKind::NoninferiorSavings;
    fixture.snapshot.monetary_budget.amount = "5".into();
    fixture.total = 5_000_000;
    let mut input = fixture.input();
    input.effect_hypothesis = 0.0;
    input.resources.expected_cost_ratio = Declared(0.8);
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        result.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible
    );
    let radius = result.fixed.unwrap().radius;

    let mut small = fixture.clone();
    small.snapshot.n_planned = 1000;
    let mut input = small.input();
    input.effect_hypothesis = 0.0;
    input.resources.expected_cost_ratio = Declared(0.8);
    let result = assess_plan_feasibility_v1(&input).unwrap();
    descriptive(&result, FeasibilityReason::PlannedNTooSmall);

    // Construct and check an actual f64 equality, not an approximately equal display.
    fixture.snapshot.noninferior_bound = -radius;
    let mut input = fixture.input();
    input.effect_hypothesis = 0.0;
    input.resources.expected_cost_ratio = Declared(0.8);
    let equality = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        equality.fixed.as_ref().unwrap().hypothesized_lcb,
        fixture.snapshot.noninferior_bound,
    );
    assert_eq!(
        equality.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible
    );
    assert!(equality.fixed.as_ref().unwrap().statistical_pass);
    assert!(equality.reasons.is_empty());
    assert!(
        !equality
            .reasons
            .contains(&FeasibilityReason::NoninferiorityEqualityPending)
    );
    let rows: Vec<_> = (0..2000)
        .map(|i| ClusterObservation {
            cluster_id: format!("e{i}"),
            d: 0.0,
            weight: 1.0,
        })
        .collect();
    let report = empirical_bernstein(&rows, 0.05).unwrap();
    assert_eq!(report.lcb, fixture.snapshot.noninferior_bound);
    fixture.snapshot.frozen = true;
    fixture.snapshot.frozen_at = Some(1);
    fixture.snapshot.candidate_digest = Some("candidate".into());
    assert_eq!(
        decide(&fixture.snapshot, report, 0.8, 1.0, true)
            .unwrap()
            .verdict,
        Verdict::Noninferior,
    );
}

#[test]
fn all_unavailable_premises_remain_explicit_and_do_not_become_zero() {
    let fixture = Fixture::fixed(1000);
    let mut input = fixture.input();
    input.effect_source = MissingPremise;
    input.variance_upper = Unavailable;
    input.units.available_units = Unavailable;
    input.units.independence = MissingPremise;
    input.units.common_mean = MissingPremise;
    input.resources.anchors_safety = MissingPremise;
    input.costs.fixed_episode_upper_micros = Unavailable;
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert!(result.fixed.is_none());
    assert_eq!(result.planned_cost_upper_micros, None);
    for reason in [
        FeasibilityReason::AssumptionsUnavailable,
        FeasibilityReason::VarianceUnavailable,
        FeasibilityReason::IndependentUnitsUnavailable,
        FeasibilityReason::CostBoundsUnavailable,
        FeasibilityReason::ResourcePremisesUnavailable,
    ] {
        descriptive(&result, reason);
    }
}

#[test]
fn available_units_cover_planned_n_even_if_required_n_is_smaller() {
    let fixture = Fixture::fixed(1000);
    for available in [0, 1, 999] {
        let mut input = fixture.input();
        input.units.available_units = Declared(available);
        let result = assess_plan_feasibility_v1(&input).unwrap();
        descriptive(&result, FeasibilityReason::PlannedUnitsExceeded);
        assert!(result.fixed.unwrap().statistical_pass);
    }
    let mut input = fixture.input();
    input.units.kind = FormalStatisticalUnit::IndependentCompleteStratifiedBlock;
    assert_eq!(
        assess_plan_feasibility_v1(&input).unwrap().disposition,
        PlanFeasibilityDisposition::NumericallyCompatible,
    );
    input.units.available_units = Declared(100_001);
    assert!(assess_plan_feasibility_v1(&input).is_err());
}

#[test]
fn every_unknown_money_field_and_authorization_is_descriptive() {
    let fixture = Fixture::fixed(1000);
    for field in 0..6 {
        let mut input = fixture.input();
        match field {
            0 => input.money.currency = Unavailable,
            1 => input.money.pricing_version = Unavailable,
            2 => input.money.per_call_cap_micros = Unavailable,
            3 => input.money.experiment_total_micros = Unavailable,
            4 => input.money.payment_subject = Unavailable,
            5 => input.money.authorization = BudgetAuthorizationModel::Unavailable,
            _ => unreachable!(),
        }
        let result = assess_plan_feasibility_v1(&input).unwrap();
        descriptive(&result, FeasibilityReason::BudgetUnavailable);
        assert_eq!(result.planned_cost_upper_micros, Some(2_001_000));
    }
    let mut input = fixture.input();
    input.money.authorization = BudgetAuthorizationModel::Unfunded;
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::Unfunded,
    );

    let mut zero = fixture.clone();
    zero.snapshot.monetary_budget.amount = "0".into();
    zero.total = 0;
    let mut input = zero.input();
    input.costs.fixed_episode_upper_micros = Declared(0);
    input.costs.paired_unit_upper_micros = Declared(0);
    input.costs.largest_call_upper_micros = Declared(0);
    let result = assess_plan_feasibility_v1(&input).unwrap();
    descriptive(&result, FeasibilityReason::Unfunded);
    assert_eq!(result.planned_cost_upper_micros, Some(0));
}

#[test]
fn checked_full_cost_cannot_wrap_or_budget_only_the_minimum_n() {
    let fixture = Fixture::fixed(1000);
    let mut input = fixture.input();
    input.costs.paired_unit_upper_micros = Declared(i64::MAX as u64);
    let overflow = assess_plan_feasibility_v1(&input).unwrap();
    descriptive(&overflow, FeasibilityReason::CostArithmeticOverflow);
    assert_eq!(overflow.planned_cost_upper_micros, None);
    input.costs.paired_unit_upper_micros = Declared(1);
    input.costs.fixed_episode_upper_micros = Declared(i64::MAX as u64);
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::CostArithmeticOverflow,
    );
    input.costs.fixed_episode_upper_micros = Declared(1000);
    input.costs.paired_unit_upper_micros = Declared(3000);
    let over_budget = assess_plan_feasibility_v1(&input).unwrap();
    assert!(
        over_budget
            .fixed
            .as_ref()
            .unwrap()
            .required_fixed_n
            .unwrap()
            < 1000
    );
    assert_eq!(over_budget.planned_cost_upper_micros, Some(3_001_000));
    descriptive(&over_budget, FeasibilityReason::BudgetExceeded);
    input.costs.paired_unit_upper_micros = Declared(2000);
    input.costs.largest_call_upper_micros = Declared(1001);
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::CallCapExceeded,
    );
    for field in 0..3 {
        let mut input = fixture.input();
        match field {
            0 => input.costs.fixed_episode_upper_micros = Unavailable,
            1 => input.costs.paired_unit_upper_micros = Unavailable,
            2 => input.costs.largest_call_upper_micros = Unavailable,
            _ => unreachable!(),
        }
        descriptive(
            &assess_plan_feasibility_v1(&input).unwrap(),
            FeasibilityReason::CostBoundsUnavailable,
        );
    }
}

#[test]
fn money_identity_precision_and_limits_are_structural_not_guesses() {
    let fixture = Fixture::fixed(1000);
    for field in 0..5 {
        let mut input = fixture.input();
        match field {
            0 => input.money.currency = Declared("EUR"),
            1 => input.money.pricing_version = Declared("other-price"),
            2 => input.money.payment_subject = Declared(""),
            3 => input.money.experiment_total_micros = Declared(2_000_000),
            4 => {
                input.money.authorization = BudgetAuthorizationModel::DeclaredLimitMicros(2_000_000)
            }
            _ => unreachable!(),
        }
        assert!(assess_plan_feasibility_v1(&input).is_err());
    }
    for amount in ["-1", "3.00000001", "9223372036854.775808"] {
        let mut fixture = fixture.clone();
        fixture.snapshot.monetary_budget.amount = amount.into();
        assert!(assess_plan_feasibility_v1(&fixture.input()).is_err());
    }
    for field in 0..6 {
        let mut input = fixture.input();
        match field {
            0 => input.money.per_call_cap_micros = Declared(u64::MAX),
            1 => input.money.experiment_total_micros = Declared(u64::MAX),
            2 => {
                input.money.authorization = BudgetAuthorizationModel::DeclaredLimitMicros(u64::MAX)
            }
            3 => input.costs.fixed_episode_upper_micros = Declared(u64::MAX),
            4 => input.costs.paired_unit_upper_micros = Declared(u64::MAX),
            5 => input.costs.largest_call_upper_micros = Declared(u64::MAX),
            _ => unreachable!(),
        }
        assert!(assess_plan_feasibility_v1(&input).is_err());
    }
}

#[test]
fn resources_use_the_old_cost_latency_and_savings_gates() {
    let fixture = Fixture::fixed(1000);
    for (cost, p95) in [(0.0, 1.0), (1.10001, 1.0), (1.0, 1.20001)] {
        let mut input = fixture.input();
        input.resources.expected_cost_ratio = Declared(cost);
        input.resources.expected_p95_ratio = Declared(p95);
        descriptive(
            &assess_plan_feasibility_v1(&input).unwrap(),
            FeasibilityReason::ResourceGateFailed,
        );
    }
    for field in 0..5 {
        let mut input = fixture.input();
        match field {
            0 => input.resources.expected_cost_ratio = Unavailable,
            1 => input.resources.expected_p95_ratio = Unavailable,
            2 => input.resources.baseline_cost_positive = MissingPremise,
            3 => input.resources.bounded_reconcilable_cost = MissingPremise,
            4 => input.resources.anchors_safety = MissingPremise,
            _ => unreachable!(),
        }
        descriptive(
            &assess_plan_feasibility_v1(&input).unwrap(),
            FeasibilityReason::ResourcePremisesUnavailable,
        );
    }
    let mut noninferior = Fixture::fixed(2000);
    noninferior.snapshot.profile = ProfileKind::NoninferiorSavings;
    noninferior.snapshot.monetary_budget.amount = "5".into();
    noninferior.total = 5_000_000;
    let mut input = noninferior.input();
    input.effect_hypothesis = 0.0;
    input.resources.expected_cost_ratio = Declared(0.95);
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::ResourceGateFailed,
    );
}

#[test]
fn invalid_floats_versions_and_domains_are_not_descriptive_successes() {
    let fixture = Fixture::fixed(1000);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.01, 1.01] {
        let mut input = fixture.input();
        input.effect_hypothesis = bad;
        assert!(assess_plan_feasibility_v1(&input).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -0.001, 2.001] {
        let mut input = fixture.input();
        input.variance_upper = Declared(bad);
        assert!(assess_plan_feasibility_v1(&input).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -0.001] {
        for cost in [true, false] {
            let mut input = fixture.input();
            if cost {
                input.resources.expected_cost_ratio = Declared(bad);
            } else {
                input.resources.expected_p95_ratio = Declared(bad);
            }
            assert!(assess_plan_feasibility_v1(&input).is_err());
        }
    }
    for bad in [0, 1, 100_001] {
        let mut input = fixture.input();
        input.max_n = bad;
        assert!(assess_plan_feasibility_v1(&input).is_err());
    }
    for field in 0..5 {
        let mut fixture = fixture.clone();
        match field {
            0 => fixture.snapshot.stats_version = "empirical_bernstein.future".into(),
            1 => {
                fixture.snapshot.frozen = true;
                fixture.snapshot.frozen_at = Some(1);
                fixture.snapshot.candidate_digest = Some("already-bound".into());
            }
            2 => fixture.snapshot.min_effect = 0.0,
            3 => fixture.snapshot.noninferior_bound = 0.0,
            4 => fixture.snapshot.n_planned = 1,
            _ => unreachable!(),
        }
        assert!(assess_plan_feasibility_v1(&fixture.input()).is_err());
    }
    let mut frozen = fixture.clone();
    frozen.snapshot.frozen = true;
    frozen.snapshot.frozen_at = Some(1);
    assert_eq!(
        assess_plan_feasibility_v1(&frozen.input())
            .unwrap()
            .disposition,
        PlanFeasibilityDisposition::NumericallyCompatible,
    );
}

#[test]
fn fixed_alpha_is_the_scoped_allocation_not_a_divided_family_budget() {
    let fixture = Fixture::fixed(1000);
    for alpha in ["0.00001", "0.05", "0.050", "0.5", "NaN"] {
        let mut fixture = fixture.clone();
        fixture.alpha.allocations[0].alpha = alpha.into();
        let result = assess_plan_feasibility_v1(&fixture.input());
        assert_eq!(result.is_ok(), alpha == "0.05");
    }
    let mut at_minimum = fixture.clone();
    at_minimum.alpha.allocations[0].alpha = "0.0001".into();
    assert!(assess_plan_feasibility_v1(&at_minimum.input()).is_ok());
    let mut input = fixture.input();
    input.fixed_claim_id = "missing";
    assert!(assess_plan_feasibility_v1(&input).is_err());
    input.fixed_claim_id = "gain";
    input.attempt_id = "other-attempt";
    assert!(assess_plan_feasibility_v1(&input).is_err());
    let mut duplicate = fixture.clone();
    duplicate
        .alpha
        .allocations
        .push(duplicate.alpha.allocations[0].clone());
    assert!(assess_plan_feasibility_v1(&duplicate.input()).is_err());
    let mut family_total = fixture.clone();
    family_total.alpha.alpha_total = "0.06".into();
    assert!(assess_plan_feasibility_v1(&family_total.input()).is_err());
    let mut wrong_kind = fixture.clone();
    wrong_kind.alpha.allocations[0].kind = FormalClaimKind::OtherFormalMetric;
    assert!(assess_plan_feasibility_v1(&wrong_kind.input()).is_err());
}

#[test]
fn enabled_binary_exact_path_reaches_fixed_without_a_cs_approval_gate() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    let path = pairs(1000, 0, 125_000);
    let mut input = fixture.input();
    input.effect_hypothesis = 0.125;
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        result.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible
    );
    let SequentialFeasibilityProjection::ReachesFixed {
        last_bound,
        checked_prefixes,
    } = result.sequential
    else {
        panic!("healthy complete path did not reach fixed");
    };
    assert_eq!(checked_prefixes, 1000);
    assert_eq!(last_bound.k, 1000);
    // The CS rejects only. Its LCB need not cross the gain threshold.
    assert!(last_bound.lcb < fixture.snapshot.min_effect);
    assert!(result.fixed.unwrap().statistical_pass);
}

#[test]
fn conservative_general_decimal_path_uses_both_exact_and_old_fixed_models() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    let path = pairs(1000, 0, 100_000);
    let mut input = fixture.input();
    input.effect_hypothesis = 0.0999999999999;
    input.variance_upper = Declared(1e-20);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    let conservative = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        conservative.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible,
    );
    input.effect_hypothesis = 0.1;
    input.variance_upper = Declared(0.0);
    let optimistic = assess_plan_feasibility_v1(&input).unwrap();
    descriptive(
        &optimistic,
        FeasibilityReason::SequentialProjectionMisaligned,
    );

    // Tiny declaration differences are not rounded away by the exact mean comparison.
    let zeros = pairs(1000, 0, 0);
    input.effect_hypothesis = f64::from_bits(1);
    input.variance_upper = Declared(0.0);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&zeros),
    };
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::SequentialProjectionMisaligned,
    );
    input.effect_hypothesis = -f64::from_bits(1);
    input.variance_upper = Declared(f64::from_bits(1));
    let conservative_zero = assess_plan_feasibility_v1(&input).unwrap();
    assert!(
        !conservative_zero
            .reasons
            .contains(&FeasibilityReason::SequentialProjectionMisaligned,)
    );
}

#[test]
fn favorable_suffix_cannot_reopen_the_first_rejecting_prefix() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    let mut path = pairs(1000, 0, 500_000);
    for unit in &mut path[..400] {
        unit.baseline_score_micros = 500_000;
        unit.candidate_score_micros = 0;
    }
    let mut input = fixture.input();
    input.effect_hypothesis = 0.09999999999999999;
    input.variance_upper = Declared(0.3);
    let fixed_counterfactual = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        fixed_counterfactual.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible,
    );
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert!(result.fixed.as_ref().unwrap().statistical_pass);
    descriptive(&result, FeasibilityReason::SequentialProjectionStopped);
    let SequentialFeasibilityProjection::Stopped {
        first_k,
        decision,
        bound,
    } = result.sequential
    else {
        panic!("negative prefix was discarded");
    };
    assert!(first_k <= 400);
    assert_eq!(bound.k, first_k);
    assert_eq!(decision, EarlyStopDecision::FutilityQualityGain);
    let mut state = SequentialRejectOnly::open(early, &fixture.alpha).unwrap();
    for unit in path.iter().take(first_k as usize) {
        state.record_complete_unit(unit.clone()).unwrap();
    }
    assert!(
        state
            .record_complete_unit(path[first_k as usize].clone())
            .is_err()
    );
}

#[test]
fn enabled_unknown_path_keeps_fixed_summary_but_cannot_be_disabled_implicitly() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    let mut input = fixture.input();
    input.effect_hypothesis = 0.125;
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Unavailable,
    };
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert!(result.fixed.as_ref().unwrap().statistical_pass);
    assert!(matches!(
        result.sequential,
        SequentialFeasibilityProjection::Unavailable
    ));
    descriptive(&result, FeasibilityReason::SequentialProjectionUnavailable);

    let path = pairs(1000, 0, 125_000);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    input.resources.anchors_safety = MissingPremise;
    let result = assess_plan_feasibility_v1(&input).unwrap();
    assert!(matches!(
        result.sequential,
        SequentialFeasibilityProjection::ReachesFixed { .. }
    ));
    descriptive(&result, FeasibilityReason::ResourcePremisesUnavailable);
}

#[test]
fn full_path_shape_is_validated_even_beyond_a_rejecting_prefix() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    for field in 0..6 {
        let mut path = pairs(1000, 1_000_000, 0);
        match field {
            0 => {
                path.pop();
            }
            1 => {
                path.push(path[999].clone());
            }
            2 => path[999].cluster_id = path[998].cluster_id.clone(),
            3 => path.swap(998, 999),
            4 => path[999].candidate_score_micros = 1_000_001,
            5 => path[999].baseline_score_micros = 1_000_001,
            _ => unreachable!(),
        }
        let mut input = fixture.input();
        input.effect_hypothesis = -1.0;
        input.sequential = SequentialPlanning::Enabled {
            plan: &early,
            assumed_complete_path: Declared(&path),
        };
        assert!(
            assess_plan_feasibility_v1(&input).is_err(),
            "shape case {field}"
        );
    }
}

#[test]
fn sequence_cannot_rebind_version_rho_alpha_attempt_or_snapshot() {
    let fixture = Fixture::with_sequence(1000);
    let path = pairs(1000, 0, 125_000);
    for field in 0..9 {
        let mut early = fixture.early();
        match field {
            0 => early.rho = "2".into(),
            1 => early.cs_version = "unregistered".into(),
            2 => early.early_stop_version = "unregistered".into(),
            3 => early.alpha_stop_i = "0.02".into(),
            4 => early.alpha_plan_digest = "d".repeat(64),
            5 => early.attempt_id = "other-attempt".into(),
            6 => early.v1_plan_snapshot_digest = "d".repeat(64),
            7 => early.min_gain_micros += 1,
            8 => early.noninferiority_margin_micros += 1,
            _ => unreachable!(),
        }
        let mut input = fixture.input();
        input.effect_hypothesis = 0.125;
        input.sequential = SequentialPlanning::Enabled {
            plan: &early,
            assumed_complete_path: Declared(&path),
        };
        assert!(
            assess_plan_feasibility_v1(&input).is_err(),
            "binding case {field}"
        );
    }
    // A different family has a different digest even with identical allocation amounts.
    let early = fixture.early();
    let mut other = fixture.clone();
    other.alpha.research_family_id = "other-family".into();
    let mut input = other.input();
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    assert!(assess_plan_feasibility_v1(&input).is_err());
    let mut unrepresentable = fixture.clone();
    unrepresentable.snapshot.min_effect = 0.0200000001;
    let mut early = unrepresentable.early();
    early.min_gain_micros = 20_000;
    let mut input = unrepresentable.input();
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    assert!(assess_plan_feasibility_v1(&input).is_err());
}

#[test]
fn mean_or_variance_optimism_cannot_pass_the_exact_path_binding() {
    let fixture = Fixture::with_sequence(1000);
    let early = fixture.early();
    let path = pairs(1000, 0, 125_000);
    let mut input = fixture.input();
    input.effect_hypothesis = 0.12500000000000003;
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::SequentialProjectionMisaligned,
    );
    let mut variable = path.clone();
    variable[0].candidate_score_micros = 250_000;
    input.effect_hypothesis = 0.125;
    input.variance_upper = Declared(0.0);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&variable),
    };
    descriptive(
        &assess_plan_feasibility_v1(&input).unwrap(),
        FeasibilityReason::SequentialProjectionMisaligned,
    );
    input.variance_upper = Unavailable;
    let unknown = assess_plan_feasibility_v1(&input).unwrap();
    assert!(unknown.fixed.is_none());
    descriptive(&unknown, FeasibilityReason::VarianceUnavailable);
}

#[test]
fn all_four_rejections_stay_reject_only_and_n_point_priority_is_unchanged() {
    let fixture = Fixture::with_sequence(1000);
    let mut regression = fixture.clone();
    regression.snapshot.profile = ProfileKind::NoninferiorSavings;
    regression.snapshot.noninferior_bound = -0.1;
    let mut early = regression.early();
    early.noninferiority_margin_micros = 100_000;
    let negative = pairs(1000, 1_000_000, 0);
    let mut state = SequentialRejectOnly::open(early, &regression.alpha).unwrap();
    let mut first = None;
    for unit in &negative {
        let decision = state.record_complete_unit(unit.clone()).unwrap();
        if decision.is_early_stop() {
            first = Some((state.k(), decision));
            break;
        }
    }
    let (stop_k, decision) = first.unwrap();
    assert_eq!(decision, EarlyStopDecision::RegressedSequential);
    // Truncate the frozen plan exactly at its first rejecting boundary:
    // the old UCB check still takes priority over FinalFixedSample at n.
    let mut short = Fixture::with_sequence(stop_k as usize);
    short.snapshot.profile = ProfileKind::NoninferiorSavings;
    short.snapshot.noninferior_bound = -0.1;
    let mut early = short.early();
    early.noninferiority_margin_micros = 100_000;
    let path = pairs(stop_k as usize, 1_000_000, 0);
    let mut input = short.input();
    input.effect_hypothesis = -1.0;
    input.resources.expected_cost_ratio = Declared(0.8);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    let assessment = assess_plan_feasibility_v1(&input).unwrap();
    let SequentialFeasibilityProjection::Stopped {
        first_k, decision, ..
    } = assessment.sequential
    else {
        panic!("n-point rejection was relabeled final");
    };
    assert_eq!(first_k, stop_k);
    assert_eq!(decision, EarlyStopDecision::RegressedSequential);

    let mut noninferior = Fixture::with_sequence(1000);
    noninferior.snapshot.profile = ProfileKind::NoninferiorSavings;
    let mut early = noninferior.early();
    early.max_regression_micros = 800_000;
    let path = pairs(1000, 500_000, 250_000);
    let mut input = noninferior.input();
    input.effect_hypothesis = -0.25;
    input.resources.expected_cost_ratio = Declared(0.8);
    input.sequential = SequentialPlanning::Enabled {
        plan: &early,
        assumed_complete_path: Declared(&path),
    };
    let assessment = assess_plan_feasibility_v1(&input).unwrap();
    assert!(matches!(
        assessment.sequential,
        SequentialFeasibilityProjection::Stopped {
            decision: EarlyStopDecision::FutilityNoninferiority,
            ..
        }
    ));

    // Critical anchors are an external existing state input, never manufactured
    // from a caller's path or added to the pure planning contract.
    let mut state = SequentialRejectOnly::open(fixture.early(), &fixture.alpha).unwrap();
    let critical = state.stop_for_critical_anchor(&"d".repeat(64)).unwrap();
    assert_eq!(critical, EarlyStopDecision::CriticalRegression);
    assert!(critical.is_early_stop());
    assert!(state.record_complete_unit(negative[0].clone()).is_err());
    for decision in [
        EarlyStopDecision::CriticalRegression,
        EarlyStopDecision::FutilityQualityGain,
        EarlyStopDecision::FutilityNoninferiority,
        EarlyStopDecision::RegressedSequential,
    ] {
        assert!(decision.is_early_stop());
    }
    assert!(!EarlyStopDecision::FinalFixedSample.is_early_stop());
}

#[test]
fn shared_fixed_radius_retains_the_original_operation_order_and_invalid_reports() {
    for (n, alpha, d) in [(2, 0.05, 0.0), (64, 0.0001, 0.125), (1000, 0.49, 0.1)] {
        let rows: Vec<_> = (0..n)
            .map(|i| ClusterObservation {
                cluster_id: format!("g{i}"),
                d: if i % 2 == 0 { d } else { -d },
                weight: 1.0,
            })
            .collect();
        let report = empirical_bernstein(&rows, alpha).unwrap();
        let ln = (2.0 / alpha).ln();
        let original = (2.0 * report.variance * ln / n as f64).sqrt()
            + 7.0 * 2.0 * ln / (3.0 * (n as f64 - 1.0));
        assert_eq!(report.radius.to_bits(), original.to_bits());
        assert_eq!(
            report.lcb.to_bits(),
            (report.mean - original).max(-1.0).to_bits()
        );
    }
    let rows = [ClusterObservation {
        cluster_id: "g".into(),
        d: 0.0,
        weight: 1.0,
    }];
    for alpha in [f64::NAN, f64::INFINITY, 0.0, 0.000099, 0.5, 0.75] {
        let report = empirical_bernstein(&rows, alpha).unwrap();
        assert_eq!(report.verdict, Verdict::Invalid);
        assert_eq!(report.reasons, ["alpha_i out of range"]);
        assert!(estimate_n(0.0, 0.02, alpha, 1000, 0, 0).is_err());
    }
    assert_eq!(empirical_bernstein(&rows, 0.05).unwrap().reasons, ["n<2"]);
    assert!(matches!(
        estimate_n(0.0, 1.0, 0.05, 20, u64::MAX, u64::MAX),
        Err(Error::Invalid(reason)) if reason == "estimator cost arithmetic overflow"
    ));
    let free = estimate_n(0.0, 0.02, 0.05, 1000, 0, 0).unwrap();
    assert!(free.feasible);
    let denied = estimate_n(0.0, 0.02, 0.05, 1000, 0, 1).unwrap();
    assert!(!denied.feasible);
    assert_eq!(denied.n, free.n);
    assert_eq!(denied.reasons, ["power_requires_n_exceeding_budget"]);
    assert!(estimate_n(f64::MAX, 1.0, 0.05, 2, 0, 0).is_err());
}

#[test]
fn strict_lcb_boundary_is_not_replaced_by_a_rounded_mde_comparison() {
    let rows: Vec<_> = (0..1000)
        .map(|i| ClusterObservation {
            cluster_id: format!("b{i}"),
            d: 0.0,
            weight: 1.0,
        })
        .collect();
    let radius = empirical_bernstein(&rows, 0.05).unwrap().radius;
    let mut fixture = Fixture::fixed(1000);
    fixture.snapshot.min_effect = 0.125 - radius;
    let mut input = fixture.input();
    input.effect_hypothesis = 0.125;
    let boundary = assess_plan_feasibility_v1(&input).unwrap();
    let fixed = boundary.fixed.as_ref().unwrap();
    assert_eq!(
        fixed.hypothesized_lcb.to_bits(),
        fixture.snapshot.min_effect.to_bits()
    );
    assert!(!fixed.statistical_pass);
    descriptive(&boundary, FeasibilityReason::PlannedNTooSmall);
    input.effect_hypothesis = f64::from_bits(0.125_f64.to_bits() + 1);
    assert!(
        assess_plan_feasibility_v1(&input)
            .unwrap()
            .fixed
            .unwrap()
            .statistical_pass
    );
    let final_rows: Vec<_> = rows
        .iter()
        .map(|row| ClusterObservation {
            cluster_id: row.cluster_id.clone(),
            d: 0.125,
            weight: 1.0,
        })
        .collect();
    let report = empirical_bernstein(&final_rows, 0.05).unwrap();
    assert_eq!(report.lcb.to_bits(), fixture.snapshot.min_effect.to_bits());
    let mut frozen = fixture.snapshot.clone();
    frozen.frozen = true;
    frozen.frozen_at = Some(1);
    frozen.candidate_digest = Some("candidate".into());
    let decision = decide(&frozen, report, 1.0, 1.0, true).unwrap();
    assert_eq!(decision.verdict, Verdict::Inconclusive);
    assert!(decision.reasons.contains(&"gain_not_demonstrated".into()));
}

fn check_noninferiority_equality_boundary(n: usize, max_n: u32, tighter_required_n: Option<u32>) {
    let rows: Vec<_> = (0..n)
        .map(|index| ClusterObservation {
            cluster_id: format!("equality-{index}"),
            d: 0.0,
            weight: 1.0,
        })
        .collect();
    let report = empirical_bernstein(&rows, 0.05).unwrap();
    let boundary = -report.radius;
    assert_eq!(report.lcb.to_bits(), boundary.to_bits());
    let wider = f64::from_bits(boundary.to_bits() + 1);
    let tighter = f64::from_bits(boundary.to_bits() - 1);
    assert!(wider < boundary && boundary < tighter);

    // The adjacent-f64 positive and negative controls precede exact equality.
    for (bound, statistical_pass, disposition, required_n, verdict) in [
        (
            wider,
            true,
            PlanFeasibilityDisposition::NumericallyCompatible,
            Some(n as u32),
            Verdict::Noninferior,
        ),
        (
            tighter,
            false,
            PlanFeasibilityDisposition::DescriptiveOnly,
            tighter_required_n,
            Verdict::Inconclusive,
        ),
        (
            boundary,
            true,
            PlanFeasibilityDisposition::NumericallyCompatible,
            Some(n as u32),
            Verdict::Noninferior,
        ),
    ] {
        let mut fixture = Fixture::fixed(n);
        fixture.snapshot.profile = ProfileKind::NoninferiorSavings;
        fixture.snapshot.noninferior_bound = bound;
        fixture.snapshot.monetary_budget.amount = "5".into();
        fixture.total = 5_000_000;
        let mut input = fixture.input();
        input.max_n = max_n;
        input.effect_hypothesis = 0.0;
        input.resources.expected_cost_ratio = Declared(0.8);
        let assessment = assess_plan_feasibility_v1(&input).unwrap();
        assert_eq!(assessment.disposition, disposition);
        let fixed = assessment.fixed.as_ref().unwrap();
        assert_eq!(fixed.assessed_n, n as u32);
        assert_eq!(fixed.statistical_pass, statistical_pass);
        assert_eq!(fixed.required_fixed_n, required_n);
        assert_eq!(fixed.radius.to_bits(), report.radius.to_bits());
        assert_eq!(fixed.hypothesized_lcb.to_bits(), boundary.to_bits());
        assert!(
            !assessment
                .reasons
                .contains(&FeasibilityReason::NoninferiorityEqualityPending)
        );
        if statistical_pass {
            assert!(assessment.reasons.is_empty());
        } else {
            descriptive(&assessment, FeasibilityReason::PlannedNTooSmall);
            assert_eq!(
                assessment
                    .reasons
                    .contains(&FeasibilityReason::RequiredFixedNNotFound),
                required_n.is_none()
            );
        }

        fixture.snapshot.frozen = true;
        fixture.snapshot.frozen_at = Some(1);
        fixture.snapshot.candidate_digest = Some("candidate".into());
        let decision = decide(
            &fixture.snapshot,
            empirical_bernstein(&rows, 0.05).unwrap(),
            0.8,
            1.0,
            true,
        )
        .unwrap();
        assert_eq!(decision.verdict, verdict);
        if verdict == Verdict::Inconclusive {
            assert_eq!(decision.reasons, ["noninferiority_not_demonstrated"]);
        } else {
            assert!(decision.reasons.is_empty());
        }
    }
}

#[test]
fn noninferiority_fixed_lcb_equality_matches_approval() {
    check_noninferiority_equality_boundary(2000, 100_000, Some(2001));
}

#[test]
fn noninferiority_required_n_includes_equal_search_cap() {
    check_noninferiority_equality_boundary(1000, 1000, None);
}
