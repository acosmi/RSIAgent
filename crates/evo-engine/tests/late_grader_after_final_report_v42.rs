//! AG-118 (E05 §6.6 L705/L707, L696; V085 sub-scope): a grader receipt that
//! arrives after the formal report is final is kept for audit, and the final
//! CompleteBatch / Invalid / EarlyStopped report and every read of it stay
//! exactly as they were.
//!
//! Each case makes the report final, then records one more valid grader
//! receipt under a new receipt id: R1-R3 over an execution pair that was
//! already scored, R2b over a pair of an Invalid ticket that was executed but
//! never scored. R2c submits such an unscored pair whose candidate output
//! cannot be graded: the caller gets the same error as before, but the final
//! Invalid reasons stay fixed (L705) and one audit entry names the request.
//! Afterwards status and both verified report views must return
//! the original variant, verdict and bytes; billing and the bound ticket's
//! outgoing edges must not move; the late receipt must be stored and named by
//! one audit entry. Unbound (pre-AG-100 history, L696) and bound tickets are
//! both covered; bound reads take the strict outgoing edge path.
//!
//! The execution side (T1-T5) is covered here too, on the same fixture: a
//! paid execution receipt that arrives after an Invalid report is final keeps
//! its receipt and output and one audit entry, while the ticket and report
//! stay byte for byte; after an EarlyStopped report it follows the grader
//! side's AG-091 path (late list on the ticket, report re-derived with the
//! same certificate); a CompleteBatch ticket still refuses one with no write.
//!
//! These fixtures execute local software APIs; they prove no provider, grader
//! process or host authority.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evaluation::{
    ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41,
    FormalStatisticalUnit, Money, OptimizationBudgetPlan, OptimizerComparisonContract, ProfileKind,
    Verdict,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopDecision, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, EvaluationTicketV2, ExecutionReceiptRequest, ExecutionReceiptV2,
    ExecutionSide, FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry,
    GraderReceiptRequest, IndependentEvaluationControl, IndependentGraderReceiptV2,
    IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef, RegisteredEvaluationControl,
    StartExecutionRequest, TicketLifecycle, VerifiedReportVariant, verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const N: u32 = 60;
const NAMESPACE: &str = "late-grader-final-fixture";
const BILLING_SCOPE: &str = "late-grader-final-billing";
const TICKET_ID: &str = "late-grader-final-ticket";
const FAMILY_ID: &str = "statistics-family";
const LATE_ID: &str = "late-grader-after-final";
const LATE_AT: i64 = 10_000;
const EXPLICIT_INVALID: &str = "explicit_environment_invalid";
/// The audit operation that names a grader receipt kept outside the final
/// report's closure (L707: the reason it is not part of this report).
const LATE_AUDIT_OPERATION: &str = "late_grader_receipt_after_final_report";
/// The audit operation that names a late grading error after the report is
/// final, instead of a second invalidation (L705: the final reasons are fixed).
const LATE_ERROR_AUDIT_OPERATION: &str = "late_grader_error_after_final_report";
/// A candidate output with a duplicate key, which the fixed grader refuses.
const BAD_JSON: &str = r#"{"answer":{"nested":1,"nested":2}}"#;
const BAD_JSON_ERROR: &str = "JSON contains invalid or duplicate fields";
/// The audit operation that names an execution receipt kept outside a final
/// report's closure.
const LATE_EXECUTION_AUDIT_OPERATION: &str = "late_execution_receipt_after_formal";
const GOOD_OUTPUT: &str = r#"{"answer":"ok"}"#;
const PENDING_CALL: &str = "pending-r58";
const LATE_EXECUTION_ID: &str = "late-r58";
const NO_STARTED_SLOT: &str = "execution target has no started slot";

fn digest(label: &str) -> String {
    hash(label.as_bytes())
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "fixture-pricing-v1".into(),
        amount: amount.into(),
    }
}

fn optimizer(arm: FixedOptimizerArm, label: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: digest(label),
        initial_s0_digest: digest("fixture-s0"),
        base_model_digest: digest("fixture-model"),
        tools_digest: digest("fixture-tools"),
        authorized_materials_digest: digest("fixture-materials"),
        task_partition_digest: digest("fixture-partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: BILLING_SCOPE.into(),
        budget_limit: money("0.02"),
    }
}

fn bundle() -> evo_core::contract::ResolvedBundle {
    let profile = Profile {
        id: "statistics-profile".into(),
        evolution_enabled: true,
        parent_digest: digest("fixture-baseline"),
        baseline_digest: digest("fixture-baseline"),
    };
    let parent = SkillSnapshot {
        content: "statistics software fixture skill".into(),
        applicability: "software fixture".into(),
        counterexample: "fixture counterexample".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    trusted_compiler_inputs::bundle(
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
        NAMESPACE,
        &digest("fixture-environment"),
    )
    .unwrap()
}

struct Fixture {
    _temporary: tempfile::TempDir,
    db_path: PathBuf,
    store: Store,
    evaluator: Context,
    worker: Context,
    control: RegisteredEvaluationControl,
    holdout: ProtectedHoldoutRecord,
}

async fn fixture(sequential: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let db_path = temporary.path().join("fixture.sqlite3");
    let store = Store::open(&db_path).await.unwrap();
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let evaluator = Context::new(NAMESPACE, "evaluator", Role::Evaluator).unwrap();
    let worker = Context::new(NAMESPACE, "executor", Role::Worker).unwrap();
    // 122 calls * 100 micros = 12,200 micros is below the 20,000 root cap.
    // The limit covers every planned call's cap, rather than just final usage.
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "statistics-root-budget".into(),
                billing_scope: BILLING_SCOPE.into(),
                allowed_namespaces: vec![NAMESPACE.into()],
                currency: "USD".into(),
                pricing_version: "fixture-pricing-v1".into(),
                payment_subject: "local-software-fixture".into(),
                authorization_receipt_digest: digest("fixture-budget-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 20_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let mut v1 = ExperimentPlan::first_low_risk("statistics-plan", N as usize).unwrap();
    v1.monetary_budget = money("0.02");
    v1.frozen = true;
    v1.frozen_at = Some(1);
    v1.validate().unwrap();
    let alpha = ResearchFamilyAlphaPlan::new(
        FAMILY_ID,
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
        "statistics-anchors-v1",
        vec![CriticalCapabilityCoverage {
            capability_id: "safety".into(),
            anchor_ids: vec!["anchor-1".into()],
        }],
    )
    .unwrap();
    let mut budget = OptimizationBudgetPlan::unfunded(BILLING_SCOPE, "payer", "USD").unwrap();
    budget.root_total = money("0.02");
    budget.admin_authorization_receipt_digest = Some(digest("fixture-budget-authorization"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "fixture-pricing-v1".into();
        stage.experiment_total.pricing_version = "fixture-pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "fixture-simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "fixture-skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    let formal = FormalExperimentPlanV41::new(
        &v1,
        digest("fixture-preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        digest("fixture-source-sampling"),
        digest("fixture-common-mean-assumption"),
        FormalStatisticalUnit::IndependentTaskCluster,
        N,
    )
    .unwrap();
    let ordered: Vec<_> = (0..N).map(|i| format!("cluster-{i}")).collect();
    let early = sequential.then(|| {
        EarlyStopPlan::new(
            v1.digest().unwrap(),
            &alpha,
            "early-reject",
            "attempt-1",
            ProfileKind::QualityGain,
            N,
            ordered.clone(),
            digest("fixture-common-mean-assumption"),
            digest("fixture-source-sampling"),
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
        id: "statistics-control".into(),
        v1_plan_snapshot: v1.clone(),
        formal_plan: formal,
        alpha_plan: alpha.clone(),
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: early.clone(),
        anchor_matrix: anchors.clone(),
        attempt_id: "attempt-1".into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: BILLING_SCOPE.into(),
        executor_actor: "executor".into(),
        evaluator_actor: "evaluator".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        oracle_digest: digest("fixture-oracle-version"),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        evidence_scope: EvaluationEvidenceScope::ProgramFixture,
        created_at_unix_seconds: 2,
    };
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        digest("fixture-candidate"),
        bundle().parent_digest,
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
        FAMILY_ID,
        alpha.digest().unwrap(),
        sequential_mode,
        "statistics-epoch",
        "statistics-slice",
        ordered.clone(),
        ordered
            .iter()
            .map(|id| digest(&format!("source-{id}")))
            .collect(),
        "fixture-sampler-v1",
        control.oracle_digest.clone(),
        anchors.digest().unwrap(),
        control.grader_digest.clone(),
        digest("fixture-environment"),
        digest("fixture-private-seed"),
        4,
    )
    .unwrap();
    let rotating_inputs: Vec<_> = ordered
        .iter()
        .enumerate()
        .map(|(i, cluster)| ProtectedInputRef {
            target_id: format!("r{i}"),
            cluster_or_anchor_id: cluster.clone(),
            input_artifact_id: format!("input-r{i}"),
            input_digest: digest(&format!("input-r{i}")),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: digest("input-a0"),
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
        id: "statistics-holdout".into(),
        registration_id: control.id.clone(),
        pair,
        manifest,
        candidate_bundle_digest: bundle().digest,
        baseline_bundle_digest: digest("fixture-baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    IndependentEvaluationControl::register_control(&evaluator, &store, control.clone())
        .await
        .unwrap();
    IndependentEvaluationControl::register_holdout(&evaluator, &store, holdout.clone())
        .await
        .unwrap();
    Fixture {
        _temporary: temporary,
        db_path,
        store,
        evaluator,
        worker,
        control,
        holdout,
    }
}

async fn stage_candidate(fixture: &Fixture) {
    let host = Context::new(NAMESPACE, "fixture-host", Role::Host).unwrap();
    let proposer = Context::new(NAMESPACE, "proposer", Role::Worker).unwrap();
    let mut sources = Vec::new();
    for id in ["S1", "S2"] {
        let body = format!("development software fixture source {id}");
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: body.as_bytes().to_vec(),
                parent_family: format!("family-{id}"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: id.into(),
                parent_family: format!("family-{id}"),
                source_digest: hash(body.as_bytes()),
                purpose: Purpose::Development,
                outcome: TraceOutcome::Success,
                diagnosis: None,
                excerpt: body.clone(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: body.len(),
        };
        store_trace_authority(&fixture.store, &host, &authority)
            .await
            .unwrap();
        sources.push(TypedSourceRef {
            kind: "run".into(),
            id: id.into(),
            content_digest: authority.trace.source_digest,
        });
    }
    let mut session = fixture.store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&host, &digest("fixture-initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    trusted_compiler_inputs::install_bundle(&fixture.store, &bundle())
        .await
        .unwrap();
    ReleaseStore::stage_bundle(
        &proposer,
        &fixture.store,
        StageBundleRequest {
            candidate_id: fixture.holdout.pair.candidate_digest.clone(),
            bundle: bundle(),
            environment_digest: fixture.holdout.manifest.environment_digest.clone(),
            proposer_actor: proposer.actor().into(),
            sources,
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap();
}

async fn execute(
    fixture: &Fixture,
    target: &str,
    side: ExecutionSide,
    answer: &str,
    ordinal: u32,
) -> String {
    let output = json!({"answer": answer}).to_string();
    execute_raw(fixture, target, side, &output, ordinal).await
}

async fn execute_raw(
    fixture: &Fixture,
    target: &str,
    side: ExecutionSide,
    output_utf8: &str,
    ordinal: u32,
) -> String {
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        TICKET_ID,
        target,
        side,
    )
    .await
    .unwrap();
    let call_id = format!("call-{ordinal}");
    let now = 10 + i64::from(ordinal) * 5;
    let lease = format!("lease-{ordinal}");
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.clone(),
                dispatch_group_id: TICKET_ID.into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: task.request_digest.clone(),
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
        billing_scope: BILLING_SCOPE.into(),
        call_id: call_id.clone(),
        actual_input_digest: task.request_digest.clone(),
        lease_token: lease,
        lease_epoch: 1,
        now: now + 1,
    };
    let dispatch = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: TICKET_ID.into(),
            target_id: target.into(),
            side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    let receipt_id = format!("receipt-{ordinal}");
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: receipt_id.clone(),
            ticket_id: TICKET_ID.into(),
            target_id: target.into(),
            side,
            request_digest: task.request_digest.clone(),
            output_artifact_id: format!("output-{ordinal}"),
            output_utf8: output_utf8.into(),
            budget_call_id: call_id.clone(),
            latency_micros: 100 + u64::from(ordinal),
            issued_at_unix_seconds: now + 2,
        },
    )
    .await
    .unwrap();
    let mut terminal_fence = fence;
    terminal_fence.now = now + 3;
    fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &terminal_fence,
            &UsageCharge {
                amount_micros: match side {
                    ExecutionSide::Candidate => 5,
                    ExecutionSide::Baseline => 10,
                },
                currency: "USD".into(),
                pricing_version: "fixture-pricing-v1".into(),
                provider_request_id: format!("fixture-request-{ordinal}"),
                usage_record_id: format!("fixture-usage-{ordinal}"),
                output_digest: receipt.output_digest.clone(),
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            BILLING_SCOPE,
            &call_id,
            dispatch.call.dispatch_id.as_deref().unwrap(),
            "fixture_complete",
            now + 4,
        )
        .await
        .unwrap();
    receipt_id
}

/// The execution pair of one target that a grader receipt names.
#[derive(Debug, Clone)]
struct ScoredPair {
    target: String,
    candidate: String,
    baseline: String,
}

fn grader_request(pair: &ScoredPair, receipt_id: &str, now: i64) -> GraderReceiptRequest {
    GraderReceiptRequest {
        receipt_id: receipt_id.into(),
        ticket_id: TICKET_ID.into(),
        target_id: pair.target.clone(),
        candidate_execution_receipt_id: pair.candidate.clone(),
        baseline_execution_receipt_id: pair.baseline.clone(),
        issued_at_unix_seconds: now,
    }
}

async fn grade(
    fixture: &Fixture,
    target: &str,
    candidate_answer: &str,
    baseline_answer: &str,
    ordinal: u32,
) -> ScoredPair {
    let pair = ScoredPair {
        target: target.into(),
        candidate: execute(
            fixture,
            target,
            ExecutionSide::Candidate,
            candidate_answer,
            ordinal * 2,
        )
        .await,
        baseline: execute(
            fixture,
            target,
            ExecutionSide::Baseline,
            baseline_answer,
            ordinal * 2 + 1,
        )
        .await,
    };
    // After both calls' receipt/settlement/close timestamps and no later than
    // the next pair's first dispatch timestamp.
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        grader_request(
            &pair,
            &format!("grader-{ordinal}"),
            20 + i64::from(ordinal) * 10,
        ),
    )
    .await
    .unwrap();
    pair
}

async fn request_ticket(fixture: &Fixture) -> evo_core::Result<EvaluationTicketV2> {
    IndependentEvaluationControl::issue_ticket(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketRequest {
            ticket_id: TICKET_ID.into(),
            registration_id: fixture.control.id.clone(),
            holdout_id: fixture.holdout.id.clone(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
}

async fn issue(fixture: &Fixture, bound: bool) {
    let ticket = if bound {
        stage_candidate(fixture).await;
        request_ticket(fixture).await.unwrap()
    } else {
        // AG-100: without a candidate record no ticket is issued. The
        // historical ticket keeps its original unbound wire, persisted as a
        // pre-AG-100 issue stored it (L696). Its later executions still
        // require a genuinely registered current candidate.
        assert_issue_refused_without_candidate(fixture).await;
        persist_legacy_ticket(fixture).await
    };
    assert_eq!(ticket.candidate_dependency_binding.is_some(), bound);
}

/// AG-100's refusal of an issue that has no candidate record to bind.
const CANDIDATE_RECORD_MISSING: &str = "formal_candidate_record_missing_at_issue";

async fn assert_issue_refused_without_candidate(fixture: &Fixture) {
    let before = catalog(&fixture.db_path);
    let result = request_ticket(fixture)
        .await
        .map(|ticket| ticket.candidate_dependency_binding.is_some());
    assert!(
        matches!(&result, Err(Error::Conflict(message)) if message == CANDIDATE_RECORD_MISSING),
        "{result:?}"
    );
    assert_eq!(
        catalog(&fixture.db_path),
        before,
        "a refused issue wrote state"
    );
}

/// Rewrites the freshly issued ticket row in place (same row, revision 1) and
/// deletes the named bound-only edges; refuses unless the stored body is the
/// expected bound row and only ticket -> holdout remains afterwards.
const LEGACY_TICKET_SQL: &str = r#"
import sqlite3,sys
db,ns,ticket,bound,legacy,holdout=sys.argv[1:7]
remove=sys.argv[7:]
con=sqlite3.connect(db)
row=con.execute("SELECT body,revision FROM objects WHERE namespace=? AND kind='artifact' AND id=?",(ns,ticket)).fetchone()
assert row==(bound,1),row
con.execute("UPDATE objects SET body=? WHERE namespace=? AND kind='artifact' AND id=?",(legacy,ns,ticket))
for dst in remove:
    count=con.execute("DELETE FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=? AND dst_kind='artifact' AND dst_id=?",(ns,ticket,dst)).rowcount
    assert count==1,(dst,count)
left=con.execute("SELECT dst_kind,dst_id FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=?",(ns,ticket)).fetchall()
assert left==[('artifact',holdout)],left
con.commit()
con.close()
print(len(remove))
"#;

/// The pre-AG-100 history (L696): the unbound ticket an issue stored when no
/// candidate was recorded, persisted row for row as in
/// late_grader_stop_preservation_v42.rs. Returns the stored ticket.
async fn persist_legacy_ticket(fixture: &Fixture) -> EvaluationTicketV2 {
    stage_candidate(fixture).await;
    let ticket = request_ticket(fixture).await.unwrap();
    assert!(ticket.candidate_dependency_binding.is_some());
    let id = physical_id("evaluation_ticket_v2", TICKET_ID);
    let mut envelope = Envelope {
        schema_version: "rsia.typed_artifact_envelope.v1".to_string(),
        id: id.clone(),
        record_kind: "evaluation_ticket_v2".to_string(),
        payload: ticket,
    };
    let bound = serde_json::to_string(&envelope).unwrap();
    envelope.payload.candidate_dependency_binding = None;
    let legacy = serde_json::to_string(&envelope).unwrap();
    let mut args = vec![
        NAMESPACE.to_string(),
        id,
        bound,
        legacy,
        physical_id("protected_holdout_v41", &fixture.holdout.id),
        fixture.holdout.pair.candidate_digest.clone(),
        physical_id("evaluation_ticket_issue_receipt_v1", TICKET_ID),
    ];
    let plan = &fixture.control.alpha_plan;
    for allocation in &plan.allocations {
        if allocation.attempt_id == fixture.control.attempt_id {
            let claim = format!("{}:{}", plan.research_family_id, allocation.claim_id);
            args.push(physical_id("alpha_claim_reservation_v1", &claim));
        }
    }
    let output = Command::new("python3")
        .args(["-B", "-c", LEGACY_TICKET_SQL])
        .arg(&fixture.db_path)
        .args(&args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        (args.len() - 5).to_string()
    );
    IndependentEvaluationControl::status(&fixture.evaluator, &fixture.store, TICKET_ID)
        .await
        .unwrap()
        .ticket
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

fn physical_id(kind: &str, id: &str) -> String {
    format!("e05-{}", fingerprint(&(kind, id)).unwrap())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    CompleteBatch,
    /// Late grading of a target scored before the invalidation.
    Invalid,
    /// Late grading of a target executed but never scored before it (R2b).
    InvalidUnscored,
    /// As R2b, but the candidate output cannot be graded (R2c).
    InvalidUngradable,
    EarlyStopped,
}

/// Drives the ticket to the final report of `endpoint` and returns the
/// execution pair the late grader names: a target already completed when the
/// report became final, or for R2b/R2c one executed but not scored.
async fn reach_final_report(fixture: &Fixture, endpoint: Endpoint) -> ScoredPair {
    match endpoint {
        Endpoint::CompleteBatch => {
            // Fixed sample, no early-stop plan: every target completes.
            let first = grade(fixture, "r0", "ok", "wrong", 1).await;
            for i in 1..N {
                grade(fixture, &format!("r{i}"), "ok", "wrong", i + 1).await;
            }
            grade(fixture, "a0", "ok", "ok", N + 1).await;
            IndependentEvaluationControl::finalize_complete(
                &fixture.evaluator,
                &fixture.store,
                TICKET_ID,
                5_000,
            )
            .await
            .unwrap();
            first
        }
        Endpoint::Invalid | Endpoint::InvalidUnscored | Endpoint::InvalidUngradable => {
            grade(fixture, "a0", "ok", "ok", 1).await;
            let scored = grade(fixture, "r0", "ok", "wrong", 2).await;
            // Both sides of r1 are executed, paid and closed, never graded.
            let candidate_output = if endpoint == Endpoint::InvalidUngradable {
                BAD_JSON.to_string()
            } else {
                json!({"answer": "ok"}).to_string()
            };
            let unscored = ScoredPair {
                target: "r1".into(),
                candidate: execute_raw(
                    fixture,
                    "r1",
                    ExecutionSide::Candidate,
                    &candidate_output,
                    6,
                )
                .await,
                baseline: execute(fixture, "r1", ExecutionSide::Baseline, "wrong", 7).await,
            };
            IndependentEvaluationControl::invalidate(
                &fixture.evaluator,
                &fixture.store,
                TICKET_ID,
                EXPLICIT_INVALID,
                1_000,
            )
            .await
            .unwrap();
            if endpoint == Endpoint::Invalid {
                scored
            } else {
                unscored
            }
        }
        Endpoint::EarlyStopped => {
            // The anchor passes first, then 13 rotating losses confirm the
            // futility stop with every dispatched call paid and closed.
            grade(fixture, "a0", "ok", "ok", 1).await;
            let first = grade(fixture, "r0", "wrong", "ok", 2).await;
            for i in 1..13 {
                grade(fixture, &format!("r{i}"), "wrong", "ok", i + 2).await;
            }
            first
        }
    }
}

/// Every observable of the final report, its billing and its bound closure.
#[derive(Debug, Clone, PartialEq)]
struct FinalObservation {
    ticket: Value,
    formal: Value,
    view: Value,
    same_session_view: Value,
    calls: Value,
    ledger: Value,
    ticket_edges: Vec<(String, String)>,
}

/// Reads status, the verified view and the in-session verified view. A read
/// refusal is reported with its exact error, which is the baseline failure.
async fn observe(fixture: &Fixture, phase: &str) -> FinalObservation {
    let status =
        IndependentEvaluationControl::status(&fixture.evaluator, &fixture.store, TICKET_ID).await;
    let view = IndependentEvaluationControl::verified_report_view(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
    )
    .await;
    let mut session = fixture.store.session().await.unwrap();
    let same_session =
        verified_report_view_in_session(&fixture.evaluator, &mut session, TICKET_ID).await;
    let calls = session
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap();
    let ledger: Value = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id("exposure_ledger_v1", FAMILY_ID),
        )
        .await
        .unwrap();
    let ticket_edges = session
        .outgoing_edges_bounded(
            &fixture.evaluator,
            "artifact",
            &physical_id("evaluation_ticket_v2", TICKET_ID),
            10_000,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let status = match status {
        Ok(status) => status,
        Err(error) => panic!("{phase}: status refused the final report: {error:?}"),
    };
    let view = match view {
        Ok(view) => view,
        Err(error) => panic!("{phase}: verified view refused the final report: {error:?}"),
    };
    let same_session = match same_session {
        Ok(view) => view,
        Err(error) => panic!("{phase}: in-session view refused the final report: {error:?}"),
    };
    FinalObservation {
        ticket: serde_json::to_value(&status.ticket).unwrap(),
        formal: serde_json::to_value(&status.formal).unwrap(),
        view: serde_json::to_value(&view).unwrap(),
        same_session_view: serde_json::to_value(&same_session).unwrap(),
        calls: serde_json::to_value(&calls).unwrap(),
        ledger,
        ticket_edges,
    }
}

/// The final report has the expected variant and outcome before any late
/// grading (V085: an early stop is never a promotable complete batch).
fn assert_final_shape(observed: &FinalObservation, endpoint: Endpoint, bound: bool) {
    let ticket: EvaluationTicketV2 = serde_json::from_value(observed.ticket.clone()).unwrap();
    let formal: Option<FormalEvaluationV2> =
        serde_json::from_value(observed.formal.clone()).unwrap();
    assert_eq!(ticket.candidate_dependency_binding.is_some(), bound);
    assert!(ticket.late_grader_receipt_ids.is_empty());
    for view in [&observed.view, &observed.same_session_view] {
        assert_eq!(view["promotion_eligible"], json!(false));
        assert_eq!(view["ticket_id"], json!(TICKET_ID));
    }
    let (lifecycle, variant, verdict) = match (endpoint, &formal) {
        (Endpoint::CompleteBatch, Some(FormalEvaluationV2::CompleteBatch { verdict, .. })) => {
            assert_eq!(*verdict, Verdict::Improved);
            (
                TicketLifecycle::CompleteBatch,
                VerifiedReportVariant::CompleteBatch,
                json!(Verdict::Improved),
            )
        }
        (
            Endpoint::Invalid | Endpoint::InvalidUnscored | Endpoint::InvalidUngradable,
            Some(FormalEvaluationV2::Invalid { reasons, .. }),
        ) => {
            assert_eq!(reasons, &ticket.invalid_reasons);
            assert!(reasons.iter().any(|reason| reason == EXPLICIT_INVALID));
            (
                TicketLifecycle::Invalid,
                VerifiedReportVariant::Invalid,
                json!(Verdict::Invalid),
            )
        }
        (Endpoint::EarlyStopped, Some(FormalEvaluationV2::EarlyStopped { certificate, .. })) => {
            assert_eq!(
                certificate.stop_reason(),
                EarlyStopDecision::FutilityQualityGain
            );
            assert_eq!(certificate.k(), 13);
            assert!(!certificate.usage_uncertain());
            (
                TicketLifecycle::EarlyStopped,
                VerifiedReportVariant::EarlyStopped,
                Value::Null,
            )
        }
        (endpoint, formal) => panic!("{endpoint:?}: unexpected final report {formal:?}"),
    };
    assert_eq!(ticket.lifecycle, lifecycle);
    for view in [&observed.view, &observed.same_session_view] {
        assert_eq!(view["variant"], json!(variant));
        assert_eq!(view["verdict"], verdict);
    }
}

const AUDIT_SQL: &str = r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
rows=[json.loads(p) for (p,) in con.execute("SELECT payload FROM audit WHERE namespace=? ORDER BY seq",(sys.argv[2],)).fetchall()]
print(json.dumps(rows))
con.close()
"#;

/// The actual audit entries naming `entity` under `operation`.
fn audit_entries(path: &Path, operation: &str, entity: &str) -> usize {
    let output = Command::new("python3")
        .args(["-B", "-c", AUDIT_SQL])
        .arg(path)
        .arg(NAMESPACE)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    rows.iter()
        .filter(|row| row["operation"] == json!(operation) && row["entity"] == json!(entity))
        .count()
}

/// The late receipt is stored as the evidence it is: a typed grader receipt for
/// this ticket, target and execution pair, with no outgoing fact edge of its
/// own, and one audit entry names it.
async fn assert_late_receipt_audited(
    fixture: &Fixture,
    pair: &ScoredPair,
    late: &IndependentGraderReceiptV2,
) {
    assert_eq!(late.receipt_id, LATE_ID);
    assert_eq!(late.ticket_id, TICKET_ID);
    assert_eq!(late.target_id, pair.target);
    assert_eq!(late.candidate_execution_receipt_id, pair.candidate);
    assert_eq!(late.baseline_execution_receipt_id, pair.baseline);
    assert_eq!(late.issued_at_unix_seconds, LATE_AT);
    let receipt_id = physical_id("independent_grader_receipt_v2", LATE_ID);
    let mut session = fixture.store.session().await.unwrap();
    let stored: Envelope<Value> = session
        .need(&fixture.evaluator, "artifact", &receipt_id)
        .await
        .unwrap();
    let edges = session
        .outgoing_edges_bounded(&fixture.evaluator, "artifact", &receipt_id, 10_000)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(stored.record_kind, "independent_grader_receipt_v2");
    assert_eq!(stored.payload, serde_json::to_value(late).unwrap());
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(
        audit_entries(&fixture.db_path, LATE_AUDIT_OPERATION, LATE_ID),
        1
    );
    assert_eq!(
        audit_entries(&fixture.db_path, LATE_ERROR_AUDIT_OPERATION, LATE_ID),
        0
    );
}

async fn late_grader_after_final_report(endpoint: Endpoint, bound: bool) {
    let fixture = fixture(endpoint == Endpoint::EarlyStopped).await;
    issue(&fixture, bound).await;
    let pair = reach_final_report(&fixture, endpoint).await;
    let before = observe(&fixture, "final").await;
    assert_final_shape(&before, endpoint, bound);
    let request = grader_request(&pair, LATE_ID, LATE_AT);
    let late = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        request.clone(),
    )
    .await;
    let late = match late {
        Ok(late) => late,
        Err(error) => panic!("a valid late grader receipt was refused: {error:?}"),
    };
    let after = observe(&fixture, "after-late").await;
    // L705/V085: the final report, its ticket pin and every read are unchanged.
    assert_eq!(after.formal, before.formal, "the final report changed");
    assert_eq!(
        after.ticket, before.ticket,
        "the final report's ticket changed"
    );
    assert_eq!(after.view, before.view);
    assert_eq!(after.same_session_view, before.same_session_view);
    // L707: billing and exposure accounting are retained exactly.
    assert_eq!(after.calls, before.calls);
    assert_eq!(after.ledger, before.ledger);
    // The strict outgoing edge set of the (bound) ticket gains nothing.
    assert_eq!(after.ticket_edges, before.ticket_edges);
    assert_eq!(after, before);
    assert_late_receipt_audited(&fixture, &pair, &late).await;
    // A retry of the same receipt is idempotent and audits nothing new.
    let retry = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        request,
    )
    .await;
    let retry = match retry {
        Ok(retry) => retry,
        Err(error) => panic!("an identical late grader retry was refused: {error:?}"),
    };
    assert_eq!(
        serde_json::to_value(&retry).unwrap(),
        serde_json::to_value(&late).unwrap()
    );
    assert_eq!(observe(&fixture, "after-retry").await, before);
    assert_late_receipt_audited(&fixture, &pair, &late).await;
    fixture.store.close().await;
}

/// R2c: a late grading error after the Invalid report is final returns the
/// same error as before, without a second invalidation, a stored receipt or
/// any change to the report, its ticket or billing.
async fn late_grading_error_after_final_report(bound: bool) {
    let fixture = fixture(false).await;
    issue(&fixture, bound).await;
    let pair = reach_final_report(&fixture, Endpoint::InvalidUngradable).await;
    let before = observe(&fixture, "final").await;
    assert_final_shape(&before, Endpoint::InvalidUngradable, bound);
    let late = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        grader_request(&pair, LATE_ID, LATE_AT),
    )
    .await
    .map(|receipt| receipt.receipt_id);
    let after = observe(&fixture, "after-late-error").await;
    let mut session = fixture.store.session().await.unwrap();
    let stored: Option<Value> = session
        .get(
            &fixture.evaluator,
            "artifact",
            &physical_id("independent_grader_receipt_v2", LATE_ID),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        matches!(&late, Err(Error::Invalid(message)) if message == BAD_JSON_ERROR),
        "{late:?}"
    );
    // L705: the final Invalid reasons and every read are unchanged.
    assert_eq!(after.formal, before.formal, "the final report changed");
    assert_eq!(
        after.ticket, before.ticket,
        "the final report's ticket changed"
    );
    assert_eq!(after.view, before.view);
    assert_eq!(after.same_session_view, before.same_session_view);
    assert_eq!(after.calls, before.calls);
    assert_eq!(after.ledger, before.ledger);
    assert_eq!(after.ticket_edges, before.ticket_edges);
    assert_eq!(after, before);
    assert!(stored.is_none(), "{stored:?}");
    assert_eq!(
        audit_entries(&fixture.db_path, LATE_ERROR_AUDIT_OPERATION, LATE_ID),
        1
    );
    assert_eq!(
        audit_entries(&fixture.db_path, LATE_AUDIT_OPERATION, LATE_ID),
        0
    );
    fixture.store.close().await;
}

/// A candidate call of r58 that is dispatched and left without usage.
struct Pending {
    dispatch_id: String,
    request_digest: String,
}

async fn start_pending(fixture: &Fixture, now: i64) -> Pending {
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        TICKET_ID,
        "r58",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: PENDING_CALL.into(),
                dispatch_group_id: TICKET_ID.into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: task.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "pending-lease".into(),
                lease_until: now + 40,
                now,
            },
        )
        .await
        .unwrap();
    let decision = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: TICKET_ID.into(),
            target_id: "r58".into(),
            side: ExecutionSide::Candidate,
            fence: BudgetCallFence {
                billing_scope: BILLING_SCOPE.into(),
                call_id: PENDING_CALL.into(),
                actual_input_digest: task.request_digest.clone(),
                lease_token: "pending-lease".into(),
                lease_epoch: 1,
                now,
            },
        },
    )
    .await
    .unwrap();
    Pending {
        dispatch_id: decision.call.dispatch_id.unwrap(),
        request_digest: task.request_digest,
    }
}

/// The authorized accounting path alone: the pending call's real usage is
/// reconciled and its execution closed; no execution receipt is recorded.
async fn reconcile_without_receipt(fixture: &Fixture, pending: &Pending) {
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    fixture
        .store
        .reconcile_budget_call_cost(
            &admin,
            BILLING_SCOPE,
            PENDING_CALL,
            &UsageCharge {
                amount_micros: 17,
                currency: "USD".into(),
                pricing_version: "fixture-pricing-v1".into(),
                provider_request_id: "fixture-late-request".into(),
                usage_record_id: "fixture-late-usage".into(),
                output_digest: hash(GOOD_OUTPUT.as_bytes()),
            },
            1_200,
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            BILLING_SCOPE,
            PENDING_CALL,
            &pending.dispatch_id,
            "late_audit_closed",
            1_201,
        )
        .await
        .unwrap();
}

fn late_execution_request(pending: &Pending) -> ExecutionReceiptRequest {
    ExecutionReceiptRequest {
        receipt_id: LATE_EXECUTION_ID.into(),
        ticket_id: TICKET_ID.into(),
        target_id: "r58".into(),
        side: ExecutionSide::Candidate,
        request_digest: pending.request_digest.clone(),
        output_artifact_id: "late-output-r58".into(),
        output_utf8: GOOD_OUTPUT.into(),
        budget_call_id: PENDING_CALL.into(),
        latency_micros: 777,
        issued_at_unix_seconds: 1_400,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LateExecutionEndpoint {
    EarlyStopped,
    Invalid,
}

/// Drives the ticket to a final report that leaves r58's dispatched candidate
/// call paid and closed but without its execution receipt. With
/// `r58_baseline`, r58's baseline side is executed, paid and closed on time
/// first (EarlyStopped only) and its receipt id is returned too.
async fn reach_report_with_unreceipted_call(
    fixture: &Fixture,
    endpoint: LateExecutionEndpoint,
    r58_baseline: bool,
) -> (Pending, Option<String>) {
    grade(fixture, "a0", "ok", "ok", 1).await;
    match endpoint {
        LateExecutionEndpoint::EarlyStopped => {
            for i in 0..12 {
                grade(fixture, &format!("r{i}"), "wrong", "ok", i + 2).await;
            }
            // Pay and close the stopping pair before taking the pending slot.
            let stopping = ScoredPair {
                target: "r12".into(),
                candidate: execute(fixture, "r12", ExecutionSide::Candidate, "wrong", 28).await,
                baseline: execute(fixture, "r12", ExecutionSide::Baseline, "ok", 29).await,
            };
            let (baseline, pending_at) = if r58_baseline {
                let receipt = execute(fixture, "r58", ExecutionSide::Baseline, "ok", 32).await;
                (Some(receipt), 180)
            } else {
                (None, 160)
            };
            let pending = start_pending(fixture, pending_at).await;
            IndependentEvaluationControl::record_grader_receipt(
                &fixture.evaluator,
                &fixture.store,
                grader_request(&stopping, "grader-14", pending_at + 1),
            )
            .await
            .unwrap();
            let draining =
                IndependentEvaluationControl::status(&fixture.evaluator, &fixture.store, TICKET_ID)
                    .await
                    .unwrap();
            assert_eq!(draining.ticket.lifecycle, TicketLifecycle::Draining);
            reconcile_without_receipt(fixture, &pending).await;
            let settled = IndependentEvaluationControl::settle_after_stop(
                &fixture.evaluator,
                &fixture.store,
                TICKET_ID,
                1_300,
            )
            .await
            .unwrap();
            assert_eq!(settled.ticket.lifecycle, TicketLifecycle::EarlyStopped);
            (pending, baseline)
        }
        LateExecutionEndpoint::Invalid => {
            assert!(!r58_baseline);
            let pending = start_pending(fixture, 160).await;
            IndependentEvaluationControl::invalidate(
                &fixture.evaluator,
                &fixture.store,
                TICKET_ID,
                EXPLICIT_INVALID,
                170,
            )
            .await
            .unwrap();
            reconcile_without_receipt(fixture, &pending).await;
            (pending, None)
        }
    }
}

fn assert_late_execution_shape(
    observed: &FinalObservation,
    endpoint: LateExecutionEndpoint,
    bound: bool,
) {
    let ticket: EvaluationTicketV2 = serde_json::from_value(observed.ticket.clone()).unwrap();
    let formal: Option<FormalEvaluationV2> =
        serde_json::from_value(observed.formal.clone()).unwrap();
    assert_eq!(ticket.candidate_dependency_binding.is_some(), bound);
    assert!(ticket.late_execution_receipt_ids.is_empty());
    assert!(ticket.late_grader_receipt_ids.is_empty());
    let variant = match (endpoint, &formal) {
        (
            LateExecutionEndpoint::EarlyStopped,
            Some(FormalEvaluationV2::EarlyStopped { certificate, .. }),
        ) => {
            assert_eq!(ticket.lifecycle, TicketLifecycle::EarlyStopped);
            assert_eq!(
                certificate.stop_reason(),
                EarlyStopDecision::FutilityQualityGain
            );
            assert_eq!(certificate.k(), 13);
            assert!(!certificate.usage_uncertain());
            VerifiedReportVariant::EarlyStopped
        }
        (LateExecutionEndpoint::Invalid, Some(FormalEvaluationV2::Invalid { reasons, .. })) => {
            assert_eq!(ticket.lifecycle, TicketLifecycle::Invalid);
            assert_eq!(reasons, &ticket.invalid_reasons);
            assert!(reasons.iter().any(|reason| reason == EXPLICIT_INVALID));
            VerifiedReportVariant::Invalid
        }
        (endpoint, formal) => panic!("{endpoint:?}: unexpected final report {formal:?}"),
    };
    for view in [&observed.view, &observed.same_session_view] {
        assert_eq!(view["variant"], json!(variant));
        assert_eq!(view["promotion_eligible"], json!(false));
    }
}

/// The late execution receipt and its output are stored as the evidence they
/// are, with no outgoing fact edge; returns the audit entries naming it.
async fn assert_late_execution_stored(fixture: &Fixture, late: &ExecutionReceiptV2) -> usize {
    let receipt_id = physical_id("execution_receipt_v2", LATE_EXECUTION_ID);
    let mut session = fixture.store.session().await.unwrap();
    let stored: Envelope<Value> = session
        .need(&fixture.evaluator, "artifact", &receipt_id)
        .await
        .unwrap();
    let output: Envelope<Value> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id("evaluation_execution_output_v1", &late.output_artifact_id),
        )
        .await
        .unwrap();
    let edges = session
        .outgoing_edges_bounded(&fixture.evaluator, "artifact", &receipt_id, 10_000)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(late.receipt_id, LATE_EXECUTION_ID);
    assert_eq!(stored.payload, serde_json::to_value(late).unwrap());
    assert_eq!(output.payload["output_utf8"], json!(GOOD_OUTPUT));
    assert_eq!(output.payload["output_digest"], json!(late.output_digest));
    assert!(edges.is_empty(), "{edges:?}");
    audit_entries(
        &fixture.db_path,
        LATE_EXECUTION_AUDIT_OPERATION,
        LATE_EXECUTION_ID,
    )
}

/// AG-091 path (EarlyStopped): the late ids join the ticket's late lists and
/// the report is re-derived from that ticket. Only the ticket digest and the
/// digests derived from it move; the certificate (stop reason, k, prefix,
/// statistics), the variant, the verdict, billing and the outgoing edges stay.
fn assert_early_report_rederived(
    before: &FinalObservation,
    after: &FinalObservation,
    late_execution_ids: &[&str],
    late_grader_ids: &[&str],
) {
    let ticket: EvaluationTicketV2 = serde_json::from_value(after.ticket.clone()).unwrap();
    assert_eq!(ticket.late_execution_receipt_ids, late_execution_ids);
    assert_eq!(ticket.late_grader_receipt_ids, late_grader_ids);
    let mut unlisted = after.ticket.clone();
    unlisted["late_execution_receipt_ids"] = json!([]);
    unlisted["late_grader_receipt_ids"] = json!([]);
    assert_eq!(
        unlisted, before.ticket,
        "the ticket changed beyond its late lists"
    );
    let digest = json!(fingerprint(&ticket).unwrap());
    assert_eq!(after.formal["ticket_digest"], digest);
    assert_eq!(after.formal["variant"], json!("early_stopped"));
    assert_eq!(after.formal["certificate"], before.formal["certificate"]);
    let mut formal = after.formal.clone();
    formal["ticket_digest"] = before.formal["ticket_digest"].clone();
    assert_eq!(
        formal, before.formal,
        "the report changed beyond its ticket pin"
    );
    for (view, original) in [
        (&after.view, &before.view),
        (&after.same_session_view, &before.same_session_view),
    ] {
        assert_eq!(view["ticket_digest"], digest);
        let mut view = view.clone();
        view["ticket_digest"] = original["ticket_digest"].clone();
        view["report_digest"] = original["report_digest"].clone();
        assert_eq!(&view, original);
    }
    assert_eq!(after.calls, before.calls);
    assert_eq!(after.ledger, before.ledger);
    assert_eq!(after.ticket_edges, before.ticket_edges);
}

/// T1-T3: a paid execution receipt that arrives after the EarlyStopped or
/// Invalid report is final (§6.6 L705/L707; V085.d/e: already run late
/// results stay paid and recorded; L1828 R33: they do not reopen the stop).
/// EarlyStopped keeps the grader side's AG-091 path, so the reads stay
/// consistent; Invalid keeps its report byte for byte and audits the receipt.
async fn late_execution_receipt_after_formal(endpoint: LateExecutionEndpoint, bound: bool) {
    let fixture = fixture(endpoint == LateExecutionEndpoint::EarlyStopped).await;
    issue(&fixture, bound).await;
    let (pending, _) = reach_report_with_unreceipted_call(&fixture, endpoint, false).await;
    let before = observe(&fixture, "final").await;
    assert_late_execution_shape(&before, endpoint, bound);
    let request = late_execution_request(&pending);
    let late = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        request.clone(),
    )
    .await;
    let late = match late {
        Ok(late) => late,
        Err(error) => panic!("a paid late execution receipt was refused: {error:?}"),
    };
    let after = observe(&fixture, "after-late-execution").await;
    match endpoint {
        LateExecutionEndpoint::EarlyStopped => {
            assert_early_report_rederived(&before, &after, &[LATE_EXECUTION_ID], &[]);
            assert_eq!(assert_late_execution_stored(&fixture, &late).await, 0);
        }
        LateExecutionEndpoint::Invalid => {
            assert_eq!(after.formal, before.formal, "the final report changed");
            assert_eq!(
                after.ticket, before.ticket,
                "the final report's ticket changed"
            );
            assert_eq!(after.view, before.view);
            assert_eq!(after.same_session_view, before.same_session_view);
            assert_eq!(after.calls, before.calls);
            assert_eq!(after.ledger, before.ledger);
            assert_eq!(after.ticket_edges, before.ticket_edges);
            assert_eq!(after, before);
            assert_eq!(assert_late_execution_stored(&fixture, &late).await, 1);
        }
    }
    // A retry of the same receipt is idempotent and writes nothing new.
    let retry = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        request,
    )
    .await;
    let retry = match retry {
        Ok(retry) => retry,
        Err(error) => panic!("an identical late execution retry was refused: {error:?}"),
    };
    assert_eq!(
        serde_json::to_value(&retry).unwrap(),
        serde_json::to_value(&late).unwrap()
    );
    assert_eq!(observe(&fixture, "after-execution-retry").await, after);
    let audited = assert_late_execution_stored(&fixture, &late).await;
    assert_eq!(
        audited,
        usize::from(endpoint == LateExecutionEndpoint::Invalid)
    );
    fixture.store.close().await;
}

/// T5: after an early stop, a late execution receipt and then a late grading
/// of the same unscored target both join their late lists on the AG-091 path;
/// every read stays consistent after each step and the certificate is the one
/// the stop confirmed.
async fn late_execution_then_grading_after_early_stop(bound: bool) {
    let fixture = fixture(true).await;
    issue(&fixture, bound).await;
    let (pending, baseline) =
        reach_report_with_unreceipted_call(&fixture, LateExecutionEndpoint::EarlyStopped, true)
            .await;
    let before = observe(&fixture, "final").await;
    assert_late_execution_shape(&before, LateExecutionEndpoint::EarlyStopped, bound);
    let late = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        late_execution_request(&pending),
    )
    .await;
    let late = match late {
        Ok(late) => late,
        Err(error) => panic!("a paid late execution receipt was refused: {error:?}"),
    };
    let executed = observe(&fixture, "after-late-execution").await;
    assert_early_report_rederived(&before, &executed, &[LATE_EXECUTION_ID], &[]);
    let pair = ScoredPair {
        target: "r58".into(),
        candidate: late.receipt_id.clone(),
        baseline: baseline.unwrap(),
    };
    let graded = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        grader_request(&pair, LATE_ID, 1_500),
    )
    .await;
    let graded = match graded {
        Ok(graded) => graded,
        Err(error) => panic!("a valid late grading of r58 was refused: {error:?}"),
    };
    let after = observe(&fixture, "after-late-grading").await;
    assert_eq!(graded.receipt_id, LATE_ID);
    assert_eq!(graded.candidate_execution_receipt_id, LATE_EXECUTION_ID);
    assert_early_report_rederived(&before, &after, &[LATE_EXECUTION_ID], &[LATE_ID]);
    assert_eq!(assert_late_execution_stored(&fixture, &late).await, 0);
    assert_eq!(
        audit_entries(&fixture.db_path, LATE_AUDIT_OPERATION, LATE_ID),
        0
    );
    fixture.store.close().await;
}

/// T4: a CompleteBatch ticket has no started slot left, so a further execution
/// receipt is refused before any write commits (already so at the baseline).
async fn complete_batch_refuses_a_late_execution_receipt(bound: bool) {
    let fixture = fixture(false).await;
    issue(&fixture, bound).await;
    let pair = reach_final_report(&fixture, Endpoint::CompleteBatch).await;
    let before = observe(&fixture, "final").await;
    assert_final_shape(&before, Endpoint::CompleteBatch, bound);
    let mut session = fixture.store.session().await.unwrap();
    let scored: Envelope<Value> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id("execution_receipt_v2", &pair.candidate),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let catalog_before = catalog(&fixture.db_path);
    let late = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: "late-after-complete".into(),
            ticket_id: TICKET_ID.into(),
            target_id: pair.target.clone(),
            side: ExecutionSide::Candidate,
            request_digest: scored.payload["request_digest"].as_str().unwrap().into(),
            output_artifact_id: "late-output-after-complete".into(),
            output_utf8: GOOD_OUTPUT.into(),
            budget_call_id: scored.payload["budget_call_id"].as_str().unwrap().into(),
            latency_micros: 1,
            issued_at_unix_seconds: LATE_AT,
        },
    )
    .await
    .map(|receipt| receipt.receipt_id);
    let catalog_after = catalog(&fixture.db_path);
    let after = observe(&fixture, "after-refused-execution").await;
    assert!(
        matches!(&late, Err(Error::Conflict(message)) if message == NO_STARTED_SLOT),
        "{late:?}"
    );
    assert_eq!(
        catalog_after, catalog_before,
        "a refused receipt wrote state"
    );
    assert_eq!(after, before);
    fixture.store.close().await;
}

/// Each case nests many large engine futures; a debug-build test thread's
/// default stack is too small for them, so it runs on its own larger stack
/// with a current-thread runtime, re-raising the case's own panic.
fn run<F, Fut>(case: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(case());
        })
        .unwrap();
    if let Err(panic) = handle.join() {
        std::panic::resume_unwind(panic);
    }
}

// R1: CompleteBatch.
#[test]
fn r1_unbound_complete_batch_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::CompleteBatch, false));
}

#[test]
fn r1_bound_complete_batch_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::CompleteBatch, true));
}

// R2: Invalid.
#[test]
fn r2_unbound_invalid_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::Invalid, false));
}

#[test]
fn r2_bound_invalid_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::Invalid, true));
}

// R2b: Invalid, late grading of a target that was never scored.
#[test]
fn r2b_unbound_invalid_keeps_its_report_after_a_late_unscored_grader() {
    run(|| late_grader_after_final_report(Endpoint::InvalidUnscored, false));
}

#[test]
fn r2b_bound_invalid_keeps_its_report_after_a_late_unscored_grader() {
    run(|| late_grader_after_final_report(Endpoint::InvalidUnscored, true));
}

// R2c: Invalid, a late grading error for a target that was never scored.
#[test]
fn r2c_unbound_invalid_keeps_its_reasons_after_a_late_grading_error() {
    run(|| late_grading_error_after_final_report(false));
}

#[test]
fn r2c_bound_invalid_keeps_its_reasons_after_a_late_grading_error() {
    run(|| late_grading_error_after_final_report(true));
}

// R3: EarlyStopped.
#[test]
fn r3_unbound_early_stopped_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::EarlyStopped, false));
}

#[test]
fn r3_bound_early_stopped_keeps_its_report_after_a_late_grader() {
    run(|| late_grader_after_final_report(Endpoint::EarlyStopped, true));
}

// T1: EarlyStopped after Draining was settled without the late receipt; the
// receipt joins the ticket's late list and the report is re-derived.
#[test]
fn t1_unbound_early_stopped_keeps_its_report_after_a_late_execution_receipt() {
    run(|| late_execution_receipt_after_formal(LateExecutionEndpoint::EarlyStopped, false));
}

#[test]
fn t1_bound_early_stopped_keeps_its_report_after_a_late_execution_receipt() {
    run(|| late_execution_receipt_after_formal(LateExecutionEndpoint::EarlyStopped, true));
}

// T2: Invalid, unbound.
#[test]
fn t2_unbound_invalid_keeps_its_report_after_a_late_execution_receipt() {
    run(|| late_execution_receipt_after_formal(LateExecutionEndpoint::Invalid, false));
}

// T3: Invalid, bound and not revoked, so no Forbidden masks the read.
#[test]
fn t3_bound_invalid_keeps_its_report_after_a_late_execution_receipt() {
    run(|| late_execution_receipt_after_formal(LateExecutionEndpoint::Invalid, true));
}

// T5: EarlyStopped, a late execution receipt then a late grading of r58.
#[test]
fn t5_bound_early_stopped_keeps_both_late_lists_consistent() {
    run(|| late_execution_then_grading_after_early_stop(true));
}

// T4: CompleteBatch guard.
#[test]
fn t4_bound_complete_batch_refuses_a_late_execution_receipt_without_writing() {
    run(|| complete_batch_refuses_a_late_execution_receipt(true));
}

fn catalog(path: &Path) -> Value {
    let source = r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
schema=con.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name").fetchall()
tables=[]
for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    quoted='"'+name.replace('"','""')+'"'
    columns=[row[1] for row in con.execute('PRAGMA table_xinfo('+quoted+')').fetchall()]
    selected=','.join('"'+column.replace('"','""')+'"' for column in columns)
    rows=[]
    for row in con.execute('SELECT '+selected+' FROM '+quoted):
        rows.append(json.dumps([[type(v).__name__,v.hex() if isinstance(v,bytes) else v] for v in row],ensure_ascii=False))
    tables.append([name,columns,sorted(rows)])
print(json.dumps({'schema':schema,'tables':tables},ensure_ascii=False))
con.close()
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", source])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
