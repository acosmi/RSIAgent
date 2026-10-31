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
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope, ExecutionReceiptRequest,
    ExecutionSide, FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry,
    GraderReceiptRequest, IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord,
    ProtectedInputRef, ReadEvaluationTicket as EvaluationTicketV2, RegisteredEvaluationControl,
    StartExecutionRequest, TicketLifecycle, verified_report_view_in_session,
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
    resolved_ticket_id: std::sync::OnceLock<String>,
    legacy_bound: bool,
}
impl Fixture {
    fn ticket_id(&self) -> &str {
        self.resolved_ticket_id
            .get()
            .map(String::as_str)
            .unwrap_or(&self.ticket_id)
    }
}

fn current_control(
    old: &RegisteredEvaluationControl,
) -> evo_engine::streaming_evaluator::RegisteredEvaluationControlV3 {
    evo_engine::streaming_evaluator::RegisteredEvaluationControlV3 {
        schema_version: evo_engine::streaming_evaluator::RegisteredEvaluationControlV3::SCHEMA
            .into(),
        id: old.id.clone(),
        v1_plan_snapshot: old.v1_plan_snapshot.clone(),
        formal_plan: old.formal_plan.clone(),
        alpha_plan: old.alpha_plan.clone(),
        optimizer_comparison: old.optimizer_comparison.clone(),
        optimization_budget: old.optimization_budget.clone(),
        early_stop_plan: old.early_stop_plan.clone(),
        anchor_matrix: old.anchor_matrix.clone(),
        attempt_id: old.attempt_id.clone(),
        fixed_sample_claim_id: old.fixed_sample_claim_id.clone(),
        billing_scope: old.billing_scope.clone(),
        executor_actor: old.executor_actor.clone(),
        evaluator_actor: old.evaluator_actor.clone(),
        proposer_actor: old.proposer_actor.clone(),
        approver_actor: old.approver_actor.clone(),
        oracle_digest: old.oracle_digest.clone(),
        grader_digest: old.grader_digest.clone(),
        fixed_grader: old.fixed_grader.clone(),
        evidence_scope: old.evidence_scope,
        created_at_unix_seconds: old.created_at_unix_seconds,
        max_regression: evo_core::evaluation::FixedRegressionThreshold::new(0.125).unwrap(),
    }
}
fn current_holdout(
    old: &ProtectedHoldoutRecord,
    control: &evo_engine::streaming_evaluator::RegisteredEvaluationControlV3,
) -> evo_engine::streaming_evaluator::ProtectedHoldoutRecordV2 {
    evo_engine::streaming_evaluator::ProtectedHoldoutRecordV2 {
        schema_version: evo_engine::streaming_evaluator::ProtectedHoldoutRecordV2::SCHEMA.into(),
        id: old.id.clone(),
        registration_id: old.registration_id.clone(),
        pair: old.pair.clone(),
        manifest: old.manifest.clone(),
        candidate_bundle_digest: old.candidate_bundle_digest.clone(),
        baseline_bundle_digest: old.baseline_bundle_digest.clone(),
        rotating_inputs: old.rotating_inputs.clone(),
        anchor_inputs: old.anchor_inputs.clone(),
        oracle_entries: old.oracle_entries.clone(),
        oracle_payload_digest: old.oracle_payload_digest.clone(),
        frozen_configuration_digest: control.frozen_configuration().unwrap().digest().unwrap(),
    }
}
async fn unregistered_current_fixture(n: u32) -> Fixture {
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
    let host = Context::new("n", "host", Role::Host).unwrap();
    for id in ["S1", "S2"] {
        store_trace_authority(&store, &host, &source(id))
            .await
            .unwrap();
    }
    let current_control = current_control(&control);
    IndependentEvaluationControl::register_current_control(
        &evaluator,
        &store,
        current_control.clone(),
    )
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
        Some(current_control.digest().unwrap().as_str())
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
        legacy_bound: false,
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
    trusted_compiler_inputs::install_bundle_current_sources(&fx.store, &release_bundle())
        .await
        .unwrap();
    ReleaseStore::stage_current_bundle(
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
    let fx = unregistered_current_fixture(n).await;
    let current_control = current_control(&fx.control);
    if bound {
        stage(&fx).await;
    }
    IndependentEvaluationControl::register_current_holdout(
        &fx.evaluator,
        &fx.store,
        current_holdout(&fx.holdout, &current_control),
    )
    .await
    .unwrap();
    fx
}

/// The same registered fixture under an explicit control schema.
// Two exact M2c23cf2 N2 histories, each with a real absent-candidate and
// staged/issued checkpoint. Archive metadata never becomes a product wire.
const AG192_N2_LEGACY_ORIGINALS: &str = "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";
async fn prepared_under(schema: &str, n: u32, bound: bool) -> Fixture {
    use std::io::Write;
    assert_eq!(n, 2);
    let case = match schema {
        RegisteredEvaluationControl::SCHEMA => "candidate_n2_v2_missing_then_bound",
        RegisteredEvaluationControl::SCHEMA_V1 => "candidate_n2_v1_missing_then_bound",
        _ => panic!("unadmitted N2 control schema: {schema}"),
    };
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("rsia.sqlite3");
    let script = r#"import base64,hashlib,json,lzma,sys
from pathlib import Path
archive=json.loads(lzma.decompress(base64.b64decode(sys.stdin.read())))
assert archive['parent_head']=='2c23cf22154194931781d4dd0799d2ee622da236' and archive['parent_tree']=='a3464ce236eec93b0d2bd3a7528e24df2f81ef00'
case=archive['cases'][sys.argv[2]];point=case['checkpoints'][sys.argv[3]];root=Path(sys.argv[1]);decoded={}
for name,pin in point['files'].items():
    path=Path(name);assert name in ('rsia.sqlite3','backup-manifest.json') or (len(path.parts)==3 and path.parts[0]=='blobs' and all(len(p)==64 and all(c in '0123456789abcdef' for c in p) for p in path.parts[1:]))
    raw=base64.b64decode(pin['base64']);assert len(raw)==pin['bytes'] and hashlib.sha256(raw).hexdigest()==pin['sha256'];decoded[name]=raw
backup=json.loads(decoded['backup-manifest.json']);assert backup['complete'] is True and backup['schema_version']=='rsia.backup.v2'
assert backup['database']['sha256']==hashlib.sha256(decoded['rsia.sqlite3']).hexdigest() and backup['database']['bytes']==len(decoded['rsia.sqlite3'])
assert set(decoded)=={'rsia.sqlite3','backup-manifest.json'}|{b['path'] for b in backup['blobs']}
for blob in backup['blobs']:assert hashlib.sha256(decoded[blob['path']]).hexdigest()==blob['sha256'] and len(decoded[blob['path']])==blob['bytes']
for name,raw in decoded.items():
    target=root/name;target.parent.mkdir(parents=True,exist_ok=True)
    with target.open('xb') as file:file.write(raw)
print(json.dumps({'control_utf8':case['control_utf8'],'holdout_utf8':case['holdout_utf8'],'ticket_id':point['ticket_id']}))
"#;
    let mut child = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(directory.path())
        .arg(case)
        .arg(if bound {
            "bound-backup"
        } else {
            "candidate-absent-backup"
        })
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(AG192_N2_LEGACY_ORIGINALS.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original: Value = serde_json::from_slice(&output.stdout).unwrap();
    let control: RegisteredEvaluationControl =
        serde_json::from_str(original["control_utf8"].as_str().unwrap()).unwrap();
    let holdout: ProtectedHoldoutRecord =
        serde_json::from_str(original["holdout_utf8"].as_str().unwrap()).unwrap();
    assert_eq!(control.schema_version, schema);
    assert_eq!(control.formal_plan.n_planned, n);
    assert!(control.early_stop_plan.is_none());
    let store = Store::open(&db_path).await.unwrap();
    let resolved_ticket_id = std::sync::OnceLock::new();
    if bound {
        resolved_ticket_id
            .set(original["ticket_id"].as_str().unwrap().to_owned())
            .unwrap();
    }
    let mut fixture = Fixture {
        _dir: directory,
        db_path,
        store,
        evaluator: Context::new("n", "evaluator", Role::Evaluator).unwrap(),
        worker: Context::new("n", "executor", Role::Worker).unwrap(),
        ticket_id: "ticket-1".into(),
        control,
        holdout,
        candidate_cost: 10,
        resolved_ticket_id,
        legacy_bound: bound,
    };
    let before = catalog(&fixture).await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let mut session = fixture.store.session().await.unwrap();
    let money = session
        .root_budget_monetary_snapshot(&admin, "billing-1")
        .await
        .unwrap();
    assert_eq!(money.known_actual_micros, "0");
    assert!(money.calls.is_empty());
    let records: Vec<Value> = session.list(&fixture.evaluator, "artifact").await.unwrap();
    for (kind, expected) in [
        ("evaluation_ticket_v2", usize::from(bound)),
        ("evaluation_ticket_issue_receipt_v1", usize::from(bound)),
        ("alpha_claim_reservation_v1", if bound { 2 } else { 0 }),
        ("exposure_ledger_v1", usize::from(bound)),
    ] {
        assert_eq!(
            records
                .iter()
                .filter(|record| record["record_kind"] == kind)
                .count(),
            expected
        );
    }
    assert_eq!(
        session
            .get::<Value>(
                &fixture.evaluator,
                "artifact",
                &fixture.holdout.pair.candidate_digest
            )
            .await
            .unwrap()
            .is_some(),
        bound
    );
    if bound {
        for source in ["S1", "S2"] {
            assert!(
                session
                    .get::<Value>(&fixture.evaluator, "run", source)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }
    assert!(!money.complete_episode_cost && !money.ordinary_p_eligible);
    session.commit().await.unwrap();
    fixture.store.close().await;
    fixture.store = Store::open(&fixture.db_path).await.unwrap();
    assert_eq!(catalog(&fixture).await, before);
    fixture
}

async fn issue(fx: &Fixture) -> evo_core::Result<EvaluationTicketV2> {
    if fx.legacy_bound {
        let status =
            IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id()).await?;
        assert_eq!(status.ticket.schema_version, "rsia.evaluation_ticket.v3");
        assert_eq!(
            status
                .ticket
                .source_plan_binding
                .as_ref()
                .unwrap()
                .schema_version,
            "rsia.evaluation_ticket_source_plan_binding.v1"
        );
        return Ok(status.ticket);
    }
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
    .inspect(|ticket| {
        let _ = fx.resolved_ticket_id.set(ticket.id.clone());
    })
}

async fn assert_issue_refused_without_candidate(fx: &Fixture) {
    let before = catalog(fx).await;
    assert!(
        matches!(issue(fx).await, Err(Error::Conflict(reason)) if reason == "formal_candidate_record_missing_at_issue")
    );
    assert_eq!(catalog(fx).await, before);
    assert!(matches!(
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id()).await,
        Err(Error::NotFound)
    ));
}

async fn ready(n: u32, bound: bool) -> Fixture {
    let fx = prepared(n, bound).await;
    if !bound {
        assert_issue_refused_without_candidate(&fx).await;
        trusted_compiler_inputs::assert_frozen_issuer_static_read("dependency-legacy")
            .await
            .unwrap();
        stage(&fx).await;
    }
    issue(&fx).await.unwrap();
    for index in 0..n {
        grade_target(&fx, &format!("r{index}"), "ok", "wrong", index + 1).await;
    }
    grade_target(&fx, "a0", "ok", "ok", n + 1).await;
    fx
}

async fn finalize(fx: &Fixture) -> FormalEvaluationV2 {
    IndependentEvaluationControl::finalize_complete(&fx.evaluator, &fx.store, fx.ticket_id(), 2_000)
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
        .budget_calls_for_group(&fx.worker, "billing-1", fx.ticket_id())
        .await
        .unwrap();
    session.commit().await.unwrap();
    calls
}

async fn all_read_errors(fx: &Fixture) -> Vec<Error> {
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
        .await
        .unwrap_err();
    let view = IndependentEvaluationControl::verified_report_view(
        &fx.evaluator,
        &fx.store,
        fx.ticket_id(),
    )
    .await
    .unwrap_err();
    let mut session = fx.store.session().await.unwrap();
    let same_session = verified_report_view_in_session(&fx.evaluator, &mut session, fx.ticket_id())
        .await
        .unwrap_err();
    drop(session);
    let admin = Context::new("n", "approver", Role::Admin).unwrap();
    let admin_view =
        IndependentEvaluationControl::verified_report_view(&admin, &fx.store, fx.ticket_id())
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

/// The stored outgoing edges of the ticket.
async fn ticket_edges(fx: &Fixture) -> Vec<(String, String)> {
    let mut session = fx.store.session().await.unwrap();
    let edges = session
        .outgoing_edges_bounded(
            &fx.evaluator,
            "artifact",
            &sid("evaluation_ticket_v2", fx.ticket_id()),
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
    let native_envelope = raw(
        &fx,
        "artifact",
        &sid("formal_evaluation_v2", fx.ticket_id()),
    )
    .await;
    assert_eq!(native_envelope.as_object().unwrap().len(), 4);
    assert_eq!(
        native_envelope["schema_version"],
        json!("rsia.typed_artifact_envelope.v1")
    );
    assert_eq!(
        native_envelope["id"],
        json!(sid("formal_evaluation_v2", fx.ticket_id()))
    );
    assert_eq!(
        native_envelope["record_kind"],
        json!("formal_evaluation_v2")
    );
    let native_formal: evo_engine::streaming_evaluator::FormalEvaluationV3 =
        serde_json::from_value(native_envelope["payload"].clone()).unwrap();
    assert_eq!(
        native_formal.schema_version,
        evo_engine::streaming_evaluator::FormalEvaluationV3::SCHEMA
    );
    assert_eq!(
        native_formal.decision_version,
        evo_engine::streaming_evaluator::FormalEvaluationV3::DECISION
    );
    assert_eq!(
        fingerprint(&native_formal.result).unwrap(),
        fingerprint(&formal).unwrap()
    );
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
        .await
        .unwrap();
    let candidate = raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await;
    let binding = status.ticket.candidate_dependency_binding.as_ref().unwrap();
    assert_eq!(
        binding.schema_version,
        "rsia.formal_candidate_dependency_binding.v1"
    );
    // Compare native typed fingerprints, not Value's reordered map serialization.
    let typed: evo_engine::release_store::CurrentReleaseCandidateRecord =
        serde_json::from_value(candidate).unwrap();
    assert_eq!(
        typed.schema_version,
        evo_engine::release_store::RELEASE_CANDIDATE_SCHEMA_V2
    );
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
        sid("evaluation_ticket_v2", fx.ticket_id()),
        sid("formal_evaluation_v2", fx.ticket_id())
    );
    assert_eq!(
        stored.ticket_physical_id,
        sid("evaluation_ticket_v2", fx.ticket_id())
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
    assert!(!stored.nodes.iter().any(|node| node.id
        == sid("formal_evaluation_v2", fx.ticket_id())
        || node.record_kind.as_deref() == Some("exposure_ledger_v1")
        || node.kind == "budget_call"));
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
            IndependentEvaluationControl::verified_report_view(ctx, &fx.store, fx.ticket_id())
                .await
                .unwrap();
        assert_eq!(view.report_digest, fingerprint(&native_formal).unwrap());
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
    verified_report_view_in_session(&fx.evaluator, &mut session, fx.ticket_id())
        .await
        .unwrap();
    let report_edges = session
        .outgoing_edges_bounded(
            &fx.evaluator,
            "artifact",
            &sid("formal_evaluation_v2", fx.ticket_id()),
            10000,
        )
        .await
        .unwrap();
    assert_eq!(report_edges, {
        let mut edges = vec![
            (
                "artifact".into(),
                sid("evaluation_ticket_v2", fx.ticket_id()),
            ),
            (
                "artifact".into(),
                sid("evaluation_resource_evidence_v1", fx.ticket_id()),
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
            fx.ticket_id()
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
        fx.ticket_id(),
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
                dispatch_group_id: fx.ticket_id().to_owned(),
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
            fx.ticket_id(),
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
                ticket_id: fx.ticket_id().to_owned(),
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
            fx.ticket_id(),
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
    let ticket = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
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
    let formal_id = sid("formal_evaluation_v2", fx.ticket_id());
    let original_formal = raw(&fx, "artifact", &formal_id).await;
    let mut no_binding = original_formal.clone();
    no_binding["payload"]["result"]
        .as_object_mut()
        .unwrap()
        .remove("stored_dependency_binding")
        .unwrap();
    write_raw(&fx, "artifact", &formal_id, &no_binding).await;
    assert_changed_reads(&fx, "formal_stored_dependency_binding_changed").await;
    let mut wrong = original_formal;
    wrong["payload"]["result"]["stored_dependency_binding"]["candidate_binding_digest"] =
        json!(d("wrong reference"));
    write_raw(&fx, "artifact", &formal_id, &wrong).await;
    assert_changed_reads(&fx, "formal_stored_dependency_binding_changed").await;
}

#[tokio::test]
async fn required_edges_use_physical_keys_and_extra_inconsistent_fact_edges_refuse() {
    for alias in [false, true] {
        let fx = ready(2, true).await;
        finalize(&fx).await;
        let report_id = sid("formal_evaluation_v2", fx.ticket_id());
        let ticket_id = sid("evaluation_ticket_v2", fx.ticket_id());
        if alias {
            remove_edge(&fx, &report_id, &ticket_id).await;
        }
        add_edge(&fx, "artifact", &report_id, "artifact", fx.ticket_id()).await;
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

async fn expected_current_candidate_nodes(fx: &Fixture) -> BTreeSet<(String, String)> {
    let candidate_id = &fx.holdout.pair.candidate_digest;
    let compiler_id = format!(
        "compiler-binding-{}",
        fingerprint(&(candidate_id, &fx.holdout.candidate_bundle_digest)).unwrap()
    );
    let mut session = fx.store.session().await.unwrap();
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StoredCompilerBinding {
        id: String,
        schema_version: String,
        candidate_id: String,
        binding: evo_engine::formal_authority::CurrentFormalCompilerInputBinding,
    }
    let compiler: StoredCompilerBinding = session
        .need(&fx.evaluator, "artifact", &compiler_id)
        .await
        .unwrap();
    assert_eq!(compiler.id, compiler_id);
    assert_eq!(
        compiler.schema_version,
        evo_engine::release_store::STORED_BINDING_SCHEMA_V2
    );
    assert_eq!(compiler.candidate_id, *candidate_id);
    session.commit().await.unwrap();
    let mut nodes = compiler
        .binding
        .closure
        .nodes
        .into_iter()
        .map(|node| (node.kind, node.id))
        .collect::<BTreeSet<_>>();
    nodes.extend([
        ("artifact".into(), candidate_id.clone()),
        ("artifact".into(), compiler_id),
        (
            "artifact".into(),
            format!("formal-bundle-{}", fx.holdout.candidate_bundle_digest),
        ),
        ("run".into(), "S1".into()),
        ("run".into(), "S2".into()),
        ("run".into(), "S3".into()),
    ]);
    nodes
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
    let expected_nodes = expected_current_candidate_nodes(&fx).await;
    let ticket = issue(&fx).await.unwrap();
    let binding = ticket.candidate_dependency_binding.unwrap();
    assert_eq!(binding.sources.len(), 2);
    assert_eq!(binding.nodes.len(), expected_nodes.len());
    assert_eq!(
        binding
            .nodes
            .iter()
            .map(|n| (n.kind.clone(), n.id.clone()))
            .collect::<BTreeSet<_>>(),
        expected_nodes
    );
    assert!(
        binding
            .nodes
            .iter()
            .any(|node| node.kind == "run" && node.id == "S3")
    );
    fixed_watermark_tombstone(&fx, "run", "S3").await;
    let before = catalog(&fx).await;
    assert!(matches!(
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id()).await,
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
    let expected_nodes = expected_current_candidate_nodes(&fx).await;
    let ticket = issue(&fx).await.unwrap();
    let binding = ticket.candidate_dependency_binding.unwrap();
    assert_eq!(binding.nodes.len(), expected_nodes.len());
    assert_eq!(
        binding
            .nodes
            .iter()
            .map(|n| (n.kind.clone(), n.id.clone()))
            .collect::<BTreeSet<_>>(),
        expected_nodes
    );
    assert_eq!(
        binding
            .nodes
            .iter()
            .map(|node| (&node.kind, &node.id))
            .collect::<BTreeSet<_>>()
            .len(),
        expected_nodes.len()
    );
    let root = binding
        .nodes
        .iter()
        .find(|node| node.id == fx.holdout.pair.candidate_digest)
        .unwrap();
    assert_eq!(root.outgoing_edge_count, 4);
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
            (
                "artifact".to_string(),
                format!("formal-bundle-{}", fx.holdout.candidate_bundle_digest)
            ),
            ("run".to_string(), "S1".to_string()),
            ("run".to_string(), "S2".to_string()),
        ])
        .unwrap()
    );
    IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
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
    let ticket = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
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
            // V3 cannot lose its candidate while retaining a source-plan.
            // Original V2 absence is checked from the frozen old producer.
            assert!(serde_json::from_value::<EvaluationTicketV2>(absent).is_err());
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
#[tokio::test]
async fn legacy_absence_preserves_exact_parent_bytes_fingerprints_and_never_upgrades_later() {
    trusted_compiler_inputs::assert_frozen_issuer_static_read("dependency-legacy")
        .await
        .unwrap();
    let frozen: Value =
        serde_json::from_str(include_str!("fixtures/ag135b_legacy_issuers.json")).unwrap();
    let ticket = frozen["fixtures"]["dependency-legacy"]["tables"]["objects"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|row| {
            let body: Value = serde_json::from_str(row[4]["text"].as_str()?).ok()?;
            (body["record_kind"] == "evaluation_ticket_v2").then(|| {
                serde_json::from_value::<EvaluationTicketV2>(body["payload"].clone()).unwrap()
            })
        })
        .unwrap();
    assert!(ticket.candidate_dependency_binding.is_none());
    assert!(ticket.source_plan_binding.is_none());
    let original = serde_json::to_vec(&LegacyTicket(&ticket)).unwrap();
    assert_eq!(serde_json::to_vec(&ticket).unwrap(), original);
    assert_eq!(fingerprint(&ticket).unwrap(), hash(&original));
    let wire = String::from_utf8(original).unwrap();
    assert!(!wire.contains("candidate_dependency_binding"));
    assert!(!wire.contains("source_plan_binding"));
}

/// AG-100 T1 (L564, L1055/L1057). A ticket requested while no candidate
/// record exists is refused under both control schemas before any write: the
/// whole database is unchanged and no ticket exists. Before AG-100 this issue
/// stored an unbound legacy ticket. Every outcome is collected first.
#[tokio::test]
async fn issue_without_a_candidate_record_is_refused_for_every_control_schema_before_any_write() {
    let current = prepared(2, false).await;
    assert_issue_refused_without_candidate(&current).await;
    current.store.close().await;
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
        let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
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
    let current = prepared(2, true).await;
    let ticket = issue(&current).await.unwrap();
    assert_eq!(ticket.schema_version, "rsia.evaluation_ticket.v4");
    assert_eq!(ticket.n_planned, 2);
    let source = ticket.source_plan_binding.as_ref().unwrap();
    assert_eq!(
        source.schema_version,
        "rsia.evaluation_ticket_source_plan_binding.v2"
    );
    assert!(source.plan_id.starts_with("experiment-plan-v3-"));
    let binding = ticket.candidate_dependency_binding.as_ref().unwrap();
    assert_eq!(binding.candidate_id, current.holdout.pair.candidate_digest);
    assert_eq!(binding.namespace, "n");
    let status = IndependentEvaluationControl::status(
        &current.evaluator,
        &current.store,
        current.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(&status.ticket).unwrap(),
        fingerprint(&ticket).unwrap()
    );
    assert!(ticket_edges(&current).await.contains(&(
        "artifact".into(),
        current.holdout.pair.candidate_digest.clone(),
    )));
    current.store.close().await;
    for schema in [
        RegisteredEvaluationControl::SCHEMA,
        RegisteredEvaluationControl::SCHEMA_V1,
    ] {
        let fx = prepared_under(schema, 2, true).await;
        let ticket = issue(&fx).await.unwrap();
        let binding = ticket.candidate_dependency_binding.as_ref().unwrap();
        assert_eq!(binding.candidate_id, fx.holdout.pair.candidate_digest);
        assert_eq!(binding.namespace, "n");
        let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
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
    trusted_compiler_inputs::assert_frozen_issuer_static_read("dependency-legacy")
        .await
        .unwrap();
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
    // Count the real Current authorization graph independently in the owned
    // SQLite fixture, then fill precisely to 10000 or 10001 total typed nodes.
    for total in [10_000, 10_001] {
        let fx = prepared(2, true).await;
        let template = serde_json::to_string(&source("graph-template")).unwrap();
        let alpha_ids: Vec<_> = fx
            .control
            .alpha_plan
            .allocations
            .iter()
            .filter(|allocation| allocation.attempt_id == fx.control.attempt_id)
            .map(|allocation| {
                sid(
                    "alpha_claim_reservation_v1",
                    &format!(
                        "{}:{}",
                        fx.control.alpha_plan.research_family_id, allocation.claim_id
                    ),
                )
            })
            .collect();
        let holdout_id = sid("protected_holdout_v41", &fx.holdout.id);
        let planning = own_sqlite(
            &fx,
            r#"
import sqlite3,sys,json,copy
con=sqlite3.connect(sys.argv[1]);template=json.loads(sys.argv[2]);total=int(sys.argv[3])
candidate=sys.argv[4];holdout=sys.argv[5];alpha_ids=json.loads(sys.argv[6])
roots=[('artifact',candidate),('artifact',holdout)]
assert len(set(roots))==2
assert len(alpha_ids)==len(set(alpha_ids))
def closure(roots):
    values=','.join('(?,?)' for _ in roots)
    query="WITH RECURSIVE closure(kind,id) AS (VALUES "+values+" UNION SELECT d.dst_kind,d.dst_id FROM dependencies d JOIN closure p ON d.namespace='n' AND d.src_kind=p.kind AND d.src_id=p.id) SELECT kind,id FROM closure ORDER BY kind,id"
    return set(con.execute(query,[part for root in roots for part in root]))
prior_keys=set(con.execute("SELECT kind,id FROM objects WHERE namespace='n'"))
prior=closure(roots);candidate_prior=closure([roots[0]])
assert prior <= prior_keys
assert not {('artifact',id) for id in alpha_ids} & prior_keys
for (body,) in con.execute("SELECT body FROM objects WHERE namespace='n'"):
    assert json.loads(body).get('record_kind') not in ('evaluation_ticket_v2','evaluation_ticket_issue_receipt_v1','alpha_claim_reservation_v1')
C=len(prior);B=len(candidate_prior);A=len(alpha_ids)
count=total-C-2-A
assert count>0
for i in range(count):
    doc=copy.deepcopy(template);node=f'node-{i:05}'
    doc['record']['id']=node;doc['trace']['run_id']=node
    con.execute('INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)',('n','run',node,'host',json.dumps(doc)))
    previous='S1' if i==0 else f'node-{i-1:05}'
    con.execute('INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)',('n','run',previous,'run',node))
# A second root-to-last edge forms a lawful diamond without adding nodes.
con.execute('INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)',('n','run','S2','run',f'node-{count-1:05}'))
assert len(closure(roots))==C+count
assert len(closure(roots))+2+A==total
con.commit()
print(json.dumps({'C':C,'B':B,'A':A,'extra':count,'total':total,'prior_keys':sorted(prior_keys)},separators=(',',':')))
con.close()
"#,
            &[
                &template,
                &total.to_string(),
                &fx.holdout.pair.candidate_digest,
                &holdout_id,
                &serde_json::to_string(&alpha_ids).unwrap(),
            ],
        );
        let planning: Value = serde_json::from_str(&planning).unwrap();
        println!(
            "current-capacity-inputs C={} B={} A={} extra={} total={}",
            planning["C"], planning["B"], planning["A"], planning["extra"], planning["total"]
        );
        let before = catalog(&fx).await;
        let result = issue(&fx).await;
        if total == 10_000 {
            let ticket = result.unwrap();
            let mut new_ids = vec![
                sid("evaluation_ticket_v2", &ticket.id),
                sid("evaluation_ticket_issue_receipt_v1", &ticket.id),
            ];
            new_ids.extend(alpha_ids.clone());
            assert_eq!(
                new_ids.iter().collect::<BTreeSet<_>>().len(),
                2 + alpha_ids.len()
            );
            for id in &new_ids {
                assert!(
                    !planning["prior_keys"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(["artifact", id]))
                );
            }
            // This SQL traversal reads the actual issued ticket's complete
            // physical closure, independently of the collector under test.
            let issued_count = own_sqlite(
                &fx,
                r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
root=sys.argv[2];new_ids=json.loads(sys.argv[3])
query="WITH RECURSIVE closure(kind,id) AS (VALUES ('artifact',?) UNION SELECT d.dst_kind,d.dst_id FROM dependencies d JOIN closure p ON d.namespace='n' AND d.src_kind=p.kind AND d.src_id=p.id) SELECT kind,id FROM closure"
nodes=set(con.execute(query,(root,)))
for id in new_ids:
    assert ('artifact',id) in nodes
    assert con.execute("SELECT 1 FROM objects WHERE namespace='n' AND kind='artifact' AND id=?",(id,)).fetchone()==(1,)
print(len(nodes))
con.close()
"#,
                &[
                    &sid("evaluation_ticket_v2", &ticket.id),
                    &serde_json::to_string(&new_ids).unwrap(),
                ],
            );
            assert_eq!(issued_count.trim(), "10000");
            assert_eq!(
                ticket
                    .candidate_dependency_binding
                    .as_ref()
                    .unwrap()
                    .nodes
                    .len(),
                9994
            );
            IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
                .await
                .unwrap();
            let view = IndependentEvaluationControl::broker_task(
                &fx.worker,
                &fx.store,
                fx.ticket_id(),
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
                        dispatch_group_id: fx.ticket_id().to_owned(),
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
                    ticket_id: fx.ticket_id().to_owned(),
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
                    receipt_id: "capacity-receipt".into(), ticket_id: fx.ticket_id().to_owned(),
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
                fx.ticket_id(),
                2000,
            )
            .await
            .unwrap_err()
        } else {
            IndependentEvaluationControl::start_execution(
                &fx.worker,
                &fx.store,
                StartExecutionRequest {
                    ticket_id: fx.ticket_id().to_owned(),
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
        fx.ticket_id(),
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
                dispatch_group_id: fx.ticket_id().to_owned(),
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
            ticket_id: fx.ticket_id().to_owned(),
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
        ticket_id: fx.ticket_id().to_owned(),
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
        fx.ticket_id(),
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
        raw(
            &fx,
            "artifact",
            &sid("evaluation_ticket_v2", fx.ticket_id()),
        )
        .await["payload"]
            .clone(),
    )
    .unwrap();
    assert_eq!(stored_ticket.lifecycle, TicketLifecycle::Invalid);
    // AG-118: after the Invalid report is final the late receipt and its
    // output are audit evidence only; the ticket keeps the bytes its report
    // pins at invalidation, and one audit entry names the receipt.
    assert!(stored_ticket.late_execution_receipt_ids.is_empty());
    let formal = raw(
        &fx,
        "artifact",
        &sid("formal_evaluation_v2", fx.ticket_id()),
    )
    .await;
    assert_eq!(
        formal["payload"]["result"]["ticket_digest"],
        json!(fingerprint(&stored_ticket).unwrap())
    );
    let stored_receipt = raw(
        &fx,
        "artifact",
        &sid("execution_receipt_v2", &late.receipt_id),
    )
    .await;
    assert_eq!(stored_receipt["payload"], json!(late));
    let output_id = sid("evaluation_execution_output_v1", &late.output_artifact_id);
    assert_eq!(
        raw(&fx, "artifact", &output_id).await["payload"]["output_digest"],
        json!(late.output_digest)
    );
    let audit = r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
rows=[json.loads(p) for (p,) in con.execute("SELECT payload FROM audit WHERE namespace=?",(sys.argv[2],))]
print(sum(1 for r in rows if r['operation']=='late_execution_receipt_after_formal' and r['entity']==sys.argv[3]))
"#;
    assert_eq!(own_sqlite(&fx, audit, &["n", &late.receipt_id]).trim(), "1");
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
                &sid("evaluation_ticket_v2", fx.ticket_id()),
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
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id(),).await,
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
    let id = sid("evaluation_ticket_v2", fx.ticket_id());
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
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
        .await
        .unwrap_err();
    assert!(
        matches!(status, Error::Conflict(ref text) if text == "frozen ticket input identity differs")
    );
    let broker = IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        fx.ticket_id(),
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
            ticket_id: fx.ticket_id().to_owned(),
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
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id())
            .await
            .unwrap();
    assert!(
        positive_status
            .ticket
            .candidate_dependency_binding
            .is_some()
    );
    let positive_view = IndependentEvaluationControl::verified_report_view(
        &fx.evaluator,
        &fx.store,
        fx.ticket_id(),
    )
    .await
    .unwrap();
    let raw_formal = raw(
        &fx,
        "artifact",
        &sid("formal_evaluation_v2", fx.ticket_id()),
    )
    .await;
    let raw_formal: evo_engine::streaming_evaluator::FormalEvaluationV3 =
        serde_json::from_value(raw_formal["payload"].clone()).unwrap();
    assert_eq!(
        positive_view.report_digest,
        fingerprint(&raw_formal).unwrap()
    );
    assert!(!positive_view.promotion_eligible);
    let mut session = fx.store.session().await.unwrap();
    verified_report_view_in_session(&fx.evaluator, &mut session, fx.ticket_id())
        .await
        .unwrap();
    let watermark_before = session.watermark(&fx.evaluator).await.unwrap();
    session.commit().await.unwrap();

    let formal_id = sid("formal_evaluation_v2", fx.ticket_id());
    let formal_before = raw(&fx, "artifact", &formal_id).await;
    let candidate_before = raw(&fx, "artifact", &fx.holdout.pair.candidate_digest).await;
    // Add only the report's own correctly typed artifact tombstone. It remains
    // outside the stored content pins, so this does not alter any pinned fact.
    fixed_watermark_tombstone(&fx, "artifact", &formal_id).await;
    let before_reads = catalog(&fx).await;

    // Collect every actual entry's result before any refusal assertion, so a
    // failing self-probe still records all three live reads.
    let status =
        IndependentEvaluationControl::status(&fx.evaluator, &fx.store, fx.ticket_id()).await;
    let view = IndependentEvaluationControl::verified_report_view(
        &fx.evaluator,
        &fx.store,
        fx.ticket_id(),
    )
    .await;
    let mut session = fx.store.session().await.unwrap();
    let same_session =
        verified_report_view_in_session(&fx.evaluator, &mut session, fx.ticket_id()).await;
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
