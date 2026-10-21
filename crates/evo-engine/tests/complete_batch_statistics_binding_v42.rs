//! Public machine-contract fixtures for CompleteBatch statistics readback.
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
    AnchorCoverageMatrix, AnchorResult, CriticalCapabilityCoverage, FrozenCandidatePair,
    HoldoutManifest, SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, CompletePairedUnit, EarlyStopDecision, EarlyStopPlan, FormalClaimKind,
    ResearchFamilyAlphaPlan, SequentialRejectOnly,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_engine::streaming_evaluator::{
    AnchorEvidenceStatus, CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
    EvaluationTargetKind, ExecutionReceiptIntent, ExecutionSide, FixedGraderMethod,
    FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry, GraderReceiptIntent,
    IndependentEvaluationControl, IssueTicketIntent, IssueTicketRequest, ProtectedHoldoutRecord,
    ProtectedInputRef, RegisteredEvaluationControl, StartExecutionRequest,
    StreamingEvaluationStatus, TargetState, TicketLifecycle, VerifiedFormalReportView,
    VerifiedReportVariant, verified_report_view_in_session,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage, RootBudgetAuthorization,
    UsageCharge,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

const N: u32 = 60;
const NAMESPACE: &str = "statistics-binding-fixture";
const BILLING_SCOPE: &str = "statistics-billing";
const TICKET_ID: &str = "statistics-ticket";
const FORMAL_KIND: &str = "formal_evaluation_v2";
// Exact conflict for a stored CompleteBatch report that differs from its facts.
const STATISTICS_CHANGED: &str = "formal_complete_batch_statistics_changed";

#[derive(Debug, Clone, Copy)]
struct RecordedExecutionTimes {
    dispatched: i64,
    receipt: i64,
    finalized: i64,
    closed: i64,
}

#[derive(Default)]
struct RecordedEventTimes {
    executions: Mutex<BTreeMap<u32, RecordedExecutionTimes>>,
    completed: Mutex<Option<i64>>,
}

fn sample_seconds() -> i64 {
    evo_core::now().unwrap()
}

fn sample_millis() -> i64 {
    evo_core::clock::unix_millis_at(std::time::SystemTime::now()).unwrap()
}

async fn issue_from_native_clock(
    fixture: &Fixture,
) -> evo_engine::streaming_evaluator::EvaluationTicketV2 {
    // The native issue owner uses one actual wall sample for seconds and
    // milliseconds. Read its committed v2 receipt; never wait or round an event.
    let issue_started_at_ms = sample_millis();
    let ticket = IndependentEvaluationControl::issue_ticket_now(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketIntent {
            ticket_id: TICKET_ID.into(),
            registration_id: fixture.control.id.clone(),
            holdout_id: fixture.holdout.id.clone(),
        },
    )
    .await
    .unwrap();
    let issue_returned_at_ms = sample_millis();
    assert!(issue_started_at_ms <= issue_returned_at_ms);
    assert!(ticket.issued_at_unix_seconds >= fixture.control.created_at_unix_seconds);

    let before_issue_read = catalog(&fixture.db_path);
    let receipt_kind = "evaluation_ticket_issue_receipt_v2";
    let receipt_id = format!("e05-{}", fingerprint(&(receipt_kind, TICKET_ID)).unwrap());
    let mut session = fixture.store.session().await.unwrap();
    let envelope: Value = session
        .need(&fixture.evaluator, "artifact", &receipt_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        before_issue_read == catalog(&fixture.db_path),
        "reading the native issue precision witness changed the full catalog"
    );
    assert_eq!(
        envelope["schema_version"],
        json!("rsia.typed_artifact_envelope.v1")
    );
    assert_eq!(envelope["id"], json!(receipt_id));
    assert_eq!(envelope["record_kind"], json!(receipt_kind));
    let receipt = &envelope["payload"];
    assert_eq!(
        receipt["schema_version"],
        json!("rsia.evaluation_ticket_issue_receipt.v2")
    );
    assert_eq!(receipt["ticket_id"], json!(TICKET_ID));
    assert_eq!(
        receipt["registration_digest"].as_str(),
        Some(ticket.registration_digest.as_str())
    );
    assert_eq!(
        receipt["manifest_digest"].as_str(),
        Some(ticket.manifest_digest.as_str())
    );
    assert_eq!(receipt["evaluator_actor"], json!(fixture.evaluator.actor()));
    assert_eq!(
        receipt["issued_at_unix_seconds"].as_i64(),
        Some(ticket.issued_at_unix_seconds)
    );
    let issued_ms = receipt["issued_at_unix_ms"].as_i64().unwrap();
    assert!(issued_ms >= fixture.holdout.manifest.issued_at_unix_ms);
    assert_eq!(issued_ms / 1_000, ticket.issued_at_unix_seconds);
    assert!(issue_started_at_ms <= issued_ms && issued_ms <= issue_returned_at_ms);
    ticket
}
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
    _temporary: Option<tempfile::TempDir>,
    db_path: PathBuf,
    artifacts: Option<PathBuf>,
    store: Store,
    evaluator: Context,
    worker: Context,
    control: RegisteredEvaluationControl,
    holdout: ProtectedHoldoutRecord,
    event_times: RecordedEventTimes,
}

// ROOT can provide a fresh existing parent directory for future authorized
// probes. Each case creates one absent leaf and retains its DB and snapshots.
// Ordinary software runs use a separate TempDir for every fixture.
fn fixture_directory(case: &str) -> (Option<tempfile::TempDir>, PathBuf, Option<PathBuf>) {
    match std::env::var_os("RSIA_AG088_CASE_OUTPUT_DIR") {
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
    fixture_with_shared_attempt(case, schema, sequential, false).await
}

async fn fixture_with_shared_attempt(
    case: &str,
    schema: &str,
    sequential: bool,
    shared_attempt: bool,
) -> Fixture {
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
                created_at: sample_seconds(),
            },
        )
        .await
        .unwrap();
    let mut v1 = ExperimentPlan::first_low_risk("statistics-plan", N as usize).unwrap();
    v1.monetary_budget = money("0.02");
    v1.frozen = true;
    v1.frozen_at = Some(sample_seconds());
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
                alpha: if shared_attempt { "0.005" } else { "0.01" }.into(),
            },
        ]
        .into_iter()
        .chain(shared_attempt.then(|| AlphaAllocation {
            claim_id: "r39-second-fixed".into(),
            attempt_id: "attempt-2".into(),
            kind: FormalClaimKind::FixedSampleGain,
            alpha: "0.005".into(),
        }))
        .collect(),
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
        created_at_unix_seconds: sample_seconds(),
    };
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        digest("fixture-candidate"),
        bundle().parent_digest,
        sample_millis(),
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
        sample_millis(),
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
        artifacts,
        store,
        evaluator,
        worker,
        control,
        holdout,
        event_times: RecordedEventTimes::default(),
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
    let now = sample_seconds();
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
                lease_until: now.checked_add(1_000).unwrap(),
                now,
            },
        )
        .await
        .unwrap();
    let dispatched_at = sample_seconds();
    let fence = BudgetCallFence {
        billing_scope: BILLING_SCOPE.into(),
        call_id: call_id.clone(),
        actual_input_digest: task.request_digest.clone(),
        lease_token: lease,
        lease_epoch: 1,
        now: dispatched_at,
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
    let receipt_started_at = sample_seconds();
    let receipt = IndependentEvaluationControl::record_execution_receipt_now(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptIntent {
            receipt_id: receipt_id.clone(),
            ticket_id: TICKET_ID.into(),
            target_id: target.into(),
            side,
            request_digest: task.request_digest,
            output_artifact_id: format!("output-{ordinal}"),
            output_utf8: json!({"answer": answer}).to_string(),
            budget_call_id: call_id.clone(),
            latency_micros: 100 + u64::from(ordinal),
        },
    )
    .await
    .unwrap();
    let receipt_returned_at = sample_seconds();
    let receipt_at = receipt.issued_at_unix_seconds;
    assert!(receipt_started_at <= receipt_at && receipt_at <= receipt_returned_at);
    let mut terminal_fence = fence;
    let finalized_at = sample_seconds();
    terminal_fence.now = finalized_at;
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
                output_digest: receipt.output_digest,
            },
        )
        .await
        .unwrap();
    let closed_at = sample_seconds();
    let closed = fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            BILLING_SCOPE,
            &call_id,
            dispatch.call.dispatch_id.as_deref().unwrap(),
            "fixture_complete",
            closed_at,
        )
        .await
        .unwrap();
    assert_eq!(dispatch.call.dispatched_at, Some(dispatched_at));
    assert_eq!(finalized.finalized_at, Some(finalized_at));
    assert_eq!(closed.execution_closed_at, Some(closed_at));
    assert!(dispatched_at <= receipt_at);
    assert!(receipt_at <= finalized_at);
    assert!(finalized_at <= closed_at);
    assert!(
        fixture
            .event_times
            .executions
            .lock()
            .unwrap()
            .insert(
                ordinal,
                RecordedExecutionTimes {
                    dispatched: dispatch.call.dispatched_at.unwrap(),
                    receipt: receipt.issued_at_unix_seconds,
                    finalized: finalized.finalized_at.unwrap(),
                    closed: closed.execution_closed_at.unwrap(),
                },
            )
            .is_none()
    );
    receipt_id
}

async fn grade(fixture: &Fixture, target: &str, baseline_answer: &str, ordinal: u32) {
    let candidate = execute(fixture, target, ExecutionSide::Candidate, "ok", ordinal * 2).await;
    let baseline = execute(
        fixture,
        target,
        ExecutionSide::Baseline,
        baseline_answer,
        ordinal * 2 + 1,
    )
    .await;
    let grader_started_at = sample_seconds();
    let graded = IndependentEvaluationControl::record_grader_receipt_now(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptIntent {
            receipt_id: format!("grader-{ordinal}"),
            ticket_id: TICKET_ID.into(),
            target_id: target.into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
        },
    )
    .await
    .unwrap();
    let grader_returned_at = sample_seconds();
    assert!(grader_started_at <= graded.issued_at_unix_seconds);
    assert!(graded.issued_at_unix_seconds <= grader_returned_at);
}

async fn complete(fixture: &Fixture, sequential: bool) -> FormalEvaluationV2 {
    stage_candidate(fixture).await;
    let issued = issue_from_native_clock(fixture).await;
    assert!(issued.candidate_dependency_binding.is_some());
    if sequential {
        // The anchor completes first; rotating callbacks arrive in reverse order.
        // The engine must still consume each frozen continuous prefix in order.
        grade(fixture, "a0", "ok", 1).await;
        for (sequence, index) in (0..N).rev().enumerate() {
            grade(fixture, &format!("r{index}"), "wrong", sequence as u32 + 2).await;
        }
    } else {
        for i in 0..N {
            grade(fixture, &format!("r{i}"), "wrong", i + 1).await;
        }
        grade(fixture, "a0", "ok", N + 1).await;
    }
    finalize_from_system(fixture).await
}

async fn finalize_from_system(fixture: &Fixture) -> FormalEvaluationV2 {
    let finalize_started_at = sample_seconds();
    let formal = IndependentEvaluationControl::finalize_complete_now(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
    )
    .await
    .unwrap();
    let finalize_returned_at = sample_seconds();
    assert!(finalize_started_at <= finalize_returned_at);
    let group = fixture
        .store
        .dispatch_group(&fixture.worker, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap()
        .unwrap();
    let committed_stop = group.stop_committed_at.unwrap();
    assert!(committed_stop <= finalize_returned_at);
    *fixture.event_times.completed.lock().unwrap() = Some(committed_stop);
    formal
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormalEnvelope {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: FormalEvaluationV2,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFormalEnvelopeR49 {
    schema_version: String,
    record_kind: String,
    id: String,
    engine_version: String,
    input_schema_version: String,
    input_digest: String,
    payload_digest: String,
    input: Value,
    payload: FormalEvaluationV2,
}
async fn native_reference_for_owned_fixture_r49(
    fixture: &Fixture,
) -> evo_core::formal_version::ReportRef {
    let read = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            evo_storage::FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    let reference = read
        .formal()
        .unwrap()
        .current_complete()
        .unwrap()
        .reference()
        .clone();
    assert_eq!(reference.ticket_id, TICKET_ID);
    assert_ne!(reference.physical_id, reference.ticket_id);
    reference
}
#[test]
fn r49_old_four_field_formal_decoder_is_static_and_not_a_native_current_carrier() {
    let old = json!({"schema_version":"rsia.typed_artifact_envelope.v1","id":"old",
        "record_kind":"formal_evaluation_v2","payload":{
            "variant":"invalid","evidence_scope":"program_fixture","ticket_id":"ticket",
            "ticket_digest":"a".repeat(64),"reasons":["legacy"]}});
    let record: FormalEnvelope = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(record.schema_version, "rsia.typed_artifact_envelope.v1");
    assert_eq!(serde_json::to_value(record).unwrap(), old);
    assert!(serde_json::from_value::<NativeFormalEnvelopeR49>(old).is_err());
}

struct ReadResults {
    status: evo_core::Result<StreamingEvaluationStatus>,
    view: evo_core::Result<VerifiedFormalReportView>,
    same_session: evo_core::Result<VerifiedFormalReportView>,
    watermark: Option<(i64, String)>,
}

async fn read_every_entry(fixture: &Fixture, phase: &str) -> ReadResults {
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
    let watermark = session.watermark(&fixture.evaluator).await.unwrap();
    drop(session);
    let observed = json!({
        "phase": phase,
        "status": {"ok": status.is_ok(), "actual_debug": format!("{status:?}")},
        "verified_report_view": {"ok": view.is_ok(), "actual_debug": format!("{view:?}")},
        "verified_report_view_in_session": {"ok": same_session.is_ok(), "actual_debug": format!("{same_session:?}")},
        "watermark": watermark,
    });
    println!("actual-three-read-results {}", observed);
    persist(
        fixture,
        &format!("{phase}-actual-three-read-results.json"),
        &observed,
    );
    ReadResults {
        status,
        view,
        same_session,
        watermark,
    }
}

fn assert_positive(
    fixture: &Fixture,
    reads: &ReadResults,
    formal: &FormalEvaluationV2,
    verdict: Verdict,
) {
    let status = reads.status.as_ref().unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::CompleteBatch);
    assert!(status.ticket.candidate_dependency_binding.is_some());
    assert_eq!(status.ticket.targets.len(), N as usize + 1);
    assert_eq!(status.ticket.next_sequence, u64::from(N) + 2);
    let mut sequences = BTreeSet::new();
    let mut rotating = BTreeSet::new();
    let mut anchors = BTreeSet::new();
    let mut rotating_events = Vec::new();
    for target in &status.ticket.targets {
        let TargetState::Completed {
            candidate_score_micros,
            baseline_score_micros,
            anchor_result,
            completion_sequence,
            ..
        } = &target.state
        else {
            panic!("positive fixture has a non-complete target");
        };
        assert!(sequences.insert(*completion_sequence));
        assert_eq!(*candidate_score_micros, 1_000_000);
        match &target.kind {
            EvaluationTargetKind::Rotating { cluster_id } => {
                assert!(rotating.insert(cluster_id.clone()));
                assert_eq!(*baseline_score_micros, 0);
                assert_eq!(*anchor_result, None);
                rotating_events.push((
                    *completion_sequence,
                    CompletePairedUnit {
                        cluster_id: cluster_id.clone(),
                        baseline_score_micros: *baseline_score_micros,
                        candidate_score_micros: *candidate_score_micros,
                    },
                ));
            }
            EvaluationTargetKind::Anchor { anchor_id } => {
                assert!(anchors.insert(anchor_id.as_str()));
                assert_eq!(*baseline_score_micros, 1_000_000);
                assert_eq!(*anchor_result, Some(AnchorResult::Passed));
            }
        }
    }
    assert_eq!(sequences, (1..=u64::from(N) + 1).collect::<BTreeSet<_>>());
    assert_eq!(
        rotating,
        (0..N)
            .map(|i| format!("cluster-{i}"))
            .collect::<BTreeSet<_>>(),
    );
    assert_eq!(anchors, BTreeSet::from(["anchor-1"]));
    if let Some(plan) = &fixture.control.early_stop_plan {
        assert_eq!(plan.rho, "1");
        let mut stream =
            SequentialRejectOnly::open(plan.clone(), &fixture.control.alpha_plan).unwrap();
        rotating_events.sort_by_key(|(sequence, _)| *sequence);
        for (index, (_, unit)) in rotating_events.into_iter().enumerate() {
            let decision = stream.record_complete_unit(unit).unwrap();
            if index + 1 == N as usize {
                assert_eq!(decision, EarlyStopDecision::FinalFixedSample);
            } else {
                assert!(!decision.is_terminal());
            }
        }
        assert_eq!(stream.k(), N);
        assert_eq!(
            stream.decision().unwrap(),
            EarlyStopDecision::FinalFixedSample
        );
    }
    assert_eq!(
        status
            .formal
            .as_ref()
            .map(|record| fingerprint(record).unwrap()),
        Some(fingerprint(formal).unwrap()),
    );
    for view in [
        reads.view.as_ref().unwrap(),
        reads.same_session.as_ref().unwrap(),
    ] {
        assert_eq!(view.variant, VerifiedReportVariant::CompleteBatch);
        assert_eq!(view.verdict, Some(verdict));
        assert_eq!(view.report_digest, fingerprint(formal).unwrap());
        assert_eq!(view.evidence_scope, EvaluationEvidenceScope::ProgramFixture);
        assert_eq!(
            view.cost_evidence_scope,
            Some(CostEvidenceScope::TicketExecutionOnly),
        );
        assert_eq!(
            view.anchor_status,
            AnchorEvidenceStatus::CompletePassedStoredClosure,
        );
        assert_eq!(
            view.dependency_status,
            DependencyEvidenceStatus::NamespaceWatermarkOnly,
        );
        assert!(!view.promotion_eligible);
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

fn detach_formal_row(catalog: &Value, id: &str) -> (Value, Vec<String>, Vec<Value>) {
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

fn assert_single_mean_intervention(before: &Value, after: &Value, id: &str, expected: &Value) {
    let (before_remainder, before_columns, before_row) = detach_formal_row(before, id);
    let (after_remainder, after_columns, mut after_row) = detach_formal_row(after, id);
    assert_eq!(before_columns, after_columns);
    assert_eq!(
        before_remainder, after_remainder,
        "another business row/schema changed",
    );
    let body = before_columns
        .iter()
        .position(|column| column == "body")
        .unwrap();
    let revision = before_columns
        .iter()
        .position(|column| column == "revision")
        .unwrap();
    assert_eq!(before_row[body][0], after_row[body][0]);
    let original: Value = serde_json::from_str(before_row[body][1].as_str().unwrap()).unwrap();
    let changed: Value = serde_json::from_str(after_row[body][1].as_str().unwrap()).unwrap();
    let original_bytes = before_row[body][1].as_str().unwrap();
    let changed_bytes = after_row[body][1].as_str().unwrap();
    assert_eq!(original_bytes.matches("\"mean\":\"1\"").count(), 1);
    assert_eq!(
        changed_bytes,
        original_bytes.replace("\"mean\":\"1\"", "\"mean\":\"0.5\""),
        "raw formal body changed beyond the single mean value",
    );
    let mut single_field = original;
    single_field["payload"]["report"]["mean"] = json!("0.5");
    assert_eq!(
        changed, single_field,
        "intervention changed more than report.mean",
    );
    assert_eq!(&changed, expected);
    assert_eq!(before_row[revision][0], after_row[revision][0]);
    assert_eq!(
        after_row[revision][1].as_i64().unwrap(),
        before_row[revision][1]
            .as_i64()
            .unwrap()
            .checked_add(1)
            .unwrap(),
    );
    // Preserve the original observable +1 revision delta of Session::put. The
    // deliberate fixture fault reproduces it; no revision is reset or hidden.
    after_row[body] = before_row[body].clone();
    after_row[revision] = before_row[revision].clone();
    assert_eq!(
        before_row, after_row,
        "another formal object column changed"
    );
}

// A deliberately malicious storage fault for this test's own newly created
// Fixture database. It is not a second production writer, a source producer or
// a way to store a changed report through the guarded product API. Keep the
// mounted Store idle: all preceding operations and the final reader commit were
// awaited, and no fixture task is spawned during this synchronous fault.
fn inject_single_mean_fault_into_owned_fixture(
    fixture: &Fixture,
    before: &Value,
    physical_id: &str,
    expected_changed: &Value,
) {
    assert!(fixture.db_path.is_absolute());
    assert_eq!(
        fixture.db_path.file_name().unwrap(),
        std::ffi::OsStr::new("fixture.sqlite3")
    );
    let parent = fixture.db_path.parent().unwrap();
    if let Some(temporary) = &fixture._temporary {
        assert_eq!(parent, temporary.path());
    } else {
        assert_eq!(Some(parent), fixture.artifacts.as_deref());
    }
    assert!(
        std::fs::symlink_metadata(&fixture.db_path)
            .unwrap()
            .is_file()
    );
    let (_, columns, row) = detach_formal_row(before, physical_id);
    assert_eq!(columns.len(), 6);
    assert_eq!(row.len(), columns.len());
    let index = |name: &str| columns.iter().position(|column| column == name).unwrap();
    for name in ["namespace", "kind", "id", "owner", "body"] {
        assert_eq!(row[index(name)][0], "str");
    }
    assert_eq!(row[index("revision")][0], "int");
    assert_eq!(row[index("namespace")][1], NAMESPACE);
    assert_eq!(row[index("kind")][1], "artifact");
    assert_eq!(row[index("id")][1], physical_id);
    let owner = row[index("owner")][1].as_str().unwrap();
    assert_eq!(owner, fixture.evaluator.actor());
    let raw = row[index("body")][1].as_str().unwrap();
    let revision = row[index("revision")][1].as_i64().unwrap();
    assert!(revision > 0);
    let next_revision = revision.checked_add(1).unwrap();
    assert_eq!(raw.matches("\"mean\":\"1\"").count(), 1);
    let changed = raw.replacen("\"mean\":\"1\"", "\"mean\":\"0.5\"", 1);
    assert_eq!(
        serde_json::from_str::<Value>(&changed).unwrap(),
        *expected_changed
    );
    let frame = json!({
        "namespace": NAMESPACE,
        "id": physical_id,
        "owner": owner,
        "body": raw,
        "revision": revision,
        "next_revision": next_revision,
        "changed_body": changed,
        "expected_changed": expected_changed,
    });
    // mode=rw cannot create a database; exact identity/body/revision CAS cannot
    // select another row. BEGIN IMMEDIATE is a single bounded fault transaction,
    // not application-lock admission. No PRAGMA, migration, checkpoint, cleanup,
    // watermark, audit, schema or global fixture configuration is changed.
    let fault = r#"
import hashlib,json,os,pathlib,sqlite3,stat,sys
path=pathlib.Path(sys.argv[1])
frame=json.load(sys.stdin)
assert path.is_absolute() and path.name=='fixture.sqlite3'
identity=os.lstat(path)
assert stat.S_ISREG(identity.st_mode) and identity.st_nlink==1
canonical=path.resolve(strict=True)
assert canonical.name=='fixture.sqlite3' and canonical.parent==path.parent.resolve(strict=True)
canonical_identity=os.lstat(canonical)
assert (canonical_identity.st_dev,canonical_identity.st_ino)==(identity.st_dev,identity.st_ino)
old=frame['body'];new=frame['changed_body']
assert type(old) is str and type(new) is str
assert old.count('"mean":"1"')==1
assert new==old.replace('"mean":"1"','"mean":"0.5"',1)
assert json.loads(new)==frame['expected_changed']
assert type(frame['revision']) is int and 0<frame['revision']<9223372036854775807
assert frame['next_revision']==frame['revision']+1
old_json=json.loads(old)
assert old_json['schema_version']=='rsia.typed_artifact_envelope.v1'
assert old_json['id']==frame['id'] and old_json['record_kind']=='formal_evaluation_v2'
assert old_json['payload']['variant']=='complete_batch'
assert old_json['payload']['stored_dependency_binding'] is not None
assert old_json['payload']['report']['mean']=='1'
expected=(frame['namespace'],'artifact',frame['id'],frame['owner'],old,frame['revision'])
connection=sqlite3.connect(canonical.as_uri()+'?mode=rw',uri=True,timeout=0.0,isolation_level=None)
try:
    now=os.lstat(path)
    assert (now.st_dev,now.st_ino)==(identity.st_dev,identity.st_ino)
    assert stat.S_ISREG(now.st_mode) and now.st_nlink==1
    connection.execute('BEGIN IMMEDIATE')
    columns=[row[1] for row in connection.execute('PRAGMA table_info(objects)')]
    assert columns==['namespace','kind','id','owner','body','revision'],columns
    actual=connection.execute('SELECT namespace,kind,id,owner,body,revision FROM objects WHERE namespace=? AND kind=? AND id=?',expected[:3]).fetchone()
    assert actual==expected,'fixture row no longer matches exact pre-fault catalog'
    changed=connection.execute('UPDATE objects SET body=?,revision=? WHERE namespace=? AND kind=? AND id=? AND owner=? AND body=? AND revision=?',(new,frame['next_revision'],*expected)).rowcount
    assert changed==1,changed
    actual_after=connection.execute('SELECT namespace,kind,id,owner,body,revision FROM objects WHERE namespace=? AND kind=? AND id=?',expected[:3]).fetchone()
    assert actual_after==(*expected[:4],new,frame['next_revision'])
    connection.execute('COMMIT')
finally:
    if connection.in_transaction:
        connection.execute('ROLLBACK')
    connection.close()
now=os.lstat(path)
assert (now.st_dev,now.st_ino)==(identity.st_dev,identity.st_ino)
assert stat.S_ISREG(now.st_mode) and now.st_nlink==1
print(json.dumps({'fixture_fault_only':True,'committed_rows':1,'revision_before':frame['revision'],'revision_after':frame['next_revision'],'body_before_sha256':hashlib.sha256(old.encode()).hexdigest(),'body_after_sha256':hashlib.sha256(new.encode()).hexdigest()}))
"#;
    let input = serde_json::to_vec(&frame).unwrap();
    let mut child = Command::new("python3")
        .args(["-B", "-c", fault])
        .arg(&fixture.db_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let input_result = {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&input)
    };
    let output = child.wait_with_output().unwrap();
    input_result.unwrap();
    assert!(
        output.status.success(),
        "owned fixture fault did not commit: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual["fixture_fault_only"], true);
    assert_eq!(actual["committed_rows"], 1);
    assert_eq!(actual["revision_before"].as_i64(), Some(revision));
    assert_eq!(actual["revision_after"].as_i64(), Some(next_revision));
    assert_eq!(actual["body_before_sha256"], hash(raw.as_bytes()));
    assert_eq!(actual["body_after_sha256"], hash(changed.as_bytes()));
    println!("actual-owned-fixture-mean-fault {actual}");
}

async fn single_mean_case(case: &str, schema: &str, sequential: bool, verdict: Verdict) {
    let fixture = fixture(case, schema, sequential).await;
    let formal = complete(&fixture, sequential).await;
    let FormalEvaluationV2::CompleteBatch {
        report,
        verdict: outer_verdict,
        stored_dependency_binding: Some(_),
        ..
    } = &formal
    else {
        panic!("positive fixture did not produce a bound CompleteBatch");
    };
    assert_eq!(report.stats_version, "rsia.empirical_bernstein.v2");
    assert_eq!(report.n, N as usize);
    assert_eq!(report.mean, "1");
    assert_eq!(report.variance, "0");
    assert_eq!(report.alpha_i, "0.04");
    assert_eq!(report.verdict, verdict);
    assert_eq!(*outer_verdict, verdict);
    assert_eq!(
        report.reasons,
        if schema == RegisteredEvaluationControl::SCHEMA_V1 {
            vec!["complete_optimization_cost_not_verified".to_owned()]
        } else {
            vec![]
        },
    );
    assert!(report.lcb.parse::<f64>().unwrap() > fixture.control.v1_plan_snapshot.min_effect);
    assert!(!formal.is_promotable());
    let positive = read_every_entry(&fixture, "positive").await;
    assert_positive(&fixture, &positive, &formal, verdict);
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let closed_root = fixture
        .store
        .stop_root_budget(
            &admin,
            BILLING_SCOPE,
            "historical_report_readback",
            sample_seconds(),
        )
        .await
        .unwrap();
    assert!(closed_root.stopped);
    let closed_root_observed = json!({
        "stopped": closed_root.stopped,
        "actual_debug": format!("{closed_root:?}"),
    });
    println!("actual-closed-root-budget {}", closed_root_observed);
    persist(&fixture, "closed-root-budget.json", &closed_root_observed);
    // A closed billing root retains historical report authority. Collect all
    // three actual reads before checking this additional positive precondition.
    let closed_positive = read_every_entry(&fixture, "positive-after-root-stop").await;
    assert_positive(&fixture, &closed_positive, &formal, verdict);
    assert_eq!(closed_positive.watermark, positive.watermark);
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.worker, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap();
    assert_eq!(calls.len(), 2 * (N as usize + 1));
    assert_eq!(
        calls
            .iter()
            .map(|call| call.call_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        calls.len(),
    );
    assert!(calls.iter().all(|call| {
        call.state == BudgetCallState::Finalized
            && call.execution_closed
            && call.actual_cost_micros.is_some_and(|micros| micros >= 0)
            && call.actual_currency.as_deref() == Some("USD")
            && call.actual_pricing_version.as_deref() == Some("fixture-pricing-v1")
    }));
    assert_eq!(
        calls
            .iter()
            .map(|call| call.actual_cost_micros.unwrap())
            .sum::<i64>(),
        15 * i64::from(N + 1),
    );
    let reference = native_reference_for_owned_fixture_r49(&fixture).await;
    let physical_id = reference.physical_id.clone();
    let original: NativeFormalEnvelopeR49 = session
        .need(&fixture.evaluator, "artifact", &physical_id)
        .await
        .unwrap();
    assert_eq!(original.schema_version, evo_core::formal_version::SCHEMA);
    assert_eq!(original.engine_version, evo_core::formal_version::ENGINE_V4);
    assert_eq!(
        original.input_schema_version,
        evo_core::formal_version::INPUT_SCHEMA_V2
    );
    assert_eq!(original.input_digest, reference.input_digest);
    assert_eq!(original.payload_digest, reference.payload_digest);
    assert_eq!(original.id, physical_id);
    assert_eq!(original.record_kind, evo_core::formal_version::KIND);
    assert_eq!(
        fingerprint(&original.payload).unwrap(),
        fingerprint(&formal).unwrap(),
    );
    session.commit().await.unwrap();
    let before = catalog(&fixture.db_path);
    persist(&fixture, "before-intervention-full-catalog.json", &before);
    persist(&fixture, "original-formal-envelope.json", &original);
    let mut altered = original.clone();
    let FormalEvaluationV2::CompleteBatch { report, .. } = &mut altered.payload else {
        unreachable!()
    };
    report.mean = "0.5".into();
    let expected = serde_json::to_value(&altered).unwrap();
    // A guarded Session now correctly refuses this corruption at commit. The
    // separate occurrence_collector_* transaction tests preserve that refusal.
    // Here only a deliberate exact CAS storage fault in this owned Fixture DB
    // recreates persisted corruption for the original three live read controls.
    // Do not close/reopen the Store: startup correctly refuses a damaged floor,
    // which would mask these engine read diagnostics instead of testing them.
    inject_single_mean_fault_into_owned_fixture(&fixture, &before, &physical_id, &expected);
    let after_intervention = catalog(&fixture.db_path);
    persist(
        &fixture,
        "after-intervention-full-catalog.json",
        &after_intervention,
    );
    persist(&fixture, "altered-formal-envelope.json", &altered);
    assert_single_mean_intervention(&before, &after_intervention, &physical_id, &expected);
    // All three live outcomes are collected and retained before any semantic
    // refusal assertion. If an existing guard rejects, ROOT must inspect it;
    // that outcome is not relabeled as the proposed statistics diagnostic.
    let actual = read_every_entry(&fixture, "after-single-mean-intervention").await;
    let after_reads = catalog(&fixture.db_path);
    persist(
        &fixture,
        "after-three-reads-full-catalog.json",
        &after_reads,
    );
    assert_eq!(
        actual.watermark, closed_positive.watermark,
        "read/intervention changed watermark",
    );
    assert_eq!(
        after_reads, after_intervention,
        "a report read wrote business state",
    );
    let refused = [
        matches!(&actual.status, Err(Error::Conflict(message)) if message == STATISTICS_CHANGED),
        matches!(&actual.view, Err(Error::Conflict(message)) if message == STATISTICS_CHANGED),
        matches!(&actual.same_session, Err(Error::Conflict(message)) if message == STATISTICS_CHANGED),
    ];
    assert_eq!(
        refused, [true; 3],
        "all actual outcomes were retained before this final semantic assertion",
    );
}

#[tokio::test]
async fn v2_complete_batch_single_mean_tamper_refuses_every_read() {
    single_mean_case(
        "v2-fixed-mean",
        RegisteredEvaluationControl::SCHEMA,
        false,
        Verdict::Improved,
    )
    .await;
}

#[tokio::test]
async fn bound_v1_reject_only_complete_batch_single_mean_tamper_refuses_every_read() {
    single_mean_case(
        "v1-reject-only-mean",
        RegisteredEvaluationControl::SCHEMA_V1,
        true,
        Verdict::Inconclusive,
    )
    .await;
}

// AG-132: exercise the original complete producer before the collector. These
// software receipts remain ProgramFixture; neither an occurrence floor nor a
// Some binding supplies production authority or a semantic-version exemption.
async fn assert_e05_collector_production_positive(
    fixture: &Fixture,
    formal: &FormalEvaluationV2,
    phase: &str,
) {
    let before = catalog(&fixture.db_path);
    let reads = read_every_entry(fixture, phase).await;
    assert_positive(fixture, &reads, formal, Verdict::Improved);
    let FormalEvaluationV2::CompleteBatch {
        stored_dependency_binding: Some(_),
        ..
    } = formal
    else {
        panic!("the complete producer must retain its actual Some binding");
    };
    assert!(
        reads
            .status
            .as_ref()
            .unwrap()
            .ticket
            .candidate_dependency_binding
            .is_some()
    );
    assert!(!formal.is_promotable());

    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.worker, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap();
    assert_eq!(calls.len(), 2 * (N as usize + 1));
    for call in &calls {
        assert_eq!(call.namespace, NAMESPACE);
        assert_eq!(call.dispatch_group_id, TICKET_ID);
        assert_eq!(call.state, BudgetCallState::Finalized);
        assert!(call.execution_closed);
        let ordinal: u32 = call.call_id.strip_prefix("call-").unwrap().parse().unwrap();
        let receipt_id = format!("receipt-{ordinal}");
        let physical = format!(
            "e05-{}",
            fingerprint(&("execution_receipt_v2", receipt_id.as_str())).unwrap()
        );
        let receipt: Value = session
            .need(&fixture.evaluator, "artifact", &physical)
            .await
            .unwrap();
        assert_eq!(receipt["record_kind"], "execution_receipt_v2");
        assert_eq!(
            receipt["payload"]["receipt_id"].as_str(),
            Some(receipt_id.as_str())
        );
        assert_eq!(
            receipt["payload"]["budget_call_id"].as_str(),
            Some(call.call_id.as_str())
        );
        assert_eq!(receipt["payload"]["ticket_id"], TICKET_ID);
        assert_eq!(
            receipt["payload"]["request_digest"].as_str(),
            Some(call.actual_input_digest.as_str())
        );
        // execute() deliberately persists the actual output and receipt first,
        // then finalizes the charge, then closes execution. Keep that normal
        // producer order: pre-finalizing to evade occurrence admission is wrong.
        let times = *fixture
            .event_times
            .executions
            .lock()
            .unwrap()
            .get(&ordinal)
            .unwrap();
        assert_eq!(call.dispatched_at, Some(times.dispatched));
        assert_eq!(
            receipt["payload"]["issued_at_unix_seconds"].as_i64(),
            Some(times.receipt)
        );
        assert_eq!(call.finalized_at, Some(times.finalized));
        assert_eq!(call.execution_closed_at, Some(times.closed));
        let output_id = receipt["payload"]["output_artifact_id"].as_str().unwrap();
        let output_physical = format!(
            "e05-{}",
            fingerprint(&("evaluation_execution_output_v1", output_id)).unwrap()
        );
        let output: Value = session
            .need(&fixture.evaluator, "artifact", &output_physical)
            .await
            .unwrap();
        assert_eq!(output["record_kind"], "evaluation_execution_output_v1");
        let output_utf8 = output["payload"]["output_utf8"].as_str().unwrap();
        let actual_digest = hash(output_utf8.as_bytes());
        assert_eq!(
            receipt["payload"]["output_digest"].as_str(),
            Some(actual_digest.as_str())
        );
        assert_eq!(
            output["payload"]["output_digest"].as_str(),
            Some(actual_digest.as_str())
        );
        assert_eq!(call.output_digest.as_deref(), Some(actual_digest.as_str()));
        assert!(call.actual_cost_micros.is_some_and(|amount| amount >= 0));
    }
    session.commit().await.unwrap();
    let observed =
        evo_storage::lifecycle::readonly_wall_seconds(&fixture.db_path, &[], &[]).unwrap();
    let completed = fixture.event_times.completed.lock().unwrap().unwrap();
    assert!(observed >= completed);
    let after = catalog(&fixture.db_path);
    assert!(
        before == after,
        "complete source qualification or its read controls changed the full catalog: {} -> {}",
        fingerprint(&before).unwrap(),
        fingerprint(&after).unwrap(),
    );
}

#[tokio::test]
async fn occurrence_collector_accepts_real_bound_complete_after_receipts_then_settlements() {
    let fixture = fixture_with_shared_attempt(
        "collector-current-production-complete",
        RegisteredEvaluationControl::SCHEMA,
        false,
        true,
    )
    .await;
    // complete() calls the real broker, execution-receipt producer, independent
    // fixed grader over stored original outputs and oracle, and final producer.
    let formal = complete(&fixture, false).await;
    assert_e05_collector_production_positive(&fixture, &formal, "collector-production-positive")
        .await;
    r39_native_current_and_late_overlay_controls(&fixture, &formal).await;
}

enum E05CollectorReportIntervention {
    Mean,
    ResourceEvidenceDigest,
}

async fn assert_e05_collector_report_intervention_rolls_back(
    case: &str,
    intervention: E05CollectorReportIntervention,
) {
    let fixture = fixture(case, RegisteredEvaluationControl::SCHEMA, false).await;
    let formal = complete(&fixture, false).await;
    // An actual accepted healthy producer must precede every malicious case.
    assert_e05_collector_production_positive(
        &fixture,
        &formal,
        "collector-before-report-intervention",
    )
    .await;
    let reference = native_reference_for_owned_fixture_r49(&fixture).await;
    let physical_id = reference.physical_id.clone();
    let mut reader = fixture.store.session().await.unwrap();
    let original: NativeFormalEnvelopeR49 = reader
        .need(&fixture.evaluator, "artifact", &physical_id)
        .await
        .unwrap();
    reader.commit().await.unwrap();
    assert_eq!(
        fingerprint(&original.payload).unwrap(),
        fingerprint(&formal).unwrap()
    );
    let mut altered = original.clone();
    let FormalEvaluationV2::CompleteBatch {
        report,
        resource_evidence_digest,
        stored_dependency_binding: Some(_),
        ..
    } = &mut altered.payload
    else {
        panic!("interventions require the original bound complete report");
    };
    let label = match intervention {
        E05CollectorReportIntervention::Mean => {
            assert_eq!(report.mean, "1");
            report.mean = "0.5".into();
            "report_mean"
        }
        E05CollectorReportIntervention::ResourceEvidenceDigest => {
            let changed = digest("ag132-wrong-report-resource-evidence-binding");
            assert_ne!(*resource_evidence_digest, changed);
            *resource_evidence_digest = changed;
            "resource_evidence_digest"
        }
    };
    let original_value = serde_json::to_value(&original).unwrap();
    let altered_value = serde_json::to_value(&altered).unwrap();
    let mut only_expected_field = original_value.clone();
    match label {
        "report_mean" => only_expected_field["payload"]["report"]["mean"] = json!("0.5"),
        "resource_evidence_digest" => {
            only_expected_field["payload"]["resource_evidence_digest"] =
                json!(digest("ag132-wrong-report-resource-evidence-binding"));
        }
        _ => unreachable!(),
    }
    assert_eq!(altered_value, only_expected_field);
    assert_ne!(altered_value, original_value);
    let before = catalog(&fixture.db_path);
    let mut writer = fixture.store.session().await.unwrap();
    // The initial write guard sees the real healthy preimage. The changed
    // report is visible only inside this exact writer transaction until its
    // commit guard invokes the complete occurrence collector and refuses it.
    writer
        .put(
            &fixture.evaluator,
            "artifact",
            &physical_id,
            fixture.evaluator.actor(),
            &altered,
        )
        .await
        .unwrap();
    let result = writer.commit().await;
    let after = catalog(&fixture.db_path);
    let refused = matches!(&result, Err(Error::ClockRegressed));
    let unchanged = before == after;
    let actual = json!({
        "intervention": label,
        "commit_actual_debug": format!("{result:?}"),
        "clock_refusal": refused,
        "full_catalog_exact_equal": unchanged,
        "before_catalog_digest": fingerprint(&before).unwrap(),
        "after_catalog_digest": fingerprint(&after).unwrap(),
    });
    // Observe both the real terminal result and all business/schema rows before
    // the semantic assertions. Do not reinterpret an earlier fixture refusal.
    println!("actual-collector-report-transaction {actual}");
    persist(
        &fixture,
        "collector-report-transaction-result.json",
        &actual,
    );
    assert!(
        refused,
        "collector did not refuse the changed report: {actual}"
    );
    assert!(
        unchanged,
        "the refused report transaction changed the full catalog: {actual}"
    );
    let mut reader = fixture.store.session().await.unwrap();
    let restored: FormalEnvelope = reader
        .need(&fixture.evaluator, "artifact", &physical_id)
        .await
        .unwrap();
    reader.commit().await.unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), original_value);
    // A refusal poisons that operation, not the restored healthy database or a
    // later read-only operation. No SQL repair or fixture rewrite is performed.
    assert_e05_collector_production_positive(&fixture, &formal, "collector-after-report-rollback")
        .await;
}

#[tokio::test]
async fn occurrence_collector_rejects_changed_real_report_mean_with_full_rollback() {
    assert_e05_collector_report_intervention_rolls_back(
        "collector-current-mean-rollback",
        E05CollectorReportIntervention::Mean,
    )
    .await;
}

#[tokio::test]
async fn occurrence_collector_rejects_changed_real_resource_digest_with_full_rollback() {
    assert_e05_collector_report_intervention_rolls_back(
        "collector-current-resource-digest-rollback",
        E05CollectorReportIntervention::ResourceEvidenceDigest,
    )
    .await;
}

#[tokio::test]
async fn occurrence_collector_keeps_first_operator_stop_reason_for_real_complete() {
    let fixture = fixture(
        "collector-current-first-stop-reason",
        RegisteredEvaluationControl::SCHEMA,
        false,
    )
    .await;
    stage_candidate(&fixture).await;
    let issued = issue_from_native_clock(&fixture).await;
    assert!(issued.candidate_dependency_binding.is_some());
    for i in 0..N {
        grade(&fixture, &format!("r{i}"), "wrong", i + 1).await;
    }
    grade(&fixture, "a0", "ok", N + 1).await;
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let stopped_at = sample_seconds();
    let stopped = fixture
        .store
        .stop_dispatch_group(
            &admin,
            BILLING_SCOPE,
            TICKET_ID,
            "operator_stop",
            stopped_at,
        )
        .await
        .unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.stop_reason.as_deref(), Some("operator_stop"));
    assert_eq!(stopped.stop_committed_at, Some(stopped_at));
    let formal = finalize_from_system(&fixture).await;
    let first_reason = fixture
        .store
        .dispatch_group(&admin, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fingerprint(&first_reason).unwrap(),
        fingerprint(&stopped).unwrap()
    );
    assert_e05_collector_production_positive(&fixture, &formal, "collector-after-first-stop").await;
}

// Lightweight source controls do not need all 122 complete-batch calls. They
// still issue a real bound ticket and persist a real original output/receipt
// while its dispatched charge is unresolved, then settle through the public API.
async fn assert_e05_collector_pending_receipt_then_real_settlement(uncertain: bool) {
    let case = if uncertain {
        "collector-current-uncertain-receipt"
    } else {
        "collector-current-dispatched-receipt"
    };
    let fixture = fixture(case, RegisteredEvaluationControl::SCHEMA, false).await;
    stage_candidate(&fixture).await;
    let issued = issue_from_native_clock(&fixture).await;
    assert!(issued.candidate_dependency_binding.is_some());
    let task = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        TICKET_ID,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let call_id = "pending-call";
    let reserved_at = sample_seconds();
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.into(),
                dispatch_group_id: TICKET_ID.into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: task.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "pending-lease".into(),
                // Preserve the original pending lease span: 1000 - 20 seconds.
                lease_until: reserved_at.checked_add(980).unwrap(),
                now: reserved_at,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: BILLING_SCOPE.into(),
        call_id: call_id.into(),
        actual_input_digest: task.request_digest.clone(),
        lease_token: "pending-lease".into(),
        lease_epoch: 1,
        now: sample_seconds(),
    };
    let dispatched = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: TICKET_ID.into(),
            target_id: "r0".into(),
            side: ExecutionSide::Candidate,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    if uncertain {
        let marked = fixture
            .store
            .mark_budget_call_uncertain(&fixture.worker, &fence, "fixture_usage_pending")
            .await
            .unwrap();
        assert_eq!(marked.state, BudgetCallState::Uncertain);
    }
    let output_utf8 = json!({"answer": "ok"}).to_string();
    let receipt_started_at = sample_seconds();
    let receipt = IndependentEvaluationControl::record_execution_receipt_now(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptIntent {
            receipt_id: "pending-receipt".into(),
            ticket_id: TICKET_ID.into(),
            target_id: "r0".into(),
            side: ExecutionSide::Candidate,
            request_digest: task.request_digest,
            output_artifact_id: "pending-output".into(),
            output_utf8: output_utf8.clone(),
            budget_call_id: call_id.into(),
            latency_micros: 123,
        },
    )
    .await
    .unwrap();
    let receipt_returned_at = sample_seconds();
    assert!(receipt_started_at <= receipt.issued_at_unix_seconds);
    assert!(receipt.issued_at_unix_seconds <= receipt_returned_at);
    assert_eq!(receipt.output_digest, hash(output_utf8.as_bytes()));
    let pending = fixture
        .store
        .budget_call(&fixture.worker, BILLING_SCOPE, call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pending.state,
        if uncertain {
            BudgetCallState::Uncertain
        } else {
            BudgetCallState::Dispatched
        }
    );
    assert_eq!(pending.actual_cost_micros, None);
    assert_eq!(pending.output_digest, None);
    assert_eq!(pending.finalized_at, None);
    assert!(!pending.execution_closed);
    let before_pending_read = catalog(&fixture.db_path);
    assert!(
        evo_storage::lifecycle::readonly_wall_seconds(&fixture.db_path, &[], &[]).unwrap()
            >= receipt.issued_at_unix_seconds
    );
    let after_pending_read = catalog(&fixture.db_path);
    assert!(
        before_pending_read == after_pending_read,
        "qualification of a real pending receipt wrote the full catalog"
    );

    let charge = UsageCharge {
        amount_micros: 5,
        currency: "USD".into(),
        pricing_version: "fixture-pricing-v1".into(),
        provider_request_id: "pending-provider-request".into(),
        usage_record_id: "pending-usage-record".into(),
        output_digest: receipt.output_digest.clone(),
    };
    let (finalized_at, closed_at) = if uncertain {
        // Keep the actual execution-close ACK while its charge is unresolved.
        // Only the subsequent Admin call reconciles the financial fact.
        let closed_at = sample_seconds();
        let closed = fixture
            .store
            .close_budget_call_execution(
                &fixture.worker,
                BILLING_SCOPE,
                call_id,
                dispatched.call.dispatch_id.as_deref().unwrap(),
                "fixture_execution_closed_before_usage",
                closed_at,
            )
            .await
            .unwrap();
        assert_eq!(closed.state, BudgetCallState::Uncertain);
        assert_eq!(closed.actual_cost_micros, None);
        assert_eq!(closed.execution_closed_at, Some(closed_at));
        let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
        let finalized_at = sample_seconds();
        let reconciled = fixture
            .store
            .reconcile_budget_call_cost(&admin, BILLING_SCOPE, call_id, &charge, finalized_at)
            .await
            .unwrap();
        assert_eq!(reconciled.finalized_at, Some(finalized_at));
        assert!(closed_at <= finalized_at);
        (
            reconciled.finalized_at.unwrap(),
            closed.execution_closed_at.unwrap(),
        )
    } else {
        // Preserve receipt -> financial-finalization ACK -> execution-close ACK.
        let mut final_fence = fence;
        let finalized_at = sample_seconds();
        final_fence.now = finalized_at;
        let finalized = fixture
            .store
            .finalize_budget_call(&fixture.worker, &final_fence, &charge)
            .await
            .unwrap();
        assert_eq!(finalized.finalized_at, Some(finalized_at));
        assert!(!finalized.execution_closed);
        let closed_at = sample_seconds();
        let closed = fixture
            .store
            .close_budget_call_execution(
                &fixture.worker,
                BILLING_SCOPE,
                call_id,
                dispatched.call.dispatch_id.as_deref().unwrap(),
                "fixture_complete",
                closed_at,
            )
            .await
            .unwrap();
        assert_eq!(closed.execution_closed_at, Some(closed_at));
        assert!(finalized_at <= closed_at);
        (
            finalized.finalized_at.unwrap(),
            closed.execution_closed_at.unwrap(),
        )
    };
    let final_call = fixture
        .store
        .budget_call(&fixture.worker, BILLING_SCOPE, call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_call.state, BudgetCallState::Finalized);
    assert_eq!(final_call.actual_cost_micros, Some(5));
    assert_eq!(
        final_call.output_digest.as_deref(),
        Some(receipt.output_digest.as_str())
    );
    assert!(final_call.execution_closed);
    assert_eq!(final_call.execution_closed_at, Some(closed_at));
    assert_eq!(final_call.finalized_at, Some(finalized_at));
    let last_terminal_event = if uncertain { finalized_at } else { closed_at };
    let before_settled_read = catalog(&fixture.db_path);
    assert!(
        evo_storage::lifecycle::readonly_wall_seconds(&fixture.db_path, &[], &[]).unwrap()
            >= last_terminal_event
    );
    let after_settled_read = catalog(&fixture.db_path);
    assert!(
        before_settled_read == after_settled_read,
        "qualification after the real later settlement wrote the full catalog"
    );
}

#[tokio::test]
async fn legacy_timed_issue_refuses_new_writes_without_catalog_change() {
    let fixture = fixture(
        "legacy-timed-issue-refusal",
        RegisteredEvaluationControl::SCHEMA,
        false,
    )
    .await;
    stage_candidate(&fixture).await;
    let before = catalog(&fixture.db_path);
    let result = IndependentEvaluationControl::issue_ticket(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketRequest {
            ticket_id: TICKET_ID.into(),
            registration_id: fixture.control.id.clone(),
            holdout_id: fixture.holdout.id.clone(),
            issued_at_unix_seconds: 5,
        },
    )
    .await;
    assert!(matches!(result, Err(Error::Invalid(ref message))
            if message == "legacy_timed_ticket_issue_disabled"));
    assert!(before == catalog(&fixture.db_path));
}

#[tokio::test]
async fn occurrence_collector_accepts_dispatched_real_receipt_before_later_finalize() {
    assert_e05_collector_pending_receipt_then_real_settlement(false).await;
}

#[tokio::test]
async fn occurrence_collector_accepts_uncertain_real_receipt_and_late_reconciliation() {
    assert_e05_collector_pending_receipt_then_real_settlement(true).await;
}

// Additional public-API regressions. These do not expose private prefix authority.
async fn r60_r63_real_pair(case: &str) -> (Fixture, ExecutionReceiptIntent, GraderReceiptIntent) {
    let fx = fixture(case, RegisteredEvaluationControl::SCHEMA, false).await;
    stage_candidate(&fx).await;
    assert!(
        issue_from_native_clock(&fx)
            .await
            .candidate_dependency_binding
            .is_some()
    );
    let task = IndependentEvaluationControl::broker_task(
        &fx.worker,
        &fx.store,
        TICKET_ID,
        "r0",
        ExecutionSide::Candidate,
    )
    .await
    .unwrap();
    let candidate = execute(&fx, "r0", ExecutionSide::Candidate, "ok", 1).await;
    let baseline = execute(&fx, "r0", ExecutionSide::Baseline, "ok", 2).await;
    let execution = ExecutionReceiptIntent {
        receipt_id: candidate.clone(),
        ticket_id: TICKET_ID.into(),
        target_id: "r0".into(),
        side: ExecutionSide::Candidate,
        request_digest: task.request_digest,
        output_artifact_id: "output-1".into(),
        output_utf8: json!({"answer": "ok"}).to_string(),
        budget_call_id: "call-1".into(),
        latency_micros: 101,
    };
    let grade = GraderReceiptIntent {
        receipt_id: "r60-r63-grader".into(),
        ticket_id: TICKET_ID.into(),
        target_id: "r0".into(),
        candidate_execution_receipt_id: candidate,
        baseline_execution_receipt_id: baseline,
    };
    (fx, execution, grade)
}

#[tokio::test]
async fn r60_r63_real_ack_rechecks_executor_and_same_id_evidence() {
    let (fx, execution, _) = r60_r63_real_pair("r60-r63-execution-ack").await;
    let before = catalog(&fx.db_path);
    let original = IndependentEvaluationControl::record_execution_receipt_now(
        &fx.worker,
        &fx.store,
        execution.clone(),
    )
    .await
    .unwrap();
    assert!(
        before == catalog(&fx.db_path),
        "exact receipt replay wrote the full catalog"
    );
    let other = Context::new(NAMESPACE, "r60-r63-other-executor", Role::Worker).unwrap();
    let denied = IndependentEvaluationControl::record_execution_receipt_now(
        &other,
        &fx.store,
        execution.clone(),
    )
    .await;
    assert!(matches!(denied, Err(Error::Forbidden)));
    assert!(
        before == catalog(&fx.db_path),
        "wrong executor wrote the full catalog"
    );
    let mut changed = execution.clone();
    changed.output_utf8 = json!({"answer": "changed"}).to_string();
    let denied =
        IndependentEvaluationControl::record_execution_receipt_now(&fx.worker, &fx.store, changed)
            .await;
    assert!(matches!(denied, Err(Error::Conflict(ref reason))
        if reason == "execution receipt id reused with different evidence"));
    assert!(
        before == catalog(&fx.db_path),
        "changed evidence wrote the full catalog"
    );
    let replay = IndependentEvaluationControl::record_execution_receipt_now(
        &fx.worker, &fx.store, execution,
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(&original).unwrap(),
        fingerprint(&replay).unwrap()
    );
    assert!(
        before == catalog(&fx.db_path),
        "healthy replay after refusal wrote the full catalog"
    );
    fx.store.close().await;
}

#[tokio::test]
async fn r60_r63_real_grade_ack_rechecks_authority_and_frozen_pair() {
    let (fx, _, intent) = r60_r63_real_pair("r60-r63-grade-ack").await;
    let before = catalog(&fx.db_path);
    let other = Context::new(NAMESPACE, "r60-r63-other-evaluator", Role::Evaluator).unwrap();
    let denied =
        IndependentEvaluationControl::record_grader_receipt_now(&other, &fx.store, intent.clone())
            .await;
    assert!(matches!(denied, Err(Error::Forbidden)));
    assert!(
        before == catalog(&fx.db_path),
        "wrong evaluator wrote the full catalog"
    );
    let graded = IndependentEvaluationControl::record_grader_receipt_now(
        &fx.evaluator,
        &fx.store,
        intent.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        graded.candidate_execution_receipt_id,
        intent.candidate_execution_receipt_id
    );
    assert_eq!(
        graded.baseline_execution_receipt_id,
        intent.baseline_execution_receipt_id
    );
    assert_eq!(graded.evaluator_actor, fx.evaluator.actor());
    let after = catalog(&fx.db_path);
    assert!(
        before != after,
        "new real grader receipt produced no persisted change"
    );
    let mut changed = intent.clone();
    std::mem::swap(
        &mut changed.candidate_execution_receipt_id,
        &mut changed.baseline_execution_receipt_id,
    );
    let denied =
        IndependentEvaluationControl::record_grader_receipt_now(&fx.evaluator, &fx.store, changed)
            .await;
    assert!(matches!(denied, Err(Error::Conflict(ref reason))
        if reason == "execution receipts do not form the frozen candidate/baseline pair"));
    assert!(
        after == catalog(&fx.db_path),
        "wrong frozen pair wrote the full catalog"
    );
    let replay =
        IndependentEvaluationControl::record_grader_receipt_now(&fx.evaluator, &fx.store, intent)
            .await
            .unwrap();
    assert_eq!(fingerprint(&graded).unwrap(), fingerprint(&replay).unwrap());
    assert!(
        after == catalog(&fx.db_path),
        "exact grade replay wrote the full catalog"
    );
    assert!(
        evo_storage::lifecycle::readonly_wall_seconds(&fx.db_path, &[], &[]).unwrap()
            >= graded.issued_at_unix_seconds
    );
    assert!(
        after == catalog(&fx.db_path),
        "post-grade qualification wrote the full catalog"
    );
    fx.store.close().await;
}

// R24-R1 Source candidate controls. No old fixture/test/assertion is replaced.
// This helper only decodes the existing full read-only catalog; no write/repair.
fn r24_r1_catalog_rows(catalog: &Value, name: &str) -> Vec<BTreeMap<String, Value>> {
    let table = catalog["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|table| table[0] == name)
        .unwrap();
    let columns: Vec<_> = table[1]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    table[2]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let cells: Vec<Value> = serde_json::from_str(row.as_str().unwrap()).unwrap();
            assert_eq!(cells.len(), columns.len());
            columns
                .iter()
                .zip(cells)
                .map(|(column, cell)| {
                    assert_eq!(cell.as_array().unwrap().len(), 2);
                    ((*column).to_owned(), cell[1].clone())
                })
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn native_complete_unattributed_commit_preserves_facts_then_reconcile_is_catalog_noop() {
    let fixture = fixture(
        "r24-r1-unattributed-commit-and-noop",
        RegisteredEvaluationControl::SCHEMA,
        false,
    )
    .await;
    stage_candidate(&fixture).await;
    let issued = issue_from_native_clock(&fixture).await;
    assert!(issued.candidate_dependency_binding.is_some());
    for i in 0..N {
        grade(&fixture, &format!("r{i}"), "wrong", i + 1).await;
    }
    grade(&fixture, "a0", "ok", N + 1).await;
    // This is an actual additional dispatch in the same authorized group,
    // deliberately absent from every execution receipt/target. No row injection.
    let call_id = "r24-r1-unattributed";
    let reserved_at = sample_seconds();
    let input = digest("r24-r1-unattributed-input");
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.into(),
                dispatch_group_id: TICKET_ID.into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: input.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "r24-r1-unattributed-lease".into(),
                lease_until: reserved_at.checked_add(1_000).unwrap(),
                now: reserved_at,
            },
        )
        .await
        .unwrap();
    let dispatched = fixture
        .store
        .begin_budget_dispatch(
            &fixture.worker,
            &BudgetCallFence {
                billing_scope: BILLING_SCOPE.into(),
                call_id: call_id.into(),
                actual_input_digest: input,
                lease_token: "r24-r1-unattributed-lease".into(),
                lease_epoch: 1,
                now: sample_seconds(),
            },
        )
        .await
        .unwrap();
    assert!(dispatched.new_dispatch);
    assert_eq!(dispatched.call.state, BudgetCallState::Dispatched);
    assert!(!dispatched.call.execution_closed);
    assert_eq!(dispatched.call.actual_cost_micros, None);
    let root_before = fixture
        .store
        .root_budget(&fixture.worker, BILLING_SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root_before.reserved_micros, 100);
    let mut read = fixture.store.session().await.unwrap();
    let calls_before = read
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap();
    assert_eq!(calls_before.len(), 2 * (N as usize + 1) + 1);
    let ledger_id = format!(
        "e05-{}",
        fingerprint(&("exposure_ledger_v1", &issued.research_family_id)).unwrap()
    );
    let ledger_before: Value = read
        .need(&fixture.evaluator, "artifact", &ledger_id)
        .await
        .unwrap();
    read.commit().await.unwrap();
    let before = catalog(&fixture.db_path);
    let before_reports:Vec<_>=r24_r1_catalog_rows(&before,"objects").into_iter().filter(|row|{
        row["namespace"]==NAMESPACE && row["kind"]=="artifact"
            && serde_json::from_str::<Value>(row["body"].as_str().unwrap()).unwrap()["record_kind"]
                ==evo_core::formal_version::KIND
    }).collect();
    assert!(before_reports.is_empty());

    let outcome = fixture
        .store
        .session()
        .await
        .unwrap()
        .begin_e05_complete(
            fixture.evaluator.clone(),
            json!({"schema_version":"rsia.e05_complete_intent.v2","ticket_id":TICKET_ID}),
        )
        .unwrap()
        .finish_outcome()
        .await;
    let failure = outcome.result.as_ref().unwrap_err();
    assert!(matches!(&failure.error,Error::Invalid(reason)
        if reason=="ticket dispatch group contains unattributed calls"));
    assert!(failure.commit_confirmed);
    assert!(!failure.commit_outcome_unknown);
    assert!(failure.rollback_error.is_none());
    assert!(
        outcome.native.commit_admitted
            && outcome.native.commit_attempted
            && outcome.native.commit_confirmed
            && !outcome.native.terminal_unconfirmed
    );
    assert_eq!(outcome.native.commit_command_sent, Some(true));
    assert!(!outcome.native.rollback_command_acknowledged && !outcome.native.rollback_confirmed);
    let after = catalog(&fixture.db_path);
    assert_ne!(before, after);
    // Conservation covers all typed fields of all 123 actual calls, including
    // the unresolved additional one, rather than a filtered financial summary.
    let mut read = fixture.store.session().await.unwrap();
    let calls_after = read
        .budget_calls_for_group(&fixture.evaluator, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap();
    assert_eq!(calls_after, calls_before);
    let ledger_after: Value = read
        .need(&fixture.evaluator, "artifact", &ledger_id)
        .await
        .unwrap();
    assert_eq!(ledger_after, ledger_before);
    let ticket_physical = format!(
        "e05-{}",
        fingerprint(&("evaluation_ticket_v2", TICKET_ID)).unwrap()
    );
    let ticket: Value = read
        .need(&fixture.evaluator, "artifact", &ticket_physical)
        .await
        .unwrap();
    read.commit().await.unwrap();
    assert_eq!(ticket["payload"]["lifecycle"], "invalid");
    assert_eq!(
        ticket["payload"]["unattributed_budget_call_ids"],
        json!([call_id])
    );
    assert!(
        ticket["payload"]["invalid_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "unattributed_dispatch_group_call")
    );
    let entry = ledger_after["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["ticket_id"] == TICKET_ID)
        .unwrap();
    assert_eq!(entry["monetary_state"], "dispatched_cost_pending");
    assert_eq!(entry["state"]["state"], "feedback_used");
    let root_after = fixture
        .store
        .root_budget(&fixture.worker, BILLING_SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&root_after).unwrap(),
        serde_json::to_value(&root_before).unwrap()
    );
    let group = fixture
        .store
        .dispatch_group(&fixture.worker, BILLING_SCOPE, TICKET_ID)
        .await
        .unwrap()
        .unwrap();
    assert!(group.stopped && group.stop_committed_at.is_some());
    assert_eq!(
        group.stop_reason.as_deref(),
        Some("unattributed_dispatch_group_call")
    );

    let audit = r24_r1_catalog_rows(&after, "audit");
    let bindings: Vec<_> = audit
        .iter()
        .filter_map(|row| {
            let payload: Value = serde_json::from_str(row["payload"].as_str().unwrap()).unwrap();
            (row["namespace"] == NAMESPACE
                && payload["operation"] == "formal.native.report_binding.v1")
                .then_some((row, payload))
        })
        .collect();
    assert_eq!(bindings.len(), 1);
    let (binding_row, payload) = &bindings[0];
    let binding: Value = serde_json::from_str(payload["entity"].as_str().unwrap()).unwrap();
    let reference: evo_core::formal_version::ReportRef =
        serde_json::from_value(binding["report_ref"].clone()).unwrap();
    reference.validate_wire().unwrap();
    assert_eq!(reference.namespace, NAMESPACE);
    assert_eq!(reference.owner, fixture.evaluator.actor());
    assert_eq!(reference.ticket_id, TICKET_ID);
    assert_eq!(
        binding["summary"]["operation"],
        "formal.native.asof.complete.v1"
    ); // request
    assert_eq!(binding_row["previous_hash"], binding["asof_digest"]);
    let asof = audit
        .iter()
        .find(|row| row["seq"] == binding["asof_sequence"])
        .unwrap();
    assert_eq!(asof["digest"], binding["asof_digest"]);
    assert_eq!(
        hash(
            format!(
                "{}\n{}",
                asof["previous_hash"].as_str().unwrap(),
                asof["payload"].as_str().unwrap()
            )
            .as_bytes()
        ),
        asof["digest"]
    );
    assert_eq!(
        hash(
            format!(
                "{}\n{}",
                binding_row["previous_hash"].as_str().unwrap(),
                binding_row["payload"].as_str().unwrap()
            )
            .as_bytes()
        ),
        binding_row["digest"]
    );
    let objects = r24_r1_catalog_rows(&after, "objects");
    let reports: Vec<_> = objects
        .iter()
        .filter(|row| {
            row["namespace"] == NAMESPACE
                && row["kind"] == "artifact"
                && row["id"] == reference.physical_id
        })
        .collect();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["revision"], 1);
    let carrier: Value = serde_json::from_str(reports[0]["body"].as_str().unwrap()).unwrap();
    assert_eq!(carrier["record_kind"], evo_core::formal_version::KIND);
    assert_eq!(carrier["id"], reference.physical_id);
    assert_eq!(carrier["input"]["operation"], "invalidate"); // actual plan effect
    assert_eq!(carrier["input"]["ticket_snapshot"], ticket["payload"]);
    let saved_ticket: evo_engine::streaming_evaluator::EvaluationTicketV2 =
        serde_json::from_value(carrier["input"]["ticket_snapshot"].clone()).unwrap();
    assert_eq!(fingerprint(&saved_ticket).unwrap(), reference.ticket_digest);
    let formal: FormalEvaluationV2 = serde_json::from_value(carrier["payload"].clone()).unwrap();
    assert!(
        matches!(&formal,FormalEvaluationV2::Invalid {ticket_id,reasons,..}
        if ticket_id==TICKET_ID && reasons==&saved_ticket.invalid_reasons)
    );
    assert_eq!(fingerprint(&formal).unwrap(), reference.payload_digest);
    let old_physical = format!("e05-{}", fingerprint(&(FORMAL_KIND, TICKET_ID)).unwrap());
    assert!(!objects.iter().any(|row| row["namespace"] == NAMESPACE
        && row["kind"] == "artifact"
        && row["id"] == old_physical));
    let saved = carrier["input"]["calls"].as_array().unwrap();
    assert_eq!(saved.len(), calls_before.len());
    for actual in &calls_before {
        let call = saved
            .iter()
            .find(|call| call["call_id"] == actual.call_id)
            .unwrap();
        let frozen = &call["snapshot"];
        assert_eq!(frozen["original_call_digest"], fingerprint(actual).unwrap());
        let mut reconstructed = frozen.clone();
        let fields = reconstructed.as_object_mut().unwrap();
        fields.remove("schema_version");
        fields.remove("original_call_digest");
        let actual_value = serde_json::to_value(actual).unwrap();
        for (field, slot) in [
            ("request_artifact", "request"),
            ("transport_artifact", "transport"),
            ("response_artifact", "response"),
        ] {
            let artifact = &actual_value[field];
            if artifact.is_null() {
                assert!(frozen[field].is_null());
            } else {
                assert_eq!(frozen[field]["namespace"], actual.namespace);
                assert_eq!(frozen[field]["billing_scope"], actual.billing_scope);
                assert_eq!(frozen[field]["call_id"], actual.call_id);
                assert_eq!(frozen[field]["slot"], slot);
                assert_eq!(frozen[field]["schema_version"], artifact["schema_version"]);
                assert_eq!(frozen[field]["digest"], artifact["digest"]);
                assert_eq!(
                    frozen[field]["body_bytes"].as_u64().unwrap(),
                    artifact["body"].as_str().unwrap().len() as u64
                );
            }
            fields.insert(field.into(), artifact.clone());
        }
        let reconstructed: evo_storage::budget::BudgetCallRecord =
            serde_json::from_value(reconstructed).unwrap();
        assert_eq!(&reconstructed, actual); // complete original typed 29+3
        assert_eq!(
            fingerprint(&reconstructed).unwrap(),
            frozen["original_call_digest"]
        );
    }
    assert_eq!(after, catalog(&fixture.db_path)); // all diagnostic reads are unchanged
    let before_noop = catalog(&fixture.db_path);
    let noop = fixture
        .store
        .session()
        .await
        .unwrap()
        .begin_e05_reconcile(
            fixture.evaluator.clone(),
            json!({"schema_version":"rsia.e05_reconcile_intent.v2","ticket_id":TICKET_ID}),
        )
        .unwrap()
        .finish_outcome()
        .await;
    assert_eq!(noop.result.unwrap(), ticket["payload"]);
    assert!(
        noop.native.commit_admitted
            && noop.native.commit_attempted
            && noop.native.commit_confirmed
            && !noop.native.terminal_unconfirmed
    );
    assert!(!noop.native.rollback_command_acknowledged && !noop.native.rollback_confirmed);
    assert_eq!(before_noop, catalog(&fixture.db_path)); // no audit/report/time rewrite
    if carrier["engine_version"] == evo_core::formal_version::ENGINE_V4 {
        let readonly_before = catalog(&fixture.db_path);
        let readonly = fixture
            .store
            .read_formal_snapshot(
                &fixture.evaluator,
                evo_storage::FormalReadSelectorR38::Ticket(TICKET_ID.into()),
            )
            .await
            .unwrap();
        let source = readonly.formal().unwrap();
        assert!(
            source.current_complete().is_none(),
            "Invalid cannot acquire Complete capability"
        );
        let current = source
            .current_formal()
            .expect("actual Invalid must receive a general native seal");
        assert_eq!(current.reference(), &reference);
        assert_eq!(current.payload(), &serde_json::to_value(&formal).unwrap());
        assert_eq!(
            current.frozen_ticket().unwrap(),
            carrier["input"]["ticket_snapshot"]
        );
        assert!(readonly_before == catalog(&fixture.db_path));
    }
    // Optional capture-only material: ordinary test execution has no export side effect.
    if let Some(r36_export_root) = std::env::var_os("RSIA_AG132_ORIGINAL_RECORD_DIR") {
        let r36_export_python = r#"
import gzip,hashlib,json,os,stat,sys
mode,root,expected_json=sys.argv[1:]
assert mode in ('body','proof')
assert os.path.isabs(root) and root==os.path.normpath(root) and root!='/'
parts=root.split('/')[1:]
assert parts and all(p and p not in ('.','..') for p in parts)
flags=os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW
def root_identity(s): return [s.st_dev,s.st_ino,s.st_uid,stat.S_IMODE(s.st_mode)]
def open_root():
    fd=os.open('/',flags)
    try:
        for part in parts:
            nxt=os.open(part,flags,dir_fd=fd)
            os.close(fd);fd=nxt
        s=os.fstat(fd)
        assert stat.S_ISDIR(s.st_mode) and s.st_uid==os.geteuid() and stat.S_IMODE(s.st_mode)==0o700
        return fd,root_identity(s)
    except BaseException:
        os.close(fd);raise
rootfd,identity=open_root()
try:
    expected=json.loads(expected_json)
    if mode=='body': assert expected is None
    else: assert expected==identity
    cap=4*1024*1024 if mode=='body' else 256*1024
    original=sys.stdin.buffer.read(cap+1)
    assert len(original)<=cap, 'material_failed: original input cap'
    if mode=='body':
        original.decode('utf-8')
        saved=gzip.compress(original,mtime=0)
        assert len(saved)<=1000000, 'material_failed: export gzip cap (not a product limit)'
        name='original-formal-record.json.gz'
    else:
        json.loads(original)
        saved=original
        name='actual-original-record-proof.json'
    checkfd,now=open_root()
    try: assert now==identity==root_identity(os.fstat(rootfd))
    finally: os.close(checkfd)
    fd=os.open(name,os.O_RDWR|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=rootfd)
    try:
        os.fchmod(fd,0o600)
        s=os.fstat(fd)
        assert stat.S_ISREG(s.st_mode) and s.st_nlink==1 and s.st_uid==os.geteuid() and stat.S_IMODE(s.st_mode)==0o600
        pending=memoryview(saved)
        while pending:
            n=os.write(fd,pending)
            assert n>0
            pending=pending[n:]
        os.fsync(fd);os.lseek(fd,0,os.SEEK_SET)
        readback=bytearray()
        while True:
            b=os.read(fd,65536)
            if not b: break
            readback.extend(b)
            assert len(readback)<=len(saved)
        assert bytes(readback)==saved
        s=os.fstat(fd);p=os.stat(name,dir_fd=rootfd,follow_symlinks=False)
        def file_state(v): return [v.st_dev,v.st_ino,v.st_mode,v.st_nlink,v.st_uid,v.st_gid,v.st_size,v.st_mtime_ns,v.st_ctime_ns]
        assert file_state(s)==file_state(p) and stat.S_ISREG(s.st_mode) and s.st_nlink==1
        assert s.st_uid==os.geteuid() and stat.S_IMODE(s.st_mode)==0o600
    finally: os.close(fd)
    os.fsync(rootfd)
    checkfd,now=open_root()
    try: assert now==identity==root_identity(os.fstat(rootfd))
    finally: os.close(checkfd)
    print(json.dumps({'schema':'Root.AG132.R36.original-export-file.v1','mode':mode,
        'directory_identity':identity,'name':name,'source_bytes':len(original),
        'source_sha256':hashlib.sha256(original).hexdigest(),'saved_bytes':len(saved),
        'saved_sha256':hashlib.sha256(saved).hexdigest(),'file_dev':s.st_dev,'file_ino':s.st_ino,
        'file_uid':s.st_uid,'file_gid':s.st_gid,'file_mode':stat.S_IMODE(s.st_mode),
        'fsync_readback':True},sort_keys=True,separators=(',',':')))
finally: os.close(rootfd)
"#;
        let r36_export =
            |mode: &str, input: &[u8], expected: &Value| -> std::result::Result<Value, String> {
                let expected = serde_json::to_string(expected).map_err(|e| e.to_string())?;
                let mut child = Command::new("python3")
                    .args(["-I", "-B", "-c", r36_export_python, mode])
                    .arg(&r36_export_root)
                    .arg(expected)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| e.to_string())?;
                // Drop stdin before waiting, and report its error only after both output pipes drain.
                let input_error = match child.stdin.take() {
                    Some(mut pipe) => pipe.write_all(input).err().map(|e| e.to_string()),
                    None => Some("export child stdin missing".to_owned()),
                };
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("export child wait/drain: {e}"))?;
                let stderr_error = std::io::stderr().lock().write_all(&output.stderr).err();
                let hex = |bytes: &[u8]| {
                    let mut text = String::with_capacity(bytes.len() * 2);
                    for byte in bytes {
                        std::fmt::Write::write_fmt(&mut text, format_args!("{byte:02x}")).unwrap();
                    }
                    text
                };
                let failure = |why: &str| {
                    format!(
                        "material_failed: {mode}: {why}; status={}; stdout_hex={}; stderr_hex={}",
                        output.status,
                        hex(&output.stdout),
                        hex(&output.stderr)
                    )
                };
                if let Some(error) = input_error {
                    return Err(failure(&format!("stdin: {error}")));
                }
                if !output.status.success() {
                    return Err(failure("export child exit"));
                }
                if let Some(error) = stderr_error {
                    return Err(failure(&format!("stderr forwarding: {error}")));
                }
                serde_json::from_slice(&output.stdout)
                    .map_err(|e| failure(&format!("receipt JSON: {e}")))
            };
        let r36_body_result = (|| -> std::result::Result<Value, String> {
            if fixture._temporary.is_none() || fixture.artifacts.is_some() {
                return Err(
                    "material_failed: capture requires the ordinary TempDir fixture".into(),
                );
            }
            let raw = reports[0]["body"]
                .as_str()
                .ok_or("material_failed: raw row body is not UTF-8 TEXT")?
                .as_bytes();
            if raw.len() > 4 * 1024 * 1024 {
                return Err("material_failed: original 4MiB body bound".into());
            }
            let receipt = r36_export("body", raw, &Value::Null)?;
            if receipt["source_bytes"] != json!(raw.len()) || receipt["source_sha256"] != hash(raw)
            {
                return Err("material_failed: raw body export receipt mismatch".into());
            }
            Ok(receipt)
        })();
        // This API returns unit. Its return is not a SQLite close ACK or retained-DB proof.
        fixture.store.close().await;
        let r36_material_result = (|| -> std::result::Result<Value, String> {
            let body_export = r36_body_result?;
            let directory_identity = body_export["directory_identity"].clone();
            let mut object_row_without_body = (*reports[0]).clone();
            object_row_without_body.remove("body");
            let proof = json!({
                "schema_version":"Root.AG132.R36.original-record-capture.v1",
                "producer_test":"native_complete_unattributed_commit_preserves_facts_then_reconcile_is_catalog_noop",
                "credit":"capture_only_no_Current_first_or_reader_acceptance",
                "record_bytes_are_original_objects_body_utf8":true,"body_export":body_export,
                "report_ref":&reference,"object_row_without_body":object_row_without_body,
                "binding_row":binding_row,"binding_entity":&binding,"asof_row":asof,
                "business_error":format!("{}",failure.error),
                "transaction_failure":{"commit_confirmed":failure.commit_confirmed,
                    "commit_outcome_unknown":failure.commit_outcome_unknown,
                    "rollback_error":failure.rollback_error.as_ref().map(|e| format!("{e}"))},
                "actual_e05_owner_native_facts":{"commit_admitted":outcome.native.commit_admitted,
                    "commit_attempted":outcome.native.commit_attempted,"commit_command_sent":outcome.native.commit_command_sent,
                    "commit_confirmed":outcome.native.commit_confirmed,
                    "rollback_command_acknowledged":outcome.native.rollback_command_acknowledged,
                    "rollback_confirmed":outcome.native.rollback_confirmed,
                    "native_close_confirmed":outcome.native.native_close_confirmed,
                    "terminal_unconfirmed":outcome.native.terminal_unconfirmed},
                "native_close_field_is_the_preexisting_E05_owner_fact_not_Store_close":true,
                "store_close_api_returned":true,"store_close_api_returns_unit":true,
                "database_exported":false,"fixture_uses_ordinary_TempDir":true,
                "N":N,"actual_group_call_count":calls_before.len(),
                "all_original_assertions_completed_before_capture":true,
                "observed_wire":{"schema_version":&carrier["schema_version"],"record_kind":&carrier["record_kind"],
                    "engine_version":&carrier["engine_version"],"input_schema_version":&carrier["input_schema_version"],
                    "inner_input_schema_version":&carrier["input"]["schema_version"]}
            });
            let bytes = serde_json::to_vec(&proof).map_err(|e| e.to_string())?;
            if bytes.len() > 256 * 1024 {
                return Err("material_failed: capture proof cap".into());
            }
            r36_export("proof", &bytes, &directory_identity)
        })();
        assert!(
            r36_material_result.is_ok(),
            "material_failed; partial original files retained: {r36_material_result:?}"
        );
    }
}

// Native R39 controls reuse the same real N=60 producer; no DTO or fabricated
// audit/Current result is supplied to the public readonly entry.
async fn r39_append_real_shared_exposure_ticket(fixture: &Fixture) {
    let mut control = fixture.control.clone();
    control.id = "r39-second-control".into();
    control.attempt_id = "attempt-2".into();
    control.fixed_sample_claim_id = "r39-second-fixed".into();
    control.v1_plan_snapshot.id = "r39-second-plan".into();
    control.v1_plan_snapshot.frozen_at = Some(sample_seconds());
    control.created_at_unix_seconds = sample_seconds();
    control.formal_plan = FormalExperimentPlanV41::new(
        &control.v1_plan_snapshot,
        digest("r39-second-preregistration"),
        &control.alpha_plan,
        &control.optimizer_comparison,
        &control.optimization_budget,
        control.anchor_matrix.digest().unwrap(),
        digest("r39-second-sampling"),
        digest("r39-second-common-mean"),
        FormalStatisticalUnit::IndependentTaskCluster,
        N,
    )
    .unwrap();
    let pair = FrozenCandidatePair::new(
        control.v1_plan_snapshot.digest().unwrap(),
        fixture.holdout.pair.candidate_digest.clone(),
        fixture.holdout.pair.baseline_digest.clone(),
        sample_millis(),
    )
    .unwrap();
    let ordered: Vec<_> = (0..N).map(|i| format!("r39-other-cluster-{i}")).collect();
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        control.alpha_plan.research_family_id.clone(),
        control.alpha_plan.digest().unwrap(),
        SequentialMode::Disabled,
        "r39-other-epoch",
        "r39-other-slice",
        ordered.clone(),
        ordered
            .iter()
            .map(|cluster| digest(&format!("r39-source-{cluster}")))
            .collect(),
        "fixture-sampler-v1",
        control.oracle_digest.clone(),
        control.anchor_matrix.digest().unwrap(),
        control.grader_digest.clone(),
        fixture.holdout.manifest.environment_digest.clone(),
        digest("r39-other-private-seed"),
        sample_millis(),
    )
    .unwrap();
    let mut holdout = fixture.holdout.clone();
    holdout.id = "r39-second-holdout".into();
    holdout.registration_id = control.id.clone();
    holdout.pair = pair;
    holdout.manifest = manifest;
    for (input, cluster) in holdout.rotating_inputs.iter_mut().zip(ordered) {
        input.cluster_or_anchor_id = cluster;
        input.input_artifact_id = format!("r39-other-{}", input.input_artifact_id);
        input.input_digest = digest(&input.input_artifact_id);
    }
    for input in &mut holdout.anchor_inputs {
        input.input_artifact_id = format!("r39-other-{}", input.input_artifact_id);
        input.input_digest = digest(&input.input_artifact_id);
    }
    IndependentEvaluationControl::register_control(
        &fixture.evaluator,
        &fixture.store,
        control.clone(),
    )
    .await
    .unwrap();
    IndependentEvaluationControl::register_holdout(
        &fixture.evaluator,
        &fixture.store,
        holdout.clone(),
    )
    .await
    .unwrap();
    let ticket = IndependentEvaluationControl::issue_ticket_now(
        &fixture.evaluator,
        &fixture.store,
        IssueTicketIntent {
            ticket_id: "r39-second-ticket".into(),
            registration_id: control.id,
            holdout_id: holdout.id,
        },
    )
    .await
    .unwrap();
    assert_eq!(ticket.id, "r39-second-ticket");
    let before = catalog(&fixture.db_path);
    let running = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            evo_storage::FormalReadSelectorR38::Ticket(ticket.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(running.live_ticket().unwrap()["id"], ticket.id);
    assert!(running.formal().is_none());
    assert!(
        before == catalog(&fixture.db_path),
        "running readonly capture changed catalog"
    );
}

async fn r39_native_current_and_late_overlay_controls(
    fixture: &Fixture,
    formal: &FormalEvaluationV2,
) {
    use evo_core::formal_version as fv;
    use evo_storage::FormalReadSelectorR38;
    let before = catalog(&fixture.db_path);
    let read = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    assert_eq!(read.namespace(), NAMESPACE);
    assert_eq!(read.logical_ticket_id(), TICKET_ID);
    assert_eq!(read.live_ticket().unwrap()["id"], TICKET_ID);
    let source = read.formal().unwrap();
    let current = source.current_complete().unwrap();
    let reference = current.reference().clone();
    assert_eq!(reference.engine_version, fv::ENGINE_V4);
    assert_eq!(reference.input_schema_version, fv::INPUT_SCHEMA_V2);
    assert_ne!(reference.physical_id, reference.ticket_id);
    assert_eq!(current.payload(), &serde_json::to_value(formal).unwrap());
    assert_eq!(current.original_calls().len(), 2 * (N as usize + 1));
    assert_eq!(current.paid_overlay().len(), 2 * (N as usize + 1));
    let hidden = format!(
        "e05-{}",
        fingerprint(&("protected_holdout_v41", &fixture.holdout.id)).unwrap()
    );
    assert!(read.records().get("artifact", &hidden).is_none());
    let view = IndependentEvaluationControl::verified_report_view_current(
        &fixture.evaluator,
        &fixture.store,
        FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference.clone())),
    )
    .await
    .unwrap();
    assert_eq!(view.schema_version, fv::VERIFIED_FORMAL_VIEW_V3);
    assert_eq!(
        view.report_ref,
        fv::AnyReportRef::VersionedV3(reference.clone())
    );
    assert_eq!(view.ticket_id, TICKET_ID);
    assert!(!view.source_legacy);
    assert_eq!(view.read_state, "current_verified");
    assert!(!view.view.promotion_eligible);
    assert!(
        read.host_evidence().unwrap().is_none(),
        "ProgramFixture content cannot become FullHost authority"
    );
    let mut wrong_body = reference.clone();
    wrong_body.carrier_digest = "0".repeat(64);
    assert!(
        IndependentEvaluationControl::verified_report_view_current(
            &fixture.evaluator,
            &fixture.store,
            FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(wrong_body))
        )
        .await
        .is_err()
    );
    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let reviewed = evo_engine::review::review_stored(
        &admin,
        &fixture.store,
        evo_engine::review::ReviewRequest {
            kind: evo_engine::review::ReviewKind::FormalReport,
            id: TICKET_ID.into(),
            detail: evo_engine::review::ReviewDetail::Metadata,
        },
    )
    .await
    .unwrap();
    assert_eq!(reviewed.schema_version, "rsia.review_response.v3");
    assert!(reviewed.payload.is_some());

    let mut attempted = fixture.store.session().await.unwrap();
    assert!(matches!(
        attempted
            .audit(
                &fixture.evaluator,
                "formal.native.consumer.record.v1",
                "{\"schema_version\":\"rsia.native.consumer_record_origin.v1\"}"
            )
            .await,
        Err(Error::Forbidden)
    ));
    attempted.commit().await.unwrap();
    assert_eq!(
        before,
        catalog(&fixture.db_path),
        "public audit cannot mint a native consumer origin"
    );
    let original_body = source.raw_body().to_owned();
    let original_payload = current.payload().clone();
    let original_root = current.frozen_root().clone();
    let exact = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference.clone())),
        )
        .await
        .unwrap();
    assert_eq!(
        exact
            .formal()
            .unwrap()
            .current_complete()
            .unwrap()
            .reference(),
        &reference
    );
    let mut wrong = reference.clone();
    wrong.ticket_id = "r39-other-logical-ticket".into();
    assert!(
        fixture
            .store
            .read_formal_snapshot(
                &fixture.evaluator,
                FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(wrong)),
            )
            .await
            .is_err()
    );
    assert!(
        before == catalog(&fixture.db_path),
        "readonly source controls wrote catalog"
    );

    r39_append_real_shared_exposure_ticket(fixture).await;
    let after_shared = catalog(&fixture.db_path);
    assert_ne!(before, after_shared);
    let read = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    let current = read.formal().unwrap().current_complete().unwrap();
    assert_eq!(read.formal().unwrap().raw_body(), original_body);
    assert_eq!(current.payload(), &original_payload);
    assert_eq!(current.reference(), &reference);
    assert!(
        after_shared == catalog(&fixture.db_path),
        "shared exposure readonly capture wrote"
    );

    let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
    let stopped = fixture
        .store
        .stop_root_budget(
            &admin,
            BILLING_SCOPE,
            "r39-later-root-close",
            sample_seconds(),
        )
        .await
        .unwrap();
    assert!(stopped.stopped);
    let after_close = catalog(&fixture.db_path);
    let read = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    let current = read.formal().unwrap().current_complete().unwrap();
    assert_eq!(current.reference(), &reference);
    assert_eq!(current.payload(), &original_payload);
    assert_eq!(current.frozen_root(), &original_root);
    assert!(
        after_close == catalog(&fixture.db_path),
        "closed-root readonly capture wrote"
    );
    let late_view = IndependentEvaluationControl::verified_report_view_current(
        &fixture.evaluator,
        &fixture.store,
        FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference.clone())),
    )
    .await
    .unwrap();
    assert_eq!(late_view.report_ref, view.report_ref);
    assert_eq!(late_view.view.report_digest, view.view.report_digest);
    assert_eq!(late_view.view.ticket_digest, view.view.ticket_digest);
    assert_eq!(after_close, catalog(&fixture.db_path));

    r39_redacted_source_classification_controls(fixture).await;
    r39_real_reader_catalog_fault_controls(fixture, &reference, &original_body).await;
}

// Malicious-storage setup confined to this test-owned DB. This is not a healthy
// second writer, a production API, a native permit or a fake in-memory Snapshot.
fn r39_owned_catalog_fault(fixture: &Fixture, changes: &Value) {
    let source = r#"
import sys,json,sqlite3,pathlib,os,stat
data=pathlib.Path(sys.argv[1]); identity=os.lstat(data)
assert stat.S_ISREG(identity.st_mode) and not data.is_symlink() and identity.st_nlink==1
frame=json.load(sys.stdin); assert frame and len(frame)<=512
allowed={'objects':('namespace','kind','id','owner','body','revision'),
'audit':('seq','namespace','payload','previous_hash','digest')}
connection=sqlite3.connect(data.resolve(strict=True).as_uri()+'?mode=rw',uri=True,timeout=0.0,isolation_level=None)
try:
 connection.execute('BEGIN IMMEDIATE')
 for change in frame:
  table=change['table']; assert table in allowed
  columns=allowed[table]; before=change['before']; after=change['after']
  assert set(before)==set(after)==set(columns)
  assert before['namespace']==after['namespace']=='statistics-binding-fixture'
  assert before['namespace']==after['namespace']
  keys=('namespace','kind','id') if table=='objects' else ('seq','namespace')
  assert all(before[key]==after[key] for key in keys)
  actual_columns=tuple(row[1] for row in connection.execute('PRAGMA table_info('+table+')'))
  assert actual_columns==columns
  where=' AND '.join(column+' IS ?' for column in columns)
  actual=connection.execute('SELECT '+','.join(columns)+' FROM '+table+' WHERE '+where,
   tuple(before[column] for column in columns)).fetchall()
  assert actual==[tuple(before[column] for column in columns)]
  query='UPDATE '+table+' SET '+','.join(column+'=?' for column in columns)+' WHERE '+where
  assert connection.execute(query,tuple(after[column] for column in columns)+tuple(before[column] for column in columns)).rowcount==1
 connection.execute('COMMIT')
finally:
 if connection.in_transaction:connection.execute('ROLLBACK')
 connection.close()
now=os.lstat(data)
assert (now.st_dev,now.st_ino)==(identity.st_dev,identity.st_ino)
print(json.dumps({'owned_fixture_only':True,'changed_rows':len(frame)}))
"#;
    let mut child = Command::new("python3")
        .args(["-B", "-c", source])
        .arg(&fixture.db_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let input = serde_json::to_vec(changes).unwrap();
    let written = child.stdin.take().unwrap().write_all(&input);
    let output = child.wait_with_output().unwrap();
    written.unwrap();
    assert!(
        output.status.success(),
        "owned source fault: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        receipt["changed_rows"].as_u64(),
        Some(changes.as_array().unwrap().len() as u64)
    );
}

fn r39_inverse_fault(changes: &Value) -> Value {
    Value::Array(changes.as_array().unwrap().iter().rev().map(|change|
        json!({"table":change["table"],"before":change["after"],"after":change["before"]})).collect())
}

async fn r39_real_reader_catalog_fault_controls(
    fixture: &Fixture,
    reference: &evo_core::formal_version::ReportRef,
    original_body: &str,
) {
    use evo_storage::FormalReadSelectorR38;
    let stable = catalog(&fixture.db_path);
    let original = r24_r1_catalog_rows(&stable, "objects")
        .into_iter()
        .find(|row| {
            row["namespace"] == NAMESPACE
                && row["kind"] == "artifact"
                && row["id"] == reference.physical_id
        })
        .unwrap();
    assert_eq!(original["body"].as_str(), Some(original_body));
    let mut body: Value = serde_json::from_str(original_body).unwrap();
    body["schema_version"] = json!("rsia.unknown.formal.schema.v999");
    body["record_kind"] = json!("r39_not_a_formal_record");
    let mut wrong_schema = original.clone();
    wrong_schema.insert("body".into(), json!(serde_json::to_string(&body).unwrap()));
    let changes = json!([{"table":"objects","before":original,"after":wrong_schema}]);
    r39_owned_catalog_fault(fixture, &changes);
    let faulted = catalog(&fixture.db_path);
    match fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
    {
        Err(Error::Invalid(code)) | Err(Error::Conflict(code)) => assert!(
            !code.starts_with("formal_source_insufficient")
                && !code.starts_with("source_insufficient"),
            "known wrong schema/record-kind cannot be a missing-source classification"
        ),
        _ => {
            panic!("real binding plus doubly unclassified artifact became NoFormal/NotFound/Clock")
        }
    }
    assert!(
        faulted == catalog(&fixture.db_path),
        "schema rejection changed catalog"
    );
    r39_owned_catalog_fault(fixture, &r39_inverse_fault(&changes));
    assert!(stable == catalog(&fixture.db_path));

    let original = r24_r1_catalog_rows(&stable, "objects")
        .into_iter()
        .find(|row| {
            row["namespace"] == NAMESPACE
                && row["kind"] == "artifact"
                && row["id"] == reference.physical_id
        })
        .unwrap();
    let mut wrong_owner = original.clone();
    wrong_owner.insert("owner".into(), json!("r39-wrong-evaluator"));
    let changes = json!([{"table":"objects","before":original,"after":wrong_owner}]);
    r39_owned_catalog_fault(fixture, &changes);
    let faulted = catalog(&fixture.db_path);
    assert!(
        fixture
            .store
            .read_formal_snapshot(
                &fixture.evaluator,
                FormalReadSelectorR38::Ticket(TICKET_ID.into()),
            )
            .await
            .is_err(),
        "wrong owner gained Current"
    );
    assert!(faulted == catalog(&fixture.db_path));
    r39_owned_catalog_fault(fixture, &r39_inverse_fault(&changes));
    assert!(stable == catalog(&fixture.db_path));

    let changes = r39_forged_native_asof_fault(&stable);
    r39_owned_catalog_fault(fixture, &changes);
    let faulted = catalog(&fixture.db_path);
    assert!(
        fixture
            .store
            .read_formal_snapshot(
                &fixture.evaluator,
                FormalReadSelectorR38::Ticket(TICKET_ID.into()),
            )
            .await
            .is_err(),
        "serialized forged native as-of gained Current"
    );
    assert!(
        faulted == catalog(&fixture.db_path),
        "as-of rejection changed catalog"
    );
    r39_owned_catalog_fault(fixture, &r39_inverse_fault(&changes));
    assert!(stable == catalog(&fixture.db_path));

    let changes = r39_coherent_bad_statistics_fault(&stable, reference, original_body);
    r39_owned_catalog_fault(fixture, &changes);
    let faulted = catalog(&fixture.db_path);
    let error = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await;
    assert!(
        matches!(error,Err(Error::Conflict(code))
        if code.contains("complete_statistics_or_reasons")),
        "coherent stored binding plus wrong statistics must fail actual recomputation"
    );
    assert!(
        faulted == catalog(&fixture.db_path),
        "bad statistics refusal wrote catalog"
    );
    r39_owned_catalog_fault(fixture, &r39_inverse_fault(&changes));
    assert!(stable == catalog(&fixture.db_path));
}

fn r39_native_row_digest(row: &BTreeMap<String, Value>) -> String {
    fn part(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u128).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    let mut bytes = Vec::new();
    for (key, value) in row {
        part(&mut bytes, key.as_bytes());
        if let Some(value) = value.as_str() {
            bytes.push(3);
            part(&mut bytes, value.as_bytes());
        } else if let Some(value) = value.as_i64() {
            bytes.push(1);
            bytes.extend_from_slice(&value.to_le_bytes());
        } else if value.is_null() {
            bytes.push(0);
        } else {
            panic!("unexpected type in the exact owned objects row");
        }
    }
    hash(&bytes)
}

// Deliberately forge a coherent storage-level binding (not a native operation)
// so the readonly test must recompute actual outputs/statistics. Audit presence
// and internally valid hashes alone cannot be enough to produce Current.
fn r39_coherent_bad_statistics_fault(
    stable: &Value,
    reference: &evo_core::formal_version::ReportRef,
    original_body: &str,
) -> Value {
    let original = r24_r1_catalog_rows(stable, "objects")
        .into_iter()
        .find(|row| {
            row["namespace"] == NAMESPACE
                && row["kind"] == "artifact"
                && row["id"] == reference.physical_id
        })
        .unwrap();
    let mut value: Value = serde_json::from_str(original_body).unwrap();
    let original_formal: FormalEvaluationV2 =
        serde_json::from_value(value["payload"].clone()).unwrap();
    assert_eq!(
        fingerprint(&original_formal).unwrap(),
        reference.payload_digest
    );
    assert_eq!(value["payload"]["report"]["mean"], "1");
    value["payload"]["report"]["mean"] = json!("0");
    let changed: FormalEvaluationV2 = serde_json::from_value(value["payload"].clone()).unwrap();
    let payload_digest = fingerprint(&changed).unwrap();
    let before_mean = "\"mean\":\"1\"";
    assert_eq!(original_body.matches(before_mean).count(), 1);
    let body = original_body.replacen(before_mean, "\"mean\":\"0\"", 1);
    let before_payload = format!("\"payload_digest\":\"{}\"", reference.payload_digest);
    assert_eq!(body.matches(&before_payload).count(), 1);
    let body = body.replacen(
        &before_payload,
        &format!("\"payload_digest\":\"{payload_digest}\""),
        1,
    );
    let after_value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(after_value["input"], value["input"]);
    assert_eq!(after_value["payload"], value["payload"]);
    let mut modified = original.clone();
    modified.insert("body".into(), json!(body));
    let mut new_ref = reference.clone();
    new_ref.payload_digest = payload_digest;
    new_ref.carrier_digest = hash(body.as_bytes());
    let new_row_digest = r39_native_row_digest(&modified);
    let mut audit = r24_r1_catalog_rows(stable, "audit");
    audit.sort_by_key(|row| row["seq"].as_i64().unwrap());
    let mut changes = vec![json!({"table":"objects","before":original,"after":modified})];
    let mut previous: Option<String> = None;
    let mut did_binding = false;
    for row in audit
        .into_iter()
        .filter(|row| row["namespace"] == NAMESPACE)
    {
        let mut payload_raw = row["payload"].as_str().unwrap().to_owned();
        let mut payload: Value = serde_json::from_str(&payload_raw).unwrap();
        if payload["operation"] == "formal.native.report_binding.v1" {
            let mut binding: Value =
                serde_json::from_str(payload["entity"].as_str().unwrap()).unwrap();
            if binding["report_ref"]["physical_id"] == reference.physical_id {
                assert!(!did_binding);
                binding["report_ref"] = serde_json::to_value(&new_ref).unwrap();
                binding["insert_row_digest"] = json!(new_row_digest);
                payload["entity"] = json!(serde_json::to_string(&binding).unwrap());
                payload_raw = serde_json::to_string(&payload).unwrap();
                previous = Some(row["previous_hash"].as_str().unwrap().to_owned());
                did_binding = true;
            }
        }
        if let Some(prior) = previous.take() {
            let digest = hash(format!("{prior}\n{payload_raw}").as_bytes());
            let mut after = row.clone();
            after.insert("payload".into(), json!(payload_raw));
            after.insert("previous_hash".into(), json!(prior));
            after.insert("digest".into(), json!(digest));
            if after != row {
                changes.push(json!({"table":"audit","before":row,"after":after}));
            }
            previous = Some(digest);
        }
    }
    assert!(did_binding);
    Value::Array(changes)
}

fn r39_forged_native_asof_fault(stable: &Value) -> Value {
    let mut audit = r24_r1_catalog_rows(stable, "audit");
    audit.sort_by_key(|row| row["seq"].as_i64().unwrap());
    let mut previous: Option<String> = None;
    let mut changed = false;
    let mut changes = Vec::new();
    for row in audit
        .into_iter()
        .filter(|row| row["namespace"] == NAMESPACE)
    {
        let mut raw = row["payload"].as_str().unwrap().to_owned();
        let mut payload: Value = serde_json::from_str(&raw).unwrap();
        if payload["operation"] == "formal.native.report_binding.v1" {
            let mut entity: Value =
                serde_json::from_str(payload["entity"].as_str().unwrap()).unwrap();
            if entity["summary"]["ticket_id"] == TICKET_ID {
                assert!(!changed);
                entity["summary"]["actor"] = json!("r39-forged-native-actor");
                payload["entity"] = json!(serde_json::to_string(&entity).unwrap());
                raw = serde_json::to_string(&payload).unwrap();
                previous = Some(row["previous_hash"].as_str().unwrap().to_owned());
                changed = true;
            }
        }
        if let Some(prior) = previous.take() {
            let digest = hash(format!("{prior}\n{raw}").as_bytes());
            let mut after = row.clone();
            after.insert("payload".into(), json!(raw));
            after.insert("previous_hash".into(), json!(prior));
            after.insert("digest".into(), json!(digest));
            if after != row {
                changes.push(json!({"table":"audit","before":row,"after":after}));
            }
            previous = Some(digest);
        }
    }
    assert!(changed);
    Value::Array(changes)
}

async fn r39_redacted_source_classification_controls(fixture: &Fixture) {
    use evo_storage::FormalReadSelectorR38;
    let stable = catalog(&fixture.db_path);
    for record_kind in [
        "registered_evaluation_control_v41",
        "evaluation_ticket_issue_receipt_v2",
    ] {
        let logical = if record_kind == "registered_evaluation_control_v41" {
            fixture.control.id.as_str()
        } else {
            TICKET_ID
        };
        let physical = format!("e05-{}", fingerprint(&(record_kind, logical)).unwrap());
        let row = r24_r1_catalog_rows(&stable, "objects")
            .into_iter()
            .find(|row| {
                row["namespace"] == NAMESPACE && row["kind"] == "artifact" && row["id"] == physical
            })
            .unwrap();
        let raw = row["body"].as_str().unwrap();
        let redacted = json!({"id":physical,"schema_version":"rsia.redacted.v1","state":"source_revoked",
            "original_kind":"artifact","original_schema":"rsia.typed_artifact_envelope.v1",
            "original_digest":hash(raw.as_bytes()),"metadata":{}});
        let mut after = row.clone();
        after.insert(
            "body".into(),
            json!(serde_json::to_string(&redacted).unwrap()),
        );
        let change = json!([{"table":"objects","before":row,"after":after}]);
        r39_owned_catalog_fault(fixture, &change);
        let damaged = catalog(&fixture.db_path);
        let error = fixture
            .store
            .read_formal_snapshot(
                &fixture.evaluator,
                FormalReadSelectorR38::Ticket(TICKET_ID.into()),
            )
            .await;
        assert!(
            matches!(error,Err(Error::Conflict(code))
            if code.starts_with("formal_source_insufficient:r39:")),
            "a validly bound redacted auxiliary source must remain SourceInsufficient"
        );
        assert!(damaged == catalog(&fixture.db_path));
        if record_kind == "registered_evaluation_control_v41" {
            let issue_physical = format!(
                "e05-{}",
                fingerprint(&("evaluation_ticket_issue_receipt_v2", TICKET_ID)).unwrap()
            );
            let issue_row = r24_r1_catalog_rows(&stable, "objects")
                .into_iter()
                .find(|row| {
                    row["namespace"] == NAMESPACE
                        && row["kind"] == "artifact"
                        && row["id"] == issue_physical
                })
                .unwrap();
            let mut wrong = issue_row.clone();
            wrong.insert("owner".into(), json!("r39-known-wrong-issue-owner"));
            let bad = json!([{"table":"objects","before":issue_row,"after":wrong}]);
            r39_owned_catalog_fault(fixture, &bad);
            let both = catalog(&fixture.db_path);
            let error = fixture
                .store
                .read_formal_snapshot(
                    &fixture.evaluator,
                    FormalReadSelectorR38::Ticket(TICKET_ID.into()),
                )
                .await;
            assert!(
                matches!(error,Err(Error::Conflict(code))
                if code.starts_with("formal_invalid_frozen_read:r39:")),
                "missing control cannot conceal an available wrong receipt owner"
            );
            assert!(both == catalog(&fixture.db_path));
            r39_owned_catalog_fault(fixture, &r39_inverse_fault(&bad));
            assert!(damaged == catalog(&fixture.db_path));
        }
        r39_owned_catalog_fault(fixture, &r39_inverse_fault(&change));
        assert!(stable == catalog(&fixture.db_path));
    }
}

async fn r44_first_critical_stop(
    fixture: &Fixture,
    draining: bool,
) -> Option<(evo_storage::budget::BudgetCallRecord, String)> {
    stage_candidate(fixture).await;
    let ticket = issue_from_native_clock(fixture).await;
    assert!(ticket.candidate_dependency_binding.is_some());
    assert_eq!(ticket.n_planned, N);
    let candidate = execute(fixture, "a0", ExecutionSide::Candidate, "wrong", 900).await;
    let baseline = execute(fixture, "a0", ExecutionSide::Baseline, "ok", 901).await;
    let pending = if draining {
        let task = IndependentEvaluationControl::broker_task(
            &fixture.worker,
            &fixture.store,
            TICKET_ID,
            "r0",
            ExecutionSide::Candidate,
        )
        .await
        .unwrap();
        let now = sample_seconds();
        let call = fixture
            .store
            .reserve_budget_call(
                &fixture.worker,
                &BudgetCallReservation {
                    billing_scope: BILLING_SCOPE.into(),
                    call_id: "r44-pending-call".into(),
                    dispatch_group_id: TICKET_ID.into(),
                    stage: BudgetStage::TaskExecution,
                    actual_input_digest: task.request_digest,
                    max_cost_micros: 100,
                    request_artifact: None,
                    lease_token: "r44-pending-lease".into(),
                    lease_until: now.checked_add(1000).unwrap(),
                    now,
                },
            )
            .await
            .unwrap();
        let fence = BudgetCallFence {
            billing_scope: BILLING_SCOPE.into(),
            call_id: call.call_id.clone(),
            actual_input_digest: call.actual_input_digest.clone(),
            lease_token: call.lease_token.clone(),
            lease_epoch: call.lease_epoch,
            now: sample_seconds(),
        };
        let dispatched = IndependentEvaluationControl::start_execution(
            &fixture.worker,
            &fixture.store,
            StartExecutionRequest {
                ticket_id: TICKET_ID.into(),
                target_id: "r0".into(),
                side: ExecutionSide::Candidate,
                fence,
            },
        )
        .await
        .unwrap();
        let dispatch_id = dispatched.call.dispatch_id.clone().unwrap();
        Some((dispatched.call, dispatch_id))
    } else {
        None
    };
    let receipt = IndependentEvaluationControl::record_grader_receipt_now(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptIntent {
            receipt_id: "r44-first-critical-grader".into(),
            ticket_id: TICKET_ID.into(),
            target_id: "a0".into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        receipt.anchor_result,
        Some(AnchorResult::CriticalRegression)
    );
    assert_eq!(receipt.candidate_score_micros, 0);
    assert_eq!(receipt.baseline_score_micros, 1_000_000);
    pending
}

#[tokio::test]
async fn r44_real_native_early_uses_new_input_and_has_no_complete_capability() {
    use evo_core::formal_version as fv;
    use evo_storage::FormalReadSelectorR38;
    let fixture = fixture(
        "r44-native-critical",
        RegisteredEvaluationControl::SCHEMA,
        false,
    )
    .await;
    assert!(r44_first_critical_stop(&fixture, false).await.is_none());
    let before = catalog(&fixture.db_path);
    let status =
        IndependentEvaluationControl::status(&fixture.evaluator, &fixture.store, TICKET_ID)
            .await
            .unwrap();
    assert_eq!(status.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    let FormalEvaluationV2::EarlyStopped { certificate, .. } = status.formal.unwrap() else {
        panic!("actual native first stop did not create an EarlyStopped member");
    };
    assert_eq!(
        serde_json::to_value(&certificate).unwrap()["stop_reason"],
        "critical_regression"
    );
    assert!(!certificate.usage_uncertain());
    let closed = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    let source = closed.formal().unwrap();
    assert!(source.current_complete().is_none());
    let current = source.current_formal().unwrap();
    let reference = current.reference().clone();
    assert_eq!(reference.engine_version, fv::ENGINE_V4);
    assert_eq!(reference.input_schema_version, fv::INPUT_SCHEMA_V2);
    assert_ne!(reference.physical_id, TICKET_ID);
    let original_wire: Value = serde_json::from_str(source.raw_body()).unwrap();
    assert_eq!(
        original_wire["input"]["native_first_stop"]["snapshot"]["initially_draining"],
        false
    );
    assert_eq!(
        original_wire["input"]["native_first_stop"]["snapshot"]["member_flags"]
            .as_str()
            .unwrap()
            .len(),
        N as usize + 1
    );
    // The actual already-written native grade asof is the sole first-stop
    // witness; no new legacy side event may accompany it.
    let capsule: fv::OriginalValueV2<fv::NativeFirstStopV2> =
        serde_json::from_value(original_wire["input"]["native_first_stop"].clone()).unwrap();
    let mut proofs = Vec::new();
    for row in r24_r1_catalog_rows(&before, "audit") {
        let payload: Value = serde_json::from_str(row["payload"].as_str().unwrap()).unwrap();
        assert_ne!(payload["operation"], "formal.native.first_stop.v1");
        if payload["operation"] != "formal.native.asof.grade.v1" {
            continue;
        }
        if let Ok(wrapper) = serde_json::from_str::<Value>(payload["entity"].as_str().unwrap())
            && wrapper["schema_version"] == "rsia.native_first_stop_asof_entity.v1"
            && wrapper["summary"]["ticket_id"] == TICKET_ID
        {
            proofs.push((row, payload, wrapper));
        }
    }
    assert_eq!(proofs.len(), 1);
    let (row, payload, wrapper) = &proofs[0];
    assert_eq!(wrapper["native_first_stop_digest"], capsule.typed_digest);
    assert_eq!(
        capsule.typed_digest,
        fingerprint(&capsule.snapshot).unwrap()
    );
    assert_eq!(wrapper["summary"]["namespace"], NAMESPACE);
    assert_eq!(wrapper["summary"]["actor"], reference.owner);
    assert_eq!(wrapper["summary"]["operation"], payload["operation"]);
    assert_eq!(
        wrapper["summary"]["previous_audit_hash"],
        row["previous_hash"]
    );
    assert_eq!(wrapper["summary"]["audit_timestamp"], payload["timestamp"]);
    assert!(
        row["seq"].as_i64().unwrap()
            > wrapper["summary"]["previous_audit_sequence"]
                .as_i64()
                .unwrap()
    );
    assert_eq!(
        row["digest"],
        evo_core::hash(
            format!(
                "{}\n{}",
                row["previous_hash"].as_str().unwrap(),
                row["payload"].as_str().unwrap()
            )
            .as_bytes()
        )
    );

    let actual_ref = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference.clone())),
        )
        .await
        .unwrap();
    assert_eq!(
        actual_ref
            .formal()
            .unwrap()
            .current_formal()
            .unwrap()
            .reference(),
        &reference
    );
    assert!(
        before == catalog(&fixture.db_path),
        "real early read wrote catalog"
    );

    // Actual same-ID source corruption, retaining the real INSERT/audit origin.
    // A well-formed hex nibble must not replace the independent original member.
    let ticket_id = format!(
        "e05-{}",
        fingerprint(&("evaluation_ticket_v2", TICKET_ID)).unwrap()
    );
    let original = r24_r1_catalog_rows(&before, "objects")
        .into_iter()
        .find(|row| {
            row["namespace"] == NAMESPACE && row["kind"] == "artifact" && row["id"] == ticket_id
        })
        .unwrap();
    let mut body: Value = serde_json::from_str(original["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["schema_version"], fv::NATIVE_TICKET_ENVELOPE);
    let flags = body["native_first_stop"]["member_flags"].as_str().unwrap();
    let changed = format!(
        "{}{}",
        if &flags[..1] == "0" { "1" } else { "0" },
        &flags[1..]
    );
    body["native_first_stop"]["member_flags"] = json!(changed);
    let mut after = original.clone();
    after.insert("body".into(), json!(serde_json::to_string(&body).unwrap()));
    let changes = json!([{"table":"objects","before":original,"after":after}]);
    r39_owned_catalog_fault(&fixture, &changes);
    let faulted = catalog(&fixture.db_path);
    let rejected = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference)),
        )
        .await;
    match rejected {
        Err(Error::Invalid(code)) | Err(Error::Conflict(code)) => {
            assert!(
                !code.starts_with("formal_source_insufficient")
                    && !code.starts_with("source_insufficient"),
                "known first-member corruption was downgraded to missing source"
            );
        }
        _ => panic!("known first-member corruption was not classified invalid"),
    }
    assert!(
        faulted == catalog(&fixture.db_path),
        "rejected source corruption wrote catalog"
    );
    r39_owned_catalog_fault(&fixture, &r39_inverse_fault(&changes));
    assert!(before == catalog(&fixture.db_path));
    fixture.store.close().await;
}

#[tokio::test]
async fn r44_first_stop_original_survives_actual_draining_late_finance_and_reconcile() {
    use evo_core::formal_version as fv;
    use evo_storage::FormalReadSelectorR38;
    let fixture = fixture(
        "r44-native-draining",
        RegisteredEvaluationControl::SCHEMA,
        false,
    )
    .await;
    let (call, dispatch) = r44_first_critical_stop(&fixture, true).await.unwrap();
    let first = IndependentEvaluationControl::status(&fixture.evaluator, &fixture.store, TICKET_ID)
        .await
        .unwrap();
    assert_eq!(first.ticket.lifecycle, TicketLifecycle::Draining);
    assert!(first.formal.is_none());
    let original_certificate = first.ticket.pending_certificate.clone().unwrap();
    assert!(original_certificate.usage_uncertain());
    let ticket_id = format!(
        "e05-{}",
        fingerprint(&("evaluation_ticket_v2", TICKET_ID)).unwrap()
    );
    let first_catalog = catalog(&fixture.db_path);
    let row = r24_r1_catalog_rows(&first_catalog, "objects")
        .into_iter()
        .find(|row| row["namespace"] == NAMESPACE && row["id"] == ticket_id)
        .unwrap();
    let first_body: Value = serde_json::from_str(row["body"].as_str().unwrap()).unwrap();
    let original_stop = first_body["native_first_stop"].clone();
    assert_eq!(original_stop["initially_draining"], true);
    let before_reconcile = catalog(&fixture.db_path);
    let unchanged = IndependentEvaluationControl::settle_after_stop_now(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
    )
    .await
    .unwrap();
    assert_eq!(unchanged.ticket.lifecycle, TicketLifecycle::Draining);
    assert!(
        before_reconcile == catalog(&fixture.db_path),
        "unsettled reconcile wrote facts"
    );

    // Genuine retained dispatched fixture call; only its monetary/terminal
    // overlay changes. It never becomes an accepted statistical observation.
    let fence = BudgetCallFence {
        billing_scope: BILLING_SCOPE.into(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: sample_seconds(),
    };
    fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &fence,
            &UsageCharge {
                amount_micros: 3,
                currency: "USD".into(),
                pricing_version: "fixture-pricing-v1".into(),
                provider_request_id: "r44-cancelled-request".into(),
                usage_record_id: "r44-cancelled-usage".into(),
                output_digest: hash(b"late cancelled fixture output"),
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            BILLING_SCOPE,
            &call.call_id,
            &dispatch,
            "late cancellation terminal",
            sample_seconds(),
        )
        .await
        .unwrap();
    let settled = IndependentEvaluationControl::settle_after_stop_now(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
    )
    .await
    .unwrap();
    assert_eq!(settled.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    let FormalEvaluationV2::EarlyStopped { certificate, .. } = settled.formal.unwrap() else {
        panic!("reconciled actual stop must have a versioned EarlyStopped member");
    };
    let expected_certificate = original_certificate.with_usage_reconciled().unwrap();
    assert_eq!(
        fingerprint(&certificate).unwrap(),
        fingerprint(&expected_certificate).unwrap()
    );
    assert_eq!(
        settled
            .ticket
            .targets
            .iter()
            .filter(|target| matches!(target.state, TargetState::Completed { .. }))
            .count(),
        1
    );
    let stable = catalog(&fixture.db_path);
    let row = r24_r1_catalog_rows(&stable, "objects")
        .into_iter()
        .find(|row| row["namespace"] == NAMESPACE && row["id"] == ticket_id)
        .unwrap();
    let terminal_body: Value = serde_json::from_str(row["body"].as_str().unwrap()).unwrap();
    assert_eq!(terminal_body["native_first_stop"], original_stop);
    let read = fixture
        .store
        .read_formal_snapshot(
            &fixture.evaluator,
            FormalReadSelectorR38::Ticket(TICKET_ID.into()),
        )
        .await
        .unwrap();
    assert!(read.formal().unwrap().current_complete().is_none());
    let current = read.formal().unwrap().current_formal().unwrap();
    let carrier: Value = serde_json::from_str(read.formal().unwrap().raw_body()).unwrap();
    assert_eq!(
        carrier["input"]["native_first_stop"]["snapshot"],
        original_stop
    );
    assert_eq!(current.reference().engine_version, fv::ENGINE_V4);
    assert_eq!(
        current.reference().input_schema_version,
        fv::INPUT_SCHEMA_V2
    );
    let repeated = IndependentEvaluationControl::settle_after_stop_now(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
    )
    .await
    .unwrap();
    assert_eq!(repeated.ticket.lifecycle, TicketLifecycle::EarlyStopped);
    assert!(
        stable == catalog(&fixture.db_path),
        "terminal reconcile/read changed immutable report or certificate"
    );
    fixture.store.close().await;
}
