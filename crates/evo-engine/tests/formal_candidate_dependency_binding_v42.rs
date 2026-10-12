//! Software fixtures for the GA-18 candidate/source and actual stored fact pins.
//! These local receipts are ProgramFixture, never production or provider proof.
use evo_core::Strategy;
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract, ProfileKind, Verdict,
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
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_engine::streaming_evaluator::{
    CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope, EvaluationTicketV2,
    ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2,
    FrozenOracleEntry, GraderReceiptRequest, IndependentEvaluationControl, IssueTicketRequest,
    ProtectedHoldoutRecord, ProtectedInputRef, RegisteredEvaluationControl, StartExecutionRequest,
    TicketLifecycle, verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{
    LifecycleStore, REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone, TypedObjectRef,
};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

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

async fn unregistered_fixture(n: u32) -> Fixture {
    let schema = RegisteredEvaluationControl::SCHEMA;
    let profile = ProfileKind::QualityGain;
    let candidate_cost = 5;
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
        "n",
        &d("environment"),
    )
    .unwrap()
}

fn source(id: &str) -> StoredTraceAuthority {
    let body = format!("trusted development body for {id}");
    StoredTraceAuthority {
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
    }
}

async fn stage(fx: &Fixture) {
    let host = Context::new("n", "host", Role::Host).unwrap();
    let proposer = Context::new("n", "proposer", Role::Worker).unwrap();
    let mut refs = Vec::new();
    for id in ["S1", "S2"] {
        let authority = source(id);
        store_trace_authority(&fx.store, &host, &authority)
            .await
            .unwrap();
        refs.push(TypedSourceRef {
            kind: "run".into(),
            id: id.into(),
            content_digest: authority.trace.source_digest,
        });
    }
    let mut session = fx.store.session().await.unwrap();
    let watermark = match session.watermark(&host).await.unwrap() {
        Some((seq, _)) => seq,
        None => session
            .bump_watermark(&host, &d("actual-initial-watermark"))
            .await
            .unwrap(),
    };
    session.commit().await.unwrap();
    trusted_compiler_inputs::install_bundle(&fx.store, &release_bundle())
        .await
        .unwrap();
    ReleaseStore::stage_bundle(
        &proposer,
        &fx.store,
        StageBundleRequest {
            candidate_id: fx.holdout.pair.candidate_digest.clone(),
            bundle: release_bundle(),
            environment_digest: fx.holdout.manifest.environment_digest.clone(),
            proposer_actor: proposer.actor().into(),
            sources: refs,
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap();
}

async fn prepared(n: u32, bound: bool) -> Fixture {
    prepared_under(RegisteredEvaluationControl::SCHEMA, n, bound).await
}

/// The same registered fixture under an explicit control schema.
async fn prepared_under(schema: &str, n: u32, bound: bool) -> Fixture {
    let mut fx = unregistered_fixture(n).await;
    fx.control.schema_version = schema.into();
    IndependentEvaluationControl::register_control(&fx.evaluator, &fx.store, fx.control.clone())
        .await
        .unwrap();
    IndependentEvaluationControl::register_holdout(&fx.evaluator, &fx.store, fx.holdout.clone())
        .await
        .unwrap();
    if bound {
        stage(&fx).await;
    }
    fx
}

async fn issue(fx: &Fixture) -> evo_core::Result<EvaluationTicketV2> {
    IndependentEvaluationControl::issue_ticket(
        &fx.evaluator,
        &fx.store,
        IssueTicketRequest {
            ticket_id: fx.ticket_id.clone(),
            registration_id: fx.control.id.clone(),
            holdout_id: fx.holdout.id.clone(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
}

async fn ready(n: u32, bound: bool) -> Fixture {
    let fx = prepared(n, bound).await;
    if bound {
        issue(&fx).await.unwrap();
    } else {
        // AG-100: without a candidate record no ticket is issued. The
        // historical ticket-only wire is the one a pre-AG-100 issue stored.
        assert_issue_refused_without_candidate(&fx).await;
        persist_legacy_ticket(&fx).await;
    }
    for index in 0..n {
        grade_target(&fx, &format!("r{index}"), "ok", "wrong", index + 1).await;
    }
    grade_target(&fx, "a0", "ok", "ok", n + 1).await;
    fx
}

async fn finalize(fx: &Fixture) -> FormalEvaluationV2 {
    IndependentEvaluationControl::finalize_complete(&fx.evaluator, &fx.store, &fx.ticket_id, 2_000)
        .await
        .unwrap()
}

fn sid(kind: &str, logical: &str) -> String {
    format!("e05-{}", fingerprint(&(kind, logical)).unwrap())
}

async fn raw(fx: &Fixture, kind: &str, id: &str) -> Value {
    let mut session = fx.store.session().await.unwrap();
    let value = session.need(&fx.evaluator, kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn write_raw(fx: &Fixture, kind: &str, id: &str, value: &Value) {
    let mut session = fx.store.session().await.unwrap();
    session
        .put(&fx.evaluator, kind, id, fx.evaluator.actor(), value)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn fixed_watermark_tombstone(fx: &Fixture, kind: &str, id: &str) {
    let mut session = fx.store.session().await.unwrap();
    let watermark = session.watermark(&fx.evaluator).await.unwrap().unwrap();
    session
        .put(
            &fx.evaluator,
            "tombstone",
            id,
            fx.evaluator.actor(),
            &RevokeTombstone {
                id: id.into(),
                schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
                source_kind: kind.into(),
                reason: "fixture fixed-watermark revocation".into(),
                watermark_seq: watermark.0 as u64,
                watermark_digest: watermark.1,
                created_at: 3_000,
            },
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn own_sqlite(fx: &Fixture, script: &str, args: &[&str]) -> String {
    // The canonical Python runtime is already part of this workspace's smoke
    // environment. This reads/mutates only the test's own temporary SQLite file;
    // no new Rust dependency or product storage authority is introduced.
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&fx.db_path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

async fn catalog(fx: &Fixture) -> String {
    own_sqlite(
        fx,
        r#"
import sqlite3,sys,json
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
result=[]
for (table,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").fetchall():
    quoted='"'+table.replace('"','""')+'"'
    columns=[row[1] for row in con.execute('PRAGMA table_info('+quoted+')')]
    rows=[]
    for row in con.execute('SELECT * FROM '+quoted):
        rows.append(json.dumps([[type(value).__name__,value.hex() if isinstance(value,bytes) else value] for value in row],sort_keys=True))
    result.append([table,columns,sorted(rows)])
print(json.dumps(result,sort_keys=True))
"#,
        &[],
    )
}

async fn actual_calls(fx: &Fixture) -> Vec<evo_storage::budget::BudgetCallRecord> {
    let mut session = fx.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fx.worker, "billing-1", &fx.ticket_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    calls
}

async fn all_read_errors(fx: &Fixture) -> Vec<Error> {
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap_err();
    let view =
        IndependentEvaluationControl::verified_report_view(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await
            .unwrap_err();
    let mut session = fx.store.session().await.unwrap();
    let same_session = verified_report_view_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap_err();
    drop(session);
    let admin = Context::new("n", "approver", Role::Admin).unwrap();
    let admin_view =
        IndependentEvaluationControl::verified_report_view(&admin, &fx.store, &fx.ticket_id)
            .await
            .unwrap_err();
    vec![status, view, same_session, admin_view]
}

async fn assert_forbidden_reads(fx: &Fixture) {
    let before = catalog(fx).await;
    for error in all_read_errors(fx).await {
        assert!(matches!(error, Error::Forbidden), "{error:?}");
    }
    assert_eq!(catalog(fx).await, before, "refused reads wrote state");
}

async fn assert_changed_reads(fx: &Fixture, message: &str) {
    let before = catalog(fx).await;
    for error in all_read_errors(fx).await {
        assert!(
            matches!(error, Error::Conflict(ref text) if text == message),
            "{error:?}"
        );
    }
    assert_eq!(catalog(fx).await, before, "refused reads wrote state");
}

async fn add_edge(fx: &Fixture, src_kind: &str, src_id: &str, dst_kind: &str, dst_id: &str) {
    let mut session = fx.store.session().await.unwrap();
    session
        .put_edge(&fx.evaluator, src_kind, src_id, dst_kind, dst_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn remove_edge(fx: &Fixture, src: &str, dst: &str) {
    let count = own_sqlite(
        fx,
        r#"
import sqlite3,sys
con=sqlite3.connect(sys.argv[1])
result=con.execute("DELETE FROM dependencies WHERE namespace='n' AND src_kind='artifact' AND src_id=? AND dst_id=?",sys.argv[2:4])
con.commit()
print(result.rowcount)
"#,
        &[src, dst],
    );
    assert_eq!(count.trim(), "1");
}

/// AG-100's refusal of an issue that has no candidate record to bind.
const CANDIDATE_RECORD_MISSING: &str = "formal_candidate_record_missing_at_issue";

/// The stored ticket row, serialized in the product envelope's field order.
#[derive(Serialize)]
struct TicketEnvelope {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: EvaluationTicketV2,
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

/// AG-100 (L564, L1055/L1057): with no candidate record no ticket is
/// issued. The refusal precedes every write: the whole database is unchanged
/// and the ticket does not exist.
async fn assert_issue_refused_without_candidate(fx: &Fixture) {
    let before = catalog(fx).await;
    let result = issue(fx)
        .await
        .map(|ticket| ticket.candidate_dependency_binding.is_some());
    assert!(
        matches!(&result, Err(Error::Conflict(message)) if message == CANDIDATE_RECORD_MISSING),
        "{result:?}"
    );
    assert_eq!(catalog(fx).await, before, "a refused issue wrote state");
    assert!(matches!(
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id).await,
        Err(Error::NotFound)
    ));
}

/// The pre-AG-100 history (L696: a stored record keeps its original
/// interpretation and is never upgraded): the unbound ticket an issue stored
/// when no candidate was recorded. AG-100 refuses that issue, so the fixture
/// persists exactly its rows. The candidate is staged and the ticket issued
/// bound (true-source order, L288); before any execution the stored envelope
/// is rewritten in place without the binding and the bound-only issue edges
/// (ticket -> candidate, -> issue receipt, -> alpha reservations) are
/// deleted. The ledger, reservations, issue receipt, ticket -> holdout edge
/// and audit entry are the ones both issues write.
async fn persist_legacy_ticket(fx: &Fixture) {
    stage(fx).await;
    let ticket = issue(fx).await.unwrap();
    assert!(ticket.candidate_dependency_binding.is_some());
    let id = sid("evaluation_ticket_v2", &fx.ticket_id);
    let mut envelope = TicketEnvelope {
        schema_version: "rsia.typed_artifact_envelope.v1".into(),
        id: id.clone(),
        record_kind: "evaluation_ticket_v2".into(),
        payload: ticket,
    };
    let bound = serde_json::to_string(&envelope).unwrap();
    envelope.payload.candidate_dependency_binding = None;
    let legacy = serde_json::to_string(&envelope).unwrap();
    let mut args = vec![
        "n".to_string(),
        id,
        bound,
        legacy,
        sid("protected_holdout_v41", &fx.holdout.id),
        fx.holdout.pair.candidate_digest.clone(),
        sid("evaluation_ticket_issue_receipt_v1", &fx.ticket_id),
    ];
    let plan = &fx.control.alpha_plan;
    for allocation in &plan.allocations {
        if allocation.attempt_id == fx.control.attempt_id {
            let claim = format!("{}:{}", plan.research_family_id, allocation.claim_id);
            args.push(sid("alpha_claim_reservation_v1", &claim));
        }
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let removed = own_sqlite(fx, LEGACY_TICKET_SQL, &args);
    assert_eq!(removed.trim(), (args.len() - 5).to_string());
}

/// The stored outgoing edges of the ticket.
async fn ticket_edges(fx: &Fixture) -> Vec<(String, String)> {
    let mut session = fx.store.session().await.unwrap();
    let edges = session
        .outgoing_edges_bounded(
            &fx.evaluator,
            "artifact",
            &sid("evaluation_ticket_v2", &fx.ticket_id),
            100,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    edges
}

#[tokio::test]
async fn bound_software_positive_pins_full_candidate_sources_and_real_fact_slice() {
    let fx = ready(20, true).await;
    let formal = finalize(&fx).await;
    assert!(!formal.is_promotable());
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    let candidate = raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await;
    let binding = status.ticket.candidate_dependency_binding.as_ref().unwrap();
    assert_eq!(
        binding.schema_version,
        "rsia.formal_candidate_dependency_binding.v1"
    );
    // Compare native typed fingerprints, not Value's reordered map serialization.
    let typed: evo_engine::release_store::ReleaseCandidateRecord =
        serde_json::from_value(candidate).unwrap();
    assert_eq!(
        binding.candidate_record_digest,
        fingerprint(&typed).unwrap()
    );
    assert_eq!(binding.sources.len(), 2);
    for id in ["S1", "S2"] {
        let node = binding
            .nodes
            .iter()
            .find(|node| node.kind == "run" && node.id == id)
            .unwrap();
        assert_eq!(node.content_digest, fingerprint(&source(id)).unwrap());
        assert_ne!(node.content_digest, source(id).trace.source_digest);
    }
    let FormalEvaluationV2::CompleteBatch {
        verdict,
        stored_dependency_binding: Some(stored),
        ticket_digest,
        ..
    } = &formal
    else {
        panic!("bound complete batch expected");
    };
    assert_eq!(*verdict, Verdict::Improved);
    assert_eq!(
        stored.schema_version,
        "rsia.formal_stored_dependency_binding.v1"
    );
    assert_eq!(stored.ticket_digest, *ticket_digest);
    assert_ne!(
        sid("evaluation_ticket_v2", &fx.ticket_id),
        sid("formal_evaluation_v2", &fx.ticket_id)
    );
    assert_eq!(
        stored.ticket_physical_id,
        sid("evaluation_ticket_v2", &fx.ticket_id)
    );
    assert!(
        stored
            .nodes
            .iter()
            .any(|node| node.id == sid("protected_holdout_v41", &fx.holdout.id))
    );
    assert!(
        stored
            .nodes
            .iter()
            .any(|node| node.id == sid("registered_evaluation_control_v41", &fx.control.id))
    );
    assert!(
        stored
            .nodes
            .iter()
            .any(|node| node.record_kind.as_deref() == Some("alpha_claim_reservation_v1"))
    );
    assert!(
        stored
            .nodes
            .iter()
            .any(|node| node.record_kind.as_deref() == Some("evaluation_execution_output_v1"))
    );
    assert!(
        !stored
            .nodes
            .iter()
            .any(|node| node.id == sid("formal_evaluation_v2", &fx.ticket_id)
                || node.record_kind.as_deref() == Some("exposure_ledger_v1")
                || node.kind == "budget_call")
    );
    assert!(
        !stored
            .nodes
            .iter()
            .any(|node| node.id.starts_with("input-")),
        "ProtectedInputRef is opaque"
    );
    for ctx in [
        &fx.evaluator,
        &Context::new("n", "approver", Role::Admin).unwrap(),
    ] {
        let view =
            IndependentEvaluationControl::verified_report_view(ctx, &fx.store, &fx.ticket_id)
                .await
                .unwrap();
        assert_eq!(view.report_digest, fingerprint(&formal).unwrap());
        assert_eq!(view.evidence_scope, EvaluationEvidenceScope::ProgramFixture);
        assert_eq!(
            view.cost_evidence_scope,
            Some(CostEvidenceScope::TicketExecutionOnly)
        );
        assert_eq!(
            view.dependency_status,
            DependencyEvidenceStatus::NamespaceWatermarkOnly
        );
        assert!(!view.promotion_eligible);
    }
    let mut session = fx.store.session().await.unwrap();
    verified_report_view_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap();
    let report_edges = session
        .outgoing_edges_bounded(
            &fx.evaluator,
            "artifact",
            &sid("formal_evaluation_v2", &fx.ticket_id),
            10000,
        )
        .await
        .unwrap();
    assert_eq!(report_edges, {
        let mut edges = vec![
            (
                "artifact".into(),
                sid("evaluation_ticket_v2", &fx.ticket_id),
            ),
            (
                "artifact".into(),
                sid("evaluation_resource_evidence_v1", &fx.ticket_id),
            ),
        ];
        edges.sort();
        edges
    });
    session.commit().await.unwrap();
    assert!(matches!(
        ReleaseStore::approve_verified(
            &Context::new("n", "approver", Role::Admin).unwrap(),
            &fx.store,
            &fx.holdout.pair.candidate_digest,
            &fx.ticket_id
        )
        .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn direct_second_source_tombstone_blocks_all_reads_with_original_watermark() {
    let fx = ready(2, true).await;
    finalize(&fx).await;
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    assert_forbidden_reads(&fx).await;
}

#[tokio::test]
async fn full_candidate_pin_precedes_source_traversal_when_source_list_is_stripped() {
    let fx = ready(2, true).await;
    finalize(&fx).await;
    let id = &fx.holdout.pair.candidate_digest;
    let mut candidate = raw(&fx, "artifact", id).await;
    candidate["sources"].as_array_mut().unwrap().pop().unwrap();
    write_raw(&fx, "artifact", id, &candidate).await;
    // Retain both original edges and add a source tombstone: full pin still wins.
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    assert_changed_reads(&fx, "formal_candidate_dependency_binding_changed").await;
}

#[tokio::test]
async fn full_source_authority_is_pinned_separately_from_its_body_digest() {
    let fx = ready(2, true).await;
    finalize(&fx).await;
    let mut authority = raw(&fx, "run", "S2").await;
    let original_body_digest = authority["trace"]["source_digest"].clone();
    authority["trace"]["seed"] = json!(2);
    assert_eq!(authority["trace"]["source_digest"], original_body_digest);
    write_raw(&fx, "run", "S2", &authority).await;
    assert_changed_reads(&fx, "formal_candidate_dependency_binding_changed").await;
}

#[tokio::test]
async fn issue_refuses_revoked_source_before_alpha_exposure_ticket_receipt_or_audit_writes() {
    let fx = prepared(2, true).await;
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    let before = catalog(&fx).await;
    assert!(matches!(issue(&fx).await, Err(Error::Forbidden)));
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn start_refusal_is_atomic_for_reserved_budget_slot_dispatch_and_exposure() {
    let fx = prepared(2, true).await;
    issue(&fx).await.unwrap();
    let view = IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        &fx.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    fx.store
        .reserve_budget_call(
            &fx.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: "reserved".into(),
                dispatch_group_id: fx.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "lease".into(),
                lease_until: 1000,
                now: 10,
            },
        )
        .await
        .unwrap();
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::broker_task(
            &fx.worker,
            &fx.store,
            &fx.ticket_id,
            "r0",
            ExecutionSide::Candidate
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        IndependentEvaluationControl::start_execution(
            &fx.worker,
            &fx.store,
            StartExecutionRequest {
                ticket_id: fx.ticket_id.clone(),
                target_id: "r0".into(),
                side: ExecutionSide::Candidate,
                fence: BudgetCallFence {
                    billing_scope: "billing-1".into(),
                    call_id: "reserved".into(),
                    actual_input_digest: view.request_digest,
                    lease_token: "lease".into(),
                    lease_epoch: 1,
                    now: 11
                },
            }
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn finalize_refusal_leaves_actual_billing_exposure_and_all_facts_unchanged() {
    let fx = ready(2, true).await;
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::finalize_complete(
            &fx.evaluator,
            &fx.store,
            &fx.ticket_id,
            2000
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn real_lifecycle_begin_revoke_preserves_billing_and_exposure_while_blocking_derivation() {
    let fx = ready(2, true).await;
    finalize(&fx).await;
    let paid = actual_calls(&fx).await;
    let exposure = raw(&fx, "artifact", &sid("exposure_ledger_v1", "family-e05")).await;
    let before_source = raw(&fx, "run", "S2").await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let cleanup = LifecycleStore::begin_revoke(
        &admin,
        &fx.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "S2".into(),
        },
        "actual nonprimary revocation",
        3000,
    )
    .await
    .unwrap();
    assert!(cleanup.pending_nodes > 0);
    assert_eq!(
        raw(&fx, "run", "S2").await,
        before_source,
        "begin_revoke is logical, before cleanup"
    );
    assert_forbidden_reads(&fx).await;
    assert_eq!(
        fingerprint(&actual_calls(&fx).await).unwrap(),
        fingerprint(&paid).unwrap()
    );
    assert_eq!(
        raw(&fx, "artifact", &sid("exposure_ledger_v1", "family-e05")).await,
        exposure
    );
    let mut request = IssueTicketRequest {
        ticket_id: "new-ticket".into(),
        registration_id: fx.control.id.clone(),
        holdout_id: fx.holdout.id.clone(),
        issued_at_unix_seconds: 3001,
    };
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::issue_ticket(&fx.evaluator, &fx.store, request.clone()).await,
        Err(Error::Forbidden)
    ));
    request.ticket_id = "another-new-ticket".into();
    assert!(matches!(
        IndependentEvaluationControl::issue_ticket(&fx.evaluator, &fx.store, request).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn complete_stored_slice_pins_latency_and_rejects_missing_or_conflicting_binding() {
    let fx = ready(2, true).await;
    finalize(&fx).await;
    let ticket = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap()
        .ticket;
    let evo_engine::streaming_evaluator::TargetState::Completed {
        candidate_receipt_id,
        ..
    } = &ticket.targets[0].state
    else {
        panic!();
    };
    let id = sid("execution_receipt_v2", candidate_receipt_id);
    let original = raw(&fx, "artifact", &id).await;
    let mut changed = original.clone();
    changed["payload"]["latency_micros"] = json!(123456);
    write_raw(&fx, "artifact", &id, &changed).await;
    assert_changed_reads(&fx, "formal_stored_dependency_binding_changed").await;
    write_raw(&fx, "artifact", &id, &original).await;
    let formal_id = sid("formal_evaluation_v2", &fx.ticket_id);
    let original_formal = raw(&fx, "artifact", &formal_id).await;
    let mut no_binding = original_formal.clone();
    no_binding["payload"]
        .as_object_mut()
        .unwrap()
        .remove("stored_dependency_binding")
        .unwrap();
    write_raw(&fx, "artifact", &formal_id, &no_binding).await;
    assert_changed_reads(&fx, "formal_stored_dependency_binding_changed").await;
    let mut wrong = original_formal;
    wrong["payload"]["stored_dependency_binding"]["candidate_binding_digest"] =
        json!(d("wrong reference"));
    write_raw(&fx, "artifact", &formal_id, &wrong).await;
    assert_changed_reads(&fx, "formal_stored_dependency_binding_changed").await;
}

#[tokio::test]
async fn required_edges_use_physical_keys_and_extra_inconsistent_fact_edges_refuse() {
    for alias in [false, true] {
        let fx = ready(2, true).await;
        finalize(&fx).await;
        let report_id = sid("formal_evaluation_v2", &fx.ticket_id);
        let ticket_id = sid("evaluation_ticket_v2", &fx.ticket_id);
        if alias {
            remove_edge(&fx, &report_id, &ticket_id).await;
        }
        add_edge(&fx, "artifact", &report_id, "artifact", &fx.ticket_id).await;
        let before = catalog(&fx).await;
        for error in all_read_errors(&fx).await {
            assert!(
                matches!(error, Error::Conflict(ref text) if text == "formal fact dependency edges differ"),
                "{error:?}"
            );
        }
        assert_eq!(catalog(&fx).await, before);
    }
}

#[tokio::test]
async fn complete_direct_source_edges_cannot_be_missing_at_issue() {
    let fx = prepared(2, true).await;
    remove_edge(&fx, &fx.holdout.pair.candidate_digest, "S2").await;
    let before = catalog(&fx).await;
    assert!(
        matches!(issue(&fx).await, Err(Error::Conflict(ref text)) if text == "candidate lacks an actual direct source edge")
    );
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn canonical_extra_source_edges_are_included_and_revoked_even_when_not_direct_refs() {
    let fx = prepared(2, true).await;
    let host = Context::new("n", "host", Role::Host).unwrap();
    store_trace_authority(&fx.store, &host, &source("S3"))
        .await
        .unwrap();
    add_edge(
        &fx,
        "artifact",
        &fx.holdout.pair.candidate_digest,
        "run",
        "S3",
    )
    .await;
    let ticket = issue(&fx).await.unwrap();
    let binding = ticket.candidate_dependency_binding.unwrap();
    assert_eq!(binding.sources.len(), 2);
    assert_eq!(binding.nodes.len(), 11);
    assert!(
        binding
            .nodes
            .iter()
            .any(|node| node.kind == "run" && node.id == "S3")
    );
    fixed_watermark_tombstone(&fx, "run", "S3").await;
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn cycles_and_diamonds_deduplicate_complete_typed_closure() {
    let fx = prepared(2, true).await;
    let host = Context::new("n", "host", Role::Host).unwrap();
    store_trace_authority(&fx.store, &host, &source("S3"))
        .await
        .unwrap();
    for (from, to) in [("S1", "S3"), ("S2", "S3"), ("S3", "S1")] {
        add_edge(&fx, "run", from, "run", to).await;
    }
    let ticket = issue(&fx).await.unwrap();
    let binding = ticket.candidate_dependency_binding.unwrap();
    assert_eq!(binding.nodes.len(), 11);
    assert_eq!(
        binding
            .nodes
            .iter()
            .map(|node| (&node.kind, &node.id))
            .collect::<BTreeSet<_>>()
            .len(),
        11
    );
    let root = binding
        .nodes
        .iter()
        .find(|node| node.id == fx.holdout.pair.candidate_digest)
        .unwrap();
    assert_eq!(root.outgoing_edge_count, 3);
    assert_eq!(
        root.outgoing_edge_digest,
        fingerprint(&vec![
            (
                "artifact".to_string(),
                format!(
                    "compiler-binding-{}",
                    fingerprint(&(
                        &fx.holdout.pair.candidate_digest,
                        &fx.holdout.candidate_bundle_digest,
                    ))
                    .unwrap()
                )
            ),
            ("run".to_string(), "S1".to_string()),
            ("run".to_string(), "S2".to_string()),
        ])
        .unwrap()
    );
    IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_unknown_kind_unknown_schema_and_redacted_nodes_fail_closed_atomically() {
    for variant in [
        "missing",
        "unknown-kind",
        "unknown-schema",
        "redacted",
        "unreadable-tombstone",
    ] {
        let fx = prepared(2, true).await;
        let (kind, id) = match variant {
            "unknown-kind" => ("unknown", "opaque"),
            "missing" => ("run", "missing"),
            _ => ("artifact", "opaque"),
        };
        match variant {
            "unknown-schema" => {
                write_raw(
                    &fx,
                    kind,
                    id,
                    &json!({"id":id,"schema_version":"rsia.unknown.v1"}),
                )
                .await
            }
            "redacted" => {
                write_raw(
                    &fx,
                    kind,
                    id,
                    &json!({"id":id,"schema_version":"rsia.redacted.v1"}),
                )
                .await
            }
            "unreadable-tombstone" => {
                write_raw(&fx, "tombstone", id, &json!({"invalid":true})).await
            }
            _ => {}
        }
        add_edge(&fx, "artifact", &fx.holdout.pair.candidate_digest, kind, id).await;
        let before = catalog(&fx).await;
        assert!(issue(&fx).await.is_err(), "{variant}");
        assert_eq!(catalog(&fx).await, before, "{variant}");
    }
}

#[tokio::test]
async fn present_invalid_candidate_never_silently_falls_back_to_legacy() {
    for variant in [
        "wrong-schema",
        "unknown-field",
        "empty-sources",
        "duplicate-sources",
        "noncanonical",
        "wrong-bundle",
        "wrong-proposer",
    ] {
        let fx = prepared(2, true).await;
        let id = &fx.holdout.pair.candidate_digest;
        let mut candidate = raw(&fx, "artifact", id).await;
        match variant {
            "wrong-schema" => candidate["schema_version"] = json!("rsia.release_candidate.v99"),
            "unknown-field" => candidate["unknown"] = json!(true),
            "empty-sources" => candidate["sources"] = json!([]),
            "duplicate-sources" => {
                let first = candidate["sources"][0].clone();
                candidate["sources"].as_array_mut().unwrap().push(first);
            }
            "noncanonical" => candidate["sources"].as_array_mut().unwrap().reverse(),
            "wrong-bundle" => candidate["bundle"]["skill"]["content"] = json!("tampered content"),
            "wrong-proposer" => candidate["proposer_actor"] = json!("executor"),
            _ => unreachable!(),
        }
        write_raw(&fx, "artifact", id, &candidate).await;
        let before = catalog(&fx).await;
        assert!(issue(&fx).await.is_err(), "{variant}");
        assert_eq!(catalog(&fx).await, before, "{variant}");
    }
}

#[tokio::test]
async fn optional_binding_only_absence_is_legacy_null_unknown_version_and_incomplete_shapes_reject()
{
    let fx = ready(2, true).await;
    let formal = finalize(&fx).await;
    let ticket = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap()
        .ticket;
    let ticket_value = serde_json::to_value(&ticket).unwrap();
    let formal_value = serde_json::to_value(&formal).unwrap();
    for (value, field, is_ticket) in [
        (&ticket_value, "candidate_dependency_binding", true),
        (&formal_value, "stored_dependency_binding", false),
    ] {
        for invalid in [
            Value::Null,
            json!({}),
            json!([]),
            json!("wrong type"),
            json!(0),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            if is_ticket {
                assert!(serde_json::from_value::<EvaluationTicketV2>(changed).is_err());
            } else {
                assert!(serde_json::from_value::<FormalEvaluationV2>(changed).is_err());
            }
        }
        let mut absent = value.clone();
        absent.as_object_mut().unwrap().remove(field);
        if is_ticket {
            assert!(
                serde_json::from_value::<EvaluationTicketV2>(absent)
                    .unwrap()
                    .candidate_dependency_binding
                    .is_none()
            );
        } else {
            assert!(matches!(
                serde_json::from_value::<FormalEvaluationV2>(absent).unwrap(),
                FormalEvaluationV2::CompleteBatch {
                    stored_dependency_binding: None,
                    ..
                }
            ));
        }
        for invalid in [
            "unknown-version",
            "unknown-field",
            "missing-identity",
            "empty-nodes",
        ] {
            let mut changed = value.clone();
            match invalid {
                "unknown-version" => {
                    changed[field]["schema_version"] = json!("rsia.formal_binding.v99")
                }
                "unknown-field" => changed[field]["unknown"] = json!(true),
                "missing-identity" => changed[field]
                    .as_object_mut()
                    .unwrap()
                    .remove("namespace")
                    .map(|_| ())
                    .unwrap(),
                "empty-nodes" => changed[field]["nodes"] = json!([]),
                _ => unreachable!(),
            }
            if is_ticket {
                assert!(
                    serde_json::from_value::<EvaluationTicketV2>(changed).is_err(),
                    "{invalid}"
                );
            } else {
                assert!(
                    serde_json::from_value::<FormalEvaluationV2>(changed).is_err(),
                    "{invalid}"
                );
            }
        }
    }
    for invalid in ["empty-sources", "duplicate-sources", "noncanonical"] {
        let mut changed = ticket_value.clone();
        let sources = &mut changed["candidate_dependency_binding"]["sources"];
        match invalid {
            "empty-sources" => *sources = json!([]),
            "duplicate-sources" => {
                let first = sources[0].clone();
                sources.as_array_mut().unwrap().push(first);
            }
            "noncanonical" => sources.as_array_mut().unwrap().reverse(),
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<EvaluationTicketV2>(changed).is_err(),
            "{invalid}"
        );
    }
}

// Exact Parent serializer order, frozen here rather than removing a field from
// a Value map (which would sort the old struct fields and hide byte drift).
struct LegacyTicket<'a>(&'a EvaluationTicketV2);
impl Serialize for LegacyTicket<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("EvaluationTicketV2", 35)?;
        record.serialize_field("schema_version", &self.0.schema_version)?;
        record.serialize_field("id", &self.0.id)?;
        record.serialize_field("registration_id", &self.0.registration_id)?;
        record.serialize_field("registration_digest", &self.0.registration_digest)?;
        record.serialize_field("holdout_id", &self.0.holdout_id)?;
        record.serialize_field("manifest_digest", &self.0.manifest_digest)?;
        record.serialize_field("v1_plan_snapshot_digest", &self.0.v1_plan_snapshot_digest)?;
        record.serialize_field("bound_v1_plan_digest", &self.0.bound_v1_plan_digest)?;
        record.serialize_field("formal_plan_digest", &self.0.formal_plan_digest)?;
        record.serialize_field("profile", &self.0.profile)?;
        record.serialize_field("n_planned", &self.0.n_planned)?;
        record.serialize_field("research_family_id", &self.0.research_family_id)?;
        record.serialize_field("alpha_plan_digest", &self.0.alpha_plan_digest)?;
        record.serialize_field("candidate_digest", &self.0.candidate_digest)?;
        record.serialize_field("baseline_digest", &self.0.baseline_digest)?;
        record.serialize_field("candidate_bundle_digest", &self.0.candidate_bundle_digest)?;
        record.serialize_field("baseline_bundle_digest", &self.0.baseline_bundle_digest)?;
        record.serialize_field("environment_digest", &self.0.environment_digest)?;
        record.serialize_field("proposer_actor", &self.0.proposer_actor)?;
        record.serialize_field("evaluator_actor", &self.0.evaluator_actor)?;
        record.serialize_field("executor_actor", &self.0.executor_actor)?;
        record.serialize_field("approver_actor", &self.0.approver_actor)?;
        record.serialize_field("issue_receipt_digest", &self.0.issue_receipt_digest)?;
        record.serialize_field("evidence_scope", &self.0.evidence_scope)?;
        record.serialize_field("attempt_id", &self.0.attempt_id)?;
        record.serialize_field("targets", &self.0.targets)?;
        record.serialize_field("lifecycle", &self.0.lifecycle)?;
        record.serialize_field("next_sequence", &self.0.next_sequence)?;
        record.serialize_field("pending_certificate", &self.0.pending_certificate)?;
        record.serialize_field(
            "stop_member_terminal_digest",
            &self.0.stop_member_terminal_digest,
        )?;
        record.serialize_field(
            "late_execution_receipt_ids",
            &self.0.late_execution_receipt_ids,
        )?;
        record.serialize_field("late_grader_receipt_ids", &self.0.late_grader_receipt_ids)?;
        record.serialize_field(
            "unattributed_budget_call_ids",
            &self.0.unattributed_budget_call_ids,
        )?;
        record.serialize_field("invalid_reasons", &self.0.invalid_reasons)?;
        record.serialize_field("issued_at_unix_seconds", &self.0.issued_at_unix_seconds)?;
        record.end()
    }
}
struct LegacyFormal<'a>(&'a FormalEvaluationV2);
impl Serialize for LegacyFormal<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let FormalEvaluationV2::CompleteBatch {
            evidence_scope,
            cost_evidence_scope,
            ticket_id,
            ticket_digest,
            manifest_digest,
            candidate_bundle_digest,
            baseline_bundle_digest,
            environment_digest,
            verdict,
            report,
            anchor_evidence_digest,
            resource_evidence_digest,
            ..
        } = self.0
        else {
            return Err(serde::ser::Error::custom("complete batch expected"));
        };
        let mut record = serializer.serialize_struct("FormalEvaluationV2", 13)?;
        record.serialize_field("variant", "complete_batch")?;
        record.serialize_field("evidence_scope", evidence_scope)?;
        record.serialize_field("cost_evidence_scope", cost_evidence_scope)?;
        record.serialize_field("ticket_id", ticket_id)?;
        record.serialize_field("ticket_digest", ticket_digest)?;
        record.serialize_field("manifest_digest", manifest_digest)?;
        record.serialize_field("candidate_bundle_digest", candidate_bundle_digest)?;
        record.serialize_field("baseline_bundle_digest", baseline_bundle_digest)?;
        record.serialize_field("environment_digest", environment_digest)?;
        record.serialize_field("verdict", verdict)?;
        record.serialize_field("report", report)?;
        record.serialize_field("anchor_evidence_digest", anchor_evidence_digest)?;
        record.serialize_field("resource_evidence_digest", resource_evidence_digest)?;
        record.end()
    }
}

#[tokio::test]
async fn legacy_absence_preserves_exact_parent_bytes_fingerprints_and_never_upgrades_later() {
    // AG-100: the unstaged issue is refused without a write; the ticket is
    // the pre-AG-100 unbound one, persisted as stored (L696).
    let fx = ready(2, false).await;
    let formal = finalize(&fx).await;
    let ticket = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap()
        .ticket;
    assert!(ticket.candidate_dependency_binding.is_none());
    assert!(matches!(
        formal,
        FormalEvaluationV2::CompleteBatch {
            stored_dependency_binding: None,
            ..
        }
    ));
    let old_ticket_bytes = serde_json::to_vec(&LegacyTicket(&ticket)).unwrap();
    let old_formal_bytes = serde_json::to_vec(&LegacyFormal(&formal)).unwrap();
    assert_eq!(serde_json::to_vec(&ticket).unwrap(), old_ticket_bytes);
    assert_eq!(serde_json::to_vec(&formal).unwrap(), old_formal_bytes);
    assert_eq!(fingerprint(&ticket).unwrap(), hash(&old_ticket_bytes));
    assert_eq!(fingerprint(&formal).unwrap(), hash(&old_formal_bytes));
    assert!(
        !String::from_utf8(old_ticket_bytes)
            .unwrap()
            .contains("candidate_dependency_binding")
    );
    assert!(
        !String::from_utf8(old_formal_bytes)
            .unwrap()
            .contains("stored_dependency_binding")
    );
    stage(&fx).await;
    fixed_watermark_tombstone(&fx, "run", "S2").await;
    let after = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    assert!(after.ticket.candidate_dependency_binding.is_none());
    assert_eq!(
        fingerprint(&after.ticket).unwrap(),
        fingerprint(&ticket).unwrap()
    );
    assert_eq!(
        fingerprint(&after.formal.unwrap()).unwrap(),
        fingerprint(&formal).unwrap()
    );
    IndependentEvaluationControl::verified_report_view(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
}

/// AG-100 T1 (L564, L1055/L1057). A ticket requested while no candidate
/// record exists is refused under both control schemas before any write: the
/// whole database is unchanged and no ticket exists. Before AG-100 this issue
/// stored an unbound legacy ticket. Every outcome is collected first.
#[tokio::test]
async fn issue_without_a_candidate_record_is_refused_for_every_control_schema_before_any_write() {
    let mut actual = Vec::new();
    for schema in [
        RegisteredEvaluationControl::SCHEMA,
        RegisteredEvaluationControl::SCHEMA_V1,
    ] {
        let fx = prepared_under(schema, 2, false).await;
        let before = catalog(&fx).await;
        let issued = issue(&fx)
            .await
            .map(|ticket| ticket.candidate_dependency_binding.is_some());
        let unchanged = catalog(&fx).await == before;
        let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await
            .map(|status| status.ticket.candidate_dependency_binding.is_some());
        println!(
            "AG100_T1 schema={schema} issue_bound={issued:?} catalog_unchanged={unchanged} status_bound={status:?}"
        );
        actual.push((schema, issued, unchanged, status));
    }
    for (schema, issued, unchanged, status) in actual {
        assert!(
            matches!(&issued, Err(Error::Conflict(message)) if message == CANDIDATE_RECORD_MISSING),
            "{schema}: {issued:?}"
        );
        assert!(unchanged, "{schema}: a refused issue wrote state");
        assert!(
            matches!(status, Err(Error::NotFound)),
            "{schema}: {status:?}"
        );
    }
}

/// AG-100 T2 (L288, L564). With the candidate staged first the ticket is
/// issued under both control schemas and binds it: the binding names the
/// frozen candidate, status reads the same ticket, and the stored ticket
/// edges include ticket -> candidate.
#[tokio::test]
async fn issue_after_the_candidate_is_staged_binds_it_for_every_control_schema() {
    for schema in [
        RegisteredEvaluationControl::SCHEMA,
        RegisteredEvaluationControl::SCHEMA_V1,
    ] {
        let fx = prepared_under(schema, 2, true).await;
        let ticket = issue(&fx).await.unwrap();
        let binding = ticket.candidate_dependency_binding.as_ref().unwrap();
        assert_eq!(binding.candidate_id, fx.holdout.pair.candidate_digest);
        assert_eq!(binding.namespace, "n");
        let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await
            .unwrap();
        assert_eq!(
            fingerprint(&status.ticket).unwrap(),
            fingerprint(&ticket).unwrap()
        );
        assert!(
            ticket_edges(&fx)
                .await
                .contains(&("artifact".into(), fx.holdout.pair.candidate_digest.clone()))
        );
    }
}

/// AG-100 T3 (L696). A ticket stored unbound before AG-100 keeps its original
/// interpretation: status reads it unbound with its original wire, the
/// executor's task is served, its only stored edge is ticket -> holdout, and
/// none of these reads writes or upgrades it although its candidate is
/// registered now.
#[tokio::test]
async fn persisted_legacy_ticket_is_read_unbound_and_served_as_stored() {
    let fx = prepared(2, false).await;
    persist_legacy_ticket(&fx).await;
    let before = catalog(&fx).await;
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    assert!(status.ticket.candidate_dependency_binding.is_none());
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Reserved);
    assert!(status.formal.is_none());
    let wire = serde_json::to_value(&status.ticket).unwrap();
    assert!(wire.get("candidate_dependency_binding").is_none());
    assert_eq!(
        serde_json::to_vec(&status.ticket).unwrap(),
        serde_json::to_vec(&LegacyTicket(&status.ticket)).unwrap()
    );
    assert_eq!(
        ticket_edges(&fx).await,
        vec![(
            "artifact".to_string(),
            sid("protected_holdout_v41", &fx.holdout.id)
        )]
    );
    IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        &fx.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let again = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&again.ticket).unwrap(),
        fingerprint(&status.ticket).unwrap()
    );
    assert_eq!(catalog(&fx).await, before, "legacy reads wrote state");
}

#[tokio::test]
async fn bounded_outgoing_api_orders_typed_ids_isolates_namespace_and_refuses_limit_plus_one() {
    let fx = prepared(2, false).await;
    for (kind, id) in [("run", "z"), ("artifact", "z"), ("run", "A"), ("run", "a")] {
        add_edge(&fx, "artifact", "edge-root", kind, id).await;
    }
    let other = Context::new("other", "host", Role::Host).unwrap();
    let mut session = fx.store.session().await.unwrap();
    session
        .put_edge(&other, "artifact", "edge-root", "run", "other-only")
        .await
        .unwrap();
    let expected = vec![
        ("artifact".into(), "z".into()),
        ("run".into(), "A".into()),
        ("run".into(), "a".into()),
        ("run".into(), "z".into()),
    ];
    assert_eq!(
        session
            .outgoing_edges_bounded(&fx.evaluator, "artifact", "edge-root", 4)
            .await
            .unwrap(),
        expected
    );
    assert!(
        matches!(session.outgoing_edges_bounded(&fx.evaluator,"artifact","edge-root",3).await,Err(Error::Conflict(ref text)) if text.contains("refused, not truncated"))
    );
    assert_eq!(
        session
            .outgoing_edges_bounded(&other, "artifact", "edge-root", 1)
            .await
            .unwrap(),
        vec![("run".into(), "other-only".into())]
    );
    assert!(
        session
            .outgoing_edges_bounded(&fx.evaluator, "run", "edge-root", 1)
            .await
            .unwrap()
            .is_empty()
    );
    for invalid in [0, 10001, usize::MAX] {
        assert!(
            session
                .outgoing_edges_bounded(&fx.evaluator, "artifact", "edge-root", invalid)
                .await
                .is_err()
        );
    }
    session.commit().await.unwrap();
    own_sqlite(
        &fx,
        r#"
import sqlite3,sys
con=sqlite3.connect(sys.argv[1])
con.executemany('INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)',
    [('n','artifact','ten-thousand','run',f'edge-{i:05}') for i in range(10001)])
con.commit()
"#,
        &[],
    );
    let mut session = fx.store.session().await.unwrap();
    assert!(
        matches!(session.outgoing_edges_bounded(&fx.evaluator,"artifact","ten-thousand",10000).await,Err(Error::Conflict(ref text)) if text.contains("refused, not truncated"))
    );
    session.commit().await.unwrap();
    own_sqlite(
        &fx,
        r#"
import sqlite3,sys
con=sqlite3.connect(sys.argv[1])
con.execute("DELETE FROM dependencies WHERE src_id='ten-thousand' AND dst_id='edge-10000'")
con.commit()
"#,
        &[],
    );
    let mut session = fx.store.session().await.unwrap();
    assert_eq!(
        session
            .outgoing_edges_bounded(&fx.evaluator, "artifact", "ten-thousand", 10000)
            .await
            .unwrap()
            .len(),
        10000
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn overall_typed_node_bound_accepts_exact_ten_thousand_and_refuses_one_more() {
    // Direct own-database adversarial graph seeding exercises closure capacity
    // independently of the separate normal-ingestion MVP run count. Every node
    // is still a real, fully valid StoredTraceAuthority; no placeholder is proof.
    // The seven compiler authorization nodes occupy real closure capacity.
    for extra in [9984, 9985] {
        let fx = prepared(2, true).await;
        let template = serde_json::to_string(&source("graph-template")).unwrap();
        own_sqlite(
            &fx,
            r#"
import sqlite3,sys,json,copy
con=sqlite3.connect(sys.argv[1]);template=json.loads(sys.argv[2]);count=int(sys.argv[3])
for i in range(count):
    doc=copy.deepcopy(template);node=f'node-{i:05}'
    doc['record']['id']=node;doc['trace']['run_id']=node
    con.execute('INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)',('n','run',node,'host',json.dumps(doc)))
    previous='S1' if i==0 else f'node-{i-1:05}'
    con.execute('INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)',('n','run',previous,'run',node))
# A second root-to-last edge forms a lawful diamond without adding nodes.
con.execute('INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)',('n','run','S2','run',f'node-{count-1:05}'))
con.commit()
"#,
            &[&template, &extra.to_string()],
        );
        let before = catalog(&fx).await;
        let result = issue(&fx).await;
        if extra == 9984 {
            let ticket = result.unwrap();
            assert_eq!(
                ticket
                    .candidate_dependency_binding
                    .as_ref()
                    .unwrap()
                    .nodes
                    .len(),
                9994
            );
            IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
                .await
                .unwrap();
            let view = IndependentEvaluationControl::broker_task(
                &fx.worker,
                &fx.store,
                &fx.ticket_id,
                "r0",
                ExecutionSide::Candidate,
            )
            .await
            .unwrap();
            let fence = BudgetCallFence {
                billing_scope: "billing-1".into(),
                call_id: "capacity-dispatched".into(),
                actual_input_digest: view.request_digest.clone(),
                lease_token: "capacity-lease".into(),
                lease_epoch: 1,
                now: 11,
            };
            fx.store
                .reserve_budget_call(
                    &fx.worker,
                    &BudgetCallReservation {
                        billing_scope: fence.billing_scope.clone(),
                        call_id: fence.call_id.clone(),
                        dispatch_group_id: fx.ticket_id.clone(),
                        stage: BudgetStage::TaskExecution,
                        actual_input_digest: fence.actual_input_digest.clone(),
                        request_artifact: None,
                        max_cost_micros: 100,
                        lease_token: fence.lease_token.clone(),
                        lease_until: 1000,
                        now: 10,
                    },
                )
                .await
                .unwrap();
            IndependentEvaluationControl::start_execution(
                &fx.worker,
                &fx.store,
                StartExecutionRequest {
                    ticket_id: fx.ticket_id.clone(),
                    target_id: "r0".into(),
                    side: ExecutionSide::Candidate,
                    fence,
                },
            )
            .await
            .unwrap();
            let before_receipt = catalog(&fx).await;
            assert!(
                matches!(IndependentEvaluationControl::record_execution_receipt(
                &fx.worker, &fx.store, ExecutionReceiptRequest {
                    receipt_id: "capacity-receipt".into(), ticket_id: fx.ticket_id.clone(),
                    target_id: "r0".into(), side: ExecutionSide::Candidate,
                    request_digest: view.request_digest, output_artifact_id: "capacity-output".into(),
                    output_utf8: json!({"answer":"yes"}).to_string(),
                    budget_call_id: "capacity-dispatched".into(), latency_micros: 100,
                    issued_at_unix_seconds: 12,
                },
            ).await, Err(Error::Conflict(ref text)) if text.contains("exceeds 10000 nodes"))
            );
            assert_eq!(
                catalog(&fx).await,
                before_receipt,
                "receipt/output/edges rolled back without erasing dispatched cost"
            );
        } else {
            assert!(
                matches!(result,Err(Error::Conflict(ref text)) if text.contains("exceeds 10000 nodes"))
            );
            assert_eq!(catalog(&fx).await, before);
        }
    }
}

#[tokio::test]
async fn candidate_content_change_refuses_start_and_finalize_atomically() {
    for complete in [false, true] {
        let fx = if complete {
            ready(2, true).await
        } else {
            let fx = prepared(2, true).await;
            issue(&fx).await.unwrap();
            fx
        };
        let mut candidate = raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await;
        candidate["sources"].as_array_mut().unwrap().pop().unwrap();
        write_raw(
            &fx,
            "artifact",
            &fx.holdout.pair.candidate_digest,
            &candidate,
        )
        .await;
        let before = catalog(&fx).await;
        let error = if complete {
            IndependentEvaluationControl::finalize_complete(
                &fx.evaluator,
                &fx.store,
                &fx.ticket_id,
                2000,
            )
            .await
            .unwrap_err()
        } else {
            IndependentEvaluationControl::start_execution(
                &fx.worker,
                &fx.store,
                StartExecutionRequest {
                    ticket_id: fx.ticket_id.clone(),
                    target_id: "r0".into(),
                    side: ExecutionSide::Candidate,
                    fence: BudgetCallFence {
                        billing_scope: "billing-1".into(),
                        call_id: "not-dispatched".into(),
                        actual_input_digest: d("wrong"),
                        lease_token: "lease".into(),
                        lease_epoch: 1,
                        now: 11,
                    },
                },
            )
            .await
            .unwrap_err()
        };
        assert!(
            matches!(error,Error::Conflict(ref text) if text=="formal_candidate_dependency_binding_changed"),
            "{error:?}"
        );
        assert_eq!(catalog(&fx).await, before);
    }
}

#[tokio::test]
async fn revocation_preserves_dispatched_unknown_usage_and_late_receipt_settlement() {
    let fx = prepared(2, true).await;
    issue(&fx).await.unwrap();
    let view = IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        &fx.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: "uncertain-call".into(),
        actual_input_digest: view.request_digest.clone(),
        lease_token: "unknown-lease".into(),
        lease_epoch: 1,
        now: 11,
    };
    fx.store
        .reserve_budget_call(
            &fx.worker,
            &BudgetCallReservation {
                billing_scope: fence.billing_scope.clone(),
                call_id: fence.call_id.clone(),
                dispatch_group_id: fx.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: fence.actual_input_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: fence.lease_token.clone(),
                lease_until: 1000,
                now: 10,
            },
        )
        .await
        .unwrap();
    let dispatched = IndependentEvaluationControl::start_execution(
        &fx.worker,
        &fx.store,
        StartExecutionRequest {
            ticket_id: fx.ticket_id.clone(),
            target_id: "r0".into(),
            side: ExecutionSide::Candidate,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    fx.store
        .mark_budget_call_uncertain(&fx.worker, &fence, "fixture_usage_unknown")
        .await
        .unwrap();
    let unknown = actual_calls(&fx).await;
    assert_eq!(unknown.len(), 1);
    assert_eq!(
        unknown[0].state,
        evo_storage::budget::BudgetCallState::Uncertain
    );
    assert!(unknown[0].actual_cost_micros.is_none());
    let exposure_id = sid("exposure_ledger_v1", "family-e05");
    let dispatched_exposure = raw(&fx, "artifact", &exposure_id).await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    LifecycleStore::begin_revoke(
        &admin,
        &fx.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "S2".into(),
        },
        "unknown_usage_source_revoke",
        12,
    )
    .await
    .unwrap();
    let request = ExecutionReceiptRequest {
        receipt_id: "late-unknown-receipt".into(),
        ticket_id: fx.ticket_id.clone(),
        target_id: "r0".into(),
        side: ExecutionSide::Candidate,
        request_digest: view.request_digest,
        output_artifact_id: "late-unknown-output".into(),
        output_utf8: json!({"answer":"yes"}).to_string(),
        budget_call_id: fence.call_id.clone(),
        latency_micros: 123,
        issued_at_unix_seconds: 15,
    };
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::record_execution_receipt(
            &fx.worker,
            &fx.store,
            request.clone(),
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
    assert_eq!(
        fingerprint(&actual_calls(&fx).await).unwrap(),
        fingerprint(&unknown).unwrap()
    );
    assert_eq!(
        raw(&fx, "artifact", &exposure_id).await,
        dispatched_exposure
    );
    // An explicit stop retains the unknown dispatched cost. A late result is
    // billing/history evidence only; it must not become accepted formal proof.
    IndependentEvaluationControl::invalidate(
        &fx.evaluator,
        &fx.store,
        &fx.ticket_id,
        "source_revoked",
        16,
    )
    .await
    .unwrap();
    assert_eq!(
        actual_calls(&fx).await[0].state,
        evo_storage::budget::BudgetCallState::Uncertain
    );
    let late =
        IndependentEvaluationControl::record_execution_receipt(&fx.worker, &fx.store, request)
            .await
            .unwrap();
    let stored_ticket: EvaluationTicketV2 = serde_json::from_value(
        raw(&fx, "artifact", &sid("evaluation_ticket_v2", &fx.ticket_id)).await["payload"].clone(),
    )
    .unwrap();
    assert_eq!(stored_ticket.lifecycle, TicketLifecycle::Invalid);
    assert_eq!(
        stored_ticket.late_execution_receipt_ids,
        vec![late.receipt_id.clone()]
    );
    let mut session = fx.store.session().await.unwrap();
    assert!(
        session
            .outgoing_edges_bounded(
                &fx.worker,
                "artifact",
                &sid("execution_receipt_v2", &late.receipt_id),
                10000,
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !session
            .outgoing_edges_bounded(
                &fx.worker,
                "artifact",
                &sid("evaluation_ticket_v2", &fx.ticket_id),
                10000,
            )
            .await
            .unwrap()
            .contains(&(
                "artifact".into(),
                sid("execution_receipt_v2", &late.receipt_id)
            ))
    );
    session.commit().await.unwrap();
    let mut late_fence = fence;
    late_fence.now = 20;
    fx.store
        .reconcile_budget_call_cost(
            &admin,
            &late_fence.billing_scope,
            &late_fence.call_id,
            &UsageCharge {
                amount_micros: 37,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: "fixture-late-provider-record".into(),
                usage_record_id: "fixture-late-usage-record".into(),
                output_digest: late.output_digest,
            },
            late_fence.now,
        )
        .await
        .unwrap();
    fx.store
        .close_budget_call_execution(
            &fx.worker,
            "billing-1",
            &late_fence.call_id,
            dispatched.call.dispatch_id.as_deref().unwrap(),
            "fixture_late_complete",
            21,
        )
        .await
        .unwrap();
    let paid = actual_calls(&fx).await;
    assert_eq!(
        paid[0].state,
        evo_storage::budget::BudgetCallState::Finalized
    );
    assert_eq!(paid[0].actual_cost_micros, Some(37));
    assert!(paid[0].execution_closed);
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id,).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(catalog(&fx).await, before);
    assert_eq!(
        fingerprint(&actual_calls(&fx).await).unwrap(),
        fingerprint(&paid).unwrap()
    );
}

#[tokio::test]
async fn known_typed_schema_with_incomplete_identity_is_not_a_dependency_proof() {
    for kind in [
        "evaluation_ticket_issue_receipt_v1",
        "alpha_claim_reservation_v1",
        "evaluation_resource_evidence_v1",
    ] {
        let fx = prepared(2, true).await;
        let (logical_id, payload) = match kind {
            "evaluation_ticket_issue_receipt_v1" => (
                "empty-issue",
                json!({
                    "schema_version":"rsia.evaluation_ticket_issue_receipt.v1",
                    "ticket_id":"empty-issue", "registration_digest":d("registration"),
                    "manifest_digest":d("manifest"), "evaluator_actor":"", "issued_at_unix_seconds":10,
                }),
            ),
            "alpha_claim_reservation_v1" => (
                "family:claim",
                json!({
                    "schema_version":"rsia.alpha_claim_reservation.v1", "research_family_id":"family",
                    "claim_id":"claim", "attempt_id":"attempt", "ticket_id":"",
                }),
            ),
            _ => (
                "empty-resource",
                json!({
                    "schema_version":"rsia.evaluation_resource_evidence.v1", "ticket_id":"empty-resource",
                    "billing_scope":"billing-1", "cost_scope":"ticket_execution_only", "calls":[],
                }),
            ),
        };
        let id = sid(kind, logical_id);
        write_raw(
            &fx,
            "artifact",
            &id,
            &json!({
                "schema_version":"rsia.typed_artifact_envelope.v1", "id":id,
                "record_kind":kind, "payload":payload,
            }),
        )
        .await;
        add_edge(
            &fx,
            "artifact",
            &fx.holdout.pair.candidate_digest,
            "artifact",
            &id,
        )
        .await;
        let before = catalog(&fx).await;
        assert!(issue(&fx).await.is_err(), "{kind}");
        assert_eq!(catalog(&fx).await, before, "{kind}");
    }
}

#[tokio::test]
async fn rotating_anchor_category_tamper_refuses_active_bound_ticket_before_dispatch() {
    let fx = prepared(2, true).await;
    issue(&fx).await.unwrap();
    let id = sid("evaluation_ticket_v2", &fx.ticket_id);
    let mut ticket = raw(&fx, "artifact", &id).await;
    // Preserve target order, IDs, protected input refs/digests and category
    // counts, but try to turn the frozen critical anchor into a rotating row.
    ticket["payload"]["targets"][0]["kind"] = json!({
        "kind":"anchor", "anchor_id":fx.holdout.rotating_inputs[0].cluster_or_anchor_id,
    });
    let last = ticket["payload"]["targets"].as_array().unwrap().len() - 1;
    ticket["payload"]["targets"][last]["kind"] = json!({
        "kind":"rotating", "cluster_id":fx.holdout.anchor_inputs[0].cluster_or_anchor_id,
    });
    write_raw(&fx, "artifact", &id, &ticket).await;
    let before = catalog(&fx).await;
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap_err();
    assert!(
        matches!(status, Error::Conflict(ref text) if text == "frozen ticket input identity differs")
    );
    let broker = IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        &fx.ticket_id,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(broker, Error::Conflict(ref text) if text == "frozen ticket input identity differs")
    );
    let start = IndependentEvaluationControl::start_execution(
        &fx.worker,
        &fx.store,
        StartExecutionRequest {
            ticket_id: fx.ticket_id.clone(),
            target_id: "r0".into(),
            side: ExecutionSide::Candidate,
            fence: BudgetCallFence {
                billing_scope: "billing-1".into(),
                call_id: "not-dispatched".into(),
                actual_input_digest: d("unused"),
                lease_token: "unused".into(),
                lease_epoch: 1,
                now: 11,
            },
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(start, Error::Conflict(ref text) if text == "frozen ticket input identity differs")
    );
    assert_eq!(catalog(&fx).await, before);
}

#[tokio::test]
async fn formal_report_direct_tombstone_refuses_every_read_without_watermark_bump() {
    let fx = ready(20, true).await;
    let formal = finalize(&fx).await;
    assert!(matches!(
        &formal,
        FormalEvaluationV2::CompleteBatch {
            verdict: Verdict::Improved,
            stored_dependency_binding: Some(_),
            ..
        }
    ));
    let positive_status =
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await
            .unwrap();
    assert!(
        positive_status
            .ticket
            .candidate_dependency_binding
            .is_some()
    );
    let positive_view =
        IndependentEvaluationControl::verified_report_view(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await
            .unwrap();
    assert_eq!(positive_view.report_digest, fingerprint(&formal).unwrap());
    assert!(!positive_view.promotion_eligible);
    let mut session = fx.store.session().await.unwrap();
    verified_report_view_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap();
    let watermark_before = session.watermark(&fx.evaluator).await.unwrap();
    session.commit().await.unwrap();

    let formal_id = sid("formal_evaluation_v2", &fx.ticket_id);
    let formal_before = raw(&fx, "artifact", &formal_id).await;
    let candidate_before = raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await;
    // Add only the report's own correctly typed artifact tombstone. It remains
    // outside the stored content pins, so this does not alter any pinned fact.
    fixed_watermark_tombstone(&fx, "artifact", &formal_id).await;
    let before_reads = catalog(&fx).await;

    // Collect every actual entry's result before any refusal assertion, so a
    // failing self-probe still records all three live reads.
    let status =
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id).await;
    let view =
        IndependentEvaluationControl::verified_report_view(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await;
    let mut session = fx.store.session().await.unwrap();
    let same_session =
        verified_report_view_in_session(&fx.evaluator, &mut session, &fx.ticket_id).await;
    let watermark_after = session.watermark(&fx.evaluator).await.unwrap();
    drop(session);
    let after_reads = catalog(&fx).await;
    println!(
        "formal-self-tombstone actual status={status:?}\nactual verified_report_view={view:?}\nactual same_session={same_session:?}"
    );
    assert_eq!(watermark_after, watermark_before, "watermark was bumped");
    assert_eq!(raw(&fx, "artifact", &formal_id).await, formal_before);
    assert_eq!(
        raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await,
        candidate_before
    );
    assert_eq!(
        after_reads, before_reads,
        "report reads wrote business state"
    );
    assert!(matches!(status, Err(Error::Forbidden)), "{status:?}");
    assert!(matches!(view, Err(Error::Forbidden)), "{view:?}");
    assert!(
        matches!(same_session, Err(Error::Forbidden)),
        "{same_session:?}"
    );
}
