//! AG-141 root stage plan bindings and the same-transaction stage gate.
//! These are local storage tests: no provider, no paid budget, no host claim.

use evo_core::capacity::CapacityItem;
use evo_core::evaluation::{
    Money, OptimizationBudgetPlan, OptimizationStage, RootStageBudgetPlanV1, StageMoneyBudget,
};
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetPreDispatchRefusal,
    BudgetStage, RootBudgetAuthorization, RootStagePlanBindRequest, RootStagePlanEnforcement,
    StagePlanAttribution, StagePlanControlSelector, StagePlanRefusal, StagePlanRefusalKind,
};
use evo_storage::lifecycle::read_control_plane_facts;
use std::path::Path;

const SCOPE: &str = "billing-1";
const SUBJECT: &str = "subject";
const CONTROL_KIND: &str = "development_control_v1";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pv1".into(),
        amount: amount.into(),
    }
}

fn set_stage(
    stages: &mut Vec<StageMoneyBudget>,
    stage: OptimizationStage,
    per_call: &str,
    total: &str,
) {
    let entry = stages
        .iter_mut()
        .find(|entry| entry.stage == stage)
        .expect("stage exists");
    entry.per_call_limit = money(per_call);
    entry.experiment_total = money(total);
}

fn plan_with(root_amount: &str, development: Option<(&str, &str)>) -> RootStageBudgetPlanV1 {
    let mut budget = OptimizationBudgetPlan::unfunded(SCOPE, SUBJECT, "USD").unwrap();
    budget.root_total = money(root_amount);
    budget.admin_authorization_receipt_digest = Some(d("receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit = money("0");
        stage.experiment_total = money("0");
    }
    if let Some((per_call, total)) = development {
        set_stage(
            &mut budget.stages,
            OptimizationStage::DevelopmentExecution,
            per_call,
            total,
        );
    }
    budget.validate().unwrap();
    RootStageBudgetPlanV1::from_optimization_budget(budget)
}

fn bind_request(
    expected_seq: i64,
    plan: RootStageBudgetPlanV1,
    now: i64,
) -> RootStagePlanBindRequest {
    RootStagePlanBindRequest {
        billing_scope: SCOPE.into(),
        expected_seq,
        plan,
        now,
    }
}

fn reservation(
    call_id: &str,
    group: &str,
    stage: BudgetStage,
    cost: i64,
    now: i64,
) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: call_id.into(),
        dispatch_group_id: group.into(),
        stage,
        actual_input_digest: d(&format!("input-{call_id}")),
        request_artifact: None,
        max_cost_micros: cost,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + 100,
        now,
    }
}

fn attribution(namespace: &str, control_id: &str) -> StagePlanAttribution {
    StagePlanAttribution::Control(StagePlanControlSelector {
        control_kind: CONTROL_KIND.into(),
        namespace: namespace.into(),
        control_id: control_id.into(),
    })
}

async fn reserve_planned(
    store: &Store,
    ctx: &Context,
    request: &BudgetCallReservation,
    attribution: &StagePlanAttribution,
) -> evo_storage::budget::BudgetPreDispatchResult<BudgetCallRecord> {
    let mut session = store.session().await.unwrap();
    let outcome = session
        .reserve_budget_call_with_stage_plan_typed(ctx, request, &[], attribution)
        .await
        .unwrap();
    if outcome.is_ok() {
        session.commit().await.unwrap();
    }
    Ok(outcome)
}

async fn authorize(store: &Store, admin: &Context, total: i64) {
    store
        .authorize_root_budget(
            admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec!["n".into(), "n2".into(), "n3".into()],
                currency: "USD".into(),
                pricing_version: "pv1".into(),
                payment_subject: SUBJECT.into(),
                authorization_receipt_digest: d("receipt"),
                per_call_cap_micros: total,
                total_limit_micros: total,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

async fn record_control(store: &Store, ctx: &Context, control_id: &str) {
    let mut session = store.session().await.unwrap();
    session
        .record_root_stage_plan_control(
            ctx,
            SCOPE,
            CONTROL_KIND,
            control_id,
            &d(&format!("control-{control_id}")),
            2,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn table_len(facts: &evo_storage::lifecycle::ControlPlaneFacts, table: &str) -> usize {
    facts
        .root_budget_tables
        .get(table)
        .map_or(0, std::vec::Vec::len)
}

async fn counts(path: &Path) -> (usize, usize, usize, usize) {
    let facts = read_control_plane_facts(path).await.unwrap();
    (
        table_len(&facts, "root_budget_dispatch_groups"),
        table_len(&facts, "root_budget_calls"),
        table_len(&facts, "root_budget_events"),
        table_len(&facts, "root_budget_stage_plan_calls"),
    )
}

#[tokio::test]
async fn binding_compares_every_root_authorization_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;

    let mut cases: Vec<(&str, Box<dyn Fn(&mut RootStageBudgetPlanV1)>)> = vec![
        (
            "payer",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.payer_id = "other".into();
            }),
        ),
        (
            "currency",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.root_total.currency = "EUR".into();
            }),
        ),
        (
            "pricing",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.root_total.pricing_version = "pv2".into();
                for stage in &mut plan.optimization_budget.stages {
                    stage.per_call_limit.pricing_version = "pv2".into();
                    stage.experiment_total.pricing_version = "pv2".into();
                }
            }),
        ),
        (
            "total",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.root_total.amount = "0.02".into();
            }),
        ),
        (
            "receipt",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.admin_authorization_receipt_digest = Some(d("other"));
            }),
        ),
        (
            "scope",
            Box::new(|plan: &mut RootStageBudgetPlanV1| {
                plan.optimization_budget.root_budget_scope_id = "other-scope".into();
            }),
        ),
    ];
    for (label, mutate) in cases.drain(..) {
        let mut plan = plan_with("0.01", Some(("0.002", "0.006")));
        mutate(&mut plan);
        let error = store
            .bind_root_stage_plan(&admin, &bind_request(0, plan, 2))
            .await
            .expect_err(label);
        assert!(
            matches!(
                error,
                Error::Conflict(_) | Error::Invalid(_) | Error::Budget
            ),
            "{label}: {error:?}"
        );
    }
    assert!(
        store
            .root_stage_plan_binding(&admin, SCOPE)
            .await
            .unwrap()
            .is_none()
    );
    store.close().await;
}

#[tokio::test]
async fn tampered_active_plan_fails_closed_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.002", "0.006"))), 2),
        )
        .await
        .unwrap();
    store.close().await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE root_budget_stage_plan_bindings SET plan_digest=? WHERE billing_scope=?")
        .bind(d("tampered"))
        .bind(SCOPE)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let store = Store::open(&path).await.unwrap();
    let error = store
        .root_stage_plan_binding(&admin, SCOPE)
        .await
        .expect_err("a tampered active plan must not read as valid");
    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    store.close().await;
}

#[tokio::test]
async fn binding_cas_appends_one_winner_and_replays_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    let plan = plan_with("0.01", Some(("0.002", "0.006")));
    let first = store
        .bind_root_stage_plan(&admin, &bind_request(0, plan.clone(), 2))
        .await
        .unwrap();
    assert_eq!(first.seq, 1);
    let replay = store
        .bind_root_stage_plan(&admin, &bind_request(0, plan.clone(), 2))
        .await
        .unwrap();
    assert_eq!(replay.seq, 1);
    assert_eq!(replay.plan_digest, first.plan_digest);
    let stale = store
        .bind_root_stage_plan(&admin, &bind_request(0, plan_with("0.01", None), 2))
        .await;
    assert!(
        matches!(stale, Err(Error::Conflict(message)) if message == "root_stage_plan_version_already_bound")
    );
    let second = store
        .bind_root_stage_plan(&admin, &bind_request(1, plan.clone(), 3))
        .await
        .unwrap();
    assert_eq!(second.seq, 2);
    assert_eq!(
        store
            .root_stage_plan_binding(&admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .seq,
        2
    );
    // An old exact replay still returns its own version after newer ones exist.
    let old = store
        .bind_root_stage_plan(&admin, &bind_request(0, plan, 2))
        .await
        .unwrap();
    assert_eq!(old.seq, 1);
    store.close().await;
}

#[tokio::test]
async fn binding_versions_stop_at_one_hundred_without_blocking_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    let plan = plan_with("0.01", Some(("0.002", "0.006")));
    for seq in 0..100 {
        let binding = store
            .bind_root_stage_plan(&admin, &bind_request(seq, plan.clone(), 2))
            .await
            .unwrap();
        assert_eq!(binding.seq, seq + 1);
    }
    let overflow = store
        .bind_root_stage_plan(&admin, &bind_request(100, plan.clone(), 2))
        .await;
    assert!(matches!(
        overflow,
        Err(Error::Capacity(conflict))
            if conflict.item == CapacityItem::RootStagePlanVersionPerRoot
                && conflict.used == 100
                && conflict.limit == 100
    ));
    let replay = store
        .bind_root_stage_plan(&admin, &bind_request(99, plan, 2))
        .await
        .unwrap();
    assert_eq!(replay.seq, 100);
    store.close().await;
}

#[tokio::test]
async fn binding_switch_preserves_stage_usage_and_refuses_shrink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    let first = reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-1",
            "group-1",
            BudgetStage::DevelopmentExecution,
            3_000,
            10,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap();
    assert!(first.is_ok());
    let shrink = store
        .bind_root_stage_plan(
            &admin,
            &bind_request(1, plan_with("0.01", Some(("0.001", "0.002"))), 2),
        )
        .await;
    assert!(
        matches!(shrink, Err(Error::Conflict(message)) if message == "root_stage_plan_below_occupied_stage_usage")
    );
    assert_eq!(
        store
            .root_stage_plan_binding(&admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .seq,
        1
    );
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(1, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-2").await;
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    assert_eq!(report.binding.as_ref().unwrap().seq, 2);
    let development = report
        .stages
        .iter()
        .find(|stage| stage.stage == BudgetStage::DevelopmentExecution)
        .unwrap();
    assert_eq!(development.allocated_micros, Some(6_000));
    assert_eq!(development.occupied_micros, 3_000);
    store.close().await;
}

#[tokio::test]
async fn bound_root_refuses_unattributed_calls_as_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    let refusal = reserve_planned(
        &store,
        &worker,
        &reservation("call-1", "group-1", BudgetStage::Reflection, 1_000, 10),
        &StagePlanAttribution::Unattributed,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            stage: BudgetStage::Reflection,
            kind: StagePlanRefusalKind::Unknown
        }
    ));
    store.close().await;
}

#[tokio::test]
async fn stage_gate_precedes_group_call_event_and_ref_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    store.close().await;
    let before = counts(&path).await;
    let store = Store::open(&path).await.unwrap();
    let refusal = reserve_planned(
        &store,
        &worker,
        &reservation("call-1", "group-1", BudgetStage::Practice, 1_000, 10),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            stage: BudgetStage::Practice,
            kind: StagePlanRefusalKind::Unallocated
        }
    ));
    store.close().await;
    assert_eq!(counts(&path).await, before);
    let store = Store::open(&path).await.unwrap();
    let mut events = store.session().await.unwrap();
    events
        .append_stage_plan_refusal(
            &worker,
            &StagePlanRefusal {
                billing_scope: SCOPE.into(),
                stage: BudgetStage::Practice,
                kind: StagePlanRefusalKind::Unallocated,
                now: 11,
            },
        )
        .await
        .unwrap();
    events.commit().await.unwrap();
    store.close().await;
    let after = counts(&path).await;
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, before.1);
    assert_eq!(after.3, before.3);
    assert_eq!(after.2, before.2 + 1);
}

#[tokio::test]
async fn stage_usage_spans_namespaces_groups_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let worker2 = Context::new("n2", "worker2", Role::Worker).unwrap();
    let admin2 = Context::new("n2", "admin2", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.006", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    record_control(&store, &admin2, "control-2").await;
    for (ctx, namespace, control, call, group, cost, now) in [
        (&worker, "n", "control-1", "call-1", "group-1", 3_000, 10),
        (&worker2, "n2", "control-2", "call-2", "group-2", 3_000, 11),
    ] {
        let reserved = reserve_planned(
            &store,
            ctx,
            &reservation(call, group, BudgetStage::DevelopmentExecution, cost, now),
            &attribution(namespace, control),
        )
        .await
        .unwrap();
        assert!(reserved.is_ok());
    }
    store.close().await;
    let store = Store::open(&path).await.unwrap();
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    let development = report
        .stages
        .iter()
        .find(|stage| stage.stage == BudgetStage::DevelopmentExecution)
        .unwrap();
    assert_eq!(development.occupied_micros, 6_000);
    let exhausted = reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-3",
            "group-3",
            BudgetStage::DevelopmentExecution,
            1,
            12,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        exhausted,
        BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::Exhausted,
            ..
        }
    ));
    store.close().await;
}

#[tokio::test]
async fn uncertain_keeps_reserved_stage_amount() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.006", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    let call = reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-1",
            "group-1",
            BudgetStage::DevelopmentExecution,
            2_000,
            10,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap()
    .unwrap();
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 10,
    };
    store
        .begin_budget_dispatch_typed(&worker, &fence)
        .await
        .unwrap()
        .unwrap();
    store
        .mark_budget_call_uncertain(&worker, &fence, "usage_unknown")
        .await
        .unwrap();
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    let development = report
        .stages
        .iter()
        .find(|stage| stage.stage == BudgetStage::DevelopmentExecution)
        .unwrap();
    assert_eq!(development.occupied_micros, 2_000);
    let blocked = reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-2",
            "group-2",
            BudgetStage::DevelopmentExecution,
            5_000,
            12,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap();
    assert!(matches!(
        blocked,
        Err(BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::Exhausted,
            ..
        })
    ));
    store.close().await;
}

#[tokio::test]
async fn zero_and_missing_allocations_refuse_by_stage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    let mut plan = plan_with("0.01", Some(("0", "0")));
    plan.curriculum = Some(evo_core::evaluation::StageMoneyCaps {
        per_call_limit: money("0"),
        experiment_total: money("0"),
    });
    store
        .bind_root_stage_plan(&admin, &bind_request(0, plan, 2))
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    for (stage, call, group) in [
        (BudgetStage::Curriculum, "call-1", "group-1"),
        (BudgetStage::CandidateGeneration, "call-2", "group-2"),
        (BudgetStage::DevelopmentExecution, "call-3", "group-3"),
    ] {
        let refusal = reserve_planned(
            &store,
            &worker,
            &reservation(call, group, stage, 1, 10),
            &attribution("n", "control-1"),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(matches!(
            refusal,
            BudgetPreDispatchRefusal::StagePlan {
                kind: StagePlanRefusalKind::Unallocated,
                ..
            }
        ));
    }
    store.close().await;
}

#[tokio::test]
async fn acceptance_stages_require_only_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(&admin, &bind_request(0, plan_with("0.01", None), 2))
        .await
        .unwrap();
    for (stage, call, group) in [
        (BudgetStage::TaskExecution, "call-1", "group-1"),
        (BudgetStage::FormalEvaluation, "call-2", "group-2"),
    ] {
        let reserved = reserve_planned(
            &store,
            &worker,
            &reservation(call, group, stage, 100, 10),
            &StagePlanAttribution::Unattributed,
        )
        .await
        .unwrap();
        assert!(reserved.is_ok(), "{stage:?}");
    }
    store.close().await;
}

#[tokio::test]
async fn unbound_root_is_not_enforced_and_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    let reserved = reserve_planned(
        &store,
        &worker,
        &reservation("call-1", "group-1", BudgetStage::Reflection, 100, 10),
        &StagePlanAttribution::Unattributed,
    )
    .await
    .unwrap();
    assert!(reserved.is_ok());
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    assert_eq!(report.enforcement, RootStagePlanEnforcement::NoBinding);
    assert!(report.binding.is_none());
    assert!(
        report
            .stages
            .iter()
            .all(|stage| stage.allocated_micros.is_none())
    );
    assert_eq!(report.course_share_bps, 2_000);
    assert!(!report.course_producer_wired);
    assert_eq!(report.course_occupied_micros, 0);
    store.close().await;
}

#[tokio::test]
async fn old_call_is_not_reinterpreted_at_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.006", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    let call = reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-1",
            "group-1",
            BudgetStage::DevelopmentExecution,
            2_000,
            10,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap()
    .unwrap();
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(1, plan_with("0.01", Some(("0.006", "0.006"))), 3),
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 10,
    };
    let refusal = store
        .begin_budget_dispatch_typed(&worker, &fence)
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::Inactive,
            ..
        }
    ));
    store.close().await;
}

#[tokio::test]
async fn report_counts_refusals_by_stage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    for now in [10, 11] {
        let mut session = store.session().await.unwrap();
        session
            .append_stage_plan_refusal(
                &worker,
                &StagePlanRefusal {
                    billing_scope: SCOPE.into(),
                    stage: BudgetStage::Practice,
                    kind: StagePlanRefusalKind::Unallocated,
                    now,
                },
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    assert_eq!(report.refusals.len(), 1);
    assert_eq!(report.refusals[0].stage, BudgetStage::Practice);
    assert_eq!(report.refusals[0].reason, "stage_plan_unallocated");
    assert_eq!(report.refusals[0].count, 2);
    assert_eq!(report.enforcement, RootStagePlanEnforcement::Enforced);
    store.close().await;
}
