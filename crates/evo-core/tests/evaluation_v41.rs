use evo_core::evaluation::{
    ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41,
    FormalStatisticalUnit, Money, OptimizationBudgetPlan, OptimizationStage,
    OptimizerComparisonContract, ProfileKind,
};
use evo_core::holdout::{
    AnchorCoverageMatrix, AnchorGateDecision, AnchorOutcome, AnchorResult,
    CriticalCapabilityCoverage, ExposureLedger, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::sequential::{
    AlphaAllocation, CompletePairedUnit, EarlyStopCertificate, EarlyStopDecision, EarlyStopPlan,
    FormalClaimKind, ResearchFamilyAlphaPlan, SequentialRejectOnly,
};
use serde_json::json;

fn d(label: &str) -> String {
    evo_core::hash(label.as_bytes())
}

fn alpha_plan() -> ResearchFamilyAlphaPlan {
    ResearchFamilyAlphaPlan::new(
        "family-1",
        "0.05",
        vec![
            AlphaAllocation {
                claim_id: "final-gain".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: "0.04".into(),
            },
            AlphaAllocation {
                claim_id: "early-reject".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::SequentialReject,
                alpha: "0.01".into(),
            },
        ],
    )
    .unwrap()
}

fn coverage() -> AnchorCoverageMatrix {
    AnchorCoverageMatrix::new(
        "quality-gain",
        "anchors-v1",
        vec![
            CriticalCapabilityCoverage {
                capability_id: "filesystem-safety".into(),
                anchor_ids: vec!["anchor-fs".into()],
            },
            CriticalCapabilityCoverage {
                capability_id: "approval-integrity".into(),
                anchor_ids: vec!["anchor-approval".into()],
            },
        ],
    )
    .unwrap()
}

fn early_plan(n: u32) -> EarlyStopPlan {
    let matrix = coverage();
    EarlyStopPlan::new(
        d("v1-plan-snapshot"),
        &alpha_plan(),
        "early-reject",
        "attempt-1",
        ProfileKind::QualityGain,
        n,
        (0..n).map(|i| format!("cluster-{i}")).collect(),
        d("iid-common-mean-review"),
        d("sampler-rules-v1"),
        matrix.digest().unwrap(),
        20_000,
        10_000,
        10_000,
    )
    .unwrap()
}

fn unit(index: u32, baseline: u32, candidate: u32) -> CompletePairedUnit {
    CompletePairedUnit {
        cluster_id: format!("cluster-{index}"),
        baseline_score_micros: baseline,
        candidate_score_micros: candidate,
    }
}

fn manifest(
    pair: &FrozenCandidatePair,
    plan: &EarlyStopPlan,
    epoch: &str,
    slice: &str,
) -> HoldoutManifest {
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        pair,
        "family-1",
        alpha_plan().digest().unwrap(),
        SequentialMode::RejectOnly {
            early_stop_plan_digest: plan.digest().unwrap(),
        },
        epoch,
        slice,
        plan.ordered_cluster_ids.clone(),
        (0..plan.n_planned)
            .map(|i| d(&format!("source-closure-{i}")))
            .collect(),
        "sampler-v1",
        "oracle-v1",
        plan.anchor_coverage_digest.clone(),
        d("grader-v1"),
        d("linux-env-v1"),
        d("private-seed-commitment"),
        101,
    )
    .unwrap();
    manifest.validate_sequential_binding(plan).unwrap();
    manifest
}

#[test]
fn v085_golden_stops_at_13_and_never_appends_after_stop() {
    let mut stream = SequentialRejectOnly::open(early_plan(60), &alpha_plan()).unwrap();
    for i in 0..12 {
        assert_eq!(
            stream.record_complete_unit(unit(i, 1_000_000, 0)).unwrap(),
            EarlyStopDecision::Continue
        );
    }
    let at_12 = stream.boundary().unwrap().unwrap();
    assert!((at_12.ucb - 0.031_041_701_1).abs() < 1e-10);
    assert_eq!(
        stream.record_complete_unit(unit(12, 1_000_000, 0)).unwrap(),
        EarlyStopDecision::FutilityQualityGain
    );
    let at_13 = stream.boundary().unwrap().unwrap();
    assert!((at_13.ucb - (-0.009_239_226_6)).abs() < 1e-10);
    assert!(stream.record_complete_unit(unit(13, 1_000_000, 0)).is_err());
    assert_eq!(stream.k(), 13);
}

#[test]
fn v085_out_of_order_waits_for_the_complete_continuous_prefix() {
    let mut stream = SequentialRejectOnly::open(early_plan(20), &alpha_plan()).unwrap();
    assert_eq!(
        stream
            .record_complete_unit(unit(1, 500_000, 500_000))
            .unwrap(),
        EarlyStopDecision::Collecting
    );
    assert_eq!(stream.k(), 0);
    stream
        .record_complete_unit(unit(0, 500_000, 500_000))
        .unwrap();
    assert_eq!(stream.k(), 2);
    assert!(
        stream
            .record_complete_unit(unit(1, 500_000, 500_000))
            .is_err()
    );
    let unknown = CompletePairedUnit {
        cluster_id: "not-preregistered".into(),
        baseline_score_micros: 0,
        candidate_score_micros: 0,
    };
    assert!(stream.record_complete_unit(unknown).is_err());
}

#[test]
fn v085_first_stopping_prefix_is_independent_of_return_order() {
    let mut stream = SequentialRejectOnly::open(early_plan(60), &alpha_plan()).unwrap();
    for i in 13..60 {
        assert_eq!(
            stream.record_complete_unit(unit(i, 0, 1_000_000)).unwrap(),
            EarlyStopDecision::Collecting
        );
    }
    for i in 0..12 {
        assert_eq!(
            stream.record_complete_unit(unit(i, 1_000_000, 0)).unwrap(),
            EarlyStopDecision::Continue
        );
    }
    assert_eq!(
        stream.record_complete_unit(unit(12, 1_000_000, 0)).unwrap(),
        EarlyStopDecision::FutilityQualityGain
    );
    assert_eq!(stream.k(), 13);
}

#[test]
fn v085_uses_ucb_for_regression_and_outward_rounding() {
    let mut stream = SequentialRejectOnly::open(early_plan(60), &alpha_plan()).unwrap();
    assert_eq!(
        stream
            .record_complete_unit(unit(0, 500_000, 500_000))
            .unwrap(),
        EarlyStopDecision::Continue
    );
    let boundary = stream.boundary().unwrap().unwrap();
    assert!(boundary.lcb < -0.01);
    assert!(boundary.ucb > 0.01);
    assert_ne!(
        stream.decision().unwrap(),
        EarlyStopDecision::RegressedSequential
    );
    assert_eq!(boundary.ucb, 1.0);
    assert_eq!(boundary.lcb, -1.0);
}

#[test]
fn v085_rejects_illegal_alpha_rho_bounds_and_duplicate_units() {
    let mut plan = early_plan(2);
    plan.alpha_stop_i = "NaN".into();
    assert!(plan.validate().is_err());
    plan.alpha_stop_i = "0.01".into();
    plan.rho = "0.5".into();
    assert!(plan.validate().is_err());
    plan.rho = "1".into();
    plan.ordered_cluster_ids[1] = plan.ordered_cluster_ids[0].clone();
    assert!(plan.validate().is_err());

    let mut stream = SequentialRejectOnly::open(early_plan(2), &alpha_plan()).unwrap();
    assert!(stream.record_complete_unit(unit(0, 1_000_001, 0)).is_err());

    let mut self_reported = early_plan(2);
    self_reported.alpha_stop_i = "0.02".into();
    assert!(self_reported.validate().is_ok());
    assert!(SequentialRejectOnly::open(self_reported, &alpha_plan()).is_err());
}

#[test]
fn alpha_is_allocated_once_for_the_whole_research_family() {
    let mut duplicate = alpha_plan();
    duplicate.allocations.push(AlphaAllocation {
        claim_id: "second-peek-budget".into(),
        attempt_id: "attempt-2".into(),
        kind: FormalClaimKind::SequentialReject,
        alpha: "0.001".into(),
    });
    duplicate.allocations.extend([
        AlphaAllocation {
            claim_id: "latency-p95".into(),
            attempt_id: "attempt-1".into(),
            kind: FormalClaimKind::OtherFormalMetric,
            alpha: "0.001".into(),
        },
        AlphaAllocation {
            claim_id: "retention-rate".into(),
            attempt_id: "attempt-1".into(),
            kind: FormalClaimKind::OtherFormalMetric,
            alpha: "0.001".into(),
        },
    ]);
    duplicate.allocations[0].alpha = "0.037".into();
    assert!(duplicate.validate().is_ok());

    let mut overspent = alpha_plan();
    overspent.allocations[0].alpha = "0.05".into();
    assert!(overspent.validate().is_err());
}

#[test]
fn v084_requires_complete_anchor_coverage_and_vetoes_one_failure() {
    let matrix = coverage();
    let passing = vec![
        AnchorOutcome {
            anchor_id: "anchor-fs".into(),
            result: AnchorResult::Passed,
            grader_receipt_digest: d("receipt-fs"),
        },
        AnchorOutcome {
            anchor_id: "anchor-approval".into(),
            result: AnchorResult::Passed,
            grader_receipt_digest: d("receipt-approval"),
        },
    ];
    assert_eq!(
        evo_core::holdout::anchor_gate(&matrix, &passing).unwrap(),
        AnchorGateDecision::Passed
    );
    assert!(evo_core::holdout::anchor_gate(&matrix, &passing[..1]).is_err());
    let mut failed = passing;
    failed[1].result = AnchorResult::CriticalRegression;
    assert_eq!(
        evo_core::holdout::anchor_gate(&matrix, &failed).unwrap(),
        AnchorGateDecision::CriticalRegression
    );
}

#[test]
fn v084_manifest_is_after_candidate_freeze_and_binds_exact_order() {
    let pair = FrozenCandidatePair::new(d("v1-plan-snapshot"), d("candidate"), d("baseline"), 100)
        .unwrap();
    let plan = early_plan(3);
    let ok = manifest(&pair, &plan, "epoch-1", "slice-1");
    assert_eq!(ok.candidate_pair_digest, pair.digest().unwrap());

    let late_pair =
        FrozenCandidatePair::new(d("v1-plan-snapshot"), d("candidate-2"), d("baseline"), 200)
            .unwrap();
    let result = HoldoutManifest::issue_after_candidates_frozen(
        &late_pair,
        "family-1",
        alpha_plan().digest().unwrap(),
        SequentialMode::Disabled,
        "epoch-1",
        "slice-2",
        plan.ordered_cluster_ids.clone(),
        vec![d("s0"), d("s1"), d("s2")],
        "sampler-v1",
        "oracle-v1",
        coverage().digest().unwrap(),
        d("grader-v1"),
        d("env-v1"),
        d("seed-commitment"),
        199,
    );
    assert!(result.is_err());

    let mut wrong_order = plan.ordered_cluster_ids.clone();
    wrong_order.swap(0, 1);
    let mut independently_frozen = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "family-1",
        alpha_plan().digest().unwrap(),
        SequentialMode::Disabled,
        "epoch-1",
        "slice-3",
        wrong_order,
        vec![d("s0"), d("s1"), d("s2")],
        "sampler-v1",
        "oracle-v1",
        coverage().digest().unwrap(),
        d("grader-v1"),
        d("env-v1"),
        d("seed-commitment"),
        101,
    )
    .unwrap();
    independently_frozen.ordered_task_cluster_ids.swap(0, 1);
    assert!(independently_frozen.validate().is_err());
}

#[test]
fn v084_exposure_restore_never_resets_across_epoch_or_cancel() {
    let pair = FrozenCandidatePair::new(d("v1-plan-snapshot"), d("same"), d("same"), 100).unwrap();
    let plan = early_plan(2);
    let first = manifest(&pair, &plan, "epoch-1", "slice-1");
    let mut ledger = ExposureLedger::new("family-1", alpha_plan().digest().unwrap(), 1).unwrap();
    ledger.reserve("ticket-1", &first).unwrap();
    ledger.mark_dispatched("ticket-1", 102).unwrap();
    assert!(ledger.release_undispatched_money("ticket-1").is_err());
    assert_eq!(ledger.query_attempts_reserved(), 1);

    let snapshot = serde_json::to_vec(&ledger).unwrap();
    let restored: ExposureLedger = serde_json::from_slice(&snapshot).unwrap();
    restored.validate_restore().unwrap();
    assert!(restored.is_cluster_consumed("cluster-0"));

    let second = manifest(&pair, &plan, "epoch-2", "slice-2");
    let mut restored = restored;
    assert!(restored.reserve("ticket-2", &second).is_err());
    assert_eq!(restored.query_attempts_reserved(), 1);

    restored.entries[0].state = evo_core::holdout::ExposureState::Dispatched {
        dispatched_at_unix_ms: -1,
    };
    assert!(restored.validate_restore().is_err());
}

fn optimizer_spec(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(implementation),
        initial_s0_digest: d("s0"),
        base_model_digest: d("base-model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 8_000,
        root_budget_scope_id: "root-budget".into(),
        budget_limit: Money::zero_unfunded("USD"),
    }
}

#[test]
fn v096_v097_zero_budget_and_same_start_are_structural_contracts() {
    let budget = OptimizationBudgetPlan::unfunded("root-budget", "payer", "USD").unwrap();
    assert!(
        budget
            .stages
            .iter()
            .all(|stage| { stage.per_call_limit.is_zero() && stage.experiment_total.is_zero() })
    );
    assert!(
        budget
            .stages
            .iter()
            .any(|stage| { stage.stage == OptimizationStage::DevelopmentScoring })
    );
    let comparison = OptimizerComparisonContract::new(
        optimizer_spec(FixedOptimizerArm::CSimple, "simple-v1"),
        optimizer_spec(FixedOptimizerArm::CSkillopt, "skillopt-v1"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    assert_eq!(comparison.d_frozen_start_digest(), d("skillopt-v1"));
    OptimizerComparisonContract::new(
        optimizer_spec(FixedOptimizerArm::CSimple, "no-change-control"),
        optimizer_spec(FixedOptimizerArm::CSkillopt, "no-change-control"),
        FixedOptimizerArm::CSimple,
    )
    .unwrap();

    let mut unequal = optimizer_spec(FixedOptimizerArm::CSkillopt, "skillopt-v1");
    unequal.initial_s0_digest = d("different-s0");
    assert!(
        OptimizerComparisonContract::new(
            optimizer_spec(FixedOptimizerArm::CSimple, "simple-v1"),
            unequal,
            FixedOptimizerArm::CSkillopt,
        )
        .is_err()
    );

    let mut unequal_budget = optimizer_spec(FixedOptimizerArm::CSkillopt, "skillopt-v1");
    unequal_budget.budget_limit.amount = "1".into();
    unequal_budget.budget_limit.pricing_version = "pricing-v1".into();
    assert!(
        OptimizerComparisonContract::new(
            optimizer_spec(FixedOptimizerArm::CSimple, "simple-v1"),
            unequal_budget,
            FixedOptimizerArm::CSkillopt,
        )
        .is_err()
    );

    let mut snapshot = ExperimentPlan::first_low_risk("formal-v41").unwrap();
    snapshot.freeze(10).unwrap();
    let alpha = alpha_plan();
    assert!(
        FormalExperimentPlanV41::new(
            &snapshot,
            d("preregistered-before-candidate"),
            &alpha,
            &comparison,
            &budget,
            coverage().digest().unwrap(),
            d("sampler-rules-v1"),
            d("iid-common-mean-review"),
            FormalStatisticalUnit::IndependentTaskCluster,
            0,
        )
        .is_err()
    );
    let formal = FormalExperimentPlanV41::new(
        &snapshot,
        d("preregistered-before-candidate"),
        &alpha,
        &comparison,
        &budget,
        coverage().digest().unwrap(),
        d("sampler-rules-v1"),
        d("iid-common-mean-review"),
        FormalStatisticalUnit::IndependentTaskCluster,
        60,
    )
    .unwrap();
    formal
        .validate_against(&snapshot, &alpha, &comparison, &budget)
        .unwrap();

    let mut wrong_n = formal.clone();
    wrong_n.n_planned = 1_000;
    assert!(
        wrong_n
            .validate_against(&snapshot, &alpha, &comparison, &budget)
            .is_err()
    );

    let mut post_candidate = snapshot.clone();
    post_candidate.bind_candidate("candidate").unwrap();
    assert!(
        formal
            .validate_against(&post_candidate, &alpha, &comparison, &budget)
            .is_err()
    );
}

#[test]
fn v096_budget_amounts_are_canonical_bounded_and_authorized() {
    let mut no_receipt = OptimizationBudgetPlan::unfunded("root-budget", "payer", "USD").unwrap();
    no_receipt.root_total.amount = "1".into();
    no_receipt.stages[0].per_call_limit.amount = "0.5".into();
    no_receipt.stages[0].experiment_total.amount = "1".into();
    assert!(no_receipt.validate().is_err());

    let mut authorized = no_receipt.clone();
    authorized.admin_authorization_receipt_digest = Some(d("admin-authorization"));
    authorized.root_total.pricing_version = "pricing-v1".into();
    for stage in &mut authorized.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    authorized.validate().unwrap();

    let mut negative = authorized.clone();
    negative.root_total.amount = "-1".into();
    assert!(negative.validate().is_err());

    let mut mixed_currency = authorized.clone();
    mixed_currency.stages[0].per_call_limit.currency = "EUR".into();
    assert!(mixed_currency.validate().is_err());

    let mut call_exceeds_stage = authorized.clone();
    call_exceeds_stage.stages[0].per_call_limit.amount = "1".into();
    call_exceeds_stage.stages[0].experiment_total.amount = "0.5".into();
    assert!(call_exceeds_stage.validate().is_err());

    let mut sums_exceed_root = authorized.clone();
    sums_exceed_root.stages[0].per_call_limit.amount = "0".into();
    sums_exceed_root.stages[0].experiment_total.amount = "0.6".into();
    sums_exceed_root.stages[1].experiment_total.amount = "0.6".into();
    assert!(sums_exceed_root.validate().is_err());

    let mut noncanonical = authorized;
    noncanonical.root_total.amount = "01".into();
    assert!(noncanonical.validate().is_err());
}

#[test]
fn new_schemas_reject_unknown_fields_and_certificates_never_promote() {
    let mut value = serde_json::to_value(alpha_plan()).unwrap();
    value["candidate_digest"] = json!("must-not-enter-alpha-plan");
    assert!(serde_json::from_value::<ResearchFamilyAlphaPlan>(value).is_err());

    let certificate: EarlyStopCertificate = serde_json::from_value(json!({
        "schema_version": "rsia.early_stop_certificate.v1",
        "ticket_id": "ticket-1",
        "v1_plan_snapshot_digest": d("v1-plan-snapshot"),
        "early_stop_plan_digest": d("early-plan"),
        "alpha_plan_digest": d("alpha-plan"),
        "manifest_digest": d("manifest"),
        "slice_id": "slice-1",
        "candidate_digest": d("candidate"),
        "baseline_digest": d("baseline"),
        "grader_digest": d("grader"),
        "issuer_receipt_digest": d("issuer-receipt"),
        "k": 13,
        "lcb": "-1",
        "ucb": "-0.009",
        "completed_prefix_digest": d("prefix"),
        "member_terminal_digest": d("terminal-members"),
        "stop_reason": "futility_quality_gain",
        "stopped_at_unix_ms": 100,
        "stop_sequence": 1,
        "usage_uncertain": true
    }))
    .unwrap();
    certificate.validate_shape().unwrap();
    assert!(!certificate.is_promotable());

    let critical_at_zero: EarlyStopCertificate = serde_json::from_value(json!({
        "schema_version": "rsia.early_stop_certificate.v1",
        "ticket_id": "ticket-2",
        "v1_plan_snapshot_digest": d("v1-plan-snapshot"),
        "early_stop_plan_digest": d("early-plan"),
        "alpha_plan_digest": d("alpha-plan"),
        "manifest_digest": d("manifest"),
        "slice_id": "slice-1",
        "candidate_digest": d("candidate"),
        "baseline_digest": d("baseline"),
        "grader_digest": d("grader"),
        "issuer_receipt_digest": d("issuer-receipt"),
        "k": 0,
        "lcb": null,
        "ucb": null,
        "completed_prefix_digest": d("empty-prefix"),
        "member_terminal_digest": d("terminal-members"),
        "stop_reason": "critical_regression",
        "stopped_at_unix_ms": 100,
        "stop_sequence": 1,
        "usage_uncertain": false
    }))
    .unwrap();
    critical_at_zero.validate_shape().unwrap();

    let mut stream = SequentialRejectOnly::open(early_plan(2), &alpha_plan()).unwrap();
    stream.record_complete_unit(unit(0, 0, 0)).unwrap();
    let wire = stream.boundary().unwrap().unwrap().to_wire().unwrap();
    let encoded = serde_json::to_string(&wire).unwrap();
    assert!(encoded.contains("\"mean\":\""));
}

#[test]
fn fixed_seed_null_and_degradation_simulation_are_program_checks_only() {
    let mut false_regressions = 0usize;
    for seed in 1..=128u64 {
        let mut state = seed;
        let mut stream = SequentialRejectOnly::open(early_plan(60), &alpha_plan()).unwrap();
        for i in 0..60 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let candidate = if state >> 63 == 0 { 0 } else { 1_000_000 };
            let decision = stream
                .record_complete_unit(unit(i, 500_000, candidate))
                .unwrap();
            if decision == EarlyStopDecision::RegressedSequential {
                false_regressions += 1;
                break;
            }
            if decision.is_terminal() {
                break;
            }
        }
    }
    assert!(
        false_regressions <= 4,
        "false regressions={false_regressions}"
    );

    let mut degraded = SequentialRejectOnly::open(early_plan(60), &alpha_plan()).unwrap();
    let mut terminal = EarlyStopDecision::Collecting;
    for i in 0..60 {
        terminal = degraded
            .record_complete_unit(unit(i, 1_000_000, 0))
            .unwrap();
        if terminal.is_terminal() {
            break;
        }
    }
    assert_eq!(terminal, EarlyStopDecision::FutilityQualityGain);
}
