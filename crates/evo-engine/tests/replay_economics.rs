use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::replay::*;
use evo_core::replay_economics::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::replay::{LiveWorldAuthority, run_replay};
use evo_engine::replay_experiment::{PersistentReplayEconomicCoordinator, ReplayEconomicJobState};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use std::collections::BTreeSet;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn economic_source_body() -> serde_json::Value {
    serde_json::json!({"schema_version":"fixture.source.v1","id":"source-1"})
}

fn budget_binding() -> EconomicBudgetBindingV1 {
    let bindings = [
        (
            CostComponentKind::HistoryCollection,
            "group-history",
            EconomicBudgetStage::HistoryCollection,
        ),
        (
            CostComponentKind::PolicyGeneration,
            "group-policy",
            EconomicBudgetStage::CandidateGeneration,
        ),
        (
            CostComponentKind::ReplayCpu,
            "group-replay-cpu",
            EconomicBudgetStage::StorageCpu,
        ),
        (
            CostComponentKind::ReplayStorage,
            "group-replay-storage",
            EconomicBudgetStage::StorageCpu,
        ),
        (
            CostComponentKind::OnlineDevelopmentFixed,
            "group-fixed",
            EconomicBudgetStage::DevelopmentExecution,
        ),
        (
            CostComponentKind::OnlineDevelopmentReplaySelected,
            "group-replay",
            EconomicBudgetStage::DevelopmentExecution,
        ),
        (
            CostComponentKind::IndependentAcceptance,
            "group-acceptance",
            EconomicBudgetStage::FormalEvaluation,
        ),
        (
            CostComponentKind::CanaryOperations,
            "group-canary",
            EconomicBudgetStage::GrayOperations,
        ),
    ]
    .into_iter()
    .map(|(component, group, stage)| ComponentBillingBindingV1 {
        component,
        source: ComponentBillingSourceV1::BudgetCall {
            dispatch_group_id: group.into(),
            stage,
        },
    });
    EconomicBudgetBindingV1 {
        billing_scope: "economic-scope".into(),
        root_budget_id: "root-1".into(),
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        components: bindings
            .chain(std::iter::once(ComponentBillingBindingV1 {
                component: CostComponentKind::HumanReview,
                source: ComponentBillingSourceV1::AdminMeasurement {
                    source_id: "admin-review-receipt".into(),
                },
            }))
            .collect(),
    }
}

fn world(id: &str, cluster: &str, source_id: &str, partition: WorldPartition) -> ReplayWorldV2 {
    let generation = d("generation");
    let baseline = d("baseline-context");
    let next = d(&format!("next-{id}"));
    let action = ReplayActionSpecV1 {
        record_seq: 1,
        generation_signature: generation.clone(),
        parent_context_signature: baseline.clone(),
        branch_seq: 1,
        target_depth: 1,
        action_kind: ActionKindV1::Widen { root_slot: 1 },
        estimated_cost_upper_micros: Some(1),
        writes_shared_workspace: false,
    };
    let transition = ReplayTransitionV2 {
        record_id: format!("transition-{id}"),
        record_seq: 1,
        generation_signature: generation.clone(),
        parent_context_signature: baseline.clone(),
        action_kind: action.action_kind.clone(),
        next_context_signature: next.clone(),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: 600_000,
            },
        },
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_micros: Some(1),
            latency_millis: Some(1),
        },
        source_ids: vec![source_id.into()],
        observation_source_id: source_id.into(),
    };
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: id.into(),
            cluster_id: cluster.into(),
            partition,
            purpose: Purpose::Development,
            generation_signature: generation,
            world_context_signature: d("world-context"),
            baseline_context_signature: baseline,
            approved_parent_digest: d("approved-parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 500_000,
            baseline_observation_source_id: format!("baseline-{source_id}"),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: source_id.into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: format!("baseline-{source_id}"),
                    content_digest: String::new(),
                },
            ],
            revoke_watermark: 1,
            prefix_coverage: vec![
                PrefixCoverageV1 {
                    context_signature: d("baseline-context"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: next,
                    exhausted: true,
                },
            ],
            action_catalog: vec![action],
        },
        transitions: vec![transition],
        sealed_digest: None,
    };
    world.manifest.source_closure[1].content_digest =
        replay_baseline_observation_digest(&world.manifest).unwrap();
    world.manifest.source_closure[0].content_digest =
        replay_observation_digest(&world.manifest, &world.transitions[0]).unwrap();
    world.seal().unwrap();
    world
}

fn experiment(report: &StoredReplayReportV1) -> ReplayEconomicExperimentV1 {
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
        paired_ticket_digest: d("paired-ticket"),
        dataset_epoch: "epoch-1".into(),
        tasks: vec![PairedTaskRef {
            ordinal: 1,
            task_id: "task-1".into(),
            task_digest: d("task-1"),
            cluster_id: "cluster-1".into(),
            arm_order: ArmOrder::FixedThenReplaySelected,
        }],
        runtime: MatchedRuntimeContract {
            w_online: 1,
            fixed_policy_digest: d("fixed-policy"),
            replay_selected_policy_digest: report.policy_digest.clone(),
            generation_strategy_digest: d("generation"),
            candidate_bundle_digest: d("candidate-bundle"),
            baseline_bundle_digest: d("baseline-bundle"),
            environment_digest: d("environment"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            runner_digest: d("runner"),
            grader_digest: d("scorer"),
            guidance_digest: d("guidance"),
            rules_digest: d("rules"),
            context_signature: d("context"),
            target_runtime_profile: "online-w1".into(),
            per_arm_node_budget: 12,
            per_arm_root_budget_micros: 1_000,
        },
        replay_selection: ReplaySelectionRef {
            report_artifact_id: report.report_id.clone(),
            report_digest: fingerprint(report).unwrap(),
            semantic_digest: report.semantic_reports_digest.clone(),
            world_pool_digest: report.pool_digest.clone(),
            policy_digest: report.policy_digest.clone(),
            profile_digest: report.profile_digest.clone(),
            caps_digest: report.caps_digest.clone(),
            selection_rule_version: REPLAY_SELECTION_RULE_V1.into(),
            selected_with_w_sim: 1,
            target_w_online: 1,
            simulation_only: vec![SimulationOnlyDiagnosticRef {
                w_sim: 2,
                report_id: "simulation-w2".into(),
                report_digest: d("simulation-w2"),
            }],
        },
        cost_plan: FullCostPlanV1 {
            component_kinds: CostComponentKind::ALL.into(),
            currency: "usd".into(),
            pricing_version: "price-v1".into(),
            payment_subject: "payer-1".into(),
        },
        budget_binding: budget_binding(),
        sources: vec![EconomicSourceRefV1 {
            id: "source-1".into(),
            digest: fingerprint(&economic_source_body()).unwrap(),
        }],
        source_watermark: 1,
    }
}

async fn setup() -> (
    tempfile::TempDir,
    Store,
    Context,
    Context,
    ReplayEconomicExperimentV1,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("economic.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let train = world(
        "world-train",
        "cluster-train",
        "run-train",
        WorldPartition::Train,
    );
    let select = world(
        "world-select",
        "cluster-select",
        "run-select",
        WorldPartition::Select,
    );
    for (id, cluster, replay_world) in [
        ("run-train", "family-train", &train),
        ("run-select", "family-select", &select),
    ] {
        let body =
            replay_observation_bytes(&replay_world.manifest, &replay_world.transitions[0]).unwrap();
        store_trace_authority(
            &store,
            &Context::new("n", "host", Role::Host).unwrap(),
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: body.clone(),
                    parent_family: cluster.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: OptimizationTrace {
                    run_id: id.into(),
                    parent_family: cluster.into(),
                    source_digest: hash(&body),
                    purpose: Purpose::Development,
                    outcome: TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: String::from_utf8(body.clone()).unwrap(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: body.len(),
            },
        )
        .await
        .unwrap();
        let baseline_id = replay_world.manifest.baseline_observation_source_id.clone();
        let baseline_body = replay_baseline_observation_bytes(&replay_world.manifest).unwrap();
        store_trace_authority(
            &store,
            &Context::new("n", "host", Role::Host).unwrap(),
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: baseline_id.clone(),
                    body: baseline_body.clone(),
                    parent_family: cluster.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: OptimizationTrace {
                    run_id: baseline_id,
                    parent_family: cluster.into(),
                    source_digest: hash(&baseline_body),
                    purpose: Purpose::Development,
                    outcome: TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: String::from_utf8(baseline_body.clone()).unwrap(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: baseline_body.len(),
            },
        )
        .await
        .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "source-1",
            admin.actor(),
            &economic_source_body(),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "economic-initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    for world in [&train, &select] {
        let mut draft = world.clone();
        draft.sealed_digest = None;
        evo_storage::replay::put_replay_world_draft(&admin, &store, &draft)
            .await
            .unwrap();
        evo_storage::replay::seal_replay_world(&admin, &store, world)
            .await
            .unwrap();
    }
    let pool = evo_storage::replay::register_replay_pool(
        &admin,
        &store,
        &[
            train.manifest.world_id.clone(),
            select.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap();
    let policy = ElasticPolicyV1::default();
    let caps = ExplorationCapsV1::online();
    let profile = ReplaySimulationProfile {
        simulation_version: SIMULATION_VERSION.into(),
        objective: ReplayObjective::ParetoAttainmentV2,
        w_sim: 1,
        probe_budget: 1,
        horizon: 1,
        lambda_work_micros: DEFAULT_LAMBDA_MICROS,
        lambda_round_micros: DEFAULT_LAMBDA_MICROS,
        fixed_seed: 7,
        global_recovery_dispatch_limit: 1,
        pool_digest: pool.pool_digest.clone(),
        purpose: Purpose::Development,
        target_runtime_profile: "online-w1".into(),
    };
    let replay = run_replay(
        &select,
        &policy,
        &profile,
        &caps,
        &LiveWorldAuthority {
            revoke_watermark: 1,
            revoked_source_ids: BTreeSet::new(),
        },
    )
    .unwrap();
    let stored_report = StoredReplayReportV1::build(
        "n",
        WorldPartition::Select,
        &pool,
        policy,
        profile,
        caps,
        vec![replay],
    )
    .unwrap();
    evo_storage::replay::put_replay_report(&admin, &store, &stored_report)
        .await
        .unwrap();
    let experiment = experiment(&stored_report);
    PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment.clone())
        .await
        .unwrap();
    (directory, store, admin, evaluator, experiment)
}

#[tokio::test]
async fn registration_start_restart_blocked_report_and_cancel_are_persistent() {
    let (directory, store, admin, evaluator, experiment) = setup().await;
    let digest =
        PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment.clone())
            .await
            .unwrap();
    assert_eq!(digest, experiment.digest().unwrap());
    let job = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        "start-key-1",
        1,
    )
    .await
    .unwrap();
    assert_eq!(job.state, ReplayEconomicJobState::BlockedSupport);
    assert!(
        job.blocked_reasons
            .iter()
            .any(|reason| reason.contains("e05"))
    );
    let same = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        "start-key-1",
        1,
    )
    .await
    .unwrap();
    assert_eq!(same.id, job.id);
    assert!(
        PersistentReplayEconomicCoordinator::start(
            &evaluator,
            &store,
            &experiment.id,
            "start-key-1",
            2,
        )
        .await
        .is_err()
    );
    let report =
        PersistentReplayEconomicCoordinator::build_blocked_report(&evaluator, &store, &job.id, 10)
            .await
            .unwrap();
    assert_eq!(report.terminal, EconomicReportTerminal::BlockedSupport);
    assert!(economic_report_is_not_formal(&report).is_err());
    store.close().await;

    let reopened = Store::open(&directory.path().join("economic.sqlite3"))
        .await
        .unwrap();
    let status = PersistentReplayEconomicCoordinator::status(&evaluator, &reopened, &job.id)
        .await
        .unwrap();
    assert_eq!(status.report_id.as_deref(), Some(report.id.as_str()));
    reopened
        .authorize_root_budget(&admin, &authorization())
        .await
        .unwrap();
    let post_report_call = reopened
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "post-report-cost",
                "group-history",
                BudgetStage::HistoryCollection,
                20,
            ),
        )
        .await
        .unwrap();
    reopened
        .release_undispatched_budget_call(
            &evaluator,
            &fence(&post_report_call, 21),
            "late non-dispatch proof",
        )
        .await
        .unwrap();
    PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &reopened,
        &job.id,
        "economic-scope",
        "post-report-cost",
        CostComponentKind::HistoryCollection,
        CostScope::OneTime,
        1,
    )
    .await
    .unwrap();
    let immutable_report = PersistentReplayEconomicCoordinator::build_blocked_report(
        &evaluator, &reopened, &job.id, 10,
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(&immutable_report).unwrap(),
        fingerprint(&report).unwrap()
    );
    let after_post_report_cost =
        PersistentReplayEconomicCoordinator::status(&evaluator, &reopened, &job.id)
            .await
            .unwrap();
    assert_eq!(after_post_report_cost.cost_receipt_count, 1);
    assert_eq!(
        after_post_report_cost.report_id.as_deref(),
        Some(report.id.as_str())
    );
    let late_call = reopened
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "late-cancel-call",
                "group-replay",
                BudgetStage::DevelopmentExecution,
                30,
            ),
        )
        .await
        .unwrap();
    let late_dispatch = reopened
        .begin_budget_dispatch(&evaluator, &fence(&late_call, 31))
        .await
        .unwrap();
    reopened
        .mark_budget_call_uncertain(&evaluator, &fence(&late_call, 32), "late billing")
        .await
        .unwrap();
    let cancelled = PersistentReplayEconomicCoordinator::cancel(
        &evaluator,
        &reopened,
        &job.id,
        "external execution remains unavailable",
    )
    .await
    .unwrap();
    assert_eq!(cancelled.state, ReplayEconomicJobState::Cancelled);
    reopened
        .close_budget_call_execution(
            &evaluator,
            "economic-scope",
            "late-cancel-call",
            late_dispatch.call.dispatch_id.as_deref().unwrap(),
            "late process closed",
            33,
        )
        .await
        .unwrap();
    PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &reopened,
        &job.id,
        "economic-scope",
        "late-cancel-call",
        CostComponentKind::OnlineDevelopmentReplaySelected,
        CostScope::PerTask,
        1,
    )
    .await
    .unwrap();
    let after_late = PersistentReplayEconomicCoordinator::status(&evaluator, &reopened, &job.id)
        .await
        .unwrap();
    assert_eq!(after_late.state, ReplayEconomicJobState::Cancelled);
    assert_eq!(after_late.cost_receipt_count, 2);
    assert!(matches!(
        PersistentReplayEconomicCoordinator::build_blocked_report(
            &evaluator, &reopened, &job.id, 10,
        )
        .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(
        PersistentReplayEconomicCoordinator::cancel(
            &evaluator,
            &reopened,
            &job.id,
            "external execution remains unavailable",
        )
        .await
        .unwrap()
        .id,
        job.id
    );
}

fn authorization() -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: "root-1".into(),
        billing_scope: "economic-scope".into(),
        allowed_namespaces: vec!["n".into()],
        currency: "usd".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        authorization_receipt_digest: d("authorization"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn reservation(
    call_id: &str,
    dispatch_group_id: &str,
    stage: BudgetStage,
    now: i64,
) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "economic-scope".into(),
        call_id: call_id.into(),
        dispatch_group_id: dispatch_group_id.into(),
        stage,
        actual_input_digest: d(&format!("input-{call_id}")),
        request_artifact: None,
        max_cost_micros: 100,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + 100,
        now,
    }
}

fn fence(call: &evo_storage::budget::BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

#[tokio::test]
async fn real_budget_call_state_maps_to_final_uncertain_and_not_incurred_without_double_count() {
    let (_directory, store, admin, evaluator, experiment) = setup().await;
    let job = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        "cost-job",
        1,
    )
    .await
    .unwrap();
    store
        .authorize_root_budget(&admin, &authorization())
        .await
        .unwrap();

    let wrong_group = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "wrong-group-call",
                "group-other-experiment",
                BudgetStage::HistoryCollection,
                2,
            ),
        )
        .await
        .unwrap();
    assert!(
        PersistentReplayEconomicCoordinator::record_budget_cost(
            &evaluator,
            &store,
            &job.id,
            "economic-scope",
            "wrong-group-call",
            CostComponentKind::HistoryCollection,
            CostScope::OneTime,
            1,
        )
        .await
        .is_err()
    );
    store
        .release_undispatched_budget_call(
            &evaluator,
            &fence(&wrong_group, 3),
            "wrong experiment group",
        )
        .await
        .unwrap();

    let mut other_authorization = authorization();
    other_authorization.root_budget_id = "other-root".into();
    other_authorization.billing_scope = "other-scope".into();
    store
        .authorize_root_budget(&admin, &other_authorization)
        .await
        .unwrap();
    let mut other_reservation = reservation(
        "other-root-call",
        "group-history",
        BudgetStage::HistoryCollection,
        4,
    );
    other_reservation.billing_scope = "other-scope".into();
    let other_call = store
        .reserve_budget_call(&evaluator, &other_reservation)
        .await
        .unwrap();
    assert!(
        PersistentReplayEconomicCoordinator::record_budget_cost(
            &evaluator,
            &store,
            &job.id,
            "other-scope",
            "other-root-call",
            CostComponentKind::HistoryCollection,
            CostScope::OneTime,
            1,
        )
        .await
        .is_err()
    );
    store
        .release_undispatched_budget_call(
            &evaluator,
            &BudgetCallFence {
                billing_scope: "other-scope".into(),
                ..fence(&other_call, 5)
            },
            "different root",
        )
        .await
        .unwrap();

    let finalized = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "fixed-call",
                "group-fixed",
                BudgetStage::DevelopmentExecution,
                6,
            ),
        )
        .await
        .unwrap();
    let dispatch = store
        .begin_budget_dispatch(&evaluator, &fence(&finalized, 7))
        .await
        .unwrap();
    let finalized = store
        .finalize_budget_call(
            &evaluator,
            &fence(&dispatch.call, 8),
            &UsageCharge {
                amount_micros: 20,
                currency: "usd".into(),
                pricing_version: "price-v1".into(),
                provider_request_id: "provider-fixed".into(),
                usage_record_id: "usage-fixed".into(),
                output_digest: d("output-fixed"),
            },
        )
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &evaluator,
            "economic-scope",
            "fixed-call",
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            9,
        )
        .await
        .unwrap();
    let fixed = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &job.id,
        "economic-scope",
        "fixed-call",
        CostComponentKind::OnlineDevelopmentFixed,
        CostScope::PerTask,
        1,
    )
    .await
    .unwrap();
    assert_eq!(fixed.measurement_state, MeasurementState::KnownFinal);
    assert_eq!(fixed.amount_micros, Some(20));

    let uncertain_call = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "replay-call",
                "group-replay",
                BudgetStage::DevelopmentExecution,
                6,
            ),
        )
        .await
        .unwrap();
    let uncertain_dispatched = store
        .begin_budget_dispatch(&evaluator, &fence(&uncertain_call, 7))
        .await
        .unwrap();
    let _uncertain_record = store
        .mark_budget_call_uncertain(&evaluator, &fence(&uncertain_call, 8), "timeout")
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &evaluator,
            "economic-scope",
            "replay-call",
            uncertain_dispatched.call.dispatch_id.as_deref().unwrap(),
            "process_confirmed_stopped",
            9,
        )
        .await
        .unwrap();
    let uncertain = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &job.id,
        "economic-scope",
        "replay-call",
        CostComponentKind::OnlineDevelopmentReplaySelected,
        CostScope::PerTask,
        2,
    )
    .await
    .unwrap();
    assert_eq!(
        uncertain.measurement_state,
        MeasurementState::UsageUncertain
    );
    assert!(uncertain.amount_micros.is_none());

    let reserved = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "released-call",
                "group-history",
                BudgetStage::HistoryCollection,
                10,
            ),
        )
        .await
        .unwrap();
    store
        .release_undispatched_budget_call(
            &evaluator,
            &fence(&reserved, 11),
            "experiment blocked before dispatch",
        )
        .await
        .unwrap();
    let released = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &job.id,
        "economic-scope",
        "released-call",
        CostComponentKind::HistoryCollection,
        CostScope::OneTime,
        4,
    )
    .await
    .unwrap();
    assert_eq!(released.measurement_state, MeasurementState::NotIncurred);
    assert!(released.proof_digest.is_some());
    let admin_receipt = CostComponentReceiptV1 {
        schema_version: COST_RECEIPT_SCHEMA.into(),
        id: "economic-cost-admin-review".into(),
        experiment_id: experiment.id.clone(),
        component: CostComponentKind::HumanReview,
        scope: CostScope::OneTime,
        source_kind: CostSourceKind::AdminMeasurement,
        source_id: "admin-review-receipt".into(),
        source_digest: d("admin-review-body"),
        billing_scope: None,
        budget_call_id: None,
        amount_micros: Some(5),
        currency: Some("usd".into()),
        pricing_version: Some("price-v1".into()),
        payment_subject: Some("payer-1".into()),
        tokens: None,
        latency_micros: None,
        storage_bytes: None,
        cpu_nanos: None,
        human_minutes: Some(1),
        measurement_state: MeasurementState::KnownFinal,
        proof_digest: Some(d("admin-review-authorization")),
        reason: None,
        created_seq: 5,
    };
    let admin_cost = PersistentReplayEconomicCoordinator::register_admin_cost(
        &admin,
        &store,
        &job.id,
        admin_receipt.clone(),
    )
    .await
    .unwrap();
    assert_eq!(admin_cost.amount_micros, Some(5));
    let mut changed_admin = admin_receipt.clone();
    changed_admin.amount_micros = Some(6);
    assert!(
        PersistentReplayEconomicCoordinator::register_admin_cost(
            &admin,
            &store,
            &job.id,
            changed_admin,
        )
        .await
        .is_err()
    );
    let mut renamed_admin = admin_receipt;
    renamed_admin.id = "economic-cost-admin-review-renamed".into();
    assert!(
        PersistentReplayEconomicCoordinator::register_admin_cost(
            &admin,
            &store,
            &job.id,
            renamed_admin,
        )
        .await
        .is_err()
    );
    let policy_call = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "policy-cost-call",
                "group-policy",
                BudgetStage::CandidateGeneration,
                20,
            ),
        )
        .await
        .unwrap();
    let replay_cpu_call = store
        .reserve_budget_call(
            &evaluator,
            &reservation(
                "replay-cpu-call",
                "group-replay-cpu",
                BudgetStage::StorageCpu,
                20,
            ),
        )
        .await
        .unwrap();
    for call in [&policy_call, &replay_cpu_call] {
        store
            .release_undispatched_budget_call(
                &evaluator,
                &fence(call, 21),
                "blocked before dispatch",
            )
            .await
            .unwrap();
    }
    let policy_cost = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &job.id,
        "economic-scope",
        "policy-cost-call",
        CostComponentKind::PolicyGeneration,
        CostScope::OneTime,
        7,
    );
    let replay_cpu_cost = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &job.id,
        "economic-scope",
        "replay-cpu-call",
        CostComponentKind::ReplayCpu,
        CostScope::OneTime,
        8,
    );
    let (policy_cost, replay_cpu_cost) = tokio::join!(policy_cost, replay_cpu_cost);
    policy_cost.unwrap();
    replay_cpu_cost.unwrap();
    assert_eq!(
        PersistentReplayEconomicCoordinator::status(&evaluator, &store, &job.id)
            .await
            .unwrap()
            .cost_receipt_count,
        6
    );
    assert!(
        PersistentReplayEconomicCoordinator::record_budget_cost(
            &evaluator,
            &store,
            &job.id,
            "economic-scope",
            "fixed-call",
            CostComponentKind::HumanReview,
            CostScope::OneTime,
            6,
        )
        .await
        .is_err()
    );
    let report =
        PersistentReplayEconomicCoordinator::build_blocked_report(&evaluator, &store, &job.id, 10)
            .await
            .unwrap();
    assert_eq!(report.terminal, EconomicReportTerminal::UsageUncertain);
    assert_eq!(
        PersistentReplayEconomicCoordinator::status(&evaluator, &store, &job.id)
            .await
            .unwrap()
            .state,
        ReplayEconomicJobState::UsageUncertain
    );
}

#[tokio::test]
async fn actor_source_and_watermark_changes_block_reuse() {
    let (_directory, store, admin, evaluator, mut experiment) = setup().await;
    let other = Context::new("n", "other-evaluator", Role::Evaluator).unwrap();
    experiment.id = "forged-actor-experiment".into();
    assert!(matches!(
        PersistentReplayEconomicCoordinator::register(&other, &store, experiment).await,
        Err(Error::Forbidden)
    ));
    let job = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        "economic-exp-1",
        "actor-check",
        1,
    )
    .await
    .unwrap();
    assert!(matches!(
        PersistentReplayEconomicCoordinator::status(&other, &store, &job.id).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        PersistentReplayEconomicCoordinator::record_budget_cost(
            &other,
            &store,
            &job.id,
            "economic-scope",
            "missing-call",
            CostComponentKind::HistoryCollection,
            CostScope::OneTime,
            1,
        )
        .await,
        Err(Error::Forbidden)
    ));
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "economic-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        PersistentReplayEconomicCoordinator::start(
            &evaluator,
            &store,
            "economic-exp-1",
            "after-revoke",
            1,
        )
        .await
        .is_err()
    );
}
