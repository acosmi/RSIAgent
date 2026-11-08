use evo_core::evaluation::Verdict;
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
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

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

fn experiment(report: &StoredReplayReportV2) -> ReplayEconomicExperimentV1 {
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
    let stored_report = StoredReplayReportV2::build(
        "n",
        WorldPartition::Select,
        &pool,
        policy,
        profile,
        caps,
        vec![replay],
    )
    .unwrap();
    evo_storage::replay::put_current_replay_report(&admin, &store, &stored_report)
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

// Actual Parent capture, copied under the card's explicit fixture allowlist.
// These constants are filled only from the retained producer and copy receipt.
// Each test opens a fresh DB copy; the committed fixture is never opened.
const M33_HISTORY_METADATA: &str = include_str!("fixtures/ag147_m33_legacy_history.json");
const M33_HISTORY_DATABASE: &[u8] = include_bytes!("fixtures/ag147_m33_legacy_history.sqlite3");
const M33_HISTORY_METADATA_SHA256: &str =
    "a0f9227027e927ec5cd6f7c8422b93b2893cb2dbfe0055e87efe3ebb3eaccb71";
const M33_HISTORY_DATABASE_SHA256: &str =
    "0d22863a74877098e16d2b2ce651a9af7eb48b517930db6ef021cfa68a013c38";

async fn actual_m33_history_copy() -> (
    tempfile::TempDir,
    Store,
    Context,
    Context,
    serde_json::Value,
) {
    assert_eq!(
        hash(M33_HISTORY_METADATA.as_bytes()),
        M33_HISTORY_METADATA_SHA256
    );
    assert_eq!(hash(M33_HISTORY_DATABASE), M33_HISTORY_DATABASE_SHA256);
    let metadata: serde_json::Value = serde_json::from_str(M33_HISTORY_METADATA).unwrap();
    assert_eq!(
        metadata["schema_version"],
        "ag147.parent-retained-history.v1"
    );
    assert_eq!(
        metadata["parent"],
        "a730b9a9935bee7a41e8a47e86f329b99012566b"
    );
    assert_eq!(metadata["namespace"], "n");
    assert_eq!(
        metadata["snapshot"]["database_sha256"].as_str(),
        Some(M33_HISTORY_DATABASE_SHA256)
    );
    assert_eq!(
        metadata["snapshot"]["database_bytes"].as_u64(),
        Some(u64::try_from(M33_HISTORY_DATABASE.len()).unwrap())
    );
    assert_eq!(metadata["scope"]["external_provider_calls"], 0);
    assert_eq!(metadata["scope"]["real_paid_effect_claimed"], false);
    assert_eq!(metadata["scope"]["current_v3_authority_claimed"], false);
    let dir = tempfile::Builder::new()
        .permissions(<std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("history-copy.sqlite3");
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).unwrap();
    file.write_all(M33_HISTORY_DATABASE).unwrap();
    drop(file);
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let mut session = store.session().await.unwrap();
    for row in metadata["artifact_rows"].as_array().unwrap() {
        let physical_id = row["physical_id"].as_str().unwrap();
        let actual = session
            .object_row(&admin, "artifact", physical_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.id, physical_id);
        assert_eq!(actual.owner, row["owner"].as_str().unwrap());
        assert_eq!(actual.body, row["body_utf8"].as_str().unwrap());
        assert_eq!(
            hash(actual.body.as_bytes()),
            row["body_sha256"].as_str().unwrap()
        );
    }
    assert_eq!(
        session.watermark(&admin).await.unwrap().unwrap().0,
        metadata["original_watermark"]["seq"].as_i64().unwrap()
    );
    session.commit().await.unwrap();
    (dir, store, admin, evaluator, metadata)
}

async fn original_m33_rows_unchanged(store: &Store, admin: &Context, metadata: &serde_json::Value) {
    let mut session = store.session().await.unwrap();
    for row in metadata["artifact_rows"].as_array().unwrap() {
        let actual = session
            .object_row(admin, "artifact", row["physical_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.owner, row["owner"].as_str().unwrap());
        assert_eq!(actual.body, row["body_utf8"].as_str().unwrap());
    }
    for name in ["management_job_raw", "management_private_input_raw"] {
        let row = &metadata[name];
        let actual = session
            .object_row(
                admin,
                row["kind"].as_str().unwrap(),
                row["physical_id"].as_str().unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.owner, row["owner"].as_str().unwrap());
        assert_eq!(actual.body, row["body_utf8"].as_str().unwrap());
    }
    session.commit().await.unwrap();
}

fn is_legacy_consumption_error(error: &Error) -> bool {
    matches!(error, Error::Invalid(reason) if reason == "legacy_replay_not_consumable")
}

#[tokio::test]
async fn actual_parent_selection_is_auditable_but_rejected_at_all_new_e11_write_gates() {
    let (directory, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    let old: StoredReplayReportV1 = serde_json::from_value(metadata["selection"].clone()).unwrap();
    let experiment: ReplayEconomicExperimentV1 =
        serde_json::from_value(metadata["experiment"].clone()).unwrap();
    let blocked: evo_engine::replay_experiment::ReplayEconomicJobV1 =
        serde_json::from_value(metadata["blocked_job"].clone()).unwrap();
    let read = evo_engine::replay::query_replay_report_record(&evaluator, &store, &old.report_id)
        .await
        .unwrap();
    assert_eq!(read.record.state(), ReplayRecordState::Legacy);
    assert!(read.source_legacy);
    assert!(read.record.current_report().is_none());
    assert_eq!(read.raw_payload, Some(metadata["selection"].clone()));
    assert_eq!(
        serde_json::to_value(read.record.report().unwrap()).unwrap(),
        metadata["selection"]
    );
    let complete_before = directory
        .path()
        .join("backups/before-legacy-new-consumption");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("before-legacy-new-consumption").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    let strong =
        evo_engine::replay::verified_replay_report_view(&evaluator, &store, &old.report_id)
            .await
            .unwrap_err();
    assert!(is_legacy_consumption_error(&strong), "{strong:?}");
    let register =
        PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment.clone())
            .await
            .unwrap_err();
    assert!(is_legacy_consumption_error(&register), "{register:?}");
    let start = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        &blocked.request_key,
        blocked.created_seq,
    )
    .await
    .unwrap_err();
    assert!(is_legacy_consumption_error(&start), "{start:?}");
    // This is the coordinator's existing final write Session. Supplying the
    // actual old job must not bypass the new selection check via idempotency.
    let final_write = PersistentReplayEconomicCoordinator::write_started_job(
        &evaluator,
        &store,
        &experiment,
        blocked,
    )
    .await
    .unwrap_err();
    assert!(is_legacy_consumption_error(&final_write), "{final_write:?}");
    original_m33_rows_unchanged(&store, &admin, &metadata).await;

    let complete_after = directory
        .path()
        .join("backups/after-legacy-new-consumption");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("after-legacy-new-consumption").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(complete_after.join("rsia.sqlite3")).unwrap(),
        std::fs::read(complete_before.join("rsia.sqlite3")).unwrap(),
        "legacy strong/register/same-key start/final Session checks changed persistent facts"
    );

    // Explicit new v3 execution uses the real old sources and a separate v3
    // report identity. It cannot relabel or overwrite the old report bytes.
    let current = evo_engine::replay::run_and_persist_pool_replay(
        &admin,
        &store,
        &old.pool_digest,
        old.partition,
        &old.policy,
        &old.profile,
        &old.caps,
    )
    .await
    .unwrap();
    assert_ne!(current.report_id, old.report_id);
    assert_eq!(current.replay_engine_version, NEXT_REPLAY_ENGINE_VERSION_V3);
    assert_eq!(current.schema_version, STORED_REPLAY_REPORT_SCHEMA_V2);
    let current_read =
        evo_engine::replay::query_replay_report_record(&evaluator, &store, &current.report_id)
            .await
            .unwrap();
    assert_eq!(
        current_read.record.state(),
        ReplayRecordState::CurrentVerified
    );
    assert!(!current_read.source_legacy);
    let old_again =
        evo_engine::replay::query_replay_report_record(&evaluator, &store, &old.report_id)
            .await
            .unwrap();
    assert_eq!(old_again.record.state(), ReplayRecordState::Legacy);
    assert_eq!(old_again.raw_payload, Some(metadata["selection"].clone()));
    assert_eq!(old_again.lineage.len(), 2);
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    store.close().await;
}

#[tokio::test]
async fn actual_parent_e11_paid_uncertain_cancel_and_owner_history_is_preserved() {
    let (_dir, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    for (job_key, status_key) in [
        ("blocked_job", "blocked_status"),
        ("cancelled_job", "cancelled_status"),
    ] {
        let id = metadata[job_key]["id"].as_str().unwrap();
        let status = PersistentReplayEconomicCoordinator::status(&evaluator, &store, id)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(status).unwrap(), metadata[status_key]);
    }
    for report_key in ["blocked_report", "cancelled_report"] {
        let id = metadata[report_key]["id"].as_str().unwrap();
        for caller in [&evaluator, &admin] {
            let audit = PersistentReplayEconomicCoordinator::query_report(caller, &store, id)
                .await
                .unwrap();
            assert!(audit.source_legacy);
            assert_eq!(
                audit.replay_read_view.record.state(),
                ReplayRecordState::Legacy
            );
            assert_eq!(audit.raw_payload, metadata[report_key]);
            assert_eq!(
                audit.replay_read_view.raw_payload,
                Some(metadata["selection"].clone())
            );
        }
        for caller in [
            Context::new("n", "other-evaluator", Role::Evaluator).unwrap(),
            Context::new("n", "agent", Role::Agent).unwrap(),
            Context::new("n", "fixture-host", Role::Host).unwrap(),
        ] {
            assert!(matches!(
                PersistentReplayEconomicCoordinator::query_report(&caller, &store, id).await,
                Err(Error::Forbidden)
            ));
        }
        let other_namespace = Context::new("other", "admin", Role::Admin).unwrap();
        assert!(
            PersistentReplayEconomicCoordinator::query_report(&other_namespace, &store, id)
                .await
                .is_err()
        );
    }
    let cancelled: evo_engine::replay_experiment::ReplayEconomicJobV1 =
        serde_json::from_value(metadata["cancelled_job"].clone()).unwrap();
    let original_cancelled = PersistentReplayEconomicCoordinator::cancel(
        &evaluator,
        &store,
        &cancelled.id,
        cancelled.cancel_reason.as_deref().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(original_cancelled).unwrap(),
        metadata["cancelled_job"]
    );
    let repeated_receipt = PersistentReplayEconomicCoordinator::record_budget_cost(
        &evaluator,
        &store,
        &cancelled.id,
        "economic-scope",
        "replay-call",
        CostComponentKind::OnlineDevelopmentReplaySelected,
        CostScope::PerTask,
        2,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&repeated_receipt).unwrap(),
        metadata["cost_receipts"][1]
    );
    assert_eq!(
        repeated_receipt.measurement_state,
        MeasurementState::UsageUncertain
    );
    assert_eq!(repeated_receipt.amount_micros, None);
    assert_eq!(
        metadata["cost_receipts"][0]["measurement_state"],
        "not_incurred"
    );
    for (call, position) in [("released-call", 0), ("replay-call", 1)] {
        let actual = store
            .budget_call(&evaluator, "economic-scope", call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            metadata["budget_calls"][position]
        );
    }
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    store.close().await;
}

// Invalid variants are deliberate corruptions of a copied actual Parent row.
// They do not create an historical/current report or any source authority.
#[tokio::test]
async fn actual_parent_report_corruption_and_future_versions_are_never_legacy_or_public_payload() {
    let mutations = [
        "body_digest",
        "semantic_digest",
        "member_binding",
        "payload_report_id",
        "row_namespace",
        "payload_namespace",
        "outer_schema",
        "inner_schema",
        "mixed_current_header",
        "extra_field",
        "unknown_engine",
    ];
    for mutation in mutations {
        let (_dir, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
        let id = metadata["selection"]["report_id"].as_str().unwrap();
        let original = metadata["artifact_rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["physical_id"].as_str() == Some(id))
            .unwrap();
        let mut body: serde_json::Value =
            serde_json::from_str(original["body_utf8"].as_str().unwrap()).unwrap();
        match mutation {
            "body_digest" => {
                body["payload"]["reports_body_digest"] = serde_json::json!(hash(b"wrong-body"))
            }
            "semantic_digest" => {
                body["payload"]["semantic_reports_digest"] =
                    serde_json::json!(hash(b"wrong-semantics"))
            }
            "member_binding" => {
                body["payload"]["member_reports"][0]["world_digest"] =
                    serde_json::json!(hash(b"other-world"))
            }
            "payload_report_id" => {
                body["payload"]["report_id"] = serde_json::json!("claimed-other-row")
            }
            "row_namespace" => body["namespace"] = serde_json::json!("other"),
            "payload_namespace" => body["payload"]["namespace"] = serde_json::json!("other"),
            "outer_schema" => {
                body["payload"]["schema_version"] =
                    serde_json::json!(STORED_REPLAY_REPORT_SCHEMA_V2)
            }
            "inner_schema" => {
                body["payload"]["member_reports"][0]["report"]["schema_version"] =
                    serde_json::json!(REPLAY_REPORT_SCHEMA_V3)
            }
            "mixed_current_header" => {
                body["payload"]["replay_engine_version"] =
                    serde_json::json!(NEXT_REPLAY_ENGINE_VERSION_V3)
            }
            "extra_field" => {
                body["payload"]["unregistered_private_field"] =
                    serde_json::json!("PRIVATE-CANARY-MUST-NOT-ESCAPE")
            }
            "unknown_engine" => {
                body["payload"]["replay_engine_version"] =
                    serde_json::json!("rsia.replay.engine.v999-PRIVATE-CANARY-MUST-NOT-ESCAPE")
            }
            _ => unreachable!(),
        }
        let mut session = store.session().await.unwrap();
        session
            .put(
                &admin,
                "artifact",
                id,
                original["owner"].as_str().unwrap(),
                &body,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        let result = evo_engine::replay::query_replay_report_record(&evaluator, &store, id).await;
        if mutation == "unknown_engine" {
            let error = result.unwrap_err();
            assert!(
                matches!(
                    error,
                    ReplayReadIsolation::UnqualifiedVersionOutsideStaticTable
                ),
                "{error:?}"
            );
            assert!(!error.to_string().contains("PRIVATE-CANARY"));
        } else {
            let read = result.unwrap();
            assert_eq!(
                read.record.state(),
                ReplayRecordState::Invalid,
                "{mutation}"
            );
            assert!(read.raw_payload.is_none(), "{mutation}");
            assert!(!read.source_legacy, "{mutation}");
            assert!(read.lineage.is_empty(), "{mutation}");
            assert!(
                !serde_json::to_string(&read)
                    .unwrap()
                    .contains("PRIVATE-CANARY")
            );
        }
        store.close().await;
    }
}

#[tokio::test]
async fn actual_parent_reader_roles_remain_distinct_and_duplicate_wire_fails_closed() {
    let (_dir, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    let id = metadata["selection"]["report_id"].as_str().unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    for caller in [&admin, &evaluator, &worker] {
        let read = evo_storage::replay::query_replay_report_record(caller, &store, id)
            .await
            .unwrap();
        assert_eq!(read.record.state(), ReplayRecordState::Legacy);
        assert_eq!(read.raw_payload, Some(metadata["selection"].clone()));
    }
    assert!(matches!(
        evo_engine::replay::query_replay_report_record(&worker, &store, id).await,
        Err(ReplayReadIsolation::Source(Error::Forbidden))
    ));
    for role in [Role::Agent, Role::Host] {
        let caller = Context::new("n", "unqualified-reader", role).unwrap();
        assert!(matches!(
            evo_storage::replay::query_replay_report_record(&caller, &store, id).await,
            Err(ReplayReadIsolation::Source(Error::Forbidden))
        ));
        assert!(matches!(
            evo_engine::replay::query_replay_report_record(&caller, &store, id).await,
            Err(ReplayReadIsolation::Source(Error::Forbidden))
        ));
    }
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    let original = metadata["artifact_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["physical_id"].as_str() == Some(id))
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(original["body_utf8"].as_str().unwrap()).unwrap();
    struct DuplicateEnvelope<'a>(&'a serde_json::Value);
    impl serde::Serialize for DuplicateEnvelope<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let object = self.0.as_object().unwrap();
            let mut map = serializer.serialize_map(Some(object.len() + 1))?;
            map.serialize_entry("namespace", "claimed-other-namespace")?;
            for (key, value) in object {
                map.serialize_entry(key, value)?;
            }
            map.end()
        }
    }
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            id,
            original["owner"].as_str().unwrap(),
            &DuplicateEnvelope(&body),
        )
        .await
        .unwrap();
    let actual = session
        .object_row(&admin, "artifact", id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.body.matches("\"namespace\":").count(), 3); // envelope twice + payload
    session.commit().await.unwrap();
    let read = evo_engine::replay::query_replay_report_record(&evaluator, &store, id)
        .await
        .unwrap();
    assert_eq!(read.record.state(), ReplayRecordState::Invalid);
    assert!(read.raw_payload.is_none());
    assert!(!read.source_legacy);
    store.close().await;
}

#[tokio::test]
async fn actual_parent_legacy_writer_retries_existing_only_and_conflicts_without_any_write() {
    let (directory, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    let old: StoredReplayReportV1 = serde_json::from_value(metadata["selection"].clone()).unwrap();
    let before = directory.path().join("backups/before-old-writer");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("before-old-writer").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    let before_bytes = std::fs::read(before.join("rsia.sqlite3")).unwrap();
    evo_storage::replay::put_replay_report(&admin, &store, &old)
        .await
        .unwrap();
    original_m33_rows_unchanged(&store, &admin, &metadata).await;

    let mut changed = old.clone();
    changed.member_reports[0].report.replay_cpu_nanos = changed.member_reports[0]
        .report
        .replay_cpu_nanos
        .checked_add(1)
        .unwrap();
    changed.reports_body_digest = fingerprint(&changed.member_reports).unwrap();
    assert_eq!(
        replay_report_semantic_digest(&changed.member_reports).unwrap(),
        old.semantic_reports_digest
    );
    assert_eq!(changed.report_id, old.report_id);
    match evo_storage::replay::put_replay_report(&admin, &store, &changed).await {
        Err(Error::Conflict(reason)) => assert_eq!(reason, "immutable replay report differs"),
        other => panic!("different body at actual old ID: {other:?}"),
    }
    assert!(matches!(
        evo_storage::replay::put_replay_report(&evaluator, &store, &old).await,
        Err(Error::Forbidden)
    ));
    let other_namespace = Context::new("other", "admin", Role::Admin).unwrap();
    assert!(matches!(
        evo_storage::replay::put_replay_report(&other_namespace, &store, &old).await,
        Err(Error::Forbidden)
    ));
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    let after = directory.path().join("backups/after-old-writer");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("after-old-writer").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(after.join("rsia.sqlite3")).unwrap(),
        before_bytes,
        "exact retries, conflicts and forbidden attempts changed persistent tables"
    );
    store.close().await;
}

#[tokio::test]
async fn actual_parent_outer_row_id_constraint_refuses_damage_before_any_reader() {
    let (directory, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    let id = metadata["selection"]["report_id"].as_str().unwrap();
    let row = metadata["artifact_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["physical_id"].as_str() == Some(id))
        .unwrap();
    let mut damaged: serde_json::Value =
        serde_json::from_str(row["body_utf8"].as_str().unwrap()).unwrap();
    damaged["id"] = serde_json::json!("claimed-other-physical-row");
    let backup_before = directory.path().join("backups/before-physical-id-refusal");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("before-physical-id-refusal").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    // Existing SQLite json_extract(body,'$.id') = id is the first guard.
    // Keep its exact mapping; never bypass it to manufacture a reader fixture.
    assert!(matches!(
        session
            .put(
                &admin,
                "artifact",
                id,
                row["owner"].as_str().unwrap(),
                &damaged
            )
            .await,
        Err(Error::Internal)
    ));
    session.commit().await.unwrap();
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    let backup_after = directory.path().join("backups/after-physical-id-refusal");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("after-physical-id-refusal").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(backup_after.join("rsia.sqlite3")).unwrap(),
        std::fs::read(backup_before.join("rsia.sqlite3")).unwrap()
    );
    // Rejection left the actual historical object readable and still Legacy.
    let original = evo_engine::replay::query_replay_report_record(&evaluator, &store, id)
        .await
        .unwrap();
    assert_eq!(original.record.state(), ReplayRecordState::Legacy);
    assert_eq!(original.raw_payload, Some(metadata["selection"].clone()));
    store.close().await;
}

#[tokio::test]
async fn invalid_derived_current_edges_preserve_the_real_parent_audit_and_all_read_facts() {
    let (directory, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    let old: StoredReplayReportV1 = serde_json::from_value(metadata["selection"].clone()).unwrap();
    let current = evo_engine::replay::run_and_persist_pool_replay(
        &admin,
        &store,
        &old.pool_digest,
        old.partition,
        &old.policy,
        &old.profile,
        &old.caps,
    )
    .await
    .unwrap();
    let healthy =
        evo_engine::replay::query_replay_report_record(&evaluator, &store, &current.report_id)
            .await
            .unwrap();
    assert_eq!(healthy.record.state(), ReplayRecordState::CurrentVerified);
    let old_before =
        evo_engine::replay::query_replay_report_record(&evaluator, &store, &old.report_id)
            .await
            .unwrap();
    assert_eq!(old_before.record.state(), ReplayRecordState::Legacy);
    assert_eq!(old_before.raw_payload, Some(metadata["selection"].clone()));
    let mut session = store.session().await.unwrap();
    let current_row = session
        .object_row(&admin, "artifact", &current.report_id)
        .await
        .unwrap()
        .unwrap();
    let old_row = session
        .object_row(&admin, "artifact", &old.report_id)
        .await
        .unwrap()
        .unwrap();
    // Only an actual public dependency edge changes. The immutable report body,
    // physical identity, self-digests, original pool and old source remain exact.
    session
        .put_edge(
            &admin,
            "artifact",
            &current.report_id,
            "artifact",
            &old.report_id,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before_reads = directory
        .path()
        .join("backups/before-invalid-neighbor-reads");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("before-invalid-neighbor-reads").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    for caller in [&evaluator, &admin] {
        assert!(matches!(
            evo_storage::replay::load_current_replay_report(caller, &store, &current.report_id).await,
            Err(Error::Invalid(reason)) if reason == "current replay dependency edges differ"
        ));
        assert!(matches!(
            evo_engine::replay::verified_replay_report_view(caller, &store, &current.report_id).await,
            Err(Error::Invalid(reason)) if reason == "current replay dependency edges differ"
        ));
        let historical =
            evo_engine::replay::query_replay_report_record(caller, &store, &old.report_id)
                .await
                .unwrap();
        assert_eq!(historical.record.state(), ReplayRecordState::Legacy);
        assert!(historical.source_legacy);
        assert_eq!(historical.raw_payload, Some(metadata["selection"].clone()));
        let neighbor = historical
            .lineage
            .iter()
            .find(|item| item.report_id == current.report_id)
            .unwrap();
        assert!(neighbor.exists);
        assert_eq!(neighbor.record_state, Some(ReplayRecordState::Invalid));
        assert!(neighbor.isolation_reason.is_none());
        let direct =
            evo_engine::replay::query_replay_report_record(caller, &store, &current.report_id)
                .await
                .unwrap();
        assert_eq!(direct.record.state(), ReplayRecordState::Invalid);
        assert!(direct.raw_payload.is_none());
        assert!(direct.record.current_report().is_none());
        let economic = PersistentReplayEconomicCoordinator::query_report(
            caller,
            &store,
            metadata["blocked_report"]["id"].as_str().unwrap(),
        )
        .await
        .unwrap();
        assert!(economic.source_legacy);
        assert_eq!(economic.raw_payload, metadata["blocked_report"]);
        assert_eq!(
            economic.replay_read_view.record.state(),
            ReplayRecordState::Legacy
        );
        assert!(
            economic
                .replay_read_view
                .lineage
                .iter()
                .any(|item| item.report_id == current.report_id
                    && item.exists
                    && item.record_state == Some(ReplayRecordState::Invalid))
        );
    }
    original_m33_rows_unchanged(&store, &admin, &metadata).await;
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", &current.report_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        current_row.body
    );
    assert_eq!(
        session
            .object_row(&admin, "artifact", &old.report_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        old_row.body
    );
    session.commit().await.unwrap();
    let after_reads = directory
        .path()
        .join("backups/after-invalid-neighbor-reads");
    store
        .backup(
            &admin,
            evo_storage::lifecycle::BackupName::parse("after-invalid-neighbor-reads").unwrap(),
        )
        .unwrap()
        .complete()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(after_reads.join("rsia.sqlite3")).unwrap(),
        std::fs::read(before_reads.join("rsia.sqlite3")).unwrap(),
        "historical/current/economic reads changed objects, dependency edges, audit or other database facts"
    );
    store.close().await;
}

// ---------------------------------------------------------------------------
// AG-178: report v2 is written, v1 reports are legacy history (plan §5.1.1, V104)
// ---------------------------------------------------------------------------

/// The stored row of the blocked v1 report in the actual Parent history above
/// (`economic_record_rows.blocked_report`), byte for byte: a real record the
/// coordinator wrote before AG-178.
const PARENT_V1_BLOCKED_REPORT_ROW: &str = r#"{"schema_version":"rsia.replay_economic_artifact_envelope.v1","id":"e11-7c0c436b0e54d2101378ed4de239b743adf79259e692686e57a23d59ad46bdef","record_kind":"replay_economic_report_v1","payload":{"schema_version":"rsia.replay_economic_report.v1","id":"replay-economic-report-deddffc2e36f640df68c9e67","experiment_id":"economic-exp-1","experiment_digest":"724b27e8d4cbc060484fe07313a08e476031c67558c2a0cf25719b93ab7b2fe6","paired_ticket_id":"paired-ticket","paired_ticket_digest":"1a708645fe31616bac1b9f93d5c5e2f13dafc8f3b855bca6195a7835c4b3e91a","paired_receipt_closure_digest":null,"completed_paired_tasks":0,"paired_units":[],"latency":null,"quality_outcome":"zero_or_inconclusive","cost_receipt_ids":[],"economics":{"status":"blocked","reasons":["no_cost_receipts"]},"terminal":"blocked_support","reasons":["e05_paired_ticket_not_found","real_provider_and_independent_runner_unavailable"]}}"#;
const PARENT_V1_BLOCKED_REPORT_ROW_SHA256: &str =
    "9b857f8a4d0b2310e0f59c6e4ce72602ea8c089f1666988789c68b3b299701e2";
const ECONOMIC_ENVELOPE_SCHEMA: &str = "rsia.replay_economic_artifact_envelope.v1";
const LEGACY_REPORT_KIND: &str = "replay_economic_report_v1";
const CURRENT_REPORT_KIND: &str = "replay_economic_report_v2";

/// The envelope `replay_experiment.rs` stores an economic record in, field for field.
#[derive(serde::Serialize)]
struct StoredEnvelope<'a, T> {
    schema_version: &'a str,
    id: &'a str,
    record_kind: &'a str,
    payload: &'a T,
}

fn economic_storage_id(record_kind: &str, id: &str) -> String {
    format!("e11-{}", fingerprint(&(record_kind, id)).unwrap())
}

/// Stores `payload` as `replay_experiment.rs` stores a record of `record_kind`.
async fn put_economic_record<T: serde::Serialize>(
    store: &Store,
    admin: &Context,
    record_kind: &str,
    id: &str,
    owner: &str,
    payload: &T,
) {
    let storage_id = economic_storage_id(record_kind, id);
    let mut session = store.session().await.unwrap();
    session
        .put(
            admin,
            "artifact",
            &storage_id,
            owner,
            &StoredEnvelope {
                schema_version: ECONOMIC_ENVELOPE_SCHEMA,
                id: &storage_id,
                record_kind,
                payload,
            },
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// Revokes the first source run of the selection world of the actual Parent history
/// and runs the cleanup until it stops.
async fn clean_the_selection_source(
    store: &Store,
    admin: &Context,
    metadata: &serde_json::Value,
) -> evo_storage::lifecycle::CleanupStatus {
    use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
    let world_id = metadata["selection"]["member_reports"][0]["world_id"]
        .as_str()
        .unwrap();
    let world =
        evo_storage::replay::load_live_replay_world(admin, store, world_id, Purpose::Development)
            .await
            .unwrap();
    let mut status = LifecycleStore::begin_revoke(
        admin,
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: world.manifest.source_closure[0].source_id.clone(),
        },
        "selection source revoked",
        evo_core::now().unwrap(),
    )
    .await
    .unwrap();
    for _ in 0..256 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status =
            LifecycleStore::cleanup_step(admin, store, &status.job_id, 8, evo_core::now().unwrap())
                .await
                .unwrap();
    }
    status
}

fn lineage_of(
    view: &evo_engine::replay_experiment::ReplayEconomicReportAuditView,
) -> Vec<(&str, &str, bool)> {
    view.report_lineage
        .iter()
        .map(|entry| {
            (
                entry.schema_version.as_str(),
                entry.report_id.as_str(),
                entry.exists,
            )
        })
        .collect()
}

/// V104.a/b on a real old record: the golden v1 row is statically readable (the v1
/// wire is unchanged, the v1 judge is kept), reads as legacy with its lineage, its
/// id is the one the v1 formula derives from its job and a v2 report of that job
/// gets another, and a revocation cleanup that reaches it completes and keeps it.
#[tokio::test]
async fn actual_parent_v1_report_golden_bytes_read_as_legacy_and_the_cleanup_keeps_them() {
    let (_dir, store, admin, evaluator, metadata) = actual_m33_history_copy().await;
    assert_eq!(
        hash(PARENT_V1_BLOCKED_REPORT_ROW.as_bytes()),
        PARENT_V1_BLOCKED_REPORT_ROW_SHA256
    );
    let row = &metadata["economic_record_rows"]["blocked_report"];
    assert_eq!(
        row["body_utf8"].as_str(),
        Some(PARENT_V1_BLOCKED_REPORT_ROW)
    );
    let physical_id = row["physical_id"].as_str().unwrap();
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", physical_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        PARENT_V1_BLOCKED_REPORT_ROW
    );
    session.commit().await.unwrap();

    let stored: serde_json::Value = serde_json::from_str(PARENT_V1_BLOCKED_REPORT_ROW).unwrap();
    let legacy: ReplayEconomicReportV1 = serde_json::from_value(stored["payload"].clone()).unwrap();
    assert_eq!(legacy.schema_version, REPORT_SCHEMA);
    assert_eq!(
        serde_json::to_string(&StoredEnvelope {
            schema_version: ECONOMIC_ENVELOPE_SCHEMA,
            id: physical_id,
            record_kind: LEGACY_REPORT_KIND,
            payload: &legacy,
        })
        .unwrap(),
        PARENT_V1_BLOCKED_REPORT_ROW,
        "the v1 wire changed"
    );
    assert_eq!(
        physical_id,
        economic_storage_id(LEGACY_REPORT_KIND, &legacy.id)
    );
    assert!(serde_json::from_value::<ReplayEconomicReportV2>(stored["payload"].clone()).is_err());
    let experiment: ReplayEconomicExperimentV1 =
        serde_json::from_value(metadata["experiment"].clone()).unwrap();
    legacy.validate_against(&experiment).unwrap();

    let mut snapshot: evo_engine::replay_experiment::ReplayEconomicJobV1 =
        serde_json::from_value(metadata["blocked_job"].clone()).unwrap();
    assert_eq!(snapshot.report_id.as_deref(), Some(legacy.id.as_str()));
    snapshot.report_id = None;
    let input = fingerprint(&snapshot).unwrap();
    assert_eq!(
        replay_economic_report_id(REPORT_SCHEMA, &input).unwrap(),
        legacy.id
    );
    let successor = replay_economic_report_id(REPORT_SCHEMA_V2, &input).unwrap();
    assert_ne!(successor, legacy.id);

    for caller in [&evaluator, &admin] {
        let read = PersistentReplayEconomicCoordinator::query_report(caller, &store, &legacy.id)
            .await
            .unwrap();
        assert_eq!(read.report_state, ReplayRecordState::Legacy);
        assert_eq!(read.report_schema_version, REPORT_SCHEMA);
        assert!(read.source_legacy);
        assert_eq!(read.raw_payload, metadata["blocked_report"]);
        assert_eq!(
            lineage_of(&read),
            [
                (REPORT_SCHEMA, legacy.id.as_str(), true),
                (REPORT_SCHEMA_V2, successor.as_str(), false),
            ]
        );
    }
    original_m33_rows_unchanged(&store, &admin, &metadata).await;

    // The same revocation on an untouched copy, for the number of nodes it visits.
    let (_baseline_dir, baseline_store, baseline_admin, _, baseline_metadata) =
        actual_m33_history_copy().await;
    let baseline =
        clean_the_selection_source(&baseline_store, &baseline_admin, &baseline_metadata).await;
    assert_eq!(
        baseline.state,
        evo_storage::lifecycle::CleanupState::Complete,
        "{baseline:?}"
    );
    baseline_store.close().await;
    // An edge to the experiment makes the cleanup reach the v1 report.
    let experiment_row = metadata["economic_record_rows"]["experiment"]["physical_id"]
        .as_str()
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&admin, "artifact", physical_id, "artifact", experiment_row)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let cleaned = clean_the_selection_source(&store, &admin, &metadata).await;
    assert_eq!(
        cleaned.state,
        evo_storage::lifecycle::CleanupState::Complete,
        "{cleaned:?}"
    );
    assert_eq!(cleaned.last_error, None);
    assert_eq!(cleaned.processed_nodes, baseline.processed_nodes + 1);
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", physical_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        PARENT_V1_BLOCKED_REPORT_ROW,
        "the cleanup kept the v1 report as it was"
    );
    session.commit().await.unwrap();
    assert!(matches!(
        PersistentReplayEconomicCoordinator::query_report(&evaluator, &store, &legacy.id).await,
        Err(Error::Conflict(_))
    ));
    store.close().await;
}

/// V104.b/c over a current selection: a v1 report reads as legacy because of its own
/// version (the selection is current) and is never handed out as the job's report;
/// a new report is v2, under a new id of the same input, and reads as current.
#[tokio::test]
async fn a_v1_report_reads_legacy_over_a_current_selection_and_new_reports_are_v2() {
    let (_directory, store, admin, evaluator, experiment) = setup().await;
    let job = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        "legacy-key",
        1,
    )
    .await
    .unwrap();
    // The report the coordinator built for this job before AG-178.
    let input = fingerprint(&job).unwrap();
    let legacy = ReplayEconomicReportV1 {
        schema_version: REPORT_SCHEMA.into(),
        id: replay_economic_report_id(REPORT_SCHEMA, &input).unwrap(),
        experiment_id: experiment.id.clone(),
        experiment_digest: experiment.digest().unwrap(),
        paired_ticket_id: experiment.paired_ticket_id.clone(),
        paired_ticket_digest: experiment.paired_ticket_digest.clone(),
        paired_receipt_closure_digest: None,
        completed_paired_tasks: 0,
        paired_units: vec![],
        latency: None,
        quality_outcome: PairedQualityOutcome::ZeroOrInconclusive,
        cost_receipt_ids: vec![],
        economics: EconomicComputation::Blocked {
            reasons: vec!["no_cost_receipts".into()],
        },
        terminal: EconomicReportTerminal::BlockedSupport,
        reasons: job.blocked_reasons.clone(),
    };
    legacy.validate_against(&experiment).unwrap();
    let mut indexed = job.clone();
    indexed.report_id = Some(legacy.id.clone());
    put_economic_record(
        &store,
        &admin,
        LEGACY_REPORT_KIND,
        &legacy.id,
        evaluator.actor(),
        &legacy,
    )
    .await;
    put_economic_record(
        &store,
        &admin,
        "replay_economic_job_v1",
        &job.id,
        evaluator.actor(),
        &indexed,
    )
    .await;

    let successor = replay_economic_report_id(REPORT_SCHEMA_V2, &input).unwrap();
    let read = PersistentReplayEconomicCoordinator::query_report(&evaluator, &store, &legacy.id)
        .await
        .unwrap();
    assert!(
        !read.replay_read_view.source_legacy,
        "the selection is current"
    );
    assert!(read.source_legacy, "the v1 report itself is legacy");
    assert_eq!(read.report_state, ReplayRecordState::Legacy);
    assert_eq!(read.report_schema_version, REPORT_SCHEMA);
    assert_eq!(read.raw_payload, serde_json::to_value(&legacy).unwrap());
    assert_eq!(
        lineage_of(&read),
        [
            (REPORT_SCHEMA, legacy.id.as_str(), true),
            (REPORT_SCHEMA_V2, successor.as_str(), false),
        ]
    );
    assert!(matches!(
        PersistentReplayEconomicCoordinator::build_blocked_report(&evaluator, &store, &job.id, 10)
            .await,
        Err(Error::Invalid(reason)) if reason == "legacy_economic_report_not_consumable"
    ));

    let fresh = PersistentReplayEconomicCoordinator::start(
        &evaluator,
        &store,
        &experiment.id,
        "current-key",
        1,
    )
    .await
    .unwrap();
    let fresh_input = fingerprint(&fresh).unwrap();
    let report = PersistentReplayEconomicCoordinator::build_blocked_report(
        &evaluator, &store, &fresh.id, 10,
    )
    .await
    .unwrap();
    assert_eq!(report.schema_version, REPORT_SCHEMA_V2);
    assert_eq!(
        report.id,
        replay_economic_report_id(REPORT_SCHEMA_V2, &fresh_input).unwrap()
    );
    let predecessor = replay_economic_report_id(REPORT_SCHEMA, &fresh_input).unwrap();
    assert_ne!(report.id, predecessor);
    assert!(report.cluster_rows.is_empty());
    let statistics = &report.quality_statistics;
    assert_eq!(statistics.stats_version, "rsia.empirical_bernstein.v2");
    assert_eq!(statistics.verdict, Verdict::Invalid);
    assert_eq!(statistics.reasons, ["n<2"]);
    assert_eq!(statistics.alpha_i, "0.05");
    assert_eq!((statistics.lcb_micros, statistics.ucb_micros), (None, None));
    assert_eq!(report.terminal, EconomicReportTerminal::BlockedSupport);
    report.validate_against(&experiment).unwrap();
    let current = PersistentReplayEconomicCoordinator::query_report(&evaluator, &store, &report.id)
        .await
        .unwrap();
    assert_eq!(current.report_state, ReplayRecordState::CurrentVerified);
    assert!(!current.source_legacy);
    assert_eq!(current.report_schema_version, REPORT_SCHEMA_V2);
    assert_eq!(current.raw_payload, serde_json::to_value(&report).unwrap());
    assert_eq!(
        lineage_of(&current),
        [
            (REPORT_SCHEMA, predecessor.as_str(), false),
            (REPORT_SCHEMA_V2, report.id.as_str(), true),
        ]
    );
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .object_row(
                &admin,
                "artifact",
                &economic_storage_id(CURRENT_REPORT_KIND, &report.id)
            )
            .await
            .unwrap()
            .is_some()
    );
    session.commit().await.unwrap();
    assert_eq!(
        PersistentReplayEconomicCoordinator::status(&evaluator, &store, &fresh.id)
            .await
            .unwrap()
            .report_id,
        Some(report.id.clone())
    );
}

// AG163 append-only prepared tests. The software fixture proves no provider,
// paid-effect, independent-process or promotion authority.
#[cfg(unix)]
struct Ag163History {
    store: Store,
    path: std::path::PathBuf,
    original: serde_json::Value,
    _dir: tempfile::TempDir,
}

#[cfg(unix)]
async fn ag163_history() -> Ag163History {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let capture = include_bytes!("fixtures/ag163_formal_v2_history.json");
    assert_eq!(
        evo_core::hash(capture),
        "2bc4ecd85ad308cd888067c797391473f43814a0a39988b6035de93dc84a2e79"
    );
    let all: serde_json::Value = serde_json::from_slice(capture).unwrap();
    assert_eq!(
        all["producer_commit"],
        "800eafc4ccaf39d5e4f3fb1b7891f9c142e6b482"
    );
    assert_eq!(all["software_fixture_only"], true);
    assert_eq!(all["real_provider_or_paid_or_OS_authority_proof"], false);
    let original = all["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "ag163-old-complete-v2")
        .unwrap()
        .clone();
    let raw = original["database_hex"].as_str().unwrap();
    assert_eq!(raw.len() % 2, 0);
    let bytes: Vec<u8> = raw
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    assert_eq!(
        bytes.len() as u64,
        original["database_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        evo_core::hash(&bytes),
        original["database_sha256"].as_str().unwrap()
    );
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&private)
        .unwrap();
    let path = private.join("ag163.sqlite3");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    Ag163History {
        _dir: dir,
        path,
        store,
        original,
    }
}

#[cfg(unix)]
fn ag163_sid(kind: &str, id: &str) -> String {
    format!("e05-{}", evo_core::fingerprint(&(kind, id)).unwrap())
}

#[cfg(unix)]
async fn ag163_body(store: &Store, context: &Context, kind: &str, id: &str) -> String {
    let mut session = store.session().await.unwrap();
    let row = session
        .object_row(context, kind, id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    row.body
}

#[cfg(unix)]
fn ag163_image(path: &std::path::Path) -> serde_json::Value {
    let program = r#"
import json,sqlite3,struct,sys,urllib.parse
def qi(s):return '"'+s.replace('"','""')+'"'
def cell(k,v):
 if k==b'null':return ['null']
 if k==b'integer':return ['integer',str(v)]
 if k==b'real':return ['real',struct.pack('>d',v).hex()]
 if k in (b'text',b'blob'):return [k.decode(),v.hex()]
 raise AssertionError(k)
c=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1],safe='/')+'?mode=ro',uri=True,isolation_level=None);c.text_factory=bytes
c.execute('PRAGMA query_only=ON');c.execute('BEGIN');assert c.execute('PRAGMA integrity_check').fetchall()==[(b'ok',)]
schema=c.execute('SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY').fetchall();tables=[]
for typ,n,_,_,ddl in schema:
 if typ!=b'table':continue
 name=n.decode();info=c.execute('PRAGMA table_xinfo('+qi(name)+')').fetchall();cols=[x[1].decode() for x in info if x[6]!=1]
 rowid=b'WITHOUT ROWID' not in (ddl or b'').upper();fields=['_rowid_'] if rowid else []
 for col in cols:fields.extend(['typeof('+qi(col)+')',qi(col)])
 rows=[]
 for v in c.execute('SELECT '+','.join(fields)+' FROM '+qi(name)):
  off=1 if rowid else 0;rows.append([str(v[0]) if rowid else None,[cell(v[off+2*i],v[off+2*i+1]) for i in range(len(cols))]])
 rows.sort(key=lambda v:json.dumps(v,separators=(',',':')));tables.append({'name':name,'columns':cols,'table_xinfo':[[cell(b'null',None) if item is None else cell(b'integer',item) if isinstance(item,int) else cell(b'text',item) for item in row] for row in info],'rowid':rowid,'rows':rows})
calls=c.execute('SELECT state,actual_cost_micros,execution_closed FROM root_budget_calls ORDER BY billing_scope,call_id').fetchall()
c.execute('ROLLBACK');c.close();print(json.dumps({'schema':[[a.hex(),b.hex(),d.hex(),p,None if sql is None else sql.hex()] for a,b,d,p,sql in schema],'tables':tables,'calls':[[s.decode(),a,x] for s,a,x in calls]},sort_keys=True,separators=(',',':')))
"#;
    let output = std::process::Command::new("python3")
        .args(["-I", "-B", "-c", program])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned readonly full database image failed"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The full image includes SQL storage classes, original TEXT/BLOB bytes,
/// schema, physical rowids and sqlite_sequence. New action records may append;
/// no pre-existing object, dependency, audit or accounting row may change.
#[cfg(unix)]
fn ag163_preserves_original_rows(before: &serde_json::Value, after: &serde_json::Value) {
    assert!(
        before["schema"] == after["schema"],
        "AG163 changed original full schema"
    );
    for old in before["tables"].as_array().unwrap() {
        let name = old["name"].as_str().unwrap();
        let new = after["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap();
        if name == "sqlite_sequence" {
            assert!(
                old["columns"] == new["columns"],
                "sqlite_sequence schema changed"
            );
            for row in old["rows"].as_array().unwrap() {
                let successor = new["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v[1][0] == row[1][0])
                    .unwrap();
                assert!(
                    successor[0] == row[0] && successor[1][0] == row[1][0],
                    "original sqlite_sequence identity changed"
                );
                assert_eq!(row[1][1][0], "integer");
                assert_eq!(successor[1][1][0], "integer");
                let before_seq: i64 = row[1][1][1].as_str().unwrap().parse().unwrap();
                let after_seq: i64 = successor[1][1][1].as_str().unwrap().parse().unwrap();
                assert!(
                    after_seq >= before_seq,
                    "actual append reset an original sqlite_sequence counter"
                );
            }
            continue;
        }
        if [
            "revoke_watermark",
            "revoke_cleanup_jobs",
            "revoke_cleanup_frontier",
        ]
        .contains(&name)
        {
            continue; // Only these exact real cleanup state tables may advance.
        }
        assert!(
            old["columns"] == new["columns"],
            "AG163 changed original columns"
        );
        let old_rows = old["rows"].as_array().unwrap();
        let new_rows = new["rows"].as_array().unwrap();
        assert!(
            old_rows.iter().all(|row| new_rows.contains(row)),
            "AG163 changed original rowid or typed bytes"
        );
        if name.starts_with("root_budget") {
            assert!(
                old_rows == new_rows,
                "AG163 changed original spent/None/late/closed budget facts"
            );
        }
    }
    assert!(
        before["calls"] == after["calls"],
        "AG163 reopened execution or changed actual call facts"
    );
}

#[cfg(unix)]
async fn ag163_legacy_health(fx: &Ag163History, evaluator: &Context) {
    let before = ag163_image(&fx.path);
    let ticket = fx.original["ticket_id"].as_str().unwrap();
    let body = ag163_body(
        &fx.store,
        evaluator,
        "artifact",
        &ag163_sid("formal_evaluation_v2", ticket),
    )
    .await;
    assert_eq!(
        evo_core::hash(body.as_bytes()),
        fx.original["original_formal_body_sha256"].as_str().unwrap()
    );
    let mut session = fx.store.session().await.unwrap();
    let read = evo_engine::streaming_evaluator::read_verified_formal_report_in_session(
        evaluator,
        &mut session,
        ticket,
    )
    .await
    .unwrap();
    assert_eq!(read.state(), evo_storage::PersistentContractState::Legacy);
    assert!(!read.historical_view().promotion_eligible);
    assert!(
        matches!(read.current_view(), Err(Error::Conflict(reason)) if reason == "source_legacy: formal report is not Current input")
    );
    session.commit().await.unwrap();
    assert!(
        ag163_image(&fx.path) == before,
        "AG163 healthy public Legacy reader changed full database"
    );
}

// Original setup public producers; no economic registration occurs here.
#[cfg(unix)]
async fn ag163_pairing_setup(
    fx: &Ag163History,
    evaluator: &Context,
    admin: &Context,
) -> ReplayEconomicExperimentV1 {
    let store = fx.store.clone();
    let evaluator = evaluator.clone();
    let admin = admin.clone();
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
            &Context::new(evaluator.namespace(), "host", Role::Host).unwrap(),
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
            &Context::new(evaluator.namespace(), "host", Role::Host).unwrap(),
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
    // The qualified formal source already owns watermark 1. Advancing it
    // would change the frozen candidate binding, so retain its actual value.
    assert_eq!(session.watermark(&admin).await.unwrap().unwrap().0, 1);
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
    let stored_report = StoredReplayReportV2::build(
        evaluator.namespace(),
        WorldPartition::Select,
        &pool,
        policy,
        profile,
        caps,
        vec![replay],
    )
    .unwrap();
    evo_storage::replay::put_current_replay_report(&admin, &store, &stored_report)
        .await
        .unwrap();
    let mut experiment = experiment(&stored_report);
    let status = evo_engine::streaming_evaluator::IndependentEvaluationControl::status(
        &evaluator,
        &store,
        fx.original["ticket_id"].as_str().unwrap(),
    )
    .await
    .unwrap();
    let ticket = status.ticket;
    experiment.id = "ag163-paired-legacy-economic".into();
    experiment.paired_ticket_id = ticket.id.clone();
    experiment.paired_ticket_digest = fingerprint(&ticket).unwrap();
    experiment.evaluator_actor = ticket.evaluator_actor.clone();
    experiment.executor_actor = ticket.executor_actor.clone();
    experiment.proposer_actor = ticket.proposer_actor.clone();
    experiment.approver_actor = ticket.approver_actor.clone();
    experiment.runtime.candidate_bundle_digest = ticket.candidate_bundle_digest.clone();
    experiment.runtime.baseline_bundle_digest = ticket.baseline_bundle_digest.clone();
    experiment.runtime.environment_digest = ticket.environment_digest.clone();
    experiment.validate().unwrap();
    experiment
}

#[cfg(unix)]
#[tokio::test]
async fn paired_legacy_formal_report_is_not_a_new_economic_input() {
    let fx = ag163_history().await;
    let namespace = fx.original["namespace"].as_str().unwrap();
    let evaluator = Context::new(
        namespace,
        fx.original["evaluator_actor"].as_str().unwrap(),
        Role::Evaluator,
    )
    .unwrap();
    let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
    ag163_legacy_health(&fx, &evaluator).await;
    let original = ag163_image(&fx.path);
    // Actual public source/world/pool/replay producers establish a Current
    // selection. No paired economic fact is pre-created or imported by SQL.
    let experiment = ag163_pairing_setup(&fx, &evaluator, &admin).await;
    let selection = evo_engine::replay::query_replay_report_record(
        &evaluator,
        &fx.store,
        &experiment.replay_selection.report_artifact_id,
    )
    .await
    .unwrap();
    assert_eq!(selection.record.state(), ReplayRecordState::CurrentVerified);
    assert!(!selection.source_legacy);
    let status = evo_engine::streaming_evaluator::IndependentEvaluationControl::status(
        &evaluator,
        &fx.store,
        &experiment.paired_ticket_id,
    )
    .await
    .unwrap();
    let ticket = status.ticket;
    assert_eq!(experiment.paired_ticket_id, ticket.id);
    assert_eq!(
        experiment.paired_ticket_digest,
        fingerprint(&ticket).unwrap()
    );
    assert_eq!(experiment.evaluator_actor, ticket.evaluator_actor);
    assert_eq!(experiment.executor_actor, ticket.executor_actor);
    assert_eq!(experiment.proposer_actor, ticket.proposer_actor);
    assert_eq!(experiment.approver_actor, ticket.approver_actor);
    assert_eq!(
        experiment.runtime.candidate_bundle_digest,
        ticket.candidate_bundle_digest
    );
    assert_eq!(
        experiment.runtime.baseline_bundle_digest,
        ticket.baseline_bundle_digest
    );
    assert_eq!(
        experiment.runtime.environment_digest,
        ticket.environment_digest
    );
    ag163_legacy_health(&fx, &evaluator).await;
    let before = ag163_image(&fx.path);
    let refused =
        PersistentReplayEconomicCoordinator::register(&evaluator, &fx.store, experiment).await;
    assert!(
        matches!(refused, Err(Error::Conflict(reason)) if reason == "source_legacy: formal report is not Current input"),
        "known paired Legacy input was consumed or rejected by an unrelated setup failure"
    );
    assert!(
        ag163_image(&fx.path) == before,
        "paired Legacy registration wrote experiment/audit/budget or any other database fact"
    );
    ag163_preserves_original_rows(&original, &ag163_image(&fx.path));
    fx.store.close().await;
    let reopened = Store::open(&fx.path).await.unwrap();
    let reopened_before = ag163_image(&fx.path);
    let mut session = reopened.session().await.unwrap();
    let read = evo_engine::streaming_evaluator::read_verified_formal_report_in_session(
        &evaluator,
        &mut session,
        fx.original["ticket_id"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(read.state(), evo_storage::PersistentContractState::Legacy);
    assert!(!read.historical_view().promotion_eligible);
    session.commit().await.unwrap();
    assert!(
        ag163_image(&fx.path) == reopened_before,
        "rejected registration or restart refreshed original Legacy facts"
    );
}

// Actual ee5 closed bytes, separate from the original 800e history domain.
// Authored before independent 2c23 reader qualification; this source is not
// evidence that that reader, this H test or any product gate has passed.
#[cfg(unix)]
#[tokio::test]
async fn derived_economic_audit_propagates_real_formal_source_legacy() {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let bytes = include_bytes!("fixtures/ag163_formal_v2_history.json");
    assert_eq!(
        evo_core::hash(bytes),
        "2bc4ecd85ad308cd888067c797391473f43814a0a39988b6035de93dc84a2e79"
    );
    let wrapper: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        wrapper["producer_commit"],
        "800eafc4ccaf39d5e4f3fb1b7891f9c142e6b482"
    );
    let sample = &wrapper["paired_economic_history"];
    assert_eq!(
        sample["schema_version"],
        "ag163.real_paired_economic_history_fixture.v1"
    );
    assert_eq!(
        sample["producer_commit"],
        "ee5ecf35076476c396a804cf6c34227add695d49"
    );
    assert_eq!(sample["name"], "ag163-real-paired-economic-history");
    let metadata = &sample["capture"];
    assert!(metadata["producer_commit"] == sample["producer_commit"]);
    assert_eq!(
        metadata["schema_version"],
        "ag163.real_paired_economic_history_capture.v1"
    );
    assert_eq!(metadata["software_fixture_only"], true);
    assert_eq!(metadata["external_provider_calls"], 0);
    assert_eq!(metadata["real_paid_or_4uid_authority_claimed"], false);
    assert_eq!(
        metadata["formal_old_api_evidence"],
        "active_domain_full_public_verification_passed"
    );
    assert_eq!(metadata["formal_state_enum_available_at_producer"], false);
    assert_eq!(
        metadata["old_economic_audit"]["report_state"],
        "current_verified"
    );
    assert_eq!(metadata["old_economic_audit"]["source_legacy"], false);
    let hex = sample["database_hex"].as_str().unwrap();
    assert_eq!(hex.len() % 2, 0);
    let original_database: Vec<u8> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    assert_eq!(original_database.len(), 438_272);
    assert_eq!(
        original_database.len() as u64,
        sample["database_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        evo_core::hash(&original_database),
        "48a20070638278b703dafa3acdbf4e132b06b2918821707137b2ce952b7e34cd"
    );
    assert!(sample["database_sha256"].as_str().unwrap() == evo_core::hash(&original_database));
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&private)
        .unwrap();
    let path = private.join("ag163-economic.sqlite3");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&original_database).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert!(
        std::fs::read(&path).unwrap() == original_database,
        "economic fixture was not the original closed database bytes"
    );
    let before = ag163_image(&path);
    assert!(
        before == sample["all_tables_typed"],
        "original schema/table_xinfo/typed values/rowid/sequence/fees differ before Store open"
    );
    assert_eq!(before["tables"].as_array().unwrap().len(), 22);
    assert!(
        before["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|table| table["name"] == "sqlite_sequence")
    );
    assert_eq!(before["calls"].as_array().unwrap().len(), 6);
    assert!(
        before["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call[0] == "finalized" && call[1] == 10 && call[2] == 1),
        "original six paid software facts were not finalized/10/closed"
    );
    let namespace = metadata["namespace"].as_str().unwrap();
    let actor = metadata["evaluator_actor"].as_str().unwrap();
    let ticket_id = metadata["ticket_id"].as_str().unwrap();
    let experiment_id = metadata["experiment"]["id"].as_str().unwrap();
    let report_id = metadata["report"]["id"].as_str().unwrap();
    let selection_id = metadata["selection"]["report_id"].as_str().unwrap();
    let job_id = metadata["job"]["id"].as_str().unwrap();
    assert!(
        metadata["experiment"]["paired_ticket_id"] == ticket_id
            && metadata["report"]["paired_ticket_id"] == ticket_id
            && metadata["report"]["experiment_id"] == experiment_id
    );
    let evaluator = Context::new(namespace, actor, Role::Evaluator).unwrap();
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    let assert_unchanged_except_0008 = |image: &serde_json::Value, action: &str| {
        let before_tables = before["tables"].as_array().unwrap();
        let after_tables = image["tables"].as_array().unwrap();
        for old in before_tables {
            let name = old["name"].as_str().unwrap();
            if name == "_sqlx_migrations" {
                continue;
            }
            let new = after_tables
                .iter()
                .find(|table| table["name"] == old["name"])
                .unwrap_or_else(|| panic!("table {name} disappeared at {action}"));
            assert_eq!(old, new, "{action} refreshed {name}");
        }
        // Opening records migration 0008: three empty root stage plan tables.
        // The ledger row and those tables are schema metadata; every pre-0008
        // table bytes and the financial call rows stay identical.
        assert_eq!(after_tables.len(), before_tables.len() + 3);
        assert_eq!(image["calls"], before["calls"], "{action} changed calls");
    };
    let after = ag163_image(&path);
    assert_unchanged_except_0008(&after, "opening the original economic history");
    let status = evo_engine::streaming_evaluator::IndependentEvaluationControl::status(
        &evaluator, &store, ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.schema_version, "rsia.evaluation_ticket.v2");
    assert!(
        serde_json::to_value(&status.ticket).unwrap() == metadata["ticket"],
        "current reader changed the original ticket payload"
    );
    assert!(
        status.ticket.candidate_dependency_binding.is_some()
            && status.ticket.source_plan_binding.is_none()
    );
    let mut session = store.session().await.unwrap();
    let formal = evo_engine::streaming_evaluator::read_verified_formal_report_in_session(
        &evaluator,
        &mut session,
        ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(formal.state(), evo_storage::PersistentContractState::Legacy);
    assert_eq!(
        formal.historical_view().variant,
        evo_engine::streaming_evaluator::VerifiedReportVariant::CompleteBatch
    );
    assert_eq!(
        formal.historical_view().evidence_scope,
        evo_engine::streaming_evaluator::EvaluationEvidenceScope::ProgramFixture
    );
    assert!(!formal.historical_view().promotion_eligible);
    assert!(matches!(formal.current_view(), Err(Error::Conflict(reason))
        if reason == "source_legacy: formal report is not Current input"));
    assert_eq!(metadata["original_rows"].as_array().unwrap().len(), 6);
    for original in metadata["original_rows"].as_array().unwrap() {
        let row = session
            .object_row(
                &evaluator,
                original["kind"].as_str().unwrap(),
                original["physical_id"].as_str().unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.id == original["physical_id"].as_str().unwrap()
                && row.owner == original["owner"].as_str().unwrap(),
            "a real original ticket/formal/selection/experiment/job/report identity changed"
        );
        assert!(
            row.body == original["body_utf8"].as_str().unwrap(),
            "one of six original raw TEXT records changed"
        );
        assert!(
            evo_core::hash(row.body.as_bytes()) == original["body_sha256"].as_str().unwrap(),
            "one of six original raw TEXT hashes changed"
        );
    }
    session.commit().await.unwrap();
    let selection =
        evo_engine::replay::query_replay_report_record(&evaluator, &store, selection_id)
            .await
            .unwrap();
    assert_eq!(selection.record.state(), ReplayRecordState::CurrentVerified);
    assert!(!selection.source_legacy);
    assert!(
        serde_json::to_value(selection.record.current_report().unwrap()).unwrap()
            == metadata["selection"],
        "the original Current selection payload changed"
    );
    let audit = PersistentReplayEconomicCoordinator::query_report(&evaluator, &store, report_id)
        .await
        .unwrap();
    assert_eq!(audit.report_state, ReplayRecordState::CurrentVerified);
    assert_eq!(
        audit.report_schema_version,
        "rsia.replay_economic_report.v2"
    );
    assert_eq!(
        audit.replay_read_view.record.state(),
        ReplayRecordState::CurrentVerified
    );
    assert!(!audit.replay_read_view.source_legacy);
    assert!(
        audit.raw_payload == metadata["report"],
        "the original economic report payload was rewritten"
    );
    assert!(
        audit.source_legacy,
        "a real paired Legacy E05 source did not propagate to the existing economic audit"
    );
    let durable_status = PersistentReplayEconomicCoordinator::status(&evaluator, &store, job_id)
        .await
        .unwrap();
    assert!(
        serde_json::to_value(&durable_status).unwrap() == metadata["status"],
        "economic audit changed the original indexed job, outcome or cost facts"
    );
    assert_unchanged_except_0008(
        &ag163_image(&path),
        "public readers of the original typed facts",
    );
    store.close().await;
    assert_unchanged_except_0008(
        &ag163_image(&path),
        "normal close of the original typed tables",
    );
}
