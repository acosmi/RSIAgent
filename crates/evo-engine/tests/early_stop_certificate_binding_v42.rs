//! Stored early-stop certificates must describe the actual first stopping event.
//! These are local ProgramFixture receipts, not provider or production proof.
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
    AnchorCoverageMatrix, CriticalCapabilityCoverage, ExposureLedger, FrozenCandidatePair,
    HoldoutManifest, MonetaryReservationState, SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, CompletePairedUnit, EarlyStopCertificate, EarlyStopDecision, EarlyStopPlan,
    FormalClaimKind, ResearchFamilyAlphaPlan, SequentialRejectOnly,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    AnchorEvidenceStatus, CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
    ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2,
    FrozenOracleEntry, GraderReceiptRequest, IndependentEvaluationControl, IssueTicketRequest,
    ProtectedHoldoutRecord, ProtectedInputRef, ReadEvaluationTicket as EvaluationTicketV2,
    RegisteredEvaluationControl, StartExecutionRequest, StreamingEvaluationStatus, TargetState,
    TicketLifecycle, VerifiedFormalReportView, VerifiedReportVariant,
    verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage, RootBudgetAuthorization,
    UsageCharge,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const N: u32 = 60;
const NAMESPACE: &str = "early-stop-binding-fixture";
const BILLING_SCOPE: &str = "early-stop-billing";
const TICKET_ID: &str = "early-stop-ticket";
const FORMAL_KIND: &str = "formal_evaluation_v2";
const CHANGED: &str = "formal_early_stop_certificate_changed";
const TICKET_KIND: &str = "evaluation_ticket_v2";
const LEDGER_KIND: &str = "exposure_ledger_v1";

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

fn bundle() -> evo_core::contract::ResolvedBundleV3 {
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
        NAMESPACE,
        &digest("fixture-environment"),
    )
    .unwrap()
}

struct Fixture {
    _temporary: Option<tempfile::TempDir>,
    db_path: PathBuf,
    artifacts: Option<PathBuf>,
    store: Store,
    evaluator: Context,
    worker: Context,
    control: RegisteredEvaluationControl,
    holdout: ProtectedHoldoutRecord,
    resolved_ticket_id: std::sync::OnceLock<String>,
}
impl Fixture {
    fn ticket_id(&self) -> &str {
        self.resolved_ticket_id
            .get()
            .map(String::as_str)
            .unwrap_or(TICKET_ID)
    }
}

// ROOT can provide a fresh existing parent directory for future authorized
// probes. Each case creates one absent leaf and retains its DB and snapshots.
// Ordinary software runs use a separate TempDir for every fixture.
fn fixture_directory(case: &str) -> (Option<tempfile::TempDir>, PathBuf, Option<PathBuf>) {
    match std::env::var_os("RSIA_AG089_CASE_OUTPUT_DIR") {
        Some(root) => {
            let root = PathBuf::from(root);
            assert!(root.is_absolute());
            assert!(std::fs::symlink_metadata(&root).unwrap().is_dir());
            let leaf = root.join(case);
            std::fs::create_dir(&leaf).unwrap();
            (None, leaf.join("fixture.sqlite3"), Some(leaf))
        }
        None => {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("fixture.sqlite3");
            (Some(dir), db, None)
        }
    }
}

fn persist<T: Serialize>(fixture: &Fixture, name: &str, value: &T) {
    if let Some(directory) = &fixture.artifacts {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(value).unwrap())
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
    }
}

async fn fixture(case: &str, schema: &str, sequential: bool) -> Fixture {
    let (temporary, db_path, artifacts) = fixture_directory(case);
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
        "statistics-family",
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
        schema_version: schema.into(),
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
        "statistics-family",
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
        resolved_ticket_id: std::sync::OnceLock::new(),
        _temporary: temporary,
        db_path,
        artifacts,
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
    trusted_compiler_inputs::install_bundle_current_sources(&fixture.store, &bundle())
        .await
        .unwrap();
    ReleaseStore::stage_current_bundle(
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
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        fixture.ticket_id(),
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
                dispatch_group_id: fixture.ticket_id().into(),
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
            ticket_id: fixture.ticket_id().into(),
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
            ticket_id: fixture.ticket_id().into(),
            target_id: target.into(),
            side,
            request_digest: task.request_digest,
            output_artifact_id: format!("output-{ordinal}"),
            output_utf8: json!({"answer": answer}).to_string(),
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
                output_digest: receipt.output_digest,
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

async fn grade(
    fixture: &Fixture,
    target: &str,
    candidate_answer: &str,
    baseline_answer: &str,
    ordinal: u32,
) {
    let candidate = execute(
        fixture,
        target,
        ExecutionSide::Candidate,
        candidate_answer,
        ordinal * 2,
    )
    .await;
    let baseline = execute(
        fixture,
        target,
        ExecutionSide::Baseline,
        baseline_answer,
        ordinal * 2 + 1,
    )
    .await;
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: format!("grader-{ordinal}"),
            ticket_id: fixture.ticket_id().into(),
            target_id: target.into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 20 + i64::from(ordinal) * 10,
        },
    )
    .await
    .unwrap();
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
    .inspect(|ticket| {
        let _ = fixture.resolved_ticket_id.set(ticket.id.clone());
    })
}

async fn issue(fixture: &Fixture, bound: bool) {
    if !bound {
        assert_issue_refused_without_candidate(fixture).await;
        trusted_compiler_inputs::assert_frozen_issuer_static_read("early-legacy")
            .await
            .unwrap();
    }
    stage_candidate(fixture).await;
    let ticket = request_ticket(fixture).await.unwrap();
    assert!(ticket.candidate_dependency_binding.is_some());
}

/// AG-100's refusal of an issue that has no candidate record to bind.
const CANDIDATE_RECORD_MISSING: &str = "formal_candidate_record_missing_at_issue";

/// AG-100 (L564, L1055/L1057): with no candidate record no ticket is
/// issued. The refusal precedes every write: the whole database is unchanged
/// and the ticket does not exist.
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
    assert!(matches!(
        IndependentEvaluationControl::status(
            &fixture.evaluator,
            &fixture.store,
            fixture.ticket_id()
        )
        .await,
        Err(Error::NotFound)
    ));
}

async fn before_futility_stop(fixture: &Fixture, bound: bool) {
    issue(fixture, bound).await;
    grade(fixture, "a0", "ok", "ok", 1).await;
    // This genuine completed result is outside the later stopping prefix.
    grade(fixture, "r13", "ok", "wrong", 2).await;
    for i in 0..12 {
        grade(fixture, &format!("r{i}"), "wrong", "ok", i + 3).await;
    }
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Running);
    assert!(status.formal.is_none());
    let plan = fixture.control.early_stop_plan.as_ref().unwrap();
    let mut stream = SequentialRejectOnly::open(plan.clone(), &fixture.control.alpha_plan).unwrap();
    stream
        .record_complete_unit(CompletePairedUnit {
            cluster_id: "cluster-13".into(),
            baseline_score_micros: 0,
            candidate_score_micros: 1_000_000,
        })
        .unwrap();
    for i in 0..12 {
        assert_eq!(
            stream
                .record_complete_unit(CompletePairedUnit {
                    cluster_id: format!("cluster-{i}"),
                    baseline_score_micros: 1_000_000,
                    candidate_score_micros: 0,
                })
                .unwrap(),
            EarlyStopDecision::Continue,
        );
    }
    assert_eq!(stream.k(), 12);
    assert!((stream.boundary().unwrap().unwrap().ucb - 0.031_041_701_1).abs() < 1e-10);
}

async fn stopped(case: &str, schema: &str, bound: bool) -> Fixture {
    let fixture = fixture(case, schema, true).await;
    before_futility_stop(&fixture, bound).await;
    grade(&fixture, "r12", "wrong", "ok", 15).await;
    fixture
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

async fn stored<T: serde::de::DeserializeOwned>(
    fixture: &Fixture,
    kind: &str,
    id: &str,
) -> Envelope<T> {
    let mut session = fixture.store.session().await.unwrap();
    let value = session
        .need(&fixture.evaluator, "artifact", &physical_id(kind, id))
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
}

async fn put<T: Serialize>(fixture: &Fixture, envelope: &Envelope<T>) {
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.evaluator,
            "artifact",
            &envelope.id,
            fixture.evaluator.actor(),
            envelope,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

struct ReadResults {
    status: evo_core::Result<StreamingEvaluationStatus>,
    view: evo_core::Result<VerifiedFormalReportView>,
    same_session: evo_core::Result<VerifiedFormalReportView>,
    watermark: Option<(i64, String)>,
}

async fn read_every_entry(fixture: &Fixture, phase: &str) -> ReadResults {
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await;
    let view = IndependentEvaluationControl::verified_report_view(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await;
    let mut session = fixture.store.session().await.unwrap();
    let same_session =
        verified_report_view_in_session(&fixture.evaluator, &mut session, fixture.ticket_id())
            .await;
    let watermark = session.watermark(&fixture.evaluator).await.unwrap();
    drop(session);
    let actual = json!({
        "phase": phase,
        "status": {"ok": status.is_ok(), "actual_debug": format!("{status:?}")},
        "view": {"ok": view.is_ok(), "actual_debug": format!("{view:?}")},
        "same_session": {"ok": same_session.is_ok(), "actual_debug": format!("{same_session:?}")},
        "watermark": watermark,
    });
    println!("actual-early-stop-three-read-results {actual}");
    persist(
        fixture,
        &format!("{phase}-three-read-results.json"),
        &actual,
    );
    ReadResults {
        status,
        view,
        same_session,
        watermark,
    }
}

fn assert_all_early_ok(reads: &ReadResults, bound: bool) {
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    assert!(status.ticket.candidate_dependency_binding.is_some());
    let _ = bound;
    assert!(matches!(
        status.formal.as_ref(),
        Some(FormalEvaluationV2::EarlyStopped { .. })
    ));
    for view in [
        reads.view.as_ref().unwrap(),
        reads.same_session.as_ref().unwrap(),
    ] {
        assert_eq!(view.variant, VerifiedReportVariant::EarlyStopped);
        assert_eq!(view.verdict, None);
        assert_eq!(view.anchor_status, AnchorEvidenceStatus::NotApplicable);
        assert_eq!(view.cost_evidence_scope, None);
        assert_eq!(view.evidence_scope, EvaluationEvidenceScope::ProgramFixture);
        assert_eq!(
            view.dependency_status,
            DependencyEvidenceStatus::NamespaceWatermarkOnly
        );
        assert!(!view.promotion_eligible);
    }
}

fn assert_futility_positive(reads: &ReadResults, bound: bool) {
    assert_all_early_ok(reads, bound);
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.targets.len(), N as usize + 1);
    assert_eq!(status.ticket.next_sequence, 16);
    let FormalEvaluationV2::EarlyStopped {
        certificate,
        ticket_digest,
        ..
    } = status.formal.as_ref().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(*ticket_digest, fingerprint(&status.ticket).unwrap());
    assert_eq!(certificate.k(), 13);
    assert_eq!(
        certificate.stop_reason(),
        EarlyStopDecision::FutilityQualityGain
    );
    assert!(!certificate.usage_uncertain());
    assert_eq!(
        fingerprint(certificate).unwrap(),
        fingerprint(status.ticket.pending_certificate.as_ref().unwrap()).unwrap(),
    );
    let wire = serde_json::to_value(certificate).unwrap();
    assert_eq!(wire["lcb"], "-1");
    let ucb = wire["ucb"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((ucb - (-0.009_239_226_6)).abs() < 1e-10);
    assert!(ucb > -0.01);
    assert_eq!(wire["stop_sequence"], 16);
    assert_eq!(wire["stopped_at_unix_ms"], 170_000);
    let completed = status
        .ticket
        .targets
        .iter()
        .filter(|target| matches!(target.state, TargetState::Completed { .. }))
        .count();
    assert_eq!(completed, 15);
    assert!(matches!(
        status
            .ticket
            .targets
            .iter()
            .find(|target| target.target_id == "r13")
            .unwrap()
            .state,
        TargetState::Completed {
            candidate_score_micros: 1_000_000,
            ..
        }
    ));
    assert!(!certificate.is_promotable());
}

async fn assert_refused_without_writes(
    fixture: &Fixture,
    phase: &str,
    watermark: &Option<(i64, String)>,
) {
    let before = catalog(&fixture.db_path);
    let reads = read_every_entry(fixture, phase).await;
    let after = catalog(&fixture.db_path);
    persist(
        fixture,
        &format!("{phase}-after-refused-reads-full-catalog.json"),
        &after,
    );
    assert_eq!(&reads.watermark, watermark);
    assert_eq!(before, after, "refused reads changed business state");
    // Collect and retain all three real outcomes before checking the semantic error.
    let refused = [
        matches!(&reads.status, Err(Error::Conflict(message)) if message == CHANGED),
        matches!(&reads.view, Err(Error::Conflict(message)) if message == CHANGED),
        matches!(&reads.same_session, Err(Error::Conflict(message)) if message == CHANGED),
    ];
    assert_eq!(
        refused, [true; 3],
        "three actual outcomes preceded this assertion"
    );
}

// Read-only catalog of this fixture's DB, including full schema/xinfo metadata,
// typed cells, virtual and shadow tables, hidden columns, and every actual row.
fn catalog(path: &Path) -> Value {
    let source = r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
schema=con.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name").fetchall()
tables=[]
xinfo=[]
for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    quoted='"'+name.replace('"','""')+'"'
    metadata=con.execute('PRAGMA table_xinfo('+quoted+')').fetchall()
    assert metadata and all(len(row)==7 for row in metadata),(name,metadata)
    xinfo.append([name,metadata])
    columns=[row[1] for row in metadata]
    selected=','.join('"'+column.replace('"','""')+'"' for column in columns)
    rows=[]
    for row in con.execute('SELECT '+selected+' FROM '+quoted):
        rows.append(json.dumps([[type(v).__name__,v.hex() if isinstance(v,bytes) else v] for v in row],ensure_ascii=False))
    tables.append([name,columns,sorted(rows)])
print(json.dumps({'schema':schema,'xinfo':xinfo,'tables':tables},ensure_ascii=False))
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

fn detach_row(catalog: &Value, id: &str) -> (Value, Vec<String>, Vec<Value>) {
    let mut remainder = catalog.clone();
    let table = remainder["tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|table| table[0] == "objects")
        .unwrap();
    let columns: Vec<_> = table[1]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column.as_str().unwrap().to_owned())
        .collect();
    let namespace = columns
        .iter()
        .position(|column| column == "namespace")
        .unwrap();
    let kind = columns.iter().position(|column| column == "kind").unwrap();
    let object_id = columns.iter().position(|column| column == "id").unwrap();
    let rows = table[2].as_array_mut().unwrap();
    let matching: Vec<_> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let row: Vec<Value> = serde_json::from_str(row.as_str().unwrap()).unwrap();
            (row[namespace][1] == NAMESPACE
                && row[kind][1] == "artifact"
                && row[object_id][1] == id)
                .then_some(i)
        })
        .collect();
    assert_eq!(matching.len(), 1);
    let removed = rows.remove(matching[0]);
    let row = serde_json::from_str(removed.as_str().unwrap()).unwrap();
    (remainder, columns, row)
}

fn assert_only_object_updates(before: &Value, after: &Value, expected: &[(String, Value)]) {
    let mut before_rest = before.clone();
    let mut after_rest = after.clone();
    for (id, expected_body) in expected {
        let (rest, columns, before_row) = detach_row(&before_rest, id);
        before_rest = rest;
        let (rest, after_columns, mut after_row) = detach_row(&after_rest, id);
        after_rest = rest;
        assert_eq!(columns, after_columns);
        let body = columns.iter().position(|column| column == "body").unwrap();
        let revision = columns
            .iter()
            .position(|column| column == "revision")
            .unwrap();
        let changed: Value = serde_json::from_str(after_row[body][1].as_str().unwrap()).unwrap();
        assert_eq!(&changed, expected_body);
        assert_eq!(before_row[body][0], after_row[body][0]);
        assert_eq!(before_row[revision][0], after_row[revision][0]);
        assert_eq!(
            after_row[revision][1].as_i64().unwrap(),
            before_row[revision][1]
                .as_i64()
                .unwrap()
                .checked_add(1)
                .unwrap(),
        );
        after_row[body] = before_row[body].clone();
        after_row[revision] = before_row[revision].clone();
        assert_eq!(after_row, before_row);
    }
    assert_eq!(before_rest, after_rest, "another row or schema changed");
}

async fn single_report_field(case: &str, schema: &str, field: &str, value: Value) {
    let fixture = stopped(case, schema, true).await;
    let positive = read_every_entry(&fixture, "positive").await;
    assert_futility_positive(&positive, true);
    let original: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let before = catalog(&fixture.db_path);
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    persist(&fixture, "original-formal-envelope.json", &original);
    let mut altered = serde_json::to_value(&original).unwrap();
    let old_value = altered["payload"]["certificate"][field].clone();
    assert_ne!(old_value, value);
    altered["payload"]["certificate"][field] = value.clone();
    let typed: Envelope<FormalEvaluationV2> = serde_json::from_value(altered.clone()).unwrap();
    let FormalEvaluationV2::EarlyStopped { certificate, .. } = &typed.payload else {
        unreachable!()
    };
    certificate.validate_shape().unwrap();
    put(&fixture, &typed).await;
    let after = catalog(&fixture.db_path);
    persist(&fixture, "after-intervention-full-catalog.json", &after);
    persist(&fixture, "altered-formal-envelope.json", &typed);
    assert_only_object_updates(&before, &after, &[(original.id.clone(), altered.clone())]);
    let (_, columns, before_row) = detach_row(&before, &original.id);
    let (_, _, after_row) = detach_row(&after, &original.id);
    let body = columns.iter().position(|column| column == "body").unwrap();
    let needle = format!("\"{field}\":{}", serde_json::to_string(&old_value).unwrap());
    let replacement = format!("\"{field}\":{}", serde_json::to_string(&value).unwrap());
    let original_raw = before_row[body][1].as_str().unwrap();
    assert_eq!(original_raw.matches(&needle).count(), 1);
    assert_eq!(
        after_row[body][1],
        original_raw.replace(&needle, &replacement)
    );
    assert_refused_without_writes(&fixture, "after-intervention", &positive.watermark).await;
}

struct Pending {
    call_id: String,
    request_digest: String,
    dispatch_id: String,
    output: String,
}

async fn start_pending(fixture: &Fixture) -> Pending {
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        fixture.ticket_id(),
        "r59",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let call_id = "pending-r59".to_owned();
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.clone(),
                dispatch_group_id: fixture.ticket_id().into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: task.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "pending-lease".into(),
                lease_until: 3_000,
                now: 170,
            },
        )
        .await
        .unwrap();
    let dispatch = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: fixture.ticket_id().into(),
            target_id: "r59".into(),
            side: ExecutionSide::Candidate,
            fence: BudgetCallFence {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.clone(),
                actual_input_digest: task.request_digest.clone(),
                lease_token: "pending-lease".into(),
                lease_epoch: 1,
                now: 170,
            },
        },
    )
    .await
    .unwrap();
    Pending {
        call_id,
        request_digest: task.request_digest,
        dispatch_id: dispatch.call.dispatch_id.unwrap(),
        output: json!({"answer":"late-audit-only"}).to_string(),
    }
}

async fn draining(case: &str) -> (Fixture, Pending) {
    let fixture = fixture(case, RegisteredEvaluationControl::SCHEMA, true).await;
    before_futility_stop(&fixture, true).await;
    let candidate = execute(&fixture, "r12", ExecutionSide::Candidate, "wrong", 30).await;
    let baseline = execute(&fixture, "r12", ExecutionSide::Baseline, "ok", 31).await;
    let pending = start_pending(&fixture).await;
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: "grader-15".into(),
            ticket_id: fixture.ticket_id().into(),
            target_id: "r12".into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 170,
        },
    )
    .await
    .unwrap();
    let status = IndependentEvaluationControl::status(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
    )
    .await
    .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Draining);
    assert!(status.formal.is_none());
    assert!(
        status
            .ticket
            .pending_certificate
            .as_ref()
            .unwrap()
            .usage_uncertain()
    );
    persist(&fixture, "actual-draining-status.json", &status);
    (fixture, pending)
}

async fn reconcile(fixture: &Fixture, pending: &Pending) {
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: "late-r59".into(),
            ticket_id: fixture.ticket_id().into(),
            target_id: "r59".into(),
            side: ExecutionSide::Candidate,
            request_digest: pending.request_digest.clone(),
            output_artifact_id: "late-output-r59".into(),
            output_utf8: pending.output.clone(),
            budget_call_id: pending.call_id.clone(),
            latency_micros: 777,
            issued_at_unix_seconds: 1_100,
        },
    )
    .await
    .unwrap();
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    fixture
        .store
        .reconcile_budget_call_cost(
            &admin,
            BILLING_SCOPE,
            &pending.call_id,
            &UsageCharge {
                amount_micros: 17,
                currency: "USD".into(),
                pricing_version: "fixture-pricing-v1".into(),
                provider_request_id: "fixture-late-request".into(),
                usage_record_id: "fixture-late-usage".into(),
                output_digest: receipt.output_digest,
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
            &pending.call_id,
            &pending.dispatch_id,
            "late_audit_closed",
            1_201,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn legal_out_of_order_first_futility_and_root_stopped_history_read_back() {
    let fixture = stopped(
        "first-futility-history",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    let positive = read_every_entry(&fixture, "positive").await;
    assert_futility_positive(&positive, true);
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, fixture.ticket_id())
        .await
        .unwrap();
    let ledger: Envelope<ExposureLedger> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id(LEDGER_KIND, "statistics-family"),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(calls.len(), 30);
    assert!(
        calls
            .iter()
            .all(|call| call.state == BudgetCallState::Finalized && call.execution_closed)
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.actual_cost_micros.unwrap())
            .sum::<i64>(),
        225,
    );
    let entry = ledger
        .payload
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id())
        .unwrap();
    assert_eq!(entry.task_cluster_ids.len(), N as usize);
    assert_eq!(entry.monetary_state, MonetaryReservationState::Finalized);
    persist(&fixture, "actual-calls.json", &calls);
    persist(&fixture, "actual-exposure.json", &ledger);
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    fixture
        .store
        .stop_root_budget(&admin, BILLING_SCOPE, "historical_read", 5_000)
        .await
        .unwrap();
    let history = read_every_entry(&fixture, "after-root-stop").await;
    assert_futility_positive(&history, true);
    assert_eq!(history.watermark, positive.watermark);
    let blocked = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: "after-stop".into(),
                dispatch_group_id: fixture.ticket_id().into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: digest("after-stop"),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "after-stop-lease".into(),
                lease_until: 7_000,
                now: 5_001,
            },
        )
        .await;
    assert!(matches!(blocked, Err(Error::Cancelled)));
}

#[tokio::test]
async fn report_only_ucb_change_refuses_all_three_readers() {
    single_report_field(
        "single-ucb",
        RegisteredEvaluationControl::SCHEMA,
        "ucb",
        json!("0"),
    )
    .await;
}

#[tokio::test]
async fn report_only_reason_cannot_turn_futility_into_confirmed_regression() {
    single_report_field(
        "single-reason-v1",
        RegisteredEvaluationControl::SCHEMA_V1,
        "stop_reason",
        json!("regressed_sequential"),
    )
    .await;
}

#[tokio::test]
async fn equal_corrupted_certificate_copies_still_need_actual_prefix_replay() {
    let fixture = stopped("coordinated-k", RegisteredEvaluationControl::SCHEMA, true).await;
    let positive = read_every_entry(&fixture, "positive").await;
    assert_futility_positive(&positive, true);
    let original_ticket: Envelope<EvaluationTicketV2> =
        stored(&fixture, TICKET_KIND, fixture.ticket_id()).await;
    let original_formal: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let before = catalog(&fixture.db_path);
    let mut ticket = original_ticket.clone();
    let mut altered =
        serde_json::to_value(ticket.payload.pending_certificate.as_ref().unwrap()).unwrap();
    altered["k"] = json!(12);
    let certificate: EarlyStopCertificate = serde_json::from_value(altered).unwrap();
    certificate.validate_shape().unwrap();
    ticket.payload.pending_certificate = Some(certificate.clone());
    let mut formal = original_formal.clone();
    let FormalEvaluationV2::EarlyStopped {
        ticket_digest,
        certificate: actual,
        ..
    } = &mut formal.payload
    else {
        unreachable!()
    };
    *ticket_digest = fingerprint(&ticket.payload).unwrap();
    *actual = certificate;
    assert_eq!(
        fingerprint(ticket.payload.pending_certificate.as_ref().unwrap()).unwrap(),
        fingerprint(actual).unwrap(),
    );
    put(&fixture, &ticket).await;
    put(&fixture, &formal).await;
    let after = catalog(&fixture.db_path);
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    persist(&fixture, "after-intervention-full-catalog.json", &after);
    persist(&fixture, "altered-ticket.json", &ticket);
    persist(&fixture, "altered-formal.json", &formal);
    assert_only_object_updates(
        &before,
        &after,
        &[
            (ticket.id.clone(), serde_json::to_value(&ticket).unwrap()),
            (formal.id.clone(), serde_json::to_value(&formal).unwrap()),
        ],
    );
    assert_refused_without_writes(&fixture, "after-coordinated-change", &positive.watermark).await;
}

#[tokio::test]
async fn contradictory_completion_sequences_cannot_describe_a_valid_first_stop() {
    let fixture = stopped(
        "duplicate-sequence",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    let positive = read_every_entry(&fixture, "positive").await;
    assert_futility_positive(&positive, true);
    let mut ticket: Envelope<EvaluationTicketV2> =
        stored(&fixture, TICKET_KIND, fixture.ticket_id()).await;
    let mut formal: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let before = catalog(&fixture.db_path);
    let target = ticket
        .payload
        .targets
        .iter_mut()
        .find(|target| target.target_id == "r0")
        .unwrap();
    let TargetState::Completed {
        completion_sequence,
        ..
    } = &mut target.state
    else {
        unreachable!()
    };
    *completion_sequence = 2; // r13 already completed at this actual sequence.
    let FormalEvaluationV2::EarlyStopped { ticket_digest, .. } = &mut formal.payload else {
        unreachable!()
    };
    *ticket_digest = fingerprint(&ticket.payload).unwrap();
    put(&fixture, &ticket).await;
    put(&fixture, &formal).await;
    let after = catalog(&fixture.db_path);
    assert_only_object_updates(
        &before,
        &after,
        &[
            (ticket.id.clone(), serde_json::to_value(&ticket).unwrap()),
            (formal.id.clone(), serde_json::to_value(&formal).unwrap()),
        ],
    );
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    persist(&fixture, "after-intervention-full-catalog.json", &after);
    assert_refused_without_writes(&fixture, "after-duplicate-sequence", &positive.watermark).await;
}

#[tokio::test]
async fn critical_anchor_k0_keeps_empty_prefix_with_enabled_or_disabled_statistics() {
    for sequential in [false, true] {
        let name = if sequential {
            "critical-enabled"
        } else {
            "critical-disabled"
        };
        let fixture = fixture(name, RegisteredEvaluationControl::SCHEMA, sequential).await;
        issue(&fixture, true).await;
        grade(&fixture, "a0", "wrong", "ok", 1).await;
        let reads = read_every_entry(&fixture, "positive").await;
        assert_all_early_ok(&reads, true);
        let status = reads.status.unwrap();
        let FormalEvaluationV2::EarlyStopped { certificate, .. } = status.formal.unwrap() else {
            unreachable!()
        };
        assert_eq!(certificate.k(), 0);
        assert_eq!(
            certificate.stop_reason(),
            EarlyStopDecision::CriticalRegression
        );
        let wire = serde_json::to_value(&certificate).unwrap();
        assert_eq!(wire["lcb"], Value::Null);
        assert_eq!(wire["ucb"], Value::Null);
        assert_eq!(
            wire["completed_prefix_digest"],
            fingerprint(&Vec::<String>::new()).unwrap(),
        );
        assert_eq!(wire["stop_sequence"], 2);
        assert_eq!(wire["stopped_at_unix_ms"], 30_000);
        assert_eq!(
            wire["early_stop_plan_digest"],
            fixture
                .control
                .early_stop_plan
                .as_ref()
                .map(|plan| plan.digest().unwrap())
                .unwrap_or_else(|| fixture.control.formal_plan.digest().unwrap()),
        );
    }
}

#[tokio::test]
async fn absent_candidate_binding_preserves_legacy_early_report_interpretation() {
    trusted_compiler_inputs::assert_frozen_issuer_static_read("early-legacy")
        .await
        .unwrap();
    single_report_field(
        "legacy-matrix-migrated-current",
        RegisteredEvaluationControl::SCHEMA_V1,
        "ucb",
        json!("0"),
    )
    .await;
}

#[tokio::test]
async fn late_reconciliation_keeps_the_original_stop_snapshot_and_restart_endpoint() {
    let (mut fixture, pending) = draining("late-reconciliation").await;
    let before: Envelope<EvaluationTicketV2> =
        stored(&fixture, TICKET_KIND, fixture.ticket_id()).await;
    let original_member = before.payload.stop_member_terminal_digest.clone();
    let mut original_certificate =
        serde_json::to_value(before.payload.pending_certificate.as_ref().unwrap()).unwrap();
    reconcile(&fixture, &pending).await;
    let settled = IndependentEvaluationControl::settle_after_stop(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        1_300,
    )
    .await
    .unwrap();
    assert_eq!(settled.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    assert_eq!(settled.ticket.stop_member_terminal_digest, original_member);
    assert_ne!(
        Some(fingerprint(&settled.ticket.targets).unwrap()),
        settled.ticket.stop_member_terminal_digest,
        "reconciliation really changed targets after the original stop snapshot",
    );
    original_certificate["usage_uncertain"] = json!(false);
    assert_eq!(
        original_certificate,
        serde_json::to_value(settled.ticket.pending_certificate.as_ref().unwrap()).unwrap(),
    );
    assert_eq!(
        settled.ticket.late_execution_receipt_ids,
        vec!["late-r59".to_owned()],
    );
    assert!(matches!(
        settled
            .ticket
            .targets
            .iter()
            .find(|target| target.target_id == "r59")
            .unwrap()
            .state,
        TargetState::CancelledAfterDispatch { .. }
    ));
    let positive = read_every_entry(&fixture, "after-reconciliation").await;
    assert_futility_positive(&positive, true);
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, fixture.ticket_id())
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(calls.len(), 31);
    assert_eq!(
        calls
            .iter()
            .map(|call| call.actual_cost_micros.unwrap())
            .sum::<i64>(),
        242,
    );
    assert!(
        calls
            .iter()
            .all(|call| call.state == BudgetCallState::Finalized && call.execution_closed)
    );
    persist(&fixture, "actual-settled-calls.json", &calls);
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    fixture
        .store
        .stop_root_budget(
            &admin,
            BILLING_SCOPE,
            "history_after_late_settlement",
            5_000,
        )
        .await
        .unwrap();
    fixture.store.close().await;
    fixture.store = Store::open(&fixture.db_path).await.unwrap();
    let restarted = read_every_entry(&fixture, "restarted-after-root-stop").await;
    assert_futility_positive(&restarted, true);
    assert_eq!(restarted.watermark, positive.watermark);
}

#[tokio::test]
async fn first_formal_write_refuses_corrupted_pending_statistics_atomically() {
    let (fixture, pending) = draining("writer-pending-k").await;
    reconcile(&fixture, &pending).await;
    let original: Envelope<EvaluationTicketV2> =
        stored(&fixture, TICKET_KIND, fixture.ticket_id()).await;
    let mut altered = original.clone();
    let mut wire =
        serde_json::to_value(altered.payload.pending_certificate.as_ref().unwrap()).unwrap();
    wire["k"] = json!(12);
    altered.payload.pending_certificate = Some(serde_json::from_value(wire).unwrap());
    put(&fixture, &altered).await;
    let before = catalog(&fixture.db_path);
    let result = IndependentEvaluationControl::settle_after_stop(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        1_300,
    )
    .await;
    let actual = json!({"ok": result.is_ok(), "actual_debug": format!("{result:?}")});
    println!("actual-first-formal-writer-result {actual}");
    persist(&fixture, "actual-first-formal-writer-result.json", &actual);
    let after = catalog(&fixture.db_path);
    persist(&fixture, "before-writer-full-catalog.json", &before);
    persist(&fixture, "after-writer-full-catalog.json", &after);
    assert_eq!(
        before, after,
        "refused writer committed settlement/ticket/formal/audit changes",
    );
    assert!(matches!(result, Err(Error::Conflict(message)) if message == CHANGED));
    // A correct pending certificate still closes normally after the failed attempt.
    put(&fixture, &original).await;
    IndependentEvaluationControl::settle_after_stop(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        1_301,
    )
    .await
    .unwrap();
    let positive = read_every_entry(&fixture, "after-original-restored").await;
    assert_futility_positive(&positive, true);
}

#[tokio::test]
async fn bound_complete_batch_keeps_the_existing_full_statistics_consumer() {
    let fixture = fixture(
        "complete-batch-regression",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    issue(&fixture, true).await;
    grade(&fixture, "a0", "ok", "ok", 1).await;
    for (position, index) in (0..N).rev().enumerate() {
        grade(
            &fixture,
            &format!("r{index}"),
            "ok",
            "wrong",
            position as u32 + 2,
        )
        .await;
    }
    let formal = IndependentEvaluationControl::finalize_complete(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        5_000,
    )
    .await
    .unwrap();
    let FormalEvaluationV2::CompleteBatch {
        report,
        verdict,
        stored_dependency_binding,
        ..
    } = &formal
    else {
        unreachable!()
    };
    assert_eq!(*verdict, Verdict::Improved);
    assert_eq!(report.n, N as usize);
    assert_eq!(report.mean, "1");
    assert!(stored_dependency_binding.is_some());
    let reads = read_every_entry(&fixture, "positive").await;
    assert_eq!(
        reads.status.unwrap().ticket.lifecycle,
        TicketLifecycle::CompleteBatch
    );
    for view in [reads.view.unwrap(), reads.same_session.unwrap()] {
        assert_eq!(view.variant, VerifiedReportVariant::CompleteBatch);
        assert_eq!(view.verdict, Some(Verdict::Improved));
        assert_eq!(
            view.cost_evidence_scope,
            Some(CostEvidenceScope::TicketExecutionOnly),
        );
        assert!(!view.promotion_eligible);
    }
}
