//! AG-141 engine boundary: a formal control registration reads the active root
//! stage plan in its final write Session, refuses an unbound root, and checks
//! the payer against the persisted payment subject. These are local
//! ProgramFixture controls, not provider or paid evidence.

use evo_core::evaluation::{
    ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41,
    FormalStatisticalUnit, Money, OptimizationBudgetPlan, OptimizationStage,
    OptimizerComparisonContract, RootStageBudgetPlanV1, StageMoneyBudget,
};
use evo_core::holdout::{AnchorCoverageMatrix, CriticalCapabilityCoverage};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::{Context, Error, Role, hash};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, FixedGraderMethod, FixedGraderSpec, IndependentEvaluationControl,
    RegisteredEvaluationControl, RegisteredEvaluationControlV3,
};
use evo_storage::Store;
use evo_storage::budget::{
    RootBudgetAuthorization, RootStagePlanBindRequest, RootStagePlanBinding,
};

// The management (Current bound) entry's real compiler-input fixture. It mints
// only the explicit Admin empty-root software program boundary; no provider,
// external host, grade or paid authority is created.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use async_trait::async_trait;
use evo_core::Strategy;
use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::skill_edit::{EvidenceRef, skill_snapshot_digest};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::formal_authority::{CompilerInputRequest, resolve_current_compiler_inputs};
use evo_engine::model::{
    CurrentBoundModelRequest, CurrentCompilerInputUse, DevelopmentControlSelector,
    ModelExecutionProvenance, ModelPort, ModelRejectionKind, ModelResponse, ReadBoundModelRequest,
    RejectedDispatch,
};
use evo_storage::budget::{BudgetArtifact, BudgetCallReservation, BudgetCallState, BudgetStage};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SCOPE: &str = "billing-1";
const SUBJECT: &str = "fixture-only";
const CONTROL_KIND: &str = "registered_evaluation_control_v41";

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

fn set_stage(stages: &mut [StageMoneyBudget], stage: OptimizationStage, total: &str) {
    let entry = stages
        .iter_mut()
        .find(|entry| entry.stage == stage)
        .unwrap();
    entry.per_call_limit = money(total);
    entry.experiment_total = money(total);
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    evaluator: Context,
    control: RegisteredEvaluationControlV3,
}

async fn fixture(payer: &str, bind: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ag141.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: SUBJECT.into(),
                authorization_receipt_digest: d("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = 2;
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
    let mut budget = OptimizationBudgetPlan::unfunded(SCOPE, payer, "USD").unwrap();
    budget.root_total = money("0.001");
    budget.admin_authorization_receipt_digest = Some(d("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit = money("0");
        stage.experiment_total = money("0");
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
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
        2,
    )
    .unwrap();
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let control = RegisteredEvaluationControlV3 {
        schema_version: RegisteredEvaluationControlV3::SCHEMA.into(),
        id: "control-1".into(),
        v1_plan_snapshot: v1,
        max_regression: evo_core::evaluation::FixedRegressionThreshold::new(0.125).unwrap(),
        formal_plan: formal,
        alpha_plan: alpha,
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: None,
        anchor_matrix: anchors,
        attempt_id: "attempt-1".into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: SCOPE.into(),
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
    control.validate().unwrap();
    if bind {
        bind_plan(&store, &admin, &control.optimization_budget, 0)
            .await
            .unwrap();
    }
    Fixture {
        _dir: dir,
        store,
        admin,
        evaluator,
        control,
    }
}

fn optimizer(arm: FixedOptimizerArm, id: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(id),
        initial_s0_digest: d("s0"),
        base_model_digest: d("model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: SCOPE.into(),
        budget_limit: money("0.001"),
    }
}

async fn bind_plan(
    store: &Store,
    admin: &Context,
    budget: &OptimizationBudgetPlan,
    expected_seq: i64,
) -> evo_core::Result<RootStagePlanBinding> {
    store
        .bind_root_stage_plan(
            admin,
            &RootStagePlanBindRequest {
                billing_scope: SCOPE.into(),
                expected_seq,
                plan: RootStageBudgetPlanV1::from_optimization_budget(budget.clone()),
            },
        )
        .await
}

async fn control_exists(fixture: &Fixture) -> bool {
    IndependentEvaluationControl::registration_progress(
        &fixture.evaluator,
        &fixture.store,
        &fixture.control.id,
        "unused-holdout",
    )
    .await
    .unwrap()
    .control_digest
    .is_some()
}

#[tokio::test]
async fn unbound_root_refuses_formal_control_with_zero_writes() {
    let fixture = fixture(SUBJECT, false).await;
    let error = IndependentEvaluationControl::register_current_control(
        &fixture.evaluator,
        &fixture.store,
        fixture.control.clone(),
    )
    .await
    .expect_err("unbound root must refuse the control");
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "registered_control_requires_active_root_stage_plan"),
        "{error:?}"
    );
    assert!(!control_exists(&fixture).await);
}

#[tokio::test]
async fn payer_mismatch_is_refused_before_the_active_plan() {
    let fixture = fixture("other-payer", false).await;
    let error = IndependentEvaluationControl::register_current_control(
        &fixture.evaluator,
        &fixture.store,
        fixture.control.clone(),
    )
    .await
    .expect_err("payer mismatch must refuse the control");
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "persistent root payment subject differs from the registered payer"),
        "{error:?}"
    );
    assert!(!control_exists(&fixture).await);
}

#[tokio::test]
async fn bound_root_registers_and_control_relation_is_stale_after_switch() {
    let fixture = fixture(SUBJECT, true).await;
    IndependentEvaluationControl::register_current_control(
        &fixture.evaluator,
        &fixture.store,
        fixture.control.clone(),
    )
    .await
    .unwrap();
    assert!(control_exists(&fixture).await);
    let active = fixture
        .store
        .root_stage_plan_binding(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.seq, 1);
    let mut session = fixture.store.session().await.unwrap();
    let relation = session
        .root_stage_plan_control_ref(&fixture.evaluator, CONTROL_KIND, &fixture.control.id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(relation.binding_seq, 1);
    assert_eq!(relation.plan_digest, active.plan_digest);
    // A second version does not rewrite the control relation: the old control
    // no longer names the active version and cannot issue a new ticket.
    let mut switched = fixture.control.optimization_budget.clone();
    set_stage(
        &mut switched.stages,
        OptimizationStage::Reflection,
        "0.0001",
    );
    let second = bind_plan(&fixture.store, &fixture.admin, &switched, 1)
        .await
        .unwrap();
    assert_eq!(second.seq, 2);
    let mut session = fixture.store.session().await.unwrap();
    let relation = session
        .root_stage_plan_control_ref(&fixture.evaluator, CONTROL_KIND, &fixture.control.id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(relation.binding_seq, 1);
    assert_ne!(relation.plan_digest, second.plan_digest);
}

#[tokio::test]
async fn control_budget_must_equal_the_active_plan_embedding() {
    let fixture = fixture(SUBJECT, false).await;
    let mut budget = fixture.control.optimization_budget.clone();
    set_stage(&mut budget.stages, OptimizationStage::Reflection, "0.0001");
    bind_plan(&fixture.store, &fixture.admin, &budget, 0)
        .await
        .unwrap();
    let error = IndependentEvaluationControl::register_current_control(
        &fixture.evaluator,
        &fixture.store,
        fixture.control.clone(),
    )
    .await
    .expect_err("control budget must match the active embedded plan");
    assert!(
        matches!(&error, Error::Conflict(message) if message == "registered control budget differs from the active root stage plan"),
        "{error:?}"
    );
    assert!(!control_exists(&fixture).await);
}

#[tokio::test]
async fn course_and_practice_stages_stay_with_their_real_producers() {
    assert_eq!(
        evo_engine::curriculum::selection_budget_stage(),
        evo_storage::budget::BudgetStage::Curriculum
    );
    assert_eq!(evo_engine::curriculum::course_share_bps(), 2_000);
    assert_eq!(
        evo_engine::practice::practice_budget_stage(),
        evo_storage::budget::BudgetStage::DevelopmentExecution
    );
    assert_eq!(
        evo_core::curriculum::curriculum_share_limit_micros(10_000).unwrap(),
        2_000
    );
    // The offline profile keeps its disabled, zero-budget contract.
    let profile = evo_core::curriculum::CurriculumControlProfileV1::offline_default(
        "profile",
        d("task-space"),
        d("oracle"),
        d("runner"),
    )
    .unwrap();
    assert_eq!(
        profile.runtime_status,
        evo_core::curriculum::CurriculumRuntimeStatus::Disabled
    );
    assert_eq!(profile.monetary_limit_micros, 0);
    assert_eq!(profile.curriculum_limit(10_000).unwrap(), 2_000);
}

#[tokio::test]
async fn unplanned_reflection_is_outside_only_on_a_zero_binding_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ag141-unbound.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-2".into(),
                billing_scope: "billing-2".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: SUBJECT.into(),
                authorization_receipt_digest: d("receipt-2"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let reserved = session
        .reserve_budget_call_with_sources_typed(
            &worker,
            &evo_storage::budget::BudgetCallReservation {
                billing_scope: "billing-2".into(),
                call_id: "call-1".into(),
                dispatch_group_id: "group-1".into(),
                stage: evo_storage::budget::BudgetStage::Reflection,
                actual_input_digest: d("input"),
                request_artifact: None,
                max_cost_micros: 10,
                lease_token: "lease".into(),
                lease_until: 20,
                now: 10,
            },
            &[],
        )
        .await
        .unwrap();
    assert!(reserved.is_ok());
    session.commit().await.unwrap();
    let report = store
        .root_stage_plan_report(&admin, "billing-2")
        .await
        .unwrap();
    assert_eq!(
        report.enforcement,
        evo_storage::budget::RootStagePlanEnforcement::NoBinding
    );
    assert!(report.binding.is_none());
    assert!(
        report
            .stages
            .iter()
            .all(|stage| stage.allocated_micros.is_none())
    );
}

#[tokio::test]
async fn legacy_register_is_an_explicit_refusal_not_a_silent_dead_end() {
    let fixture = fixture(SUBJECT, false).await;
    let legacy = RegisteredEvaluationControl {
        schema_version: RegisteredEvaluationControl::SCHEMA.into(),
        id: fixture.control.id.clone(),
        v1_plan_snapshot: fixture.control.v1_plan_snapshot.clone(),
        formal_plan: fixture.control.formal_plan.clone(),
        alpha_plan: fixture.control.alpha_plan.clone(),
        optimizer_comparison: fixture.control.optimizer_comparison.clone(),
        optimization_budget: fixture.control.optimization_budget.clone(),
        early_stop_plan: fixture.control.early_stop_plan.clone(),
        anchor_matrix: fixture.control.anchor_matrix.clone(),
        attempt_id: fixture.control.attempt_id.clone(),
        fixed_sample_claim_id: fixture.control.fixed_sample_claim_id.clone(),
        billing_scope: fixture.control.billing_scope.clone(),
        executor_actor: fixture.control.executor_actor.clone(),
        evaluator_actor: fixture.control.evaluator_actor.clone(),
        proposer_actor: fixture.control.proposer_actor.clone(),
        approver_actor: fixture.control.approver_actor.clone(),
        oracle_digest: fixture.control.oracle_digest.clone(),
        grader_digest: fixture.control.grader_digest.clone(),
        fixed_grader: fixture.control.fixed_grader.clone(),
        evidence_scope: fixture.control.evidence_scope,
        created_at_unix_seconds: fixture.control.created_at_unix_seconds,
    };
    legacy.validate().unwrap();
    let error =
        IndependentEvaluationControl::register_control(&fixture.evaluator, &fixture.store, legacy)
            .await
            .expect_err("legacy controls have no Current registration path");
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "legacy registered control is not Current input"),
        "{error:?}"
    );
    assert!(!control_exists(&fixture).await);
    fixture.store.close().await;
}

// ---------------------------------------------------------------------------
// Broker begin-stage refusal at the management (Current bound) entry.
// Fixture shapes follow `broker_bound_recovery_close_v42.rs`; the program
// boundary is the explicit Admin empty-root software fixture only.
// ---------------------------------------------------------------------------

const STAGE_SCOPE: &str = "stage-billing";
const STAGE_ROOT: &str = "stage-root";
const STAGE_PAYER: &str = "software-fixture";
const STAGE_NAMESPACE: &str = "n";
const STAGE_PROFILE: &str = "profile";
const STAGE_RUNS: [&str; 2] = ["run-failure", "run-success"];
const STAGE_CONTROL_KIND: &str = "development_control_v1";

fn stage_admin() -> Context {
    Context::new(STAGE_NAMESPACE, "fixture-admin", Role::Admin).unwrap()
}

fn stage_worker() -> Context {
    Context::new(STAGE_NAMESPACE, "worker", Role::Worker).unwrap()
}

fn stage_host() -> Context {
    Context::new(STAGE_NAMESPACE, "host", Role::Host).unwrap()
}

async fn stage_authorize(store: &Store, admin: &Context) {
    store
        .authorize_root_budget(
            admin,
            &RootBudgetAuthorization {
                root_budget_id: STAGE_ROOT.into(),
                billing_scope: STAGE_SCOPE.into(),
                allowed_namespaces: vec![STAGE_NAMESPACE.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: STAGE_PAYER.into(),
                authorization_receipt_digest: d("stage-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 100,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

/// A valid 15-stage plan whose charged stages are all allocated zero. On a
/// bound root this refuses every planned charge stage as `Unallocated`.
fn stage_plan() -> RootStageBudgetPlanV1 {
    stage_plan_with_reflection("0", "0")
}

/// The same plan with a Reflection allocation large enough to sit above an
/// already committed 20-micro Reservation; used when the call must reach the
/// begin gate instead of being refused while the version is appended.
fn stage_reflection_plan() -> RootStageBudgetPlanV1 {
    stage_plan_with_reflection("0.0001", "0.0001")
}

fn stage_plan_with_reflection(
    reflection_per_call: &str,
    reflection_total: &str,
) -> RootStageBudgetPlanV1 {
    let mut budget = OptimizationBudgetPlan::unfunded(STAGE_SCOPE, STAGE_PAYER, "USD").unwrap();
    budget.root_total = money("0.0001");
    budget.admin_authorization_receipt_digest = Some(d("stage-authorization"));
    for stage in &mut budget.stages {
        stage.per_call_limit = money("0");
        stage.experiment_total = money("0");
    }
    let reflection = budget
        .stages
        .iter_mut()
        .find(|stage| stage.stage == OptimizationStage::Reflection)
        .unwrap();
    reflection.per_call_limit = money(reflection_per_call);
    reflection.experiment_total = money(reflection_total);
    budget.validate().unwrap();
    RootStageBudgetPlanV1::from_optimization_budget(budget)
}

async fn stage_bind(
    store: &Store,
    admin: &Context,
    expected_seq: i64,
    plan: RootStageBudgetPlanV1,
) -> evo_core::Result<RootStagePlanBinding> {
    store
        .bind_root_stage_plan(
            admin,
            &RootStagePlanBindRequest {
                billing_scope: STAGE_SCOPE.into(),
                expected_seq,
                plan,
            },
        )
        .await
}

fn stage_trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

fn stage_parent_skill() -> SkillSnapshot {
    SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    }
}

fn stage_no_capabilities() -> HostCapabilities {
    HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    }
}

/// Two Host-issued trusted runs, their selection grant and one watermark bump.
async fn stage_seeded_store() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage-plan-broker.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let host = stage_host();
    for ((id, family), outcome) in [(STAGE_RUNS[0], "family-a"), (STAGE_RUNS[1], "family-b")]
        .into_iter()
        .zip([TraceOutcome::TaskFailure, TraceOutcome::Success])
    {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: stage_trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(&store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        &store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: STAGE_RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    (dir, path, store)
}

fn stage_compiler_request(watermark: u64) -> CompilerInputRequest {
    let environment = hash(b"environment");
    let fixture = trusted_compiler_inputs::inputs(
        STAGE_NAMESPACE,
        STAGE_PROFILE,
        stage_parent_skill(),
        Strategy::default(),
        stage_no_capabilities(),
        &environment,
    );
    CompilerInputRequest {
        profile: fixture.profile,
        search_parent: stage_parent_skill(),
        baseline: stage_parent_skill(),
        search_parent_strategy: Strategy::default(),
        expected_parent_strategy_digest: None,
        baseline_strategy: Strategy::default(),
        capabilities: stage_no_capabilities(),
        environment_digest: environment,
        purpose: Purpose::Development,
        revoke_watermark: watermark,
    }
}

async fn stage_current_compiler_use(store: &Store, watermark: u64) -> CurrentCompilerInputUse {
    let request = stage_compiler_request(watermark);
    Box::pin(trusted_compiler_inputs::install_request(store, &request))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let actual = Box::pin(resolve_current_compiler_inputs(
        &mut session,
        &stage_worker(),
        &request,
    ))
    .await
    .unwrap();
    let inputs = CurrentCompilerInputUse::new(request, actual.binding().clone());
    inputs
        .verify_current(&mut session, &stage_worker())
        .await
        .unwrap();
    session.commit().await.unwrap();
    inputs
}

fn stage_model_request(tag: &str, watermark: u64, sources: &[&str], content: &str) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: format!("stage-model-{tag}"),
            namespace: STAGE_NAMESPACE.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "stage-episode".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: skill_snapshot_digest(&stage_parent_skill()).unwrap(),
            bundle_digest: hash(b"parent-bundle"),
            source_closure: sources
                .iter()
                .map(|run| EvidenceRef {
                    id: (*run).into(),
                    digest: hash(run.as_bytes()),
                })
                .collect(),
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: watermark,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: content.into(),
        }],
    )
    .unwrap()
}

fn stage_bound_request(
    tag: &str,
    watermark: u64,
    sources: &[&str],
    content: &str,
    inputs: &CurrentCompilerInputUse,
) -> CurrentBoundModelRequest {
    CurrentBoundModelRequest::new(
        stage_model_request(tag, watermark, sources, content),
        inputs.clone(),
    )
    .unwrap()
}

fn stage_read_bound<T: serde::Serialize>(bound: &T) -> ReadBoundModelRequest {
    let original = serde_json::to_vec(bound).expect("fixture bound wire serialization");
    let read: ReadBoundModelRequest =
        serde_json::from_slice(&original).expect("fixture exact supported bound wire");
    assert_eq!(serde_json::to_vec(&read).unwrap(), original);
    read
}

#[derive(Clone)]
struct NoSendTransport {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelTransport for NoSendTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        _request: &ModelRequest,
        _dispatch_id: &str,
    ) -> evo_core::Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("a pre-dispatch stage refusal must send nothing")
    }
}

fn stage_broker(store: &Store, calls: &Arc<AtomicUsize>) -> PersistentModelBroker<NoSendTransport> {
    PersistentModelBroker::with_clock(
        store.clone(),
        NoSendTransport {
            calls: calls.clone(),
        },
        BrokerConfig {
            billing_scope: STAGE_SCOPE.into(),
            actor: "broker".into(),
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(|| 10),
    )
    .unwrap()
}

fn stage_assert_refusal(response: &ModelResponse, code: &str) {
    match response {
        ModelResponse::Rejected {
            kind,
            reason,
            dispatch,
            ..
        } => {
            assert_eq!(reason, &format!("broker_pre_dispatch_v1.{code}"));
            assert_eq!(*dispatch, RejectedDispatch::NotDispatched);
            assert_eq!(*kind, ModelRejectionKind::BudgetUnavailable);
        }
        other => panic!("expected a pre-dispatch refusal, got {other:?}"),
    }
}

/// The management begin entry must release a *committed* Reserved call and
/// write one refusal event in the same commit. The call is reserved before any
/// plan exists (so storage's same-reservation early return bypasses the
/// reservation-stage gate); binding the zero-stage plan afterwards makes its
/// stage attribution Unknown at begin.
#[tokio::test]
async fn management_begin_stage_refusal_releases_committed_reserved_and_writes_one_event() {
    let (_dir, _path, store) = stage_seeded_store().await;
    let admin = stage_admin();
    stage_authorize(&store, &admin).await;
    let inputs = stage_current_compiler_use(&store, 1).await;
    let bound = stage_bound_request(
        "management-begin-refusal",
        1,
        &[STAGE_RUNS[0]],
        "fixture",
        &inputs,
    );
    let request = bound.request.clone();
    let read = stage_read_bound(&bound);
    let artifact = BudgetArtifact::from_serializable(read.schema_version.clone(), &read).unwrap();
    let bound_artifact =
        BudgetArtifact::from_serializable(bound.schema_version.clone(), &bound).unwrap();
    assert_eq!(artifact, bound_artifact);
    let mut session = store.session().await.unwrap();
    let reserved = session
        .reserve_budget_call_with_sources_typed(
            &stage_worker(),
            &BudgetCallReservation {
                billing_scope: STAGE_SCOPE.into(),
                call_id: request.request_id.clone(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::Reflection,
                actual_input_digest: request.cache_key_digest.clone(),
                request_artifact: Some(artifact.clone()),
                max_cost_micros: 20,
                lease_token: "precommitted".into(),
                lease_until: 70,
                now: 10,
            },
            &[STAGE_RUNS[0].into()],
        )
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(reserved.state, BudgetCallState::Reserved);
    let stored = store
        .budget_call(&admin, STAGE_SCOPE, &request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.actual_input_digest, request.cache_key_digest);
    assert_eq!(stored.request_artifact.as_ref(), Some(&artifact));
    stage_bind(&store, &admin, 0, stage_reflection_plan())
        .await
        .unwrap();

    let mut session = store.session().await.unwrap();
    let before = session
        .root_budget_monetary_snapshot(&admin, STAGE_SCOPE)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(before.root.reserved_micros, 20);
    assert_eq!(before.calls.len(), 1);
    assert_eq!(before.dispatch_groups.len(), 1);

    let calls = Arc::new(AtomicUsize::new(0));
    let broker = stage_broker(&store, &calls);
    let before_clock = evo_core::now().unwrap();
    let response = broker
        .dispatch_compiler_bound(stage_read_bound(&bound))
        .await
        .unwrap();
    let after_clock = evo_core::now().unwrap();
    stage_assert_refusal(&response, "stage_plan_unknown");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut session = store.session().await.unwrap();
    let after = session
        .root_budget_monetary_snapshot(&admin, STAGE_SCOPE)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(after.root.reserved_micros, 0);
    assert_eq!(after.calls.len(), before.calls.len());
    assert_eq!(after.dispatch_groups.len(), before.dispatch_groups.len());
    let call = after
        .calls
        .iter()
        .find(|call| call.call_id == request.request_id)
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Released);
    assert_eq!(
        call.terminal_reason.as_deref(),
        Some("broker_pre_dispatch_v1.stage_plan_unknown")
    );
    assert_eq!(call.dispatch_id, None);
    let refusals: Vec<_> = after
        .ledger_events
        .iter()
        .filter(|event| event.event_kind == "stage_plan_refused")
        .collect();
    assert_eq!(refusals.len(), 1);
    let details: serde_json::Value = serde_json::from_str(&refusals[0].details).unwrap();
    assert_eq!(details["stage"], "reflection");
    assert_eq!(details["reason"], "stage_plan_unknown");
    assert_eq!(details["binding_seq"], 1);
    assert!(
        refusals[0].event_at >= before_clock && refusals[0].event_at <= after_clock,
        "event_at {} is outside the trusted interval",
        refusals[0].event_at
    );
    assert_ne!(refusals[0].event_at, 10);
    store.close().await;
}

/// A Current planned model call on a bound root with zero stage allocation is
/// refused before any reservation/group/call/event write and sends nothing; the
/// same request without a persisted plan origin is refused as Unknown. The two
/// controls therefore stay distinct.
#[tokio::test]
async fn current_planned_zero_stage_sends_nothing_and_unplanned_stays_distinct() {
    let (_dir, _path, store) = stage_seeded_store().await;
    let admin = stage_admin();
    stage_authorize(&store, &admin).await;
    stage_bind(&store, &admin, 0, stage_plan()).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .record_root_stage_plan_control(
            &admin,
            STAGE_SCOPE,
            STAGE_CONTROL_KIND,
            "control-1",
            &d("stage-control-digest"),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let inputs = stage_current_compiler_use(&store, 1).await;
    let bound = stage_bound_request("zero-stage", 1, &[STAGE_RUNS[0]], "fixture", &inputs);
    let mut session = store.session().await.unwrap();
    let before = session
        .root_budget_monetary_snapshot(&admin, STAGE_SCOPE)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let broker = stage_broker(&store, &calls);
    let planned = broker
        .dispatch_compiler_bound_with_development_control(
            stage_read_bound(&bound),
            Some(DevelopmentControlSelector {
                kind: STAGE_CONTROL_KIND.into(),
                namespace: STAGE_NAMESPACE.into(),
                id: "control-1".into(),
            }),
        )
        .await
        .unwrap();
    stage_assert_refusal(&planned, "stage_plan_unallocated");
    let unplanned = broker
        .dispatch_compiler_bound(stage_read_bound(&bound))
        .await
        .unwrap();
    stage_assert_refusal(&unplanned, "stage_plan_unknown");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut session = store.session().await.unwrap();
    let after = session
        .root_budget_monetary_snapshot(&admin, STAGE_SCOPE)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(after.root.reserved_micros, 0);
    assert_eq!(after.calls.len(), before.calls.len());
    assert_eq!(after.dispatch_groups.len(), before.dispatch_groups.len());
    // Each refused reservation writes exactly one refusal fact and no call or
    // group: one Unallocated for the planned control and one Unknown for the
    // unplanned one.
    let refusals: Vec<_> = after
        .ledger_events
        .iter()
        .filter(|event| event.event_kind == "stage_plan_refused")
        .collect();
    assert_eq!(refusals.len(), 2);
    let reasons: BTreeSet<String> = refusals
        .iter()
        .map(|event| {
            let details: serde_json::Value = serde_json::from_str(&event.details).unwrap();
            assert_eq!(details["stage"], "reflection");
            assert_eq!(details["binding_seq"], 1);
            details["reason"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(
        reasons,
        BTreeSet::from([
            "stage_plan_unallocated".to_string(),
            "stage_plan_unknown".to_string(),
        ])
    );
    assert_eq!(after.ledger_events.len(), before.ledger_events.len() + 2);
    store.close().await;
}
