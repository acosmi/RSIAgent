use evo_core::Error;
use evo_core::evaluation::{ClusterObservation, Verdict, empirical_bernstein};
use evo_core::fingerprint;
use evo_core::hash;
use evo_core::replay_economics::*;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn plan() -> FullCostPlanV1 {
    FullCostPlanV1 {
        component_kinds: CostComponentKind::ALL.into(),
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
    }
}

fn budget_binding() -> EconomicBudgetBindingV1 {
    EconomicBudgetBindingV1 {
        billing_scope: "economic-scope".into(),
        root_budget_id: "economic-root".into(),
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        components: CostComponentKind::ALL
            .into_iter()
            .map(|component| ComponentBillingBindingV1 {
                component,
                source: ComponentBillingSourceV1::AdminMeasurement {
                    source_id: format!("admin-source-{component:?}"),
                },
            })
            .collect(),
    }
}

fn experiment() -> ReplayEconomicExperimentV1 {
    ReplayEconomicExperimentV1 {
        schema_version: EXPERIMENT_SCHEMA.into(),
        id: "economic-exp-1".into(),
        profile_id: "profile-1".into(),
        evaluator_actor: "evaluator".into(),
        executor_actor: "executor".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        primary_claim: EconomicClaim::NoninferiorSavings,
        min_gain_micros: 20_000,
        noninferiority_margin_micros: 10_000,
        preregistration_digest: d("preregistered"),
        paired_ticket_id: "paired-ticket".into(),
        paired_ticket_digest: d("ticket"),
        dataset_epoch: "epoch-1".into(),
        tasks: vec![
            PairedTaskRef {
                ordinal: 1,
                task_id: "task-1".into(),
                task_digest: d("task-1"),
                cluster_id: "cluster-1".into(),
                arm_order: ArmOrder::FixedThenReplaySelected,
            },
            PairedTaskRef {
                ordinal: 2,
                task_id: "task-2".into(),
                task_digest: d("task-2"),
                cluster_id: "cluster-2".into(),
                arm_order: ArmOrder::ReplaySelectedThenFixed,
            },
        ],
        runtime: MatchedRuntimeContract {
            w_online: 1,
            fixed_policy_digest: d("fixed-policy"),
            replay_selected_policy_digest: d("selected-policy"),
            generation_strategy_digest: d("generator"),
            candidate_bundle_digest: d("candidate-bundle"),
            baseline_bundle_digest: d("baseline-bundle"),
            environment_digest: d("environment"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            runner_digest: d("runner"),
            grader_digest: d("grader"),
            guidance_digest: d("guidance"),
            rules_digest: d("rules"),
            context_signature: d("context"),
            target_runtime_profile: "online-w1".into(),
            per_arm_node_budget: 12,
            per_arm_root_budget_micros: 10_000,
        },
        replay_selection: ReplaySelectionRef {
            report_artifact_id: "e10-selection".into(),
            report_digest: d("e10-report"),
            semantic_digest: d("e10-semantic"),
            world_pool_digest: d("world-pool"),
            policy_digest: d("selected-policy"),
            profile_digest: d("replay-profile"),
            caps_digest: d("replay-caps"),
            selection_rule_version: REPLAY_SELECTION_RULE_V1.into(),
            selected_with_w_sim: 1,
            target_w_online: 1,
            simulation_only: vec![SimulationOnlyDiagnosticRef {
                w_sim: 2,
                report_id: "simulation-w2".into(),
                report_digest: d("simulation-w2"),
            }],
        },
        cost_plan: plan(),
        budget_binding: budget_binding(),
        sources: vec![EconomicSourceRefV1 {
            id: "source-1".into(),
            digest: d("source-1"),
        }],
        source_watermark: 1,
    }
}

fn known(component: CostComponentKind, amount: i64, index: usize) -> CostComponentReceiptV1 {
    CostComponentReceiptV1 {
        schema_version: COST_RECEIPT_SCHEMA.into(),
        id: format!("cost-{index}"),
        experiment_id: "economic-exp-1".into(),
        component,
        scope: match component {
            CostComponentKind::OnlineDevelopmentFixed
            | CostComponentKind::OnlineDevelopmentReplaySelected => CostScope::PerTask,
            CostComponentKind::CanaryOperations => CostScope::Operations,
            _ => CostScope::OneTime,
        },
        source_kind: CostSourceKind::AdminMeasurement,
        source_id: format!("measurement-{index}"),
        source_digest: d(&format!("measurement-{index}")),
        billing_scope: None,
        budget_call_id: None,
        amount_micros: Some(amount),
        currency: Some("usd".into()),
        pricing_version: Some("price-v1".into()),
        payment_subject: Some("payer-1".into()),
        tokens: None,
        latency_micros: None,
        storage_bytes: None,
        cpu_nanos: None,
        human_minutes: None,
        measurement_state: MeasurementState::KnownFinal,
        proof_digest: Some(d(&format!("proof-{index}"))),
        reason: None,
        created_seq: index as u64 + 1,
    }
}

fn complete_costs(fixed: i64, replay: i64) -> Vec<CostComponentReceiptV1> {
    CostComponentKind::ALL
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            let amount = match kind {
                CostComponentKind::OnlineDevelopmentFixed => fixed,
                CostComponentKind::OnlineDevelopmentReplaySelected => replay,
                _ => 10,
            };
            known(kind, amount, index)
        })
        .collect()
}

fn paired_units(experiment: &ReplayEconomicExperimentV1) -> Vec<ReplayEconomicUnitReceiptV1> {
    experiment
        .tasks
        .iter()
        .map(|task| ReplayEconomicUnitReceiptV1 {
            schema_version: "rsia.replay_economic_unit_receipt.v1".into(),
            id: format!("unit-{}", task.ordinal),
            experiment_id: experiment.id.clone(),
            ordinal: task.ordinal,
            task_digest: task.task_digest.clone(),
            cluster_id: task.cluster_id.clone(),
            fixed_score_micros: Some(500_000),
            replay_selected_score_micros: Some(500_000),
            critical_capabilities_preserved: Some(true),
            fixed_execution_receipt_id: Some(format!("fixed-execution-{}", task.ordinal)),
            replay_execution_receipt_id: Some(format!("replay-execution-{}", task.ordinal)),
            fixed_latency_micros: Some(u64::from(task.ordinal) * 100),
            replay_latency_micros: Some(u64::from(task.ordinal) * 80),
            budget_call_ids: vec![format!("unit-call-{}", task.ordinal)],
            terminal: PairedUnitTerminal::Complete,
        })
        .collect()
}

#[test]
fn preregistration_is_one_paired_ticket_online_w1_and_simulation_is_separate() {
    let mut registered = experiment();
    registered.validate().unwrap();
    registered.replay_selection.selected_with_w_sim = 2;
    assert!(registered.validate().is_err());
    let mut duplicate_cluster = experiment();
    duplicate_cluster.tasks[1].cluster_id = "cluster-1".into();
    assert!(duplicate_cluster.validate().is_err());
    let mut value = serde_json::to_value(experiment()).unwrap();
    value["second_ticket_id"] = serde_json::json!("forged-ticket");
    assert!(serde_json::from_value::<ReplayEconomicExperimentV1>(value).is_err());
}

#[test]
fn full_costs_compute_checked_net_and_break_even_while_unknown_is_never_zero() {
    let costs = complete_costs(200, 100);
    let result = compute_economics(&plan(), &costs, 2, 10).unwrap();
    assert_eq!(
        result,
        EconomicComputation::Complete {
            one_time_increment_micros: 70,
            fixed_arm_total_micros: 200,
            replay_arm_total_micros: 100,
            marginal_saving_per_task_micros: 50,
            projected_tasks: 10,
            net_saving_micros: 430,
            break_even_tasks: Some(2),
        }
    );

    let mut uncertain = costs.clone();
    uncertain[0].measurement_state = MeasurementState::UsageUncertain;
    uncertain[0].amount_micros = None;
    uncertain[0].currency = None;
    uncertain[0].pricing_version = None;
    uncertain[0].payment_subject = None;
    uncertain[0].reason = Some("final bill is not closed".into());
    assert!(matches!(
        compute_economics(&plan(), &uncertain, 2, 10).unwrap(),
        EconomicComputation::UsageUncertain { .. }
    ));

    let mut calls_only = costs.clone();
    calls_only[1].measurement_state = MeasurementState::KnownNonmonetary;
    calls_only[1].amount_micros = None;
    calls_only[1].currency = None;
    calls_only[1].pricing_version = None;
    calls_only[1].payment_subject = None;
    calls_only[1].tokens = Some(10);
    assert!(matches!(
        compute_economics(&plan(), &calls_only, 2, 10).unwrap(),
        EconomicComputation::Blocked { .. }
    ));
}

#[test]
fn nonpositive_marginal_saving_has_no_break_even_and_pricing_mismatch_blocks() {
    let costs = complete_costs(100, 200);
    let EconomicComputation::Complete {
        break_even_tasks,
        marginal_saving_per_task_micros,
        ..
    } = compute_economics(&plan(), &costs, 2, 10).unwrap()
    else {
        panic!("expected complete accounting")
    };
    assert_eq!(marginal_saving_per_task_micros, -50);
    assert_eq!(break_even_tasks, None);

    let mut mismatch = complete_costs(200, 100);
    mismatch[0].currency = Some("eur".into());
    assert!(matches!(
        compute_economics(&plan(), &mismatch, 2, 10).unwrap(),
        EconomicComputation::Blocked { .. }
    ));
}

#[test]
fn zero_negative_and_positive_terminals_cannot_be_promoted_or_overstated() {
    let experiment = experiment();
    let economics = compute_economics(&plan(), &complete_costs(200, 100), 2, 10).unwrap();
    let units = paired_units(&experiment);
    let latency = summarize_unit_latencies(&units).unwrap();
    assert_eq!(latency.fixed_p50_micros, 100);
    assert_eq!(latency.fixed_p95_micros, 200);
    assert_eq!(latency.replay_selected_p50_micros, 80);
    assert_eq!(latency.replay_selected_p95_micros, 160);
    for (quality, terminal) in [
        (
            PairedQualityOutcome::ZeroOrInconclusive,
            EconomicReportTerminal::CompleteZeroOrInconclusive,
        ),
        (
            PairedQualityOutcome::Negative,
            EconomicReportTerminal::CompleteNegative,
        ),
    ] {
        let mut report_units = units.clone();
        if quality == PairedQualityOutcome::Negative {
            for unit in &mut report_units {
                unit.replay_selected_score_micros = Some(400_000);
            }
        }
        let report = ReplayEconomicReportV1 {
            schema_version: REPORT_SCHEMA.into(),
            id: format!("report-{terminal:?}"),
            experiment_id: experiment.id.clone(),
            experiment_digest: experiment.digest().unwrap(),
            paired_ticket_id: experiment.paired_ticket_id.clone(),
            paired_ticket_digest: experiment.paired_ticket_digest.clone(),
            paired_receipt_closure_digest: Some(d("paired-receipts")),
            completed_paired_tasks: 2,
            paired_units: report_units,
            latency: Some(latency.clone()),
            quality_outcome: quality,
            cost_receipt_ids: complete_costs(200, 100)
                .into_iter()
                .map(|receipt| receipt.id)
                .collect(),
            economics: economics.clone(),
            terminal,
            reasons: vec![],
        };
        report.validate_against(&experiment).unwrap();
        assert!(economic_report_is_not_formal(&report).is_err());
    }
    let mut overstated = ReplayEconomicReportV1 {
        schema_version: REPORT_SCHEMA.into(),
        id: "overstated".into(),
        experiment_id: experiment.id.clone(),
        experiment_digest: experiment.digest().unwrap(),
        paired_ticket_id: experiment.paired_ticket_id.clone(),
        paired_ticket_digest: experiment.paired_ticket_digest.clone(),
        paired_receipt_closure_digest: None,
        completed_paired_tasks: 1,
        paired_units: vec![],
        latency: None,
        quality_outcome: PairedQualityOutcome::PassedGain,
        cost_receipt_ids: vec![],
        economics,
        terminal: EconomicReportTerminal::CompletePositive,
        reasons: vec![],
    };
    assert!(overstated.validate_against(&experiment).is_err());
    overstated.terminal = EconomicReportTerminal::BlockedSupport;
    overstated.completed_paired_tasks = 0;
    overstated.quality_outcome = PairedQualityOutcome::ZeroOrInconclusive;
    overstated.economics = EconomicComputation::Blocked {
        reasons: vec!["paired receipts unavailable".into()],
    };
    overstated.validate_against(&experiment).unwrap();
    assert_ne!(fingerprint(&overstated).unwrap(), d("formal"));
}

// ---------------------------------------------------------------------------
// AG-178: report v2, cluster aggregation and the empirical-Bernstein bounds
// ---------------------------------------------------------------------------

/// `experiment()` with `count` paired tasks, one independent cluster each, and the
/// given primary claim.
fn experiment_with_tasks(count: u32, claim: EconomicClaim) -> ReplayEconomicExperimentV1 {
    let mut registered = experiment();
    registered.primary_claim = claim;
    registered.tasks = (1..=count)
        .map(|ordinal| PairedTaskRef {
            ordinal,
            task_id: format!("task-{ordinal}"),
            task_digest: d(&format!("task-{ordinal}")),
            cluster_id: format!("cluster-{ordinal}"),
            arm_order: if ordinal % 2 == 1 {
                ArmOrder::FixedThenReplaySelected
            } else {
                ArmOrder::ReplaySelectedThenFixed
            },
        })
        .collect();
    registered.validate().unwrap();
    registered
}

/// The complete paired units of `experiment`; the replay-selected score of unit `i`
/// differs from the fixed score (500_000 micros) by `deltas[i]`.
fn units_with_deltas(
    experiment: &ReplayEconomicExperimentV1,
    deltas: &[i64],
) -> Vec<ReplayEconomicUnitReceiptV1> {
    let mut units = paired_units(experiment);
    assert_eq!(units.len(), deltas.len());
    for (unit, delta) in units.iter_mut().zip(deltas) {
        unit.replay_selected_score_micros = Some(u32::try_from(500_000 + delta).unwrap());
    }
    units
}

/// A complete v2 report over `units` that claims `quality_outcome` and `terminal`;
/// its cluster rows and statistics are the ones the units give.
fn v2_report(
    experiment: &ReplayEconomicExperimentV1,
    units: Vec<ReplayEconomicUnitReceiptV1>,
    quality_outcome: PairedQualityOutcome,
    terminal: EconomicReportTerminal,
) -> ReplayEconomicReportV2 {
    let (cluster_rows, quality_statistics) = e11_report_quality_v2(&units).unwrap();
    let latency = (!units.is_empty()).then(|| summarize_unit_latencies(&units).unwrap());
    ReplayEconomicReportV2 {
        schema_version: REPORT_SCHEMA_V2.into(),
        id: format!("report-v2-{terminal:?}"),
        experiment_id: experiment.id.clone(),
        experiment_digest: experiment.digest().unwrap(),
        paired_ticket_id: experiment.paired_ticket_id.clone(),
        paired_ticket_digest: experiment.paired_ticket_digest.clone(),
        paired_receipt_closure_digest: Some(d("paired-receipts")),
        completed_paired_tasks: u32::try_from(units.len()).unwrap(),
        paired_units: units,
        cluster_rows,
        quality_statistics,
        latency,
        quality_outcome,
        cost_receipt_ids: complete_costs(200, 100)
            .into_iter()
            .map(|receipt| receipt.id)
            .collect(),
        economics: compute_economics(&plan(), &complete_costs(200, 100), 2, 10).unwrap(),
        terminal,
        reasons: vec![],
    }
}

fn row(cluster_id: &str, unit_count: u32, mean_delta_micros: i64) -> EconomicClusterRowV2 {
    EconomicClusterRowV2 {
        cluster_id: cluster_id.into(),
        unit_count,
        mean_delta_micros,
        weight: EQUAL_CLUSTER_WEIGHT,
    }
}

fn conflict(result: evo_core::Result<()>) -> String {
    match result {
        Err(Error::Conflict(reason)) => reason,
        other => panic!("expected a Conflict, got {other:?}"),
    }
}

/// The v4.5 §19.5 defect: two clusters whose per-task mean (+150_000 micros) is far
/// above `min_gain_micros` (20_000) but whose bound proves nothing. The v1 judge,
/// kept for static reads of old records, accepts the gain; v2 refuses it and every
/// other passing claim, and accepts only what the bound supports.
#[test]
fn the_cluster_lcb_not_the_per_task_mean_judges_paired_quality() {
    let experiment = experiment_with_tasks(2, EconomicClaim::QualityGain);
    let units = units_with_deltas(&experiment, &[400_000, -100_000]);
    let gain = v2_report(
        &experiment,
        units.clone(),
        PairedQualityOutcome::PassedGain,
        EconomicReportTerminal::CompletePositive,
    );
    let statistics = &gain.quality_statistics;
    assert_eq!(statistics.stats_version, "rsia.empirical_bernstein.v2");
    assert_eq!(statistics.verdict, Verdict::Inconclusive);
    assert_eq!(statistics.n, 2);
    assert_eq!(statistics.alpha_i, "0.05");
    assert!((statistics.mean.parse::<f64>().unwrap() - 0.15).abs() < 1e-12);
    assert_eq!(statistics.lcb_micros, Some(-1_000_000));
    assert_eq!(statistics.ucb_micros, Some(1_000_000));
    assert_eq!(
        conflict(gain.validate_against(&experiment)),
        "quality-gain outcome differs from the cluster LCB or critical gates"
    );
    v2_report(
        &experiment,
        units.clone(),
        PairedQualityOutcome::ZeroOrInconclusive,
        EconomicReportTerminal::CompleteZeroOrInconclusive,
    )
    .validate_against(&experiment)
    .unwrap();

    // The same evidence in a v1 report: its per-task mean passes the gain.
    let legacy = ReplayEconomicReportV1 {
        schema_version: REPORT_SCHEMA.into(),
        id: "report-v1-mean".into(),
        experiment_id: gain.experiment_id.clone(),
        experiment_digest: gain.experiment_digest.clone(),
        paired_ticket_id: gain.paired_ticket_id.clone(),
        paired_ticket_digest: gain.paired_ticket_digest.clone(),
        paired_receipt_closure_digest: gain.paired_receipt_closure_digest.clone(),
        completed_paired_tasks: 2,
        paired_units: units.clone(),
        latency: gain.latency.clone(),
        quality_outcome: PairedQualityOutcome::PassedGain,
        cost_receipt_ids: gain.cost_receipt_ids.clone(),
        economics: gain.economics.clone(),
        terminal: EconomicReportTerminal::CompletePositive,
        reasons: vec![],
    };
    legacy.validate_against(&experiment).unwrap();

    // Noninferiority is judged by the same lower bound.
    let savings = experiment_with_tasks(2, EconomicClaim::NoninferiorSavings);
    let units = units_with_deltas(&savings, &[400_000, -100_000]);
    assert_eq!(
        conflict(
            v2_report(
                &savings,
                units.clone(),
                PairedQualityOutcome::PassedNoninferior,
                EconomicReportTerminal::CompletePositive,
            )
            .validate_against(&savings)
        ),
        "noninferiority outcome differs from the cluster LCB or critical gates"
    );
    // A negative mean is no regression either while the upper bound is above the
    // margin (plan §3.4 Q-57: only UCB < -margin is a regression).
    let units = units_with_deltas(&savings, &[-100_000, -100_000]);
    assert_eq!(
        conflict(
            v2_report(
                &savings,
                units,
                PairedQualityOutcome::Negative,
                EconomicReportTerminal::CompleteNegative,
            )
            .validate_against(&savings)
        ),
        "negative outcome differs from the cluster UCB and critical gates"
    );
}

/// With enough independent clusters the bound reaches the thresholds: the claims
/// the LCB (or UCB) supports hold, and the report records the values it used.
#[test]
fn a_cluster_bound_that_reaches_the_threshold_supports_the_claim() {
    let gain = experiment_with_tasks(400, EconomicClaim::QualityGain);
    let report = v2_report(
        &gain,
        units_with_deltas(&gain, &[100_000; 400]),
        PairedQualityOutcome::PassedGain,
        EconomicReportTerminal::CompletePositive,
    );
    report.validate_against(&gain).unwrap();
    let observations: Vec<ClusterObservation> = report
        .cluster_rows
        .iter()
        .map(|row| ClusterObservation {
            cluster_id: row.cluster_id.clone(),
            d: 0.1,
            weight: 1.0,
        })
        .collect();
    let raw = empirical_bernstein(&observations, E11_REPORT_ALPHA_I).unwrap();
    let lcb_micros = report.quality_statistics.lcb_micros.unwrap();
    assert_eq!(lcb_micros, (raw.lcb * 1_000_000.0).floor() as i64);
    assert!(
        lcb_micros >= i64::from(gain.min_gain_micros),
        "{lcb_micros}"
    );
    assert!((lcb_micros as f64) < 0.1 * 1_000_000.0);
    assert_eq!(report.quality_statistics.n, 400);
    assert_eq!(report.cluster_rows.len(), 400);
    assert!(report.cluster_rows.iter().all(|row| row.unit_count == 1
        && row.mean_delta_micros == 100_000
        && row.weight == EQUAL_CLUSTER_WEIGHT));

    // A capability lost in one unit: no gain, whatever the bound; a negative
    // outcome holds instead.
    let mut lost = units_with_deltas(&gain, &[100_000; 400]);
    lost[7].critical_capabilities_preserved = Some(false);
    let lost_gain = v2_report(
        &gain,
        lost.clone(),
        PairedQualityOutcome::PassedGain,
        EconomicReportTerminal::CompletePositive,
    );
    assert_eq!(
        conflict(lost_gain.validate_against(&gain)),
        "quality-gain outcome differs from the cluster LCB or critical gates"
    );
    v2_report(
        &gain,
        lost,
        PairedQualityOutcome::Negative,
        EconomicReportTerminal::CompleteNegative,
    )
    .validate_against(&gain)
    .unwrap();

    let savings = experiment_with_tasks(400, EconomicClaim::NoninferiorSavings);
    let noninferior = v2_report(
        &savings,
        units_with_deltas(&savings, &[50_000; 400]),
        PairedQualityOutcome::PassedNoninferior,
        EconomicReportTerminal::CompletePositive,
    );
    noninferior.validate_against(&savings).unwrap();
    assert!(
        noninferior.quality_statistics.lcb_micros.unwrap()
            >= -i64::from(savings.noninferiority_margin_micros)
    );
    let negative = v2_report(
        &savings,
        units_with_deltas(&savings, &[-100_000; 400]),
        PairedQualityOutcome::Negative,
        EconomicReportTerminal::CompleteNegative,
    );
    negative.validate_against(&savings).unwrap();
    assert!(
        negative.quality_statistics.ucb_micros.unwrap()
            < -i64::from(savings.noninferiority_margin_micros)
    );

    // The recorded rows and statistics are recomputed, not trusted.
    let mut tampered = report.clone();
    tampered.quality_statistics.lcb_micros = Some(lcb_micros + 1);
    assert_eq!(
        conflict(tampered.validate_against(&gain)),
        "economic report cluster statistics differ from paired units"
    );
    let mut tampered = report.clone();
    tampered.quality_statistics.alpha_i = "0.1".into();
    assert_eq!(
        conflict(tampered.validate_against(&gain)),
        "economic report cluster statistics differ from paired units"
    );
    let mut tampered = report;
    tampered.cluster_rows[0].mean_delta_micros += 1;
    assert_eq!(
        conflict(tampered.validate_against(&gain)),
        "economic report cluster statistics differ from paired units"
    );
}

/// Fewer than two clusters, or a normalized cluster mean outside [-1, 1], leaves no
/// bound: the report records the invalid summary and no passing claim holds.
#[test]
fn fewer_than_two_clusters_or_an_out_of_range_cluster_is_invalid_and_never_passes() {
    let single = experiment_with_tasks(1, EconomicClaim::QualityGain);
    let units = units_with_deltas(&single, &[500_000]);
    let gain = v2_report(
        &single,
        units.clone(),
        PairedQualityOutcome::PassedGain,
        EconomicReportTerminal::CompletePositive,
    );
    let statistics = &gain.quality_statistics;
    assert_eq!(statistics.verdict, Verdict::Invalid);
    assert_eq!(statistics.reasons, ["n<2"]);
    assert_eq!(statistics.lcb_micros, None);
    assert_eq!(statistics.ucb_micros, None);
    assert_eq!(statistics.alpha_i, "0.05");
    assert_eq!(gain.cluster_rows, [row("cluster-1", 1, 500_000)]);
    assert_eq!(
        conflict(gain.validate_against(&single)),
        "quality-gain outcome differs from the cluster LCB or critical gates"
    );
    for (quality, terminal) in [
        (
            PairedQualityOutcome::Invalid,
            EconomicReportTerminal::Invalid,
        ),
        (
            PairedQualityOutcome::ZeroOrInconclusive,
            EconomicReportTerminal::CompleteZeroOrInconclusive,
        ),
    ] {
        v2_report(&single, units.clone(), quality, terminal)
            .validate_against(&single)
            .unwrap();
    }
    // An empty report (the blocked report) records the same invalid summary.
    let (rows, empty) = e11_report_quality_v2(&[]).unwrap();
    assert!(rows.is_empty());
    assert_eq!(empty.verdict, Verdict::Invalid);
    assert_eq!(empty.n, 0);
    assert_eq!(empty.reasons, ["n<2"]);
    assert_eq!((empty.lcb_micros, empty.ucb_micros), (None, None));

    // Normalization: d = micros / 1e6, so +-1_000_000 is the edge of the domain.
    let outside = economic_quality_statistics_v2(
        &[row("a", 1, 1_000_001), row("b", 1, 0)],
        E11_REPORT_ALPHA_I,
    )
    .unwrap();
    assert_eq!(outside.verdict, Verdict::Invalid);
    assert_eq!(
        outside.reasons,
        ["non-finite or out-of-range cluster observation"]
    );
    assert_eq!((outside.lcb_micros, outside.ucb_micros), (None, None));
    let edge = economic_quality_statistics_v2(
        &[row("a", 1, 1_000_000), row("b", 1, -1_000_000)],
        E11_REPORT_ALPHA_I,
    )
    .unwrap();
    assert_eq!(edge.verdict, Verdict::Inconclusive);
    assert_eq!(edge.lcb_micros, Some(-1_000_000));
    // A row without units or with another weight is refused outright.
    for broken in [
        EconomicClusterRowV2 {
            weight: 2,
            ..row("a", 1, 0)
        },
        row("a", 0, 0),
    ] {
        assert!(
            economic_quality_statistics_v2(&[broken, row("b", 1, 0)], E11_REPORT_ALPHA_I).is_err()
        );
    }
}

/// Units are aggregated per cluster first (a cluster mean, floored), each cluster
/// mean is normalized to d = micros / 1e6, and the bounds are kept in micros
/// rounded away from a claim: floor for the LCB, ceil for the UCB.
#[test]
fn clusters_aggregate_first_and_bounds_round_away_from_the_claim() {
    let base = paired_units(&experiment())[0].clone();
    let unit = |cluster: &str, delta: i64| ReplayEconomicUnitReceiptV1 {
        cluster_id: cluster.into(),
        replay_selected_score_micros: Some(u32::try_from(500_000 + delta).unwrap()),
        ..base.clone()
    };
    let rows = economic_cluster_rows_v2(&[
        unit("b", -3),
        unit("a", 3),
        unit("c", 5),
        unit("b", -4),
        unit("a", 4),
    ])
    .unwrap();
    assert_eq!(rows, [row("a", 2, 3), row("b", 2, -4), row("c", 1, 5)]);
    let mut unscored = unit("a", 0);
    unscored.fixed_score_micros = None;
    assert!(economic_cluster_rows_v2(&[unscored]).is_err());

    let rows: Vec<_> = (1..=30)
        .map(|index| {
            row(
                &format!("cluster-{index:02}"),
                1,
                270_000 + (index % 7) * 10_000,
            )
        })
        .collect();
    let statistics = economic_quality_statistics_v2(&rows, E11_REPORT_ALPHA_I).unwrap();
    let observations: Vec<ClusterObservation> = rows
        .iter()
        .map(|row| ClusterObservation {
            cluster_id: row.cluster_id.clone(),
            d: row.mean_delta_micros as f64 / 1_000_000.0,
            weight: 1.0,
        })
        .collect();
    let raw = empirical_bernstein(&observations, E11_REPORT_ALPHA_I).unwrap();
    let lcb = raw.lcb * 1_000_000.0;
    let ucb = (raw.mean + raw.radius).min(1.0) * 1_000_000.0;
    assert!(lcb > -1_000_000.0 && ucb < 1_000_000.0, "{lcb} {ucb}");
    assert!(lcb.fract() != 0.0 && ucb.fract() != 0.0, "{lcb} {ucb}");
    let (lcb_micros, ucb_micros) = (
        statistics.lcb_micros.unwrap(),
        statistics.ucb_micros.unwrap(),
    );
    assert_eq!(lcb_micros, lcb.floor() as i64);
    assert_eq!(ucb_micros, ucb.ceil() as i64);
    assert!((lcb_micros as f64) < lcb && (ucb_micros as f64) > ucb);
    assert_eq!(statistics.n, 30);
    assert_eq!(statistics.verdict, Verdict::Inconclusive);
    assert!(statistics.reasons.is_empty());
    assert_eq!(statistics.alpha_i, "0.05");
    for (text, value) in [
        (&statistics.mean, raw.mean),
        (&statistics.variance, raw.variance),
        (&statistics.radius, raw.radius),
        (&statistics.lcb, raw.lcb),
    ] {
        assert!(
            !text.contains(['e', 'E']) && text.parse::<f64>().unwrap().is_finite(),
            "{text}"
        );
        assert!(
            (text.parse::<f64>().unwrap() - value).abs() < 1e-12,
            "{text}"
        );
    }
}

/// The version takes part in the id: one input gets a v1 id and a different v2 id,
/// and either id derives both (plan §5.1.1 items 1 and 5). The closed table keeps the
/// v1 judge for static reads and marks only v2 current.
#[test]
fn report_versions_get_distinct_ids_and_a_derivable_lineage() {
    let input = d("job-snapshot");
    let v1 = replay_economic_report_id(REPORT_SCHEMA, &input).unwrap();
    let v2 = replay_economic_report_id(REPORT_SCHEMA_V2, &input).unwrap();
    assert_eq!(v1, format!("replay-economic-report-{}", &input[..24]));
    assert_eq!(v2, format!("replay-economic-report-v2-{}", &input[..24]));
    assert_ne!(v1, v2);
    let lineage = vec![
        EconomicReportLineageId {
            schema_version: REPORT_SCHEMA.into(),
            report_id: v1.clone(),
        },
        EconomicReportLineageId {
            schema_version: REPORT_SCHEMA_V2.into(),
            report_id: v2.clone(),
        },
    ];
    assert_eq!(replay_economic_report_lineage(&v1), Some(lineage.clone()));
    assert_eq!(replay_economic_report_lineage(&v2), Some(lineage));
    for underived in [
        "report-v2-x",
        "replay-economic-report-v2-short",
        "replay-economic-report-",
    ] {
        assert_eq!(
            replay_economic_report_lineage(underived),
            None,
            "{underived}"
        );
    }
    assert!(replay_economic_report_id("rsia.replay_economic_report.v3", &input).is_err());
    assert!(replay_economic_report_id(REPORT_SCHEMA_V2, "not-a-digest").is_err());

    let legacy = economic_report_version(REPORT_SCHEMA).unwrap();
    assert!(legacy.retains_judge && !legacy.current);
    let current = economic_report_version(REPORT_SCHEMA_V2).unwrap();
    assert!(current.retains_judge && current.current);
    assert_eq!(ECONOMIC_REPORT_VERSIONS.len(), 2);
    assert!(economic_report_version("rsia.replay_economic_report.v3").is_err());

    // A v1 body is not a v2 report, and a v2 report under the v1 schema is refused.
    let experiment = experiment();
    let mut report = v2_report(
        &experiment,
        vec![],
        PairedQualityOutcome::ZeroOrInconclusive,
        EconomicReportTerminal::BlockedSupport,
    );
    report.paired_receipt_closure_digest = None;
    report.economics = EconomicComputation::Blocked {
        reasons: vec!["no_cost_receipts".into()],
    };
    report.validate_against(&experiment).unwrap();
    let mut body = serde_json::to_value(&report).unwrap();
    body.as_object_mut().unwrap().remove("cluster_rows");
    body.as_object_mut().unwrap().remove("quality_statistics");
    body["schema_version"] = serde_json::json!(REPORT_SCHEMA);
    let as_v1: ReplayEconomicReportV1 = serde_json::from_value(body.clone()).unwrap();
    as_v1.validate_against(&experiment).unwrap();
    assert!(serde_json::from_value::<ReplayEconomicReportV2>(body).is_err());
    let mut relabelled = report;
    relabelled.schema_version = REPORT_SCHEMA.into();
    assert_eq!(
        conflict(relabelled.validate_against(&experiment)),
        "economic report differs from preregistered paired experiment"
    );
    assert!(economic_report_is_not_formal(&relabelled).is_err());
}
