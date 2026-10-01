//! Versioned complete-batch verdicts and frozen registration compatibility.
// The execution fixture is copied from streaming_evaluator.rs; all model calls
// are local deterministic receipts, not provider invocations.
use evo_core::Strategy;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
    compile_bundle,
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
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod,
    FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry, GraderReceiptRequest,
    IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef,
    RegisteredEvaluationControl, StartExecutionRequest, TicketLifecycle,
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
const V2: &str = "rsia.registered_evaluation_control.v2";

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
}

async fn unregistered_fixture(schema: &str, profile: ProfileKind, candidate_cost: i64) -> Fixture {
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

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05").unwrap();
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
        schema_version: schema.into(),
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
        candidate_bundle_digest: release_bundle().digest,
        baseline_bundle_digest: d("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    Fixture {
        _dir: dir,
        db_path,
        store,
        evaluator,
        worker,
        ticket_id: "ticket-1".into(),
        holdout,
        control,
        candidate_cost,
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

async fn fixture(schema: &str, profile: ProfileKind, candidate_cost: i64) -> Fixture {
    let fixture = unregistered_fixture(schema, profile, candidate_cost).await;
    IndependentEvaluationControl::register_control(
        &fixture.evaluator,
        &fixture.store,
        fixture.control.clone(),
    )
    .await
    .unwrap();
    IndependentEvaluationControl::register_holdout(
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
    IndependentEvaluationControl::issue_ticket(
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
        &fixture.ticket_id,
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
        &fixture.ticket_id,
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
        &fixture.ticket_id,
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

fn release_bundle() -> evo_core::contract::ResolvedBundle {
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
    compile_bundle(CompileParts {
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
    })
    .unwrap()
}

async fn assert_release_forbidden(fixture: &Fixture) {
    let host = Context::new("n", "host", Role::Host).unwrap();
    let proposer = Context::new("n", "proposer", Role::Worker).unwrap();
    let approver = Context::new("n", "approver", Role::Admin).unwrap();
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
    let candidate = ReleaseStore::stage_bundle(
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
    .unwrap();
    let view = IndependentEvaluationControl::verified_report_view(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
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
            &fixture.ticket_id,
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

#[tokio::test]
async fn explicit_v2_contract_is_accepted_and_unknown_versions_are_rejected() {
    let fixture = unregistered_fixture(V1, ProfileKind::QualityGain, 10).await;
    let mut current = fixture.control.clone();
    current.schema_version = V2.into();
    assert!(
        current.validate().is_ok(),
        "explicit v2 control must be supported"
    );
    current.schema_version = "rsia.registered_evaluation_control.v999".into();
    assert!(matches!(IndependentEvaluationControl::register_control(
        &fixture.evaluator, &fixture.store, current,
    ).await, Err(Error::Invalid(_))));
}

#[tokio::test]
async fn v1_control_keeps_the_exact_legacy_typed_bytes_and_digest() {
    let fixture = fixture(V1, ProfileKind::QualityGain, 10).await;
    let legacy = serde_json::to_vec(&LegacyControl(&fixture.control)).unwrap();
    assert_eq!(legacy, serde_json::to_vec(&fixture.control).unwrap());
    assert_eq!(fixture.control.digest().unwrap(), hash(&legacy));
    let ticket = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        &fixture.ticket_id,
    )
    .await
    .unwrap()
    .ticket;
    assert_eq!(ticket.registration_digest, hash(&legacy));
    assert!(matches!(IndependentEvaluationControl::register_control(
        &fixture.evaluator, &fixture.store, fixture.control.clone(),
    ).await, Err(Error::Conflict(message)) if message == "evaluation control already registered"));
    println!(
        "legacy_typed_bytes={} sha256={}",
        legacy.len(),
        hash(&legacy)
    );
}

#[tokio::test]
async fn v1_keeps_all_four_legacy_postprocessing_results() {
    for (profile, gain, candidate_cost, expected_raw, raw_reason) in [
        (ProfileKind::QualityGain, true, 10, Verdict::Improved, None),
        (
            ProfileKind::QualityGain,
            false,
            10,
            Verdict::Regressed,
            Some("negative_lcb"),
        ),
        (
            ProfileKind::QualityGain,
            true,
            0,
            Verdict::Invalid,
            Some("baseline_cost_zero_forbids_ratio_savings"),
        ),
        (
            ProfileKind::NoninferiorSavings,
            true,
            9,
            Verdict::Noninferior,
            None,
        ),
    ] {
        let fixture = fixture(V1, profile, candidate_cost).await;
        let raw = raw_decision(&fixture, gain);
        assert_eq!(raw.verdict, expected_raw);
        complete(&fixture, gain).await;
        let formal = finalize(&fixture).await;
        let (verdict, reasons) = batch(&formal);
        assert_eq!(verdict, Verdict::Inconclusive);
        let mut expected_reasons = raw.reasons;
        if profile == ProfileKind::NoninferiorSavings {
            expected_reasons.push("cost_savings_statistical_proof_unsupported".into());
        }
        expected_reasons.push("complete_optimization_cost_not_verified".into());
        assert_eq!(*reasons, expected_reasons);
        if let Some(reason) = raw_reason {
            assert!(reasons.iter().any(|item| item == reason));
        }
        assert_readback(&fixture, &formal).await;
        println!(
            "legacy profile={profile:?} raw={expected_raw:?} stored={verdict:?} reasons={reasons:?}"
        );
    }
}

#[tokio::test]
async fn v2_improved_batch_reads_back_and_a_valid_staged_candidate_cannot_promote() {
    let fixture = fixture(V2, ProfileKind::QualityGain, 10).await;
    let raw = raw_decision(&fixture, true);
    assert_eq!(raw.verdict, Verdict::Improved);
    assert!(raw.lcb > 0.02);
    complete(&fixture, true).await;
    let formal = finalize(&fixture).await;
    let (verdict, reasons) = batch(&formal);
    assert_eq!(verdict, Verdict::Improved);
    assert!(reasons.is_empty());
    assert_readback(&fixture, &formal).await;
    assert_release_forbidden(&fixture).await;
}

#[tokio::test]
async fn v2_negative_lcb_is_not_a_confirmed_regression() {
    let fixture = fixture(V2, ProfileKind::QualityGain, 10).await;
    assert_eq!(raw_decision(&fixture, false).verdict, Verdict::Regressed);
    complete(&fixture, false).await;
    let formal = finalize(&fixture).await;
    let (verdict, reasons) = batch(&formal);
    assert_eq!(verdict, Verdict::Inconclusive);
    assert_eq!(
        *reasons,
        vec![
            "negative_lcb",
            "regression_not_confirmed_ucb_boundary_not_crossed"
        ]
    );
    assert_readback(&fixture, &formal).await;
}

#[tokio::test]
async fn v2_zero_candidate_cost_stays_invalid_in_both_profiles() {
    for profile in [ProfileKind::QualityGain, ProfileKind::NoninferiorSavings] {
        let fixture = fixture(V2, profile, 0).await;
        assert_eq!(raw_decision(&fixture, true).verdict, Verdict::Invalid);
        complete(&fixture, true).await;
        let formal = finalize(&fixture).await;
        let (verdict, reasons) = batch(&formal);
        assert_eq!(verdict, Verdict::Invalid);
        assert_eq!(*reasons, vec!["baseline_cost_zero_forbids_ratio_savings"]);
        assert_readback(&fixture, &formal).await;
    }
}

#[tokio::test]
async fn v2_noninferior_requires_cost_statistics_and_other_inconclusive_reasons_stay() {
    for (candidate_cost, expected_raw, expected_reasons) in [
        (
            9,
            Verdict::Noninferior,
            vec!["cost_savings_statistical_proof_unsupported"],
        ),
        (10, Verdict::Inconclusive, vec!["savings_not_demonstrated"]),
    ] {
        let fixture = fixture(V2, ProfileKind::NoninferiorSavings, candidate_cost).await;
        assert_eq!(raw_decision(&fixture, true).verdict, expected_raw);
        complete(&fixture, true).await;
        let formal = finalize(&fixture).await;
        let (verdict, reasons) = batch(&formal);
        assert_eq!(verdict, Verdict::Inconclusive);
        assert_eq!(*reasons, expected_reasons);
        assert_readback(&fixture, &formal).await;
    }
}

#[tokio::test]
async fn both_frozen_versions_survive_restart_without_changing_any_bound_digest() {
    for (schema, expected) in [(V1, Verdict::Inconclusive), (V2, Verdict::Improved)] {
        let mut fixture = fixture(schema, ProfileKind::QualityGain, 10).await;
        let control_before = stored(
            &fixture,
            "registered_evaluation_control_v41",
            &fixture.control.id,
        )
        .await;
        let issue_before = stored(
            &fixture,
            "evaluation_ticket_issue_receipt_v1",
            &fixture.ticket_id,
        )
        .await;
        let ticket_before = IndependentEvaluationControl::status(
            &fixture.evaluator,
            &fixture.store,
            &fixture.ticket_id,
        )
        .await
        .unwrap()
        .ticket;
        rotating(&fixture, 0, 7, true).await;
        fixture.store.close().await;
        fixture.store = Store::open(&fixture.db_path).await.unwrap();
        rotating(&fixture, 7, 20, true).await;
        grade_target(&fixture, "a0", "ok", "ok", 21).await;
        let formal = finalize(&fixture).await;
        assert_eq!(batch(&formal).0, expected);
        assert_readback(&fixture, &formal).await;
        assert_eq!(
            stored(
                &fixture,
                "registered_evaluation_control_v41",
                &fixture.control.id
            )
            .await,
            control_before
        );
        assert_eq!(
            stored(
                &fixture,
                "evaluation_ticket_issue_receipt_v1",
                &fixture.ticket_id
            )
            .await,
            issue_before
        );
        let after = IndependentEvaluationControl::status(
            &fixture.evaluator,
            &fixture.store,
            &fixture.ticket_id,
        )
        .await
        .unwrap()
        .ticket;
        assert_eq!(after.registration_digest, ticket_before.registration_digest);
        assert_eq!(
            after.issue_receipt_digest,
            ticket_before.issue_receipt_digest
        );
        assert_eq!(
            after.bound_v1_plan_digest,
            ticket_before.bound_v1_plan_digest
        );
        let FormalEvaluationV2::CompleteBatch { ticket_digest, .. } = &formal else {
            unreachable!()
        };
        assert_eq!(*ticket_digest, fingerprint(&after).unwrap());
        fixture.store.close().await;
        fixture.store = Store::open(&fixture.db_path).await.unwrap();
        assert_readback(&fixture, &formal).await;
        println!(
            "restart schema={schema} registration={} issue={} bound_plan={}",
            after.registration_digest, after.issue_receipt_digest, after.bound_v1_plan_digest
        );
    }
}

#[tokio::test]
async fn switching_a_frozen_control_version_is_rejected_before_formal_writes() {
    for (schema, replacement) in [(V1, V2), (V2, V1)] {
        let fixture = fixture(schema, ProfileKind::QualityGain, 10).await;
        complete(&fixture, true).await;
        let mut envelope = stored(
            &fixture,
            "registered_evaluation_control_v41",
            &fixture.control.id,
        )
        .await;
        envelope["payload"]["schema_version"] = json!(replacement);
        let mut session = fixture.store.session().await.unwrap();
        session
            .put(
                &fixture.evaluator,
                "artifact",
                &storage_id("registered_evaluation_control_v41", &fixture.control.id),
                fixture.evaluator.actor(),
                &envelope,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(IndependentEvaluationControl::finalize_complete(
            &fixture.evaluator, &fixture.store, &fixture.ticket_id, 1_000,
        ).await, Err(Error::Conflict(message)) if message.contains("control") && message.contains("digest")));
        let mut session = fixture.store.session().await.unwrap();
        assert!(
            session
                .get::<Value>(
                    &fixture.evaluator,
                    "artifact",
                    &storage_id("formal_evaluation_v2", &fixture.ticket_id)
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .get::<Value>(
                    &fixture.evaluator,
                    "artifact",
                    &storage_id("evaluation_resource_evidence_v1", &fixture.ticket_id)
                )
                .await
                .unwrap()
                .is_none()
        );
        let ticket: Value = session
            .need(
                &fixture.evaluator,
                "artifact",
                &storage_id("evaluation_ticket_v2", &fixture.ticket_id),
            )
            .await
            .unwrap();
        assert_eq!(ticket["payload"]["lifecycle"], "ready_for_final");
        session.commit().await.unwrap();
    }
}

async fn seed_legacy_registration_job(fixture: &Fixture, request_key: &str) -> ManagementJob {
    let request = LegacyRequest::ExperimentRegister {
        schema_version: "rsia.management.experiment_register.v1",
        request_key,
        control: LegacyControl(&fixture.control),
        holdout: &fixture.holdout,
    };
    let key_hash = hash(
        format!(
            "{}\0{}\0experiment.register\0{request_key}",
            fixture.evaluator.namespace(),
            fixture.evaluator.actor()
        )
        .as_bytes(),
    );
    let job_id = format!("management-job-{}", &key_hash[..24]);
    let private_id = format!("management-input-{}", &key_hash[..24]);
    let job = ManagementJob {
        id: job_id.clone(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "experiment.register".into(),
        request_key: request_key.into(),
        payload_digest: fingerprint(&request).unwrap(),
        owner_actor: fixture.evaluator.actor().into(),
        owner_role: Role::Evaluator,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: private_id.clone(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 5,
    };
    let private = json!({"id":private_id, "schema_version":"rsia.management_private_input.v1",
        "operation":"experiment.register", "payload_digest":job.payload_digest,
        "payload":serde_json::to_value(&request).unwrap()});
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.evaluator,
            "artifact",
            &job.private_input_ref,
            fixture.evaluator.actor(),
            &private,
        )
        .await
        .unwrap();
    session
        .put(
            &fixture.evaluator,
            "job",
            &job.id,
            fixture.evaluator.actor(),
            &job,
        )
        .await
        .unwrap();
    for dependency in [
        storage_id("registered_evaluation_control_v41", &fixture.control.id),
        storage_id("protected_holdout_v41", &fixture.holdout.id),
    ] {
        session
            .put_edge(
                &fixture.evaluator,
                "artifact",
                &job.private_input_ref,
                "artifact",
                &dependency,
            )
            .await
            .unwrap();
    }
    for input in fixture
        .holdout
        .rotating_inputs
        .iter()
        .chain(&fixture.holdout.anchor_inputs)
    {
        session
            .put_edge(
                &fixture.evaluator,
                "artifact",
                &job.private_input_ref,
                "artifact",
                &input.input_artifact_id,
            )
            .await
            .unwrap();
    }
    session
        .put_edge(
            &fixture.evaluator,
            "job",
            &job.id,
            "artifact",
            &job.private_input_ref,
        )
        .await
        .unwrap();
    session
        .cache(
            &fixture.evaluator,
            "experiment.register",
            request_key,
            &request,
            &job.id,
            &job,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    job
}

async fn wait_management(
    dispatcher: &ManagementDispatcher,
    context: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..200 {
        let job = dispatcher.status(context, job_id).await.unwrap();
        if matches!(
            job.state,
            ManagementJobState::Succeeded
                | ManagementJobState::Failed
                | ManagementJobState::Cancelled
                | ManagementJobState::Blocked
        ) {
            return job;
        }
        tokio::task::yield_now().await;
    }
    panic!("management job did not finish");
}

#[tokio::test]
async fn queued_legacy_private_inputs_recover_with_or_without_a_registered_control() {
    for registered in [false, true] {
        let mut fixture = unregistered_fixture(V1, ProfileKind::QualityGain, 10).await;
        if registered {
            IndependentEvaluationControl::register_control(
                &fixture.evaluator,
                &fixture.store,
                fixture.control.clone(),
            )
            .await
            .unwrap();
        }
        let queued = seed_legacy_registration_job(&fixture, "legacy-recovery").await;
        let private_before = {
            let mut session = fixture.store.session().await.unwrap();
            let private: Value = session
                .need(&fixture.evaluator, "artifact", &queued.private_input_ref)
                .await
                .unwrap();
            session.commit().await.unwrap();
            private
        };
        fixture.store.close().await;
        fixture.store = Store::open(&fixture.db_path).await.unwrap();
        let dispatcher =
            ManagementDispatcher::new(fixture.store.clone(), vec![fixture.evaluator.clone()])
                .unwrap();
        assert_eq!(dispatcher.recover_pending().await.unwrap(), 1);
        let done = wait_management(&dispatcher, &fixture.evaluator, &queued.id).await;
        assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
        let Some(ManagementResult::ExperimentRegistered { control_digest, .. }) = &done.result
        else {
            panic!("{done:?}")
        };
        assert_eq!(
            *control_digest,
            fingerprint(&LegacyControl(&fixture.control)).unwrap()
        );
        let request = ExperimentRegisterRequest {
            schema_version: "rsia.management.experiment_register.v1".into(),
            request_key: "legacy-recovery".into(),
            control: fixture.control.clone(),
            holdout: fixture.holdout.clone(),
        };
        let replay = dispatcher
            .submit(
                &fixture.evaluator,
                "experiment.register",
                serde_json::to_value(&request).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.id, queued.id);
        assert_eq!(replay.generation, done.generation);
        assert_eq!(replay.payload_digest, queued.payload_digest);
        let mut changed = request;
        changed.control.schema_version = V2.into();
        assert!(
            matches!(dispatcher.submit(&fixture.evaluator, "experiment.register", serde_json::to_value(changed).unwrap()).await,
            Err(Error::Conflict(message)) if message.contains("idempotency"))
        );
        let mut session = fixture.store.session().await.unwrap();
        let private_after: Value = session
            .need(&fixture.evaluator, "artifact", &queued.private_input_ref)
            .await
            .unwrap();
        assert_eq!(private_after, private_before);
        session.commit().await.unwrap();
        issue(&fixture).await;
        let persisted = stored(
            &fixture,
            "registered_evaluation_control_v41",
            &fixture.control.id,
        )
        .await;
        assert_eq!(persisted["payload"]["schema_version"], V1);
        println!(
            "legacy_management registered_before={registered} payload={} control={control_digest}",
            queued.payload_digest
        );
    }
}
