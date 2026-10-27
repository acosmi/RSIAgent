//! Bound Invalid reports retain the actual ticket's complete ordered failures.
//! ProgramFixture receipts and stored equality are local software evidence only.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evaluation::{
    ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41,
    FormalStatisticalUnit, Money, OptimizationBudgetPlan, OptimizerComparisonContract, Verdict,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, ExposureLedger, FrozenCandidatePair,
    HoldoutManifest, MonetaryReservationState, SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    AnchorEvidenceStatus, DependencyEvidenceStatus, EvaluationEvidenceScope,
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
const NAMESPACE: &str = "invalid-reason-binding-fixture";
const BILLING_SCOPE: &str = "invalid-reason-billing";
const TICKET_ID: &str = "invalid-reason-ticket";
const FORMAL_KIND: &str = "formal_evaluation_v2";
const CHANGED: &str = "formal_invalid_outcome_changed";
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
    match std::env::var_os("RSIA_AG090_CASE_OUTPUT_DIR") {
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

async fn fixture(case: &str, schema: &str) -> Fixture {
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
        early_stop_plan: None,
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
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "statistics-family",
        alpha.digest().unwrap(),
        SequentialMode::Disabled,
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
        trusted_compiler_inputs::assert_frozen_issuer_static_read("invalid-control-current")
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
    });
    println!("actual-invalid-three-read-results {actual}");
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

async fn invalidated(case: &str, schema: &str, bound: bool) -> Fixture {
    let fixture = fixture(case, schema).await;
    issue(&fixture, bound).await;
    // Each side reserves, dispatches, records, pays and closes before the next
    // side starts. The root's single execution slot is never widened.
    grade(&fixture, "a0", "ok", "ok", 0).await;
    grade(&fixture, "r0", "ok", "ok", 1).await;
    let formal = IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        "environment_mismatch",
        40,
    )
    .await
    .unwrap();
    persist(&fixture, "actual-first-invalidation.json", &formal);
    fixture
}

fn assert_invalid_ok(reads: &ReadResults, bound: bool, expected_reasons: &[&str]) {
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::Invalid);
    assert!(status.ticket.candidate_dependency_binding.is_some());
    let _ = bound;
    assert_eq!(
        status.ticket.invalid_reasons,
        expected_reasons
            .iter()
            .map(|reason| (*reason).to_owned())
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        status.ticket.evidence_scope,
        EvaluationEvidenceScope::ProgramFixture
    );
    let FormalEvaluationV2::Invalid {
        evidence_scope,
        ticket_id,
        ticket_digest,
        reasons,
    } = status.formal.as_ref().unwrap()
    else {
        panic!("actual writer did not produce Invalid");
    };
    assert_eq!(*evidence_scope, EvaluationEvidenceScope::ProgramFixture);
    assert_eq!(ticket_id, &status.ticket.id);
    assert_eq!(ticket_digest, &fingerprint(&status.ticket).unwrap());
    assert_eq!(
        reasons,
        &expected_reasons
            .iter()
            .map(|reason| (*reason).to_owned())
            .collect::<Vec<_>>(),
    );
    assert!(!status.formal.as_ref().unwrap().is_promotable());
    assert_eq!(status.ticket.targets.len(), N as usize + 1);
    assert_eq!(
        status
            .ticket
            .targets
            .iter()
            .filter(|target| matches!(target.state, TargetState::Completed { .. }))
            .count(),
        2,
    );
    assert_eq!(
        status
            .ticket
            .targets
            .iter()
            .filter(|target| matches!(target.state, TargetState::NotDispatched { .. }))
            .count(),
        N as usize - 1,
    );
    for view in [
        reads.view.as_ref().unwrap(),
        reads.same_session.as_ref().unwrap(),
    ] {
        assert_eq!(view.variant, VerifiedReportVariant::Invalid);
        assert_eq!(view.verdict, Some(Verdict::Invalid));
        assert_eq!(view.anchor_status, AnchorEvidenceStatus::NotApplicable);
        assert_eq!(view.cost_evidence_scope, None);
        assert_eq!(view.evidence_scope, EvaluationEvidenceScope::ProgramFixture);
        assert_eq!(
            view.dependency_status,
            DependencyEvidenceStatus::NamespaceWatermarkOnly,
        );
        assert!(!view.promotion_eligible);
        assert_eq!(
            view.report_digest,
            fingerprint(status.formal.as_ref().unwrap()).unwrap()
        );
        assert_eq!(view.ticket_digest, fingerprint(&status.ticket).unwrap());
    }
}

async fn positive_without_writes(
    fixture: &Fixture,
    phase: &str,
    bound: bool,
    expected_reasons: &[&str],
) -> ReadResults {
    let before = catalog(&fixture.db_path);
    persist(
        fixture,
        &format!("{phase}-before-readers-full-catalog.json"),
        &before,
    );
    let reads = read_every_entry(fixture, phase).await;
    let after = catalog(&fixture.db_path);
    persist(
        fixture,
        &format!("{phase}-after-readers-full-catalog.json"),
        &after,
    );
    assert!(
        reads.session_commit.is_ok(),
        "actual caller-held Session commit failed: {:?}",
        reads.session_commit,
    );
    assert_eq!(before, after, "successful readers changed business state");
    assert_invalid_ok(&reads, bound, expected_reasons);
    reads
}

async fn paid_facts(fixture: &Fixture, phase: &str) -> Value {
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
    persist(fixture, &format!("{phase}-actual-calls.json"), &calls);
    persist(fixture, &format!("{phase}-actual-exposure.json"), &ledger);
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|call| {
        call.state == BudgetCallState::Finalized
            && call.execution_closed
            && call.actual_cost_micros.is_some()
    }));
    assert_eq!(
        calls
            .iter()
            .map(|call| call.actual_cost_micros.unwrap())
            .sum::<i64>(),
        30,
    );
    let entry = ledger
        .payload
        .entries
        .iter()
        .find(|entry| entry.ticket_id == fixture.ticket_id())
        .unwrap();
    assert_eq!(entry.task_cluster_ids.len(), N as usize);
    assert_eq!(
        entry.task_cluster_ids.iter().collect::<BTreeSet<_>>().len(),
        N as usize,
    );
    assert_eq!(entry.monetary_state, MonetaryReservationState::Finalized);
    json!({"calls": calls, "exposure": ledger})
}

async fn assert_refused_without_writes(
    fixture: &Fixture,
    phase: &str,
    watermark: &Option<(i64, String)>,
    message: &str,
) {
    let before = catalog(&fixture.db_path);
    persist(
        fixture,
        &format!("{phase}-before-readers-full-catalog.json"),
        &before,
    );
    let reads = read_every_entry(fixture, phase).await;
    let after = catalog(&fixture.db_path);
    persist(
        fixture,
        &format!("{phase}-after-readers-full-catalog.json"),
        &after,
    );
    assert!(
        reads.session_commit.is_ok(),
        "actual caller-held Session commit failed: {:?}",
        reads.session_commit,
    );
    assert_eq!(&reads.watermark, watermark);
    assert_eq!(before, after, "refused readers changed business state");
    // All three actual results and the complete catalogs precede this assertion.
    let refused = [
        matches!(&reads.status, Err(Error::Conflict(text)) if text == message),
        matches!(&reads.view, Err(Error::Conflict(text)) if text == message),
        matches!(&reads.same_session, Err(Error::Conflict(text)) if text == message),
    ];
    assert_eq!(refused, [true; 3], "three actual outcomes were retained");
}

async fn append_second_actual_reason(fixture: &Fixture, now: i64) {
    let formal = IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        "resource_limit",
        now,
    )
    .await
    .unwrap();
    persist(fixture, "actual-second-invalidation.json", &formal);
}

async fn positive_history(case: &str, schema: &str) {
    let mut fixture = invalidated(case, schema, true).await;
    let initial =
        positive_without_writes(&fixture, "first-positive", true, &["environment_mismatch"]).await;
    let accounting = paid_facts(&fixture, "first-positive").await;
    let original_ticket: Envelope<EvaluationTicketV2> =
        stored(&fixture, TICKET_KIND, fixture.ticket_id()).await;
    let original_formal: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let before_repeat = catalog(&fixture.db_path);
    persist(&fixture, "before-repeat-full-catalog.json", &before_repeat);
    let repeated = IndependentEvaluationControl::invalidate(
        &fixture.evaluator,
        &fixture.store,
        fixture.ticket_id(),
        "environment_mismatch",
        41,
    )
    .await
    .unwrap();
    persist(&fixture, "actual-repeat-invalidation.json", &repeated);
    assert_eq!(
        fingerprint(&repeated).unwrap(),
        fingerprint(&original_formal.payload).unwrap()
    );
    let after_repeat = catalog(&fixture.db_path);
    persist(&fixture, "after-repeat-full-catalog.json", &after_repeat);
    assert_only_object_updates(
        &before_repeat,
        &after_repeat,
        &[
            (
                physical_id(TICKET_KIND, fixture.ticket_id()),
                serde_json::to_value(&original_ticket).unwrap(),
            ),
            (
                physical_id(FORMAL_KIND, fixture.ticket_id()),
                serde_json::to_value(&original_formal).unwrap(),
            ),
        ],
    );
    assert_eq!(paid_facts(&fixture, "after-repeat").await, accounting);
    append_second_actual_reason(&fixture, 42).await;
    let ordered = positive_without_writes(
        &fixture,
        "ordered-positive",
        true,
        &["environment_mismatch", "resource_limit"],
    )
    .await;
    assert_eq!(ordered.watermark, initial.watermark);
    assert_eq!(paid_facts(&fixture, "ordered-positive").await, accounting);
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    fixture
        .store
        .stop_root_budget(&admin, BILLING_SCOPE, "historical_read", 5_000)
        .await
        .unwrap();
    fixture.store = Store::open(&fixture.db_path).await.unwrap();
    let reopened = positive_without_writes(
        &fixture,
        "reopened-after-root-stop",
        true,
        &["environment_mismatch", "resource_limit"],
    )
    .await;
    assert_eq!(reopened.watermark, initial.watermark);
    assert_eq!(
        paid_facts(&fixture, "reopened-after-root-stop").await,
        accounting
    );
}

async fn single_reasons_case(case: &str, schema: &str, reordered: bool, replacement: Value) {
    let fixture = invalidated(case, schema, true).await;
    if reordered {
        append_second_actual_reason(&fixture, 41).await;
    }
    let expected = if reordered {
        vec!["environment_mismatch", "resource_limit"]
    } else {
        vec!["environment_mismatch"]
    };
    let positive = positive_without_writes(&fixture, "positive", true, &expected).await;
    paid_facts(&fixture, "positive").await;
    let original: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let mut wire = serde_json::to_value(&original).unwrap();
    wire["payload"]["reasons"] = replacement;
    let altered: Envelope<FormalEvaluationV2> = serde_json::from_value(wire.clone()).unwrap();
    let before = catalog(&fixture.db_path);
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    put(&fixture, &altered).await;
    let after = catalog(&fixture.db_path);
    persist(&fixture, "after-intervention-full-catalog.json", &after);
    persist(&fixture, "single-reasons-intervention.json", &wire);
    assert_only_object_updates(
        &before,
        &after,
        &[(physical_id(FORMAL_KIND, fixture.ticket_id()), wire)],
    );
    assert_refused_without_writes(&fixture, "post-readback", &positive.watermark, CHANGED).await;
}

#[tokio::test]
async fn bound_invalid_v2_keeps_actual_order_paid_close_and_restart_reads() {
    positive_history("bound-positive-v2", RegisteredEvaluationControl::SCHEMA).await;
}

#[tokio::test]
async fn bound_invalid_v1_control_keeps_actual_order_paid_close_and_restart_reads() {
    positive_history("bound-positive-v1", RegisteredEvaluationControl::SCHEMA_V1).await;
}

#[tokio::test]
async fn report_only_empty_reasons_refuses_every_reader() {
    single_reasons_case(
        "empty-reasons",
        RegisteredEvaluationControl::SCHEMA,
        false,
        json!([]),
    )
    .await;
}

#[tokio::test]
async fn report_only_valid_replacement_reason_refuses_every_reader() {
    single_reasons_case(
        "replaced-reason-v1-control",
        RegisteredEvaluationControl::SCHEMA_V1,
        false,
        json!(["resource_limit"]),
    )
    .await;
}

#[tokio::test]
async fn report_only_reordered_actual_reasons_refuses_every_reader() {
    single_reasons_case(
        "reordered-two-actual-reasons",
        RegisteredEvaluationControl::SCHEMA,
        true,
        json!(["resource_limit", "environment_mismatch"]),
    )
    .await;
}

#[tokio::test]
async fn old_identity_refusal_precedes_new_reason_guard() {
    let fixture = invalidated(
        "identity-precedence",
        RegisteredEvaluationControl::SCHEMA,
        true,
    )
    .await;
    let positive =
        positive_without_writes(&fixture, "positive", true, &["environment_mismatch"]).await;
    paid_facts(&fixture, "positive").await;
    let original: Envelope<FormalEvaluationV2> =
        stored(&fixture, FORMAL_KIND, fixture.ticket_id()).await;
    let mut wire = serde_json::to_value(&original).unwrap();
    wire["payload"]["ticket_digest"] = json!(digest("wrong-ticket-digest"));
    wire["payload"]["reasons"] = json!([]);
    let altered: Envelope<FormalEvaluationV2> = serde_json::from_value(wire.clone()).unwrap();
    let before = catalog(&fixture.db_path);
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    put(&fixture, &altered).await;
    let after = catalog(&fixture.db_path);
    persist(&fixture, "after-intervention-full-catalog.json", &after);
    assert_only_object_updates(
        &before,
        &after,
        &[(physical_id(FORMAL_KIND, fixture.ticket_id()), wire)],
    );
    assert_refused_without_writes(
        &fixture,
        "post-readback",
        &positive.watermark,
        "invalid formal evaluation dependency closure mismatch",
    )
    .await;
}

#[tokio::test]
async fn absent_binding_keeps_v1_bytes_fingerprints_and_legacy_invalid_reads() {
    trusted_compiler_inputs::assert_frozen_issuer_static_read("invalid-control-v1")
        .await
        .unwrap();
    single_reasons_case(
        "legacy-matrix-migrated-current",
        RegisteredEvaluationControl::SCHEMA_V1,
        false,
        json!([]),
    )
    .await;
}
