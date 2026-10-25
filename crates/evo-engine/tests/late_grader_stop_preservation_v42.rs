//! Delayed grading errors retain the confirmed first-stop endpoint and real audit.
//! These fixtures execute local software APIs; they do not prove provider authority.
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
    HoldoutManifest, SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopDecision, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, load_stored_source, store_trace_authority,
};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    AnchorEvidenceStatus, DependencyEvidenceStatus, EvaluationEvidenceScope,
    ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod, FixedGraderSpec, FormalEvaluationV2,
    FrozenOracleEntry, GraderReceiptRequest, IndependentEvaluationControl,
    IndependentGraderReceiptV2, IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef,
    ReadEvaluationTicket as EvaluationTicketV2, RegisteredEvaluationControl, StartExecutionRequest,
    StreamingEvaluationStatus, TicketLifecycle, VerifiedFormalReportView, VerifiedReportVariant,
    verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const N: u32 = 60;
const NAMESPACE: &str = "late-grader-stop-fixture";
const BILLING_SCOPE: &str = "late-grader-stop-billing";
const TICKET_ID: &str = "late-grader-stop-ticket";
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
    match std::env::var_os("RSIA_AG091_CASE_OUTPUT_DIR") {
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
    let finalized = fixture
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
    let closed = fixture
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
    persist(
        fixture,
        &format!("actual-execution-{ordinal}.json"),
        &json!({
            "actual_task_debug": format!("{task:?}"),
            "dispatch": {"call": dispatch.call, "new_dispatch": dispatch.new_dispatch},
            "receipt": receipt, "finalized": finalized, "closed": closed,
            "ordinal": ordinal, "raw_output_utf8": output_utf8,
        }),
    );
    receipt_id
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
        trusted_compiler_inputs::assert_frozen_issuer_static_read("late-stop-legacy")
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

#[derive(Debug, Clone, Serialize)]
struct DeferredPair {
    target: String,
    candidate: String,
    baseline: String,
}

async fn execute(
    fixture: &Fixture,
    target: &str,
    side: ExecutionSide,
    answer: &str,
    ordinal: u32,
) -> String {
    execute_raw(
        fixture,
        target,
        side,
        &json!({"answer": answer}).to_string(),
        ordinal,
    )
    .await
}

async fn defer_pair(
    fixture: &Fixture,
    target: &str,
    candidate_output: &str,
    baseline_output: &str,
) -> DeferredPair {
    let pair = DeferredPair {
        target: target.into(),
        candidate: execute_raw(
            fixture,
            target,
            ExecutionSide::Candidate,
            candidate_output,
            0,
        )
        .await,
        baseline: execute_raw(fixture, target, ExecutionSide::Baseline, baseline_output, 1).await,
    };
    persist(fixture, "actual-deferred-pair.json", &pair);
    pair
}

async fn grader_call(
    fixture: &Fixture,
    pair: &DeferredPair,
    receipt_id: &str,
    now: i64,
    phase: &str,
) -> evo_core::Result<IndependentGraderReceiptV2> {
    let result = IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: receipt_id.into(),
            ticket_id: fixture.ticket_id().into(),
            target_id: pair.target.clone(),
            candidate_execution_receipt_id: pair.candidate.clone(),
            baseline_execution_receipt_id: pair.baseline.clone(),
            issued_at_unix_seconds: now,
        },
    )
    .await;
    let actual = json!({
        "phase": phase,
        "actual_request": {
            "receipt_id": receipt_id, "ticket_id": fixture.ticket_id(),
            "target_id": pair.target,
            "candidate_execution_receipt_id": pair.candidate,
            "baseline_execution_receipt_id": pair.baseline,
            "issued_at_unix_seconds": now,
        },
        "ok": result.is_ok(),
        "actual_debug": format!("{result:?}"),
        "actual_success_receipt": result.as_ref().ok(),
    });
    println!("actual-late-grader-call {actual}");
    persist(fixture, &format!("{phase}-grader-call.json"), &actual);
    result
}

async fn grade(
    fixture: &Fixture,
    target: &str,
    candidate_answer: &str,
    baseline_answer: &str,
    ordinal: u32,
) {
    let pair = DeferredPair {
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
    grader_call(
        fixture,
        &pair,
        &format!("grader-{ordinal}"),
        20 + i64::from(ordinal) * 10,
        &format!("setup-{ordinal}"),
    )
    .await
    .unwrap();
}

async fn futility(fixture: &Fixture, anchor_first: bool) {
    let first = if anchor_first {
        grade(fixture, "a0", "ok", "ok", 1).await;
        2
    } else {
        1
    };
    for i in 0..13 {
        grade(fixture, &format!("r{i}"), "wrong", "ok", first + i).await;
    }
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

struct ReadResults {
    status: evo_core::Result<StreamingEvaluationStatus>,
    view: evo_core::Result<VerifiedFormalReportView>,
    same_session: evo_core::Result<VerifiedFormalReportView>,
    session_commit: evo_core::Result<()>,
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
    let session_commit = session.commit().await;
    let actual = json!({
        "phase": phase,
        "status": {"ok": status.is_ok(), "actual_debug": format!("{status:?}")},
        "view": {"ok": view.is_ok(), "actual_debug": format!("{view:?}")},
        "same_session": {"ok": same_session.is_ok(), "actual_debug": format!("{same_session:?}")},
        "session_commit": {"ok": session_commit.is_ok(), "actual_debug": format!("{session_commit:?}")},
        "watermark": watermark,
        "actual_ticket": status.as_ref().ok().map(|value| &value.ticket),
        "actual_formal": status.as_ref().ok().and_then(|value| value.formal.as_ref()),
    });
    println!("actual-late-grader-three-read-results {actual}");
    persist(
        fixture,
        &format!("{phase}-three-read-results.json"),
        &actual,
    );
    ReadResults {
        status,
        view,
        same_session,
        session_commit,
        watermark,
    }
}

fn assert_early(reads: &ReadResults, bound: bool, reason: EarlyStopDecision, k: u32) {
    assert!(reads.session_commit.is_ok(), "{:?}", reads.session_commit);
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    assert!(status.ticket.candidate_dependency_binding.is_some());
    let _ = bound;
    let Some(FormalEvaluationV2::EarlyStopped { certificate, .. }) = &status.formal else {
        panic!("expected actual early-stopped report");
    };
    assert_eq!(certificate.stop_reason(), reason);
    assert_eq!(certificate.k(), k);
    assert!(!certificate.usage_uncertain());
    assert_eq!(
        fingerprint(certificate).unwrap(),
        fingerprint(status.ticket.pending_certificate.as_ref().unwrap()).unwrap(),
    );
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

fn assert_invalid(reads: &ReadResults, reason: &str) {
    assert!(reads.session_commit.is_ok(), "{:?}", reads.session_commit);
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Invalid);
    let Some(FormalEvaluationV2::Invalid { reasons, .. }) = &status.formal else {
        panic!("expected actual invalid report");
    };
    assert_eq!(reasons, &status.ticket.invalid_reasons);
    assert!(reasons.iter().any(|value| value == reason));
    for view in [
        reads.view.as_ref().unwrap(),
        reads.same_session.as_ref().unwrap(),
    ] {
        assert_eq!(view.variant, VerifiedReportVariant::Invalid);
        assert_eq!(view.verdict, Some(Verdict::Invalid));
        assert!(!view.promotion_eligible);
    }
}

fn assert_draining(reads: &ReadResults) {
    assert!(reads.session_commit.is_ok(), "{:?}", reads.session_commit);
    let status = reads.status.as_ref().unwrap();
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
    // No formal report is expected while accounting is unresolved.
    assert!(matches!(&reads.view, Err(Error::NotFound)));
    assert!(matches!(&reads.same_session, Err(Error::NotFound)));
}

fn assert_exact_endpoint(before: &ReadResults, after: &ReadResults) {
    assert!(before.session_commit.is_ok());
    assert!(after.session_commit.is_ok(), "{:?}", after.session_commit);
    let before = before.status.as_ref().unwrap();
    let after = after.status.as_ref().unwrap();
    assert_eq!(
        fingerprint(&before.ticket).unwrap(),
        fingerprint(&after.ticket).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.formal).unwrap(),
        serde_json::to_value(&after.formal).unwrap()
    );
}

#[derive(Debug, Clone, PartialEq)]
struct World {
    catalog: Value,
    api: Value,
}

async fn world(fixture: &Fixture, _bound: bool, phase: &str) -> World {
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, fixture.ticket_id())
        .await
        .unwrap();
    let ledger: Envelope<ExposureLedger> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id(LEDGER_KIND, &fixture.control.alpha_plan.research_family_id),
        )
        .await
        .unwrap();
    let control: Envelope<RegisteredEvaluationControl> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id("registered_evaluation_control_v41", &fixture.control.id),
        )
        .await
        .unwrap();
    let holdout: Envelope<ProtectedHoldoutRecord> = session
        .need(
            &fixture.evaluator,
            "artifact",
            &physical_id("protected_holdout_v41", &fixture.holdout.id),
        )
        .await
        .unwrap();
    let sources = vec![
        load_stored_source(&mut session, &fixture.evaluator, "S1")
            .await
            .unwrap(),
        load_stored_source(&mut session, &fixture.evaluator, "S2")
            .await
            .unwrap(),
    ];
    let watermark = session.watermark(&fixture.evaluator).await.unwrap();
    let commit = session.commit().await;
    let group = fixture
        .store
        .dispatch_group(&fixture.evaluator, BILLING_SCOPE, fixture.ticket_id())
        .await
        .unwrap();
    let api = json!({
        "calls": calls, "ledger": ledger, "control": control, "holdout": holdout,
        "sources": sources, "watermark": watermark, "dispatch_group": group,
        "session_commit": {"ok": commit.is_ok(), "actual_debug": format!("{commit:?}")},
    });
    let catalog = catalog(&fixture.db_path);
    persist(fixture, &format!("{phase}-api-evidence.json"), &api);
    persist(fixture, &format!("{phase}-full-catalog.json"), &catalog);
    assert!(commit.is_ok(), "{commit:?}");
    World { catalog, api }
}

fn assert_all_paid_closed(world: &World) {
    assert!(world.api["calls"].as_array().unwrap().iter().all(|call| {
        call["state"] == "finalized"
            && call["execution_closed"] == true
            && call["actual_cost_micros"]
                .as_i64()
                .is_some_and(|amount| amount >= 0)
    }));
    assert_eq!(
        world.api["ledger"]["payload"]["entries"][0]["monetary_state"],
        "finalized"
    );
}

const BAD_JSON: &str = r#"{"answer":{"nested":1,"nested":2}}"#;
const GOOD_JSON: &str = r#"{"answer":"ok"}"#;

#[allow(clippy::too_many_arguments)]
async fn stopped_error_case(
    case: &str,
    schema: &str,
    sequential: bool,
    critical: bool,
    target: &str,
    candidate: &str,
    baseline: &str,
    expected_error: &str,
) {
    let fixture = fixture(case, schema, sequential).await;
    issue(&fixture, true).await;
    let deferred = defer_pair(&fixture, target, candidate, baseline).await;
    if critical {
        grade(&fixture, "a0", "wrong", "ok", 1).await;
    } else {
        futility(&fixture, target != "a0").await;
    }
    let reason = if critical {
        EarlyStopDecision::CriticalRegression
    } else {
        EarlyStopDecision::FutilityQualityGain
    };
    let k = if critical { 0 } else { 13 };
    let positive = read_every_entry(&fixture, "positive").await;
    let before = world(&fixture, true, "before-late").await;
    assert_early(&positive, true, reason, k);
    assert_all_paid_closed(&before);
    let result = grader_call(&fixture, &deferred, "delayed-grader", 1_000, "late").await;
    // Collect every post observation before a semantic preservation assertion.
    let post = read_every_entry(&fixture, "post").await;
    let after = world(&fixture, true, "after-late").await;
    fixture.store.close().await;
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == expected_error),
        "{result:?}"
    );
    assert_early(&post, true, reason, k);
    assert_exact_endpoint(&positive, &post);
    assert_eq!(positive.watermark, post.watermark);
    assert_all_paid_closed(&after);
    assert_eq!(
        before, after,
        "a rejected late grading must not rewrite any logical fact"
    );
}

#[tokio::test]
async fn candidate_schema_error_keeps_enabled_futility_endpoint() {
    stopped_error_case(
        "candidate-schema-futility",
        RegisteredEvaluationControl::SCHEMA,
        true,
        false,
        "r59",
        BAD_JSON,
        GOOD_JSON,
        "JSON contains invalid or duplicate fields",
    )
    .await;
}

#[tokio::test]
async fn baseline_schema_error_keeps_v1_disabled_critical_k0_endpoint() {
    stopped_error_case(
        "baseline-schema-disabled-critical",
        RegisteredEvaluationControl::SCHEMA_V1,
        false,
        true,
        "r59",
        GOOD_JSON,
        BAD_JSON,
        "JSON contains invalid or duplicate fields",
    )
    .await;
}

#[tokio::test]
async fn baseline_anchor_error_keeps_the_original_statistical_stop() {
    stopped_error_case(
        "baseline-anchor-futility",
        RegisteredEvaluationControl::SCHEMA,
        true,
        false,
        "a0",
        GOOD_JSON,
        r#"{"answer":"wrong"}"#,
        "baseline failed a frozen critical anchor; comparison is invalid",
    )
    .await;
}

#[tokio::test]
async fn valid_late_grading_records_audit_without_changing_first_stop() {
    let fixture = fixture("valid-late", RegisteredEvaluationControl::SCHEMA, true).await;
    issue(&fixture, true).await;
    let deferred = defer_pair(&fixture, "r59", GOOD_JSON, GOOD_JSON).await;
    futility(&fixture, true).await;
    let positive = read_every_entry(&fixture, "positive").await;
    let before = world(&fixture, true, "before-late").await;
    assert_early(&positive, true, EarlyStopDecision::FutilityQualityGain, 13);
    assert_all_paid_closed(&before);
    let result = grader_call(&fixture, &deferred, "valid-delayed-grader", 1_000, "late").await;
    let post = read_every_entry(&fixture, "post").await;
    let after = world(&fixture, true, "after-late").await;
    fixture.store.close().await;
    assert!(result.is_ok(), "{result:?}");
    assert_early(&post, true, EarlyStopDecision::FutilityQualityGain, 13);
    let original = &positive.status.as_ref().unwrap().ticket;
    let latest = &post.status.as_ref().unwrap().ticket;
    assert_eq!(
        fingerprint(&original.pending_certificate).unwrap(),
        fingerprint(&latest.pending_certificate).unwrap()
    );
    assert_eq!(
        fingerprint(&original.targets).unwrap(),
        fingerprint(&latest.targets).unwrap()
    );
    assert_eq!(
        original.stop_member_terminal_digest,
        latest.stop_member_terminal_digest
    );
    assert_eq!(
        latest.late_grader_receipt_ids,
        vec!["valid-delayed-grader".to_string()]
    );
    assert_eq!(before.api, after.api);
    assert_ne!(before.catalog, after.catalog);
    assert_all_paid_closed(&after);
}

#[tokio::test]
async fn active_schema_errors_still_create_real_invalid_outcomes() {
    for (case, candidate, baseline, reason) in [
        (
            "active-candidate",
            BAD_JSON,
            GOOD_JSON,
            "candidate_output_schema_invalid",
        ),
        (
            "active-baseline",
            GOOD_JSON,
            BAD_JSON,
            "baseline_output_schema_invalid",
        ),
        (
            "active-anchor",
            GOOD_JSON,
            r#"{"answer":"wrong"}"#,
            "baseline_anchor_invalid",
        ),
    ] {
        let fixture = fixture(case, RegisteredEvaluationControl::SCHEMA, true).await;
        issue(&fixture, true).await;
        let target = if case == "active-anchor" { "a0" } else { "r59" };
        let pair = defer_pair(&fixture, target, candidate, baseline).await;
        let before_reads = read_every_entry(&fixture, "before-active").await;
        let before = world(&fixture, true, "before-active").await;
        assert!(before_reads.session_commit.is_ok());
        assert_eq!(
            before_reads.status.as_ref().unwrap().ticket.lifecycle,
            TicketLifecycle::Running
        );
        assert!(matches!(&before_reads.view, Err(Error::NotFound)));
        assert!(matches!(&before_reads.same_session, Err(Error::NotFound)));
        let result = grader_call(&fixture, &pair, "active-grader", 30, "active").await;
        let post = read_every_entry(&fixture, "post").await;
        let after = world(&fixture, true, "after-active").await;
        fixture.store.close().await;
        assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
        assert_invalid(&post, reason);
        assert_eq!(before.api["calls"], after.api["calls"]);
        assert_ne!(before.catalog, after.catalog);
    }
}

#[tokio::test]
async fn absent_binding_keeps_legacy_late_invalidation_and_wire_absence() {
    let fixture = fixture("legacy-none", RegisteredEvaluationControl::SCHEMA_V1, true).await;
    issue(&fixture, false).await;
    let pair = defer_pair(&fixture, "r59", BAD_JSON, GOOD_JSON).await;
    futility(&fixture, true).await;
    let positive = read_every_entry(&fixture, "positive").await;
    let before = world(&fixture, false, "before-late").await;
    assert_early(&positive, false, EarlyStopDecision::FutilityQualityGain, 13);
    let ticket = &positive.status.as_ref().unwrap().ticket;
    let bytes = serde_json::to_vec(ticket).unwrap();
    let roundtrip: EvaluationTicketV2 = serde_json::from_slice(&bytes).unwrap();
    let wire = serde_json::to_value(ticket).unwrap();
    persist(
        &fixture,
        "actual-current-wire.json",
        &json!({"ticket_utf8": String::from_utf8(bytes.clone()).unwrap(), "fingerprint": fingerprint(ticket).unwrap()}),
    );
    assert!(wire.get("candidate_dependency_binding").is_some());
    assert_eq!(serde_json::to_vec(&roundtrip).unwrap(), bytes);
    assert_eq!(
        fingerprint(&roundtrip).unwrap(),
        fingerprint(ticket).unwrap()
    );
    let result = grader_call(&fixture, &pair, "legacy-late-grader", 1_000, "late").await;
    let post = read_every_entry(&fixture, "post").await;
    let after = world(&fixture, false, "after-late").await;
    fixture.store.close().await;
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == "JSON contains invalid or duplicate fields"),
        "{result:?}"
    );
    assert_invalid(&post, "candidate_output_schema_invalid");
    assert_eq!(before.api, after.api);
    assert_ne!(before.catalog, after.catalog);
    assert!(
        serde_json::to_value(&post.status.as_ref().unwrap().ticket)
            .unwrap()
            .get("candidate_dependency_binding")
            .is_some()
    );
}

struct Pending {
    call_id: String,
    dispatch_id: String,
    request_digest: String,
}

async fn start_pending(fixture: &Fixture) -> Pending {
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        fixture.ticket_id(),
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
                call_id: "pending-r58".into(),
                dispatch_group_id: fixture.ticket_id().into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: task.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "pending-lease".into(),
                lease_until: 200,
                now: 160,
            },
        )
        .await
        .unwrap();
    let decision = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: fixture.ticket_id().into(),
            target_id: "r58".into(),
            side: ExecutionSide::Candidate,
            fence: BudgetCallFence {
                billing_scope: BILLING_SCOPE.into(),
                call_id: "pending-r58".into(),
                actual_input_digest: task.request_digest.clone(),
                lease_token: "pending-lease".into(),
                lease_epoch: 1,
                now: 160,
            },
        },
    )
    .await
    .unwrap();
    persist(
        fixture,
        "actual-pending-dispatch.json",
        &json!({"call": decision.call, "new_dispatch": decision.new_dispatch}),
    );
    Pending {
        call_id: "pending-r58".into(),
        dispatch_id: decision.call.dispatch_id.unwrap(),
        request_digest: task.request_digest,
    }
}

async fn reconcile_pending(fixture: &Fixture, pending: &Pending) {
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: "late-r58".into(),
            ticket_id: fixture.ticket_id().into(),
            target_id: "r58".into(),
            side: ExecutionSide::Candidate,
            request_digest: pending.request_digest.clone(),
            output_artifact_id: "late-output-r58".into(),
            output_utf8: GOOD_JSON.into(),
            budget_call_id: pending.call_id.clone(),
            latency_micros: 777,
            issued_at_unix_seconds: 1_100,
        },
    )
    .await
    .unwrap();
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let reconciled = fixture
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
                output_digest: receipt.output_digest.clone(),
            },
            1_200,
        )
        .await
        .unwrap();
    let closed = fixture
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
    persist(
        fixture,
        "actual-authorized-reconciliation.json",
        &json!({"receipt": receipt, "reconciled": reconciled, "closed": closed}),
    );
}

#[tokio::test]
async fn draining_error_preserves_uncertain_money_then_real_reconciliation() {
    let mut fixture = fixture(
        "draining-error-reconcile",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    issue(&fixture, true).await;
    let deferred = defer_pair(&fixture, "r59", BAD_JSON, GOOD_JSON).await;
    grade(&fixture, "a0", "ok", "ok", 1).await;
    for i in 0..12 {
        grade(&fixture, &format!("r{i}"), "wrong", "ok", i + 2).await;
    }
    // Pay and close the final pair before taking the single concurrency slot.
    let stopping_pair = DeferredPair {
        target: "r12".into(),
        candidate: execute(&fixture, "r12", ExecutionSide::Candidate, "wrong", 28).await,
        baseline: execute(&fixture, "r12", ExecutionSide::Baseline, "ok", 29).await,
    };
    let pending = start_pending(&fixture).await;
    grader_call(&fixture, &stopping_pair, "grader-14", 161, "actual-stop")
        .await
        .unwrap();
    let positive = read_every_entry(&fixture, "draining-positive").await;
    let before = world(&fixture, true, "before-late").await;
    assert_draining(&positive);
    let original = positive
        .status
        .as_ref()
        .unwrap()
        .ticket
        .pending_certificate
        .as_ref()
        .unwrap();
    assert_eq!(
        original.stop_reason(),
        EarlyStopDecision::FutilityQualityGain
    );
    assert_eq!(original.k(), 13);
    let result = grader_call(&fixture, &deferred, "delayed-invalid-grader", 170, "late").await;
    let post = read_every_entry(&fixture, "draining-post").await;
    let after = world(&fixture, true, "after-late").await;
    // Execute the authorized accounting path even before the final preservation assertions.
    reconcile_pending(&fixture, &pending).await;
    let settled = IndependentEvaluationControl::settle_after_stop(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        1_300,
    )
    .await;
    persist(
        &fixture,
        "actual-settle-result.json",
        &json!({"ok": settled.is_ok(), "actual_debug": format!("{settled:?}")}),
    );
    let final_reads = read_every_entry(&fixture, "reconciled").await;
    let final_world = world(&fixture, true, "reconciled").await;
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let root_stop = fixture
        .store
        .stop_root_budget(
            &admin,
            BILLING_SCOPE,
            "historical_stop_after_reconciliation",
            5_000,
        )
        .await;
    persist(
        &fixture,
        "actual-root-stop.json",
        &json!({"ok": root_stop.is_ok(), "actual_debug": format!("{root_stop:?}")}),
    );
    fixture.store.close().await;
    fixture.store = Store::open(&fixture.db_path).await.unwrap();
    let restarted = read_every_entry(&fixture, "restarted").await;
    let restarted_world = world(&fixture, true, "restarted").await;
    fixture.store.close().await;
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == "JSON contains invalid or duplicate fields"),
        "{result:?}"
    );
    assert_draining(&post);
    assert_exact_endpoint(&positive, &post);
    assert_eq!(positive.watermark, post.watermark);
    assert_eq!(
        before, after,
        "late error must not settle, release or invalidate uncertain accounting"
    );
    assert_eq!(
        after.api["ledger"]["payload"]["entries"][0]["monetary_state"],
        "dispatched_cost_pending"
    );
    assert!(
        after.api["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|call| call["call_id"] == "pending-r58"
                && call["actual_cost_micros"].is_null()
                && call["execution_closed"] == false)
    );
    assert!(settled.is_ok(), "{settled:?}");
    assert!(root_stop.is_ok(), "{root_stop:?}");
    assert_early(
        &final_reads,
        true,
        EarlyStopDecision::FutilityQualityGain,
        13,
    );
    assert_early(&restarted, true, EarlyStopDecision::FutilityQualityGain, 13);
    let settled_certificate = final_reads
        .status
        .as_ref()
        .unwrap()
        .ticket
        .pending_certificate
        .as_ref()
        .unwrap();
    assert_eq!(
        fingerprint(&original.clone().with_usage_reconciled().unwrap()).unwrap(),
        fingerprint(settled_certificate).unwrap(),
    );
    assert_eq!(
        positive
            .status
            .as_ref()
            .unwrap()
            .ticket
            .stop_member_terminal_digest,
        final_reads
            .status
            .as_ref()
            .unwrap()
            .ticket
            .stop_member_terminal_digest,
    );
    assert_all_paid_closed(&final_world);
    assert_all_paid_closed(&restarted_world);
    assert_eq!(final_world.api["calls"].as_array().unwrap().len(), 31);
    assert_eq!(
        final_world.api["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| call["actual_cost_micros"].as_i64().unwrap())
            .sum::<i64>(),
        242
    );
}

#[tokio::test]
async fn explicit_invalidation_keeps_its_existing_separate_authority_path() {
    let fixture = fixture(
        "explicit-invalidation",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    issue(&fixture, true).await;
    let _deferred = defer_pair(&fixture, "r59", GOOD_JSON, GOOD_JSON).await;
    futility(&fixture, true).await;
    let positive = read_every_entry(&fixture, "positive").await;
    let before = world(&fixture, true, "before-explicit").await;
    assert_early(&positive, true, EarlyStopDecision::FutilityQualityGain, 13);
    let result = IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        "explicit_environment_invalid",
        1_000,
    )
    .await;
    persist(
        &fixture,
        "actual-explicit-invalidation.json",
        &json!({"ok": result.is_ok(), "actual_debug": format!("{result:?}")}),
    );
    let post = read_every_entry(&fixture, "post").await;
    let after = world(&fixture, true, "after-explicit").await;
    fixture.store.close().await;
    assert!(result.is_ok(), "{result:?}");
    assert_invalid(&post, "explicit_environment_invalid");
    assert_eq!(before.api, after.api);
    assert_ne!(before.catalog, after.catalog);
}

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
