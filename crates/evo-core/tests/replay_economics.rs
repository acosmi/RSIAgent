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
