//! Author tests of the original start_execution entry. Setup uses explicit
//! Admin EmptyRoot and a staged candidate. This file is not ROOT's independent
//! Parent counter and does not claim ordinary-P or Parent runtime credit.
#![allow(dead_code, unused_imports)]
// The execution fixture is copied from streaming_evaluator.rs; all model calls
// are local deterministic receipts, not provider invocations.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::Strategy;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evaluation::{
    BernsteinReport, ClusterObservation, ControlCondition, ExperimentPlan, FixedOptimizerArm,
    FixedOptimizerSpec, FormalExperimentPlanV41, FormalStatisticalUnit, Money,
    OptimizationBudgetPlan, OptimizerComparisonContract, ProfileKind, Verdict, decide,
    empirical_bernstein,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::{
    ExperimentRegisterRequest, ManagementDispatcher, ManagementJob, ManagementJobState,
    ManagementResult,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod,
    FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry, GraderReceiptRequest,
    IndependentEvaluationControl, IssueTicketRequest,
    ProtectedHoldoutRecordV2 as ProtectedHoldoutRecord, ProtectedInputRef,
    RegisteredEvaluationControlV3 as RegisteredEvaluationControl, StartExecutionRequest,
    TicketLifecycle,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;

const V1: &str = "rsia.registered_evaluation_control.v1";
const V2: &str = "rsia.registered_evaluation_control.v3";

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
    candidate_cost: i64,
    resolved_ticket_id: std::sync::OnceLock<String>,
}
impl Fixture {
    fn ticket_id(&self) -> &str {
        self.resolved_ticket_id
            .get()
            .map(String::as_str)
            .unwrap_or(&self.ticket_id)
    }
}

async fn unregistered_fixture(_schema: &str, profile: ProfileKind, candidate_cost: i64) -> Fixture {
    let n = 20;
    let sequential = false;
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

    let host = Context::new("n", "host", Role::Host).unwrap();
    let body = "trusted release source";
    let source = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "release-run".into(),
            body: body.as_bytes().to_vec(),
            parent_family: "release-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "release-run".into(),
            parent_family: "release-family".into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(&store, &host, &source).await.unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = n as usize;
    v1.profile = profile;
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
            profile,
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
        max_regression: evo_core::evaluation::FixedRegressionThreshold::new(0.125).unwrap(),
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
    IndependentEvaluationControl::register_current_control(&evaluator, &store, control.clone())
        .await
        .unwrap();
    let progress = IndependentEvaluationControl::registration_progress(
        &evaluator,
        &store,
        &control.id,
        "holdout-1",
    )
    .await
    .unwrap();
    assert_eq!(
        progress.control_digest.as_deref(),
        Some(control.digest().unwrap().as_str())
    );
    // The candidate pair binds the actual E01 snapshot, not an arbitrary label.
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        d("candidate"),
        release_bundle().parent_digest,
        3,
    )
    .unwrap();
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
        frozen_configuration_digest: control.frozen_configuration().unwrap().digest().unwrap(),
        pair,
        manifest,
        candidate_bundle_digest: release_bundle().digest,
        baseline_bundle_digest: d("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    let fixture = Fixture {
        resolved_ticket_id: std::sync::OnceLock::new(),
        _dir: dir,
        db_path,
        store,
        evaluator,
        worker,
        ticket_id: "ticket-1".into(),
        holdout,
        control,
        candidate_cost,
    };
    stage_fixture_candidate(&fixture).await;
    fixture
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
        fixture.ticket_id(),
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
                dispatch_group_id: fixture.ticket_id().to_owned(),
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
            ticket_id: fixture.ticket_id().to_owned(),
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
            ticket_id: fixture.ticket_id().to_owned(),
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
                amount_micros: match side {
                    ExecutionSide::Candidate => fixture.candidate_cost,
                    ExecutionSide::Baseline => 10,
                },
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
            ticket_id: fixture.ticket_id().to_owned(),
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

async fn fixture(schema: &str, profile: ProfileKind, candidate_cost: i64) -> Fixture {
    let fixture = unregistered_fixture(schema, profile, candidate_cost).await;
    IndependentEvaluationControl::register_current_holdout(
        &fixture.evaluator,
        &fixture.store,
        fixture.holdout.clone(),
    )
    .await
    .unwrap();
    issue(&fixture).await;
    fixture
}

async fn issue(fixture: &Fixture) {
    let ticket = IndependentEvaluationControl::issue_ticket(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketRequest {
            ticket_id: fixture.ticket_id.clone(),
            registration_id: fixture.control.id.clone(),
            holdout_id: fixture.holdout.id.clone(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
    .unwrap();
    fixture.resolved_ticket_id.set(ticket.id).unwrap();
}

fn storage_id(kind: &str, id: &str) -> String {
    format!("e05-{}", fingerprint(&(kind, id)).unwrap())
}

async fn stored(fixture: &Fixture, kind: &str, id: &str) -> Value {
    let mut session = fixture.store.session().await.unwrap();
    let value = session
        .need(&fixture.evaluator, "artifact", &storage_id(kind, id))
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
}

async fn rotating(fixture: &Fixture, start: u32, end: u32, gain: bool) {
    let (candidate, baseline) = if gain {
        ("ok", "wrong")
    } else {
        ("wrong", "ok")
    };
    for index in start..end {
        grade_target(
            fixture,
            &format!("r{index}"),
            candidate,
            baseline,
            index + 1,
        )
        .await;
    }
}

async fn complete(fixture: &Fixture, gain: bool) {
    rotating(fixture, 0, 20, gain).await;
    grade_target(fixture, "a0", "ok", "ok", 21).await;
}

fn raw_decision(fixture: &Fixture, gain: bool) -> BernsteinReport {
    let mut plan = fixture.control.v1_plan_snapshot.clone();
    plan.bind_candidate(d("candidate")).unwrap();
    let rows: Vec<_> = (0..20)
        .map(|index| ClusterObservation {
            cluster_id: format!("cluster-{index}"),
            d: if gain { 1.0 } else { -1.0 },
            weight: 1.0,
        })
        .collect();
    decide(
        &plan,
        empirical_bernstein(&rows, 0.04).unwrap(),
        fixture.candidate_cost as f64 / 10.0,
        1.0,
        true,
    )
    .unwrap()
}

async fn finalize(fixture: &Fixture) -> FormalEvaluationV2 {
    IndependentEvaluationControl::finalize_complete(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        1_000,
    )
    .await
    .unwrap()
}

fn batch(formal: &FormalEvaluationV2) -> (Verdict, &Vec<String>) {
    let FormalEvaluationV2::CompleteBatch {
        verdict, report, ..
    } = formal
    else {
        panic!("expected complete batch, got {formal:?}");
    };
    assert_eq!(*verdict, report.verdict);
    assert_eq!(report.stats_version, "rsia.empirical_bernstein.v2");
    (*verdict, &report.reasons)
}

async fn assert_readback(fixture: &Fixture, formal: &FormalEvaluationV2) {
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::CompleteBatch);
    assert_eq!(
        serde_json::to_vec(status.formal.as_ref().unwrap()).unwrap(),
        serde_json::to_vec(formal).unwrap()
    );
    let view = IndependentEvaluationControl::verified_report_view(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(view.report_digest, fingerprint(formal).unwrap());
    assert!(!formal.is_promotable());
    assert!(!view.promotion_eligible);
    for reason in [
        "complete_optimization_cost_not_verified",
        "program_fixture_not_production_authority",
        "independent_process_isolation_not_verified",
        "dependency_revocation_closure_unverified",
    ] {
        assert!(view.ineligibility_reasons.iter().any(|item| item == reason));
    }
}

fn release_bundle() -> evo_core::contract::ResolvedBundleV3 {
    let profile = Profile {
        id: "p1".into(),
        evolution_enabled: true,
        parent_digest: d("baseline"),
        baseline_digest: d("baseline"),
    };
    let parent = SkillSnapshot {
        content: "fixture skill".into(),
        applicability: "fixture".into(),
        counterexample: "counterexample".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    trusted_compiler_inputs::bundle_content_current(
        CompileParts {
            profile: &profile,
            parent: &parent,
            baseline: &SkillSnapshot::empty(),
            parent_strategy: &Strategy::default(),
            baseline_strategy: &Strategy::default(),
            skill_patch: &SkillPatch::default(),
            improver_patch: &ImproverPatch::default(),
            caps: &HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: &BTreeSet::new(),
        },
        "n",
        &d("environment"),
    )
    .unwrap()
}

async fn stage_fixture_candidate(
    fixture: &Fixture,
) -> evo_engine::release_store::CurrentReleaseCandidateRecord {
    let host = Context::new("n", "host", Role::Host).unwrap();
    let proposer = Context::new("n", "proposer", Role::Worker).unwrap();
    trusted_compiler_inputs::install_bundle_current_sources(&fixture.store, &release_bundle())
        .await
        .unwrap();
    let body = "trusted release source";
    let source = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "release-run".into(),
            body: body.as_bytes().to_vec(),
            parent_family: "release-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "release-run".into(),
            parent_family: "release-family".into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(&fixture.store, &host, &source)
        .await
        .unwrap();
    let mut session = fixture.store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&host, &d("release-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    ReleaseStore::stage_current_bundle(
        &proposer,
        &fixture.store,
        StageBundleRequest {
            candidate_id: d("candidate"),
            bundle: release_bundle(),
            environment_digest: d("environment"),
            proposer_actor: proposer.actor().into(),
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: source.record.id,
                content_digest: source.trace.source_digest,
            }],
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap()
}

async fn assert_release_forbidden(fixture: &Fixture) {
    let approver = Context::new("n", "approver", Role::Admin).unwrap();
    let mut session = fixture.store.session().await.unwrap();
    let candidate: evo_engine::release_store::ReleaseCandidateRecord = session
        .need(&approver, "artifact", &d("candidate"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let view = IndependentEvaluationControl::verified_report_view(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(view.candidate_digest, candidate.id);
    assert_eq!(view.candidate_bundle_digest, candidate.bundle_digest);
    assert_eq!(view.baseline_parent_digest, candidate.parent_digest);
    assert_eq!(view.environment_digest, candidate.environment_digest);
    assert!(matches!(
        ReleaseStore::approve_verified(
            &approver,
            &fixture.store,
            &candidate.id,
            fixture.ticket_id(),
        )
        .await,
        Err(Error::Forbidden)
    ));
}

// Independent serializer of the pre-AG-052 control's exact ordered fields.
// It deliberately does not delegate to RegisteredEvaluationControl::serialize.
struct LegacyControl<'a>(&'a RegisteredEvaluationControl);

impl Serialize for LegacyControl<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let control = self.0;
        let mut wire = serializer.serialize_struct("RegisteredEvaluationControl", 21)?;
        wire.serialize_field("schema_version", &control.schema_version)?;
        wire.serialize_field("id", &control.id)?;
        wire.serialize_field("v1_plan_snapshot", &control.v1_plan_snapshot)?;
        wire.serialize_field("formal_plan", &control.formal_plan)?;
        wire.serialize_field("alpha_plan", &control.alpha_plan)?;
        wire.serialize_field("optimizer_comparison", &control.optimizer_comparison)?;
        wire.serialize_field("optimization_budget", &control.optimization_budget)?;
        wire.serialize_field("early_stop_plan", &control.early_stop_plan)?;
        wire.serialize_field("anchor_matrix", &control.anchor_matrix)?;
        wire.serialize_field("attempt_id", &control.attempt_id)?;
        wire.serialize_field("fixed_sample_claim_id", &control.fixed_sample_claim_id)?;
        wire.serialize_field("billing_scope", &control.billing_scope)?;
        wire.serialize_field("executor_actor", &control.executor_actor)?;
        wire.serialize_field("evaluator_actor", &control.evaluator_actor)?;
        wire.serialize_field("proposer_actor", &control.proposer_actor)?;
        wire.serialize_field("approver_actor", &control.approver_actor)?;
        wire.serialize_field("oracle_digest", &control.oracle_digest)?;
        wire.serialize_field("grader_digest", &control.grader_digest)?;
        wire.serialize_field("fixed_grader", &control.fixed_grader)?;
        wire.serialize_field("evidence_scope", &control.evidence_scope)?;
        wire.serialize_field("created_at_unix_seconds", &control.created_at_unix_seconds)?;
        wire.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "operation")]
enum LegacyRequest<'a> {
    ExperimentRegister {
        schema_version: &'static str,
        request_key: &'a str,
        control: LegacyControl<'a>,
        holdout: &'a ProtectedHoldoutRecord,
    },
}

async fn reserve_member(
    f: &Fixture,
    side: ExecutionSide,
) -> (BrokerTaskViewForCounter, BudgetCallFence) {
    let view =
        IndependentEvaluationControl::broker_task(&f.worker, &f.store, f.ticket_id(), "r0", side)
            .await
            .unwrap();
    let call_id = format!("counter-{side:?}").to_lowercase();
    let call = f
        .store
        .reserve_budget_call(
            &f.worker,
            &BudgetCallReservation {
                billing_scope: f.control.billing_scope.clone(),
                call_id: call_id.clone(),
                dispatch_group_id: f.ticket_id().to_owned(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 10,
                lease_token: "counter-lease".into(),
                lease_until: 1000,
                now: 10,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: f.control.billing_scope.clone(),
        call_id,
        actual_input_digest: view.request_digest.clone(),
        lease_token: call.lease_token,
        lease_epoch: call.lease_epoch,
        now: 11,
    };
    (view, fence)
}
type BrokerTaskViewForCounter = evo_engine::streaming_evaluator::BrokerTaskView;
#[tokio::test]
async fn original_start_entry_has_a_real_healthy_reserved_to_dispatched_control() {
    let f = fixture(V2, ProfileKind::QualityGain, 1).await;
    let (view, fence) = reserve_member(&f, ExecutionSide::Candidate).await;
    let dispatch = IndependentEvaluationControl::start_execution(
        &f.worker,
        &f.store,
        StartExecutionRequest {
            ticket_id: f.ticket_id().to_owned(),
            target_id: view.target_id,
            side: view.side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    assert!(dispatch.new_dispatch);
    assert_eq!(
        dispatch.call.state,
        evo_storage::budget::BudgetCallState::Dispatched
    );
    let status = IndependentEvaluationControl::status(&f.evaluator, &f.store, f.ticket_id())
        .await
        .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Running);
    assert!(dispatch.call.actual_cost_micros.is_none());
    f.store.close().await;
}
async fn revoke_after_reservation_refuses(side: ExecutionSide) {
    let f = fixture(V2, ProfileKind::QualityGain, 1).await;
    let (view, fence) = reserve_member(&f, side).await;
    let before = stored(&f, "evaluation_ticket_v2", f.ticket_id()).await;
    let admin = Context::new("n", "approver", Role::Admin).unwrap();
    evo_engine::lifecycle::LifecycleCoordinator::revoke_source(
        &admin,
        &f.store,
        "release-run",
        "author counter actual revoke",
        12,
    )
    .await
    .unwrap();
    let result = IndependentEvaluationControl::start_execution(
        &f.worker,
        &f.store,
        StartExecutionRequest {
            ticket_id: f.ticket_id().to_owned(),
            target_id: view.target_id,
            side: view.side,
            fence: fence.clone(),
        },
    )
    .await;
    assert!(
        result.is_err(),
        "original start must re-read source/P/B before dispatch"
    );
    let after = stored(&f, "evaluation_ticket_v2", f.ticket_id()).await;
    assert_eq!(before, after);
    let call = f
        .store
        .budget_call(&f.worker, &f.control.billing_scope, &fence.call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, evo_storage::budget::BudgetCallState::Reserved);
    assert!(call.dispatch_id.is_none());
    assert!(call.actual_cost_micros.is_none());
    let root = f
        .store
        .root_budget(&f.worker, &f.control.billing_scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 0);
    f.store.close().await;
}
#[tokio::test]
async fn original_candidate_start_refuses_revoked_direct_source_before_running_or_exposure() {
    revoke_after_reservation_refuses(ExecutionSide::Candidate).await;
}
#[tokio::test]
async fn original_baseline_start_refuses_revoked_direct_source_before_running_or_exposure() {
    revoke_after_reservation_refuses(ExecutionSide::Baseline).await;
}
