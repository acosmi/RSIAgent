//! AG-141 `rsia.root_stage_budget_plan.v1` contract: the original 11-stage
//! plan stays byte-identical, and the four named extras are exact integer
//! micros with `None` distinct from an allocated zero.

use evo_core::evaluation::{
    Money, OPTIMIZATION_BUDGET_SCHEMA, OptimizationBudgetPlan, OptimizationStage,
    ROOT_STAGE_BUDGET_PLAN_SCHEMA, RootStageBudgetPlanV1, StageMoneyBudget, StageMoneyCaps,
    money_micros,
};

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pv1".into(),
        amount: amount.into(),
    }
}

fn old_plan(root: &str) -> OptimizationBudgetPlan {
    let mut plan = OptimizationBudgetPlan::unfunded("billing-1", "subject", "USD").unwrap();
    plan.root_total = money(root);
    plan.admin_authorization_receipt_digest = Some("a".repeat(64));
    for stage in &mut plan.stages {
        stage.per_call_limit = money("0");
        stage.experiment_total = money("0");
    }
    plan.validate().unwrap();
    plan
}

fn set_stage(
    stages: &mut [StageMoneyBudget],
    stage: OptimizationStage,
    per_call: &str,
    total: &str,
) {
    let entry = stages
        .iter_mut()
        .find(|entry| entry.stage == stage)
        .unwrap();
    entry.per_call_limit = money(per_call);
    entry.experiment_total = money(total);
}

#[test]
fn the_old_v1_schema_and_eleven_stages_are_unchanged() {
    assert_eq!(
        OPTIMIZATION_BUDGET_SCHEMA,
        "rsia.optimization_budget_plan.v1"
    );
    assert_eq!(
        ROOT_STAGE_BUDGET_PLAN_SCHEMA,
        "rsia.root_stage_budget_plan.v1"
    );
    let plan = old_plan("0.01");
    assert_eq!(plan.schema_version, OPTIMIZATION_BUDGET_SCHEMA);
    assert_eq!(plan.stages.len(), 11);
    let stages: Vec<_> = plan.stages.iter().map(|stage| stage.stage).collect();
    assert_eq!(
        stages,
        vec![
            OptimizationStage::HistoryCollection,
            OptimizationStage::Reflection,
            OptimizationStage::Merge,
            OptimizationStage::Ranking,
            OptimizationStage::DevelopmentExecution,
            OptimizationStage::DevelopmentScoring,
            OptimizationStage::Practice,
            OptimizationStage::Consolidation,
            OptimizationStage::Guidance,
            OptimizationStage::HumanReview,
            OptimizationStage::StorageCpu,
        ]
    );
    let embedded = RootStageBudgetPlanV1::from_optimization_budget(plan.clone());
    embedded.validate().unwrap();
    assert_eq!(
        embedded.optimization_budget.digest().unwrap(),
        plan.digest().unwrap()
    );
}

#[test]
fn fifteen_stage_mapping_and_none_is_not_zero() {
    let plan = old_plan("0.01");
    let mut current = RootStageBudgetPlanV1::from_optimization_budget(plan);
    assert!(current.curriculum.is_none());
    assert!(current.candidate_generation.is_none());
    assert!(current.meta_evaluation.is_none());
    assert!(current.gray_operations.is_none());
    current.curriculum = Some(StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.002"),
    });
    current.candidate_generation = Some(StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.002"),
    });
    current.meta_evaluation = Some(StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.002"),
    });
    current.gray_operations = Some(StageMoneyCaps {
        per_call_limit: money("0"),
        experiment_total: money("0"),
    });
    current.validate().unwrap();
    // The allocated zero is distinct from an absent allocation.
    assert!(current.gray_operations.is_some());
    assert!(current.curriculum.is_some());
}

#[test]
fn money_is_exact_integer_micros() {
    assert_eq!(money_micros(&money("0.000001"), "x").unwrap(), 1);
    assert_eq!(money_micros(&money("1"), "x").unwrap(), 1_000_000);
    assert!(money_micros(&money("0.00000001"), "x").is_err());
    assert!(money_micros(&money("0.00100001"), "x").is_err());
    assert!(money_micros(&money("01"), "x").is_err());
    assert!(money_micros(&money(&format!("{}", i64::MAX)), "x").is_err());
}

#[test]
fn plan_rejects_residual_overflow_and_ordering() {
    let plan = old_plan("0.01");
    let mut residual = RootStageBudgetPlanV1::from_optimization_budget(plan.clone());
    residual.curriculum = Some(StageMoneyCaps {
        per_call_limit: money("0.00000001"),
        experiment_total: money("0.00000001"),
    });
    assert!(residual.validate().is_err());
    let mut ordering = RootStageBudgetPlanV1::from_optimization_budget(plan.clone());
    ordering.curriculum = Some(StageMoneyCaps {
        per_call_limit: money("0.003"),
        experiment_total: money("0.002"),
    });
    assert!(ordering.validate().is_err());
    let mut sum = RootStageBudgetPlanV1::from_optimization_budget(plan.clone());
    let cap = || StageMoneyCaps {
        per_call_limit: money("0.004"),
        experiment_total: money("0.004"),
    };
    sum.curriculum = Some(cap());
    sum.candidate_generation = Some(cap());
    sum.meta_evaluation = Some(cap());
    sum.gray_operations = Some(cap());
    assert!(sum.validate().is_err());
    let mut wrong_pricing = RootStageBudgetPlanV1::from_optimization_budget(plan);
    wrong_pricing.curriculum = Some(StageMoneyCaps {
        per_call_limit: Money {
            currency: "USD".into(),
            pricing_version: "pv2".into(),
            amount: "0.001".into(),
        },
        experiment_total: money("0.001"),
    });
    assert!(wrong_pricing.validate().is_err());
}

#[test]
fn a_stage_total_may_equal_the_root() {
    let plan = old_plan("0.01");
    let mut current = RootStageBudgetPlanV1::from_optimization_budget(plan);
    set_stage(
        &mut current.optimization_budget.stages,
        OptimizationStage::Reflection,
        "0.01",
        "0.01",
    );
    current.validate().unwrap();
}

#[test]
fn formal_plan_versions_bind_distinct_budget_preimages() {
    let old = old_plan("0.01");
    let first = RootStageBudgetPlanV1::from_optimization_budget(old.clone());
    let mut second = first.clone();
    second.curriculum = Some(StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.002"),
    });
    second.validate().unwrap();
    // The new version has its own digest while the embedded original 11-stage
    // plan and its digest stay byte-identical.
    assert_ne!(first.digest().unwrap(), second.digest().unwrap());
    assert_eq!(
        first.optimization_budget.digest().unwrap(),
        second.optimization_budget.digest().unwrap()
    );
    assert_eq!(
        serde_json::to_string(&first.optimization_budget).unwrap(),
        serde_json::to_string(&second.optimization_budget).unwrap()
    );
    assert_eq!(
        first.optimization_budget.digest().unwrap(),
        old.digest().unwrap()
    );
}
