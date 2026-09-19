use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract, ProfileKind,
};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, ExposureLedger, ExposureState,
    FrozenCandidatePair, HoldoutManifest, MonetaryReservationState, SequentialMode,
};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, ExecutionReceiptRequest, ExecutionReceiptV2, ExecutionSide,
    FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry,
    GraderReceiptRequest, IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord,
    ProtectedInputRef, RegisteredEvaluationControl, StartExecutionRequest, TargetState,
    TicketLifecycle, verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use serde_json::json;
use std::path::PathBuf;

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        amount: amount.into(),
    }
}

fn optimizer(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(implementation),
        initial_s0_digest: d("s0"),
        base_model_digest: d("model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: "billing-1".into(),
        budget_limit: money("0.001"),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
    store: Store,
    evaluator: Context,
    worker: Context,
    ticket_id: String,
    holdout: ProtectedHoldoutRecord,
    control: RegisteredEvaluationControl,
}

async fn stored_execution_receipt(
    store: &Store,
    context: &Context,
    receipt_id: &str,
) -> ExecutionReceiptV2 {
    let storage_id = format!(
        "e05-{}",
        fingerprint(&("execution_receipt_v2", receipt_id)).unwrap()
    );
    let mut session = store.session().await.unwrap();
    let envelope: serde_json::Value = session
        .need(context, "artifact", &storage_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    serde_json::from_value(envelope["payload"].clone()).unwrap()
}

async fn stored_output_envelope(
    store: &Store,
    context: &Context,
    output_id: &str,
) -> serde_json::Value {
    let storage_id = format!(
        "e05-{}",
        fingerprint(&("evaluation_execution_output_v1", output_id)).unwrap()
    );
    let mut session = store.session().await.unwrap();
    let envelope = session
        .need(context, "artifact", &storage_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    envelope
}

async fn stored_ledger(store: &Store, context: &Context, family_id: &str) -> ExposureLedger {
    let storage_id = format!(
        "e05-{}",
        fingerprint(&("exposure_ledger_v1", family_id)).unwrap()
    );
    let mut session = store.session().await.unwrap();
    let envelope: serde_json::Value = session
        .need(context, "artifact", &storage_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    serde_json::from_value(envelope["payload"].clone()).unwrap()
}

async fn fixture(n: u32, sequential: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("e05.sqlite3");
    let store = Store::open(&db_path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let worker = Context::new("n", "executor", Role::Worker).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "billing-1".into(),
                allowed_namespaces: vec!["n".into(), "other".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: d("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05").unwrap();
    v1.n_planned = n as usize;
    v1.monetary_budget = money("0.001");
    v1.frozen = true;
    v1.frozen_at = Some(1);
    v1.validate().unwrap();

    let alpha = ResearchFamilyAlphaPlan::new(
        "family-e05",
        "0.05",
        vec![
            AlphaAllocation {
                claim_id: "fixed-gain".into(),
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
    .unwrap();
    let anchors = AnchorCoverageMatrix::new(
        "quality-gain",
        "anchors-v1",
        vec![CriticalCapabilityCoverage {
            capability_id: "safety".into(),
            anchor_ids: vec!["anchor-1".into()],
        }],
    )
    .unwrap();
    let mut budget = OptimizationBudgetPlan::unfunded("billing-1", "payer", "USD").unwrap();
    budget.root_total = money("0.001");
    budget.admin_authorization_receipt_digest = Some(d("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    assert!(v1.conditions.contains(&ControlCondition::FixedImproverC));
    let formal = FormalExperimentPlanV41::new(
        &v1,
        d("preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        d("sampling"),
        d("common-mean"),
        FormalStatisticalUnit::IndependentTaskCluster,
        n,
    )
    .unwrap();
    let ordered_clusters: Vec<_> = (0..n).map(|index| format!("cluster-{index}")).collect();
    let early = sequential.then(|| {
        EarlyStopPlan::new(
            v1.digest().unwrap(),
            &alpha,
            "early-reject",
            "attempt-1",
            ProfileKind::QualityGain,
            n,
            ordered_clusters.clone(),
            d("common-mean"),
            d("sampling"),
            anchors.digest().unwrap(),
            20_000,
            10_000,
            10_000,
        )
        .unwrap()
    });
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let control = RegisteredEvaluationControl {
        schema_version: RegisteredEvaluationControl::SCHEMA.into(),
        id: "control-1".into(),
        v1_plan_snapshot: v1.clone(),
        formal_plan: formal,
        alpha_plan: alpha.clone(),
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: early.clone(),
        anchor_matrix: anchors.clone(),
        attempt_id: "attempt-1".into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: "billing-1".into(),
        executor_actor: "executor".into(),
        evaluator_actor: "evaluator".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        oracle_digest: d("oracle-version"),
        grader_digest: evo_core::fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        evidence_scope: EvaluationEvidenceScope::ProgramFixture,
        created_at_unix_seconds: 2,
    };
    IndependentEvaluationControl::register_control(&evaluator, &store, control.clone())
        .await
        .unwrap();

    // The candidate pair binds the actual E01 snapshot, not an arbitrary label.
    let pair =
        FrozenCandidatePair::new(v1.digest().unwrap(), d("candidate"), d("baseline"), 3).unwrap();
    let sequential_mode = match &early {
        Some(plan) => SequentialMode::RejectOnly {
            early_stop_plan_digest: plan.digest().unwrap(),
        },
        None => SequentialMode::Disabled,
    };
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "family-e05",
        alpha.digest().unwrap(),
        sequential_mode,
        "epoch-1",
        "slice-1",
        ordered_clusters.clone(),
        ordered_clusters
            .iter()
            .map(|id| d(&format!("source-{id}")))
            .collect(),
        "sampler-v1",
        d("oracle-version"),
        anchors.digest().unwrap(),
        evo_core::fingerprint(&FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        })
        .unwrap(),
        d("environment"),
        d("private-seed"),
        4,
    )
    .unwrap();
    let rotating_inputs: Vec<_> = ordered_clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| ProtectedInputRef {
            target_id: format!("r{index}"),
            cluster_or_anchor_id: cluster.clone(),
            input_artifact_id: format!("input-r{index}"),
            input_digest: d(&format!("input-r{index}")),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: d("input-a0"),
    }];
    let oracle_entries: Vec<_> = rotating_inputs
        .iter()
        .chain(anchor_inputs.iter())
        .map(|input| FrozenOracleEntry {
            target_id: input.target_id.clone(),
            expected_answer_json: json!("ok").to_string(),
        })
        .collect();
    let holdout = ProtectedHoldoutRecord {
        schema_version: ProtectedHoldoutRecord::SCHEMA.into(),
        id: "holdout-1".into(),
        registration_id: "control-1".into(),
        pair,
        manifest,
        candidate_bundle_digest: d("candidate-bundle"),
        baseline_bundle_digest: d("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    IndependentEvaluationControl::register_holdout(&evaluator, &store, holdout.clone())
        .await
        .unwrap();
    let ticket = IndependentEvaluationControl::issue_ticket(
        &evaluator,
        &store,
        IssueTicketRequest {
            ticket_id: "ticket-1".into(),
            registration_id: "control-1".into(),
            holdout_id: "holdout-1".into(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
    .unwrap();
    Fixture {
        _dir: dir,
        db_path,
        store,
        evaluator,
        worker,
        ticket_id: ticket.id,
        holdout,
        control,
    }
}

async fn execute_output(
    fixture: &Fixture,
    target_id: &str,
    side: ExecutionSide,
    output_utf8: &str,
    ordinal: u32,
) -> String {
    let view = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        &fixture.ticket_id,
        target_id,
        side,
    )
    .await
    .unwrap();
    let broker_json = serde_json::to_string(&view).unwrap();
    assert!(!broker_json.contains("oracle"));
    assert!(!broker_json.contains("grader"));
    assert!(!broker_json.contains("\"ok\""));
    let call_id = format!("call-{target_id}-{side:?}-{ordinal}").to_lowercase();
    let lease = format!("lease-{ordinal}");
    let now = 10 + i64::from(ordinal) * 5;
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: call_id.clone(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: lease.clone(),
                lease_until: now + 1_000,
                now,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: call_id.clone(),
        actual_input_digest: view.request_digest.clone(),
        lease_token: lease,
        lease_epoch: 1,
        now: now + 1,
    };
    let dispatched = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    let receipt_id = format!("receipt-{target_id}-{side:?}-{ordinal}").to_lowercase();
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: receipt_id.clone(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            side,
            request_digest: view.request_digest,
            output_artifact_id: format!("output-{target_id}-{side:?}-{ordinal}").to_lowercase(),
            output_utf8: output_utf8.into(),
            budget_call_id: call_id.clone(),
            latency_micros: 100 + u64::from(ordinal),
            issued_at_unix_seconds: now + 2,
        },
    )
    .await
    .unwrap();
    let mut final_fence = fence;
    final_fence.now = now + 3;
    fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &final_fence,
            &UsageCharge {
                amount_micros: 10,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: format!("provider-{ordinal}"),
                usage_record_id: format!("usage-{ordinal}"),
                output_digest: receipt.output_digest,
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            "billing-1",
            &call_id,
            dispatched.call.dispatch_id.as_deref().unwrap(),
            "fixture_complete",
            now + 4,
        )
        .await
        .unwrap();
    receipt_id
}

async fn execute_side(
    fixture: &Fixture,
    target_id: &str,
    side: ExecutionSide,
    answer: &str,
    ordinal: u32,
) -> String {
    execute_output(
        fixture,
        target_id,
        side,
        &json!({"answer": answer}).to_string(),
        ordinal,
    )
    .await
}

struct PendingExecution {
    fence: BudgetCallFence,
    dispatch_id: String,
    call_id: String,
    target_id: String,
    side: ExecutionSide,
    request_digest: String,
    output_utf8: String,
    output_digest: String,
}

async fn start_without_receipt(
    fixture: &Fixture,
    target_id: &str,
    side: ExecutionSide,
    ordinal: u32,
) -> PendingExecution {
    let view = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        &fixture.ticket_id,
        target_id,
        side,
    )
    .await
    .unwrap();
    let call_id = format!("pending-{target_id}-{side:?}-{ordinal}").to_lowercase();
    let lease = format!("pending-lease-{ordinal}");
    let now = 100 + i64::from(ordinal) * 5;
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: call_id.clone(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: lease.clone(),
                lease_until: now + 1_000,
                now,
            },
        )
        .await
        .unwrap();
    let request_digest = view.request_digest;
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: call_id.clone(),
        actual_input_digest: request_digest.clone(),
        lease_token: lease,
        lease_epoch: 1,
        now: now + 1,
    };
    let dispatched = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    let output_utf8 = json!({"answer":"late"}).to_string();
    PendingExecution {
        fence,
        dispatch_id: dispatched.call.dispatch_id.unwrap(),
        call_id,
        target_id: target_id.into(),
        side,
        request_digest,
        output_digest: hash(output_utf8.as_bytes()),
        output_utf8,
    }
}

async fn reconcile_pending(fixture: &Fixture, pending: &PendingExecution, now: i64) {
    let mut fence = pending.fence.clone();
    fence.now = now;
    fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &fence,
            &UsageCharge {
                amount_micros: 10,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: format!("provider-pending-{now}"),
                usage_record_id: format!("usage-pending-{now}"),
                output_digest: pending.output_digest.clone(),
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            "billing-1",
            &pending.call_id,
            &pending.dispatch_id,
            "late_usage_reconciled",
            now + 1,
        )
        .await
        .unwrap();
}

async fn grade_target(
    fixture: &Fixture,
    target_id: &str,
    candidate_answer: &str,
    baseline_answer: &str,
    ordinal: u32,
) -> String {
    let candidate = execute_side(
        fixture,
        target_id,
        ExecutionSide::Candidate,
        candidate_answer,
        ordinal * 2,
    )
    .await;
    let baseline = execute_side(
        fixture,
        target_id,
        ExecutionSide::Baseline,
        baseline_answer,
        ordinal * 2 + 1,
    )
    .await;
    let receipt_id = format!("grade-{target_id}-{ordinal}");
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: receipt_id.clone(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 500 + i64::from(ordinal),
        },
    )
    .await
    .unwrap();
    receipt_id
}

#[tokio::test]
async fn out_of_order_stop_is_persistent_and_blocks_new_dispatch() {
    let fixture = fixture(14, true).await;
    grade_target(&fixture, "r13", "ok", "wrong", 1).await;
    for index in 0..12 {
        grade_target(&fixture, &format!("r{index}"), "wrong", "ok", index + 2).await;
    }
    let r12_candidate = execute_side(&fixture, "r12", ExecutionSide::Candidate, "wrong", 80).await;
    let r12_baseline = execute_side(&fixture, "r12", ExecutionSide::Baseline, "ok", 81).await;
    // This call is truly dispatched by E04 and persistently bound to the
    // anchor slot, but no execution receipt has returned when r12 triggers stop.
    let pending = start_without_receipt(&fixture, "a0", ExecutionSide::Candidate, 90).await;
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: "grade-r12-stop".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: "r12".into(),
            candidate_execution_receipt_id: r12_candidate,
            baseline_execution_receipt_id: r12_baseline,
            issued_at_unix_seconds: 1_000,
        },
    )
    .await
    .unwrap();
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Draining);
    assert!(status.formal.is_none());
    // Reopening the database preserves the dispatched-without-receipt state.
    let reopened = Store::open(&fixture.db_path).await.unwrap();
    let restarted =
        IndependentEvaluationControl::status(&fixture.evaluator, &reopened, &fixture.ticket_id)
            .await
            .unwrap();
    assert_eq!(restarted.ticket.lifecycle, TicketLifecycle::Draining);
    let late_receipt_id = "receipt-a0-candidate-late";
    IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &reopened,
        ExecutionReceiptRequest {
            receipt_id: late_receipt_id.into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: pending.target_id.clone(),
            side: pending.side,
            request_digest: pending.request_digest.clone(),
            output_artifact_id: "output-a0-candidate-late".into(),
            output_utf8: pending.output_utf8.clone(),
            budget_call_id: pending.call_id.clone(),
            latency_micros: 999,
            issued_at_unix_seconds: 1_100,
        },
    )
    .await
    .unwrap();
    let after_late =
        IndependentEvaluationControl::status(&fixture.evaluator, &reopened, &fixture.ticket_id)
            .await
            .unwrap();
    assert_eq!(after_late.ticket.lifecycle, TicketLifecycle::Draining);
    assert!(
        &after_late
            .ticket
            .late_execution_receipt_ids
            .iter()
            .any(|id| id == late_receipt_id)
    );
    assert!(matches!(
        after_late
            .ticket
            .targets
            .iter()
            .find(|target| target.target_id == "a0")
            .unwrap()
            .state,
        TargetState::RunningUsageUnknown { .. }
    ));
    let forged_late = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &reopened,
        ExecutionReceiptRequest {
            receipt_id: "receipt-a0-forged-late".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: pending.target_id.clone(),
            side: pending.side,
            request_digest: pending.request_digest.clone(),
            output_artifact_id: "output-a0-forged-late".into(),
            output_utf8: pending.output_utf8.clone(),
            budget_call_id: "wrong-call".into(),
            latency_micros: 1,
            issued_at_unix_seconds: 1_101,
        },
    )
    .await;
    assert!(forged_late.is_err());
    reopened.close().await;
    reconcile_pending(&fixture, &pending, 1_200).await;
    let status = IndependentEvaluationControl::settle_after_stop(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
        1_300,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    assert!(
        status
            .ticket
            .late_execution_receipt_ids
            .iter()
            .any(|id| id == late_receipt_id)
    );
    assert!(matches!(
        &status
            .ticket
            .targets
            .iter()
            .find(|target| target.target_id == "a0")
            .unwrap()
            .state,
        TargetState::CancelledAfterDispatch { .. }
    ));
    let FormalEvaluationV2::EarlyStopped { certificate, .. } = status.formal.unwrap() else {
        panic!("expected early-stopped formal evaluation");
    };
    assert_eq!(certificate.k(), 13);
    assert!(!certificate.usage_uncertain());
    let ledger = stored_ledger(
        &fixture.store,
        &fixture.evaluator,
        &fixture.control.alpha_plan.research_family_id,
    )
    .await;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id)
        .unwrap();
    assert!(matches!(entry.state, ExposureState::FeedbackUsed { .. }));
    assert_eq!(entry.monetary_state, MonetaryReservationState::Finalized);
    assert!(
        !FormalEvaluationV2::EarlyStopped {
            evidence_scope: EvaluationEvidenceScope::ProgramFixture,
            ticket_id: fixture.ticket_id.clone(),
            ticket_digest: d("synthetic"),
            certificate,
        }
        .is_promotable()
    );

    let blocked = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: "after-stop".into(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: d("after-stop"),
                request_artifact: None,
                max_cost_micros: 10,
                lease_token: "lease-after".into(),
                lease_until: 9_999,
                now: 900,
            },
        )
        .await;
    assert!(matches!(blocked, Err(Error::Cancelled)));

    let reconnected = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(reconnected.ticket.lifecycle, TicketLifecycle::EarlyStopped);
}

#[tokio::test]
async fn forged_bindings_and_score_rewrites_are_rejected() {
    let fixture = fixture(2, false).await;
    let view = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        &fixture.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let forged = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: "forged".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: "r0".into(),
            side: ExecutionSide::Candidate,
            request_digest: d("wrong-ticket-slice-environment"),
            output_artifact_id: "forged-output".into(),
            output_utf8: json!({"answer":"ok"}).to_string(),
            budget_call_id: "missing-call".into(),
            latency_micros: 1,
            issued_at_unix_seconds: 10,
        },
    )
    .await;
    assert!(forged.is_err());
    assert_ne!(view.request_digest, d("wrong-ticket-slice-environment"));

    let candidate = execute_side(&fixture, "r0", ExecutionSide::Candidate, "ok", 10).await;
    let baseline = execute_side(&fixture, "r0", ExecutionSide::Baseline, "wrong", 11).await;
    let request = GraderReceiptRequest {
        receipt_id: "grade-once".into(),
        ticket_id: fixture.ticket_id.clone(),
        target_id: "r0".into(),
        candidate_execution_receipt_id: candidate.clone(),
        baseline_execution_receipt_id: baseline.clone(),
        issued_at_unix_seconds: 100,
    };
    let first = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        request.clone(),
    )
    .await
    .unwrap();
    let same = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        request,
    )
    .await
    .unwrap();
    assert_eq!(
        first.scored_output_pair_digest,
        same.scored_output_pair_digest
    );
    let rewrite = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: "grade-rewrite".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: "r0".into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 101,
        },
    )
    .await;
    assert!(rewrite.is_err());
}

#[tokio::test]
async fn critical_anchor_stops_at_zero_and_complete_batch_uses_real_receipts() {
    let critical = fixture(2, true).await;
    grade_target(&critical, "a0", "wrong", "ok", 1).await;
    let status = IndependentEvaluationControl::status(
        &critical.evaluator,
        &critical.store,
        &critical.ticket_id,
    )
    .await
    .unwrap();
    let FormalEvaluationV2::EarlyStopped { certificate, .. } = status.formal.unwrap() else {
        panic!("critical anchor must early-stop");
    };
    assert_eq!(certificate.k(), 0);

    let complete = fixture(2, false).await;
    grade_target(&complete, "r0", "ok", "wrong", 10).await;
    grade_target(&complete, "r1", "ok", "wrong", 20).await;
    grade_target(&complete, "a0", "ok", "ok", 30).await;
    let formal = IndependentEvaluationControl::finalize_complete(
        &complete.evaluator,
        &complete.store,
        &complete.ticket_id,
        1_000,
    )
    .await
    .unwrap();
    let FormalEvaluationV2::CompleteBatch { report, .. } = &formal else {
        panic!("expected complete batch");
    };
    let report_json = serde_json::to_value(report).unwrap();
    for field in ["mean", "variance", "radius", "lcb", "alpha_i"] {
        assert!(report_json[field].is_string());
    }
    assert!(!formal.is_promotable());
    let view = IndependentEvaluationControl::verified_report_view(
        &complete.evaluator,
        &complete.store,
        &complete.ticket_id,
    )
    .await
    .unwrap();
    assert!(!view.promotion_eligible);
    assert!(
        view.ineligibility_reasons
            .iter()
            .any(|reason| reason == "complete_optimization_cost_not_verified")
    );
    let mut session = complete.store.session().await.unwrap();
    let in_transaction =
        verified_report_view_in_session(&complete.evaluator, &mut session, &complete.ticket_id)
            .await
            .unwrap();
    assert_eq!(in_transaction.report_digest, view.report_digest);
    session.commit().await.unwrap();
}

#[tokio::test]
async fn new_epoch_does_not_refund_query_or_alpha_claim() {
    let fixture = fixture(2, false).await;
    let mut second = fixture.holdout.clone();
    second.id = "holdout-2".into();
    second.manifest.epoch_id = "epoch-2".into();
    second.manifest.slice_id = "slice-2".into();
    second.manifest.issued_at_unix_ms = 6;
    IndependentEvaluationControl::register_holdout(&fixture.evaluator, &fixture.store, second)
        .await
        .unwrap();
    let issue = IndependentEvaluationControl::issue_ticket(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketRequest {
            ticket_id: "ticket-2".into(),
            registration_id: "control-1".into(),
            holdout_id: "holdout-2".into(),
            issued_at_unix_seconds: 7,
        },
    )
    .await;
    assert!(issue.is_err());
}

#[tokio::test]
async fn another_authorized_namespace_cannot_reset_the_research_family() {
    let fixture = fixture(2, false).await;
    let other = Context::new("other", "other-evaluator", Role::Evaluator).unwrap();
    let mut control = fixture.control.clone();
    control.id = "other-control".into();
    control.evaluator_actor = "other-evaluator".into();
    control.executor_actor = "other-executor".into();
    control.proposer_actor = "other-proposer".into();
    control.approver_actor = "other-approver".into();
    assert!(matches!(
        IndependentEvaluationControl::register_control(&other, &fixture.store, control).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn missing_rows_become_persistent_invalid_and_cannot_reconnect_as_complete() {
    let fixture = fixture(2, false).await;
    let formal = IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
        "missing_execution_rows",
        20,
    )
    .await
    .unwrap();
    assert!(matches!(formal, FormalEvaluationV2::Invalid { .. }));
    assert!(!formal.is_promotable());
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Invalid);
    assert!(
        IndependentEvaluationControl::broker_task(
            &fixture.worker,
            &fixture.store,
            &fixture.ticket_id,
            "r0",
            ExecutionSide::Candidate,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn recursive_duplicate_json_key_persists_an_invalid_terminal() {
    let fixture = fixture(2, false).await;
    let candidate = execute_output(
        &fixture,
        "r0",
        ExecutionSide::Candidate,
        r#"{"answer":{"nested":1,"nested":2}}"#,
        70,
    )
    .await;
    let baseline = execute_side(&fixture, "r0", ExecutionSide::Baseline, "ok", 71).await;
    let result = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: "grade-duplicate-json".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: "r0".into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 900,
        },
    )
    .await;
    assert!(result.is_err());
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Invalid);
    assert!(matches!(
        status.formal,
        Some(FormalEvaluationV2::Invalid { .. })
    ));
}

#[tokio::test]
async fn completed_receipt_retry_is_idempotent_and_output_id_is_immutable() {
    let fixture = fixture(2, false).await;
    grade_target(&fixture, "r0", "ok", "ok", 1).await;
    let receipt_id = "receipt-r0-candidate-2";
    let stored = stored_execution_receipt(&fixture.store, &fixture.worker, receipt_id).await;
    let output_before =
        stored_output_envelope(&fixture.store, &fixture.worker, &stored.output_artifact_id).await;
    let retried = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: stored.receipt_id.clone(),
            ticket_id: stored.ticket_id.clone(),
            target_id: stored.target_id.clone(),
            side: stored.side,
            request_digest: stored.request_digest.clone(),
            output_artifact_id: stored.output_artifact_id.clone(),
            output_utf8: json!({"answer":"ok"}).to_string(),
            budget_call_id: stored.budget_call_id.clone(),
            latency_micros: stored.latency_micros,
            issued_at_unix_seconds: stored.issued_at_unix_seconds,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(&retried).unwrap(),
        fingerprint(&stored).unwrap()
    );

    let pending = start_without_receipt(&fixture, "r1", ExecutionSide::Candidate, 50).await;
    let collision = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: "receipt-r1-output-collision".into(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: pending.target_id,
            side: pending.side,
            request_digest: pending.request_digest,
            output_artifact_id: stored.output_artifact_id.clone(),
            output_utf8: json!({"answer":"different"}).to_string(),
            budget_call_id: pending.call_id,
            latency_micros: 1,
            issued_at_unix_seconds: 1_500,
        },
    )
    .await;
    assert!(collision.is_err());
    let output_after =
        stored_output_envelope(&fixture.store, &fixture.worker, &stored.output_artifact_id).await;
    assert_eq!(output_after, output_before);
    let after = stored_execution_receipt(&fixture.store, &fixture.worker, receipt_id).await;
    assert_eq!(fingerprint(&after).unwrap(), fingerprint(&stored).unwrap());
}

#[tokio::test]
async fn dispatch_updates_exposure_ledger_before_receipt_and_survives_restart() {
    let fixture = fixture(2, false).await;
    let pending = start_without_receipt(&fixture, "r0", ExecutionSide::Candidate, 60).await;
    fixture.store.close().await;
    let reopened = Store::open(&fixture.db_path).await.unwrap();
    let ledger = stored_ledger(
        &reopened,
        &fixture.evaluator,
        &fixture.control.alpha_plan.research_family_id,
    )
    .await;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id)
        .unwrap();
    assert_eq!(
        entry.monetary_state,
        MonetaryReservationState::DispatchedCostPending
    );
    assert!(matches!(
        entry.state,
        ExposureState::Dispatched {
            dispatched_at_unix_ms
        } if dispatched_at_unix_ms == pending.fence.now * 1_000
    ));
}

#[tokio::test]
async fn fully_undispatched_invalidation_releases_money_but_keeps_exposure_entry() {
    let fixture = fixture(2, false).await;
    IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
        "operator_invalid",
        10,
    )
    .await
    .unwrap();
    let ledger = stored_ledger(
        &fixture.store,
        &fixture.evaluator,
        &fixture.control.alpha_plan.research_family_id,
    )
    .await;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id)
        .unwrap();
    assert!(matches!(entry.state, ExposureState::Reserved));
    assert_eq!(
        entry.monetary_state,
        MonetaryReservationState::ReleasedUndispatched
    );
}

#[tokio::test]
async fn unattributed_dispatched_group_call_prevents_clean_terminal_accounting() {
    let fixture = fixture(2, false).await;
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: "unattributed-call".into(),
        actual_input_digest: d("unattributed-request"),
        lease_token: "unattributed-lease".into(),
        lease_epoch: 1,
        now: 20,
    };
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: fence.call_id.clone(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: fence.actual_input_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: fence.lease_token.clone(),
                lease_until: 1_000,
                now: 10,
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence)
        .await
        .unwrap();
    IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
        "manual_invalid",
        30,
    )
    .await
    .unwrap();
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Invalid);
    assert_eq!(
        status.ticket.unattributed_budget_call_ids,
        vec!["unattributed-call"]
    );
    assert!(
        status
            .ticket
            .invalid_reasons
            .iter()
            .any(|reason| reason == "unattributed_dispatch_group_call")
    );
    let call = fixture
        .store
        .budget_call(&fixture.worker, "billing-1", "unattributed-call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, evo_storage::budget::BudgetCallState::Dispatched);
    let ledger = stored_ledger(
        &fixture.store,
        &fixture.evaluator,
        &fixture.control.alpha_plan.research_family_id,
    )
    .await;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id)
        .unwrap();
    assert!(matches!(
        entry.state,
        ExposureState::Dispatched {
            dispatched_at_unix_ms: 20_000
        }
    ));
    assert_eq!(
        entry.monetary_state,
        MonetaryReservationState::DispatchedCostPending
    );
}

#[tokio::test]
async fn dropped_start_transaction_rolls_back_dispatch_and_slot_side_effects() {
    let fixture = fixture(2, false).await;
    let view = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        &fixture.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: "rollback-call".into(),
        actual_input_digest: view.request_digest,
        lease_token: "rollback-lease".into(),
        lease_epoch: 1,
        now: 20,
    };
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: fence.billing_scope.clone(),
                call_id: fence.call_id.clone(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: fence.actual_input_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: fence.lease_token.clone(),
                lease_until: 1_000,
                now: 10,
            },
        )
        .await
        .unwrap();
    {
        let mut session = fixture.store.session().await.unwrap();
        session
            .begin_budget_dispatch(&fixture.worker, &fence)
            .await
            .unwrap();
        // Dropping without commit is the same failure boundary used by E05
        // if its post-dispatch ticket write fails.
    }
    let call = fixture
        .store
        .budget_call(&fixture.worker, "billing-1", "rollback-call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, evo_storage::budget::BudgetCallState::Reserved);
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap();
    assert!(matches!(
        status.ticket.targets[0].state,
        evo_engine::streaming_evaluator::TargetState::Planned
    ));
}
