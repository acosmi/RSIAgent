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
