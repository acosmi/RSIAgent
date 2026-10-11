//! Public machine-contract fixtures for CompleteBatch statistics readback.
//! These are local ProgramFixture receipts, not provider or production proof.
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
    compile_bundle,
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
    EvaluationTargetKind, ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod,
    FixedGraderSpec, FormalEvaluationV2, FrozenOracleEntry, GraderReceiptRequest,
    IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef,
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
const NAMESPACE: &str = "statistics-binding-fixture";
const BILLING_SCOPE: &str = "statistics-billing";
const TICKET_ID: &str = "statistics-ticket";
const FORMAL_KIND: &str = "formal_evaluation_v2";
// Exact conflict for a stored CompleteBatch report that differs from its facts.
const STATISTICS_CHANGED: &str = "formal_complete_batch_statistics_changed";

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

struct Fixture {
    _temporary: Option<tempfile::TempDir>,
    db_path: PathBuf,
    artifacts: Option<PathBuf>,
    store: Store,
    evaluator: Context,
    worker: Context,
    control: RegisteredEvaluationControl,
    holdout: ProtectedHoldoutRecord,
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
        digest("fixture-baseline"),
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
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: format!("grader-{ordinal}"),
            ticket_id: TICKET_ID.into(),
            target_id: target.into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            // This is after both calls' receipt/settlement/close timestamps and
            // no later than the next pair's first dispatch timestamp.
            issued_at_unix_seconds: 20 + i64::from(ordinal) * 10,
        },
    )
    .await
    .unwrap();
}

async fn complete(fixture: &Fixture, sequential: bool) -> FormalEvaluationV2 {
    stage_candidate(fixture).await;
    let issued = IndependentEvaluationControl::issue_ticket(
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
    .unwrap();
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
    IndependentEvaluationControl::finalize_complete(
        &fixture.evaluator,
        &fixture.store,
        TICKET_ID,
        5_000,
    )
    .await
    .unwrap()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormalEnvelope {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: FormalEvaluationV2,
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
    // Session::put advances only this object's revision automatically. Record
    // and verify that delta, rather than resetting it with a SQL mutation.
    after_row[body] = before_row[body].clone();
    after_row[revision] = before_row[revision].clone();
    assert_eq!(
        before_row, after_row,
        "another formal object column changed"
    );
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
        .stop_root_budget(&admin, BILLING_SCOPE, "historical_report_readback", 5_001)
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
    let physical_id = format!("e05-{}", fingerprint(&(FORMAL_KIND, TICKET_ID)).unwrap());
    let original: FormalEnvelope = session
        .need(&fixture.evaluator, "artifact", &physical_id)
        .await
        .unwrap();
    assert_eq!(original.schema_version, "rsia.typed_artifact_envelope.v1");
    assert_eq!(original.id, physical_id);
    assert_eq!(original.record_kind, FORMAL_KIND);
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
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.evaluator,
            "artifact",
            &physical_id,
            fixture.evaluator.actor(),
            &altered,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
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
