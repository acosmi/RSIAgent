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
    BudgetStage, E05TicketWitness, RootBudgetAuthorization, RootStagePlanBindRequest,
    RootStagePlanEnforcement, StagePlanAttribution, StagePlanControlSelector, StagePlanRefusal,
    StagePlanRefusalKind,
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
    stages: &mut [StageMoneyBudget],
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
    _now: i64,
) -> RootStagePlanBindRequest {
    RootStagePlanBindRequest {
        billing_scope: SCOPE.into(),
        expected_seq,
        plan,
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
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn ticket_storage_id(id: &str) -> String {
    format!(
        "e05-{}",
        evo_core::fingerprint(&("evaluation_ticket_v2", id)).unwrap()
    )
}

fn table_len(facts: &evo_storage::lifecycle::ControlPlaneFacts, table: &str) -> usize {
    facts
        .root_budget_tables
        .get(table)
        .map_or(0, std::vec::Vec::len)
}

async fn counts(path: &Path) -> (usize, usize, usize, usize, i64, i64) {
    let facts = read_control_plane_facts(path).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_micros FROM root_budgets WHERE billing_scope=?")
            .bind(SCOPE)
            .fetch_one(&pool)
            .await
            .unwrap();
    let call_refs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects WHERE namespace='n' AND json_valid(body)
         AND json_extract(body,'$.schema_version')='rsia.budget_call_ref.v1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    (
        table_len(&facts, "root_budget_dispatch_groups"),
        table_len(&facts, "root_budget_calls"),
        table_len(&facts, "root_budget_events"),
        table_len(&facts, "root_budget_stage_plan_calls"),
        reserved,
        call_refs,
    )
}

#[tokio::test]
async fn binding_compares_every_root_authorization_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;

    for (label, expected) in [
        ("payer", "root_stage_plan_payer_mismatch"),
        ("currency", "root_stage_plan_currency_or_pricing_mismatch"),
        ("pricing", "root_stage_plan_currency_or_pricing_mismatch"),
        ("total", "root_stage_plan_root_total_mismatch"),
        ("receipt", "root_stage_plan_receipt_mismatch"),
        ("scope", "root_stage_plan_scope_mismatch"),
    ] {
        let mut plan = plan_with("0.01", Some(("0.002", "0.006")));
        match label {
            "payer" => plan.optimization_budget.payer_id = "other".into(),
            "currency" => {
                // Keep the plan internally consistent so validation cannot
                // reject it before the binding gate compares it with the root.
                plan.optimization_budget.root_total.currency = "EUR".into();
                for stage in &mut plan.optimization_budget.stages {
                    stage.per_call_limit.currency = "EUR".into();
                    stage.experiment_total.currency = "EUR".into();
                }
            }
            "pricing" => {
                plan.optimization_budget.root_total.pricing_version = "pv2".into();
                for stage in &mut plan.optimization_budget.stages {
                    stage.per_call_limit.pricing_version = "pv2".into();
                    stage.experiment_total.pricing_version = "pv2".into();
                }
            }
            "total" => plan.optimization_budget.root_total.amount = "0.02".into(),
            "receipt" => {
                plan.optimization_budget.admin_authorization_receipt_digest = Some(d("other"));
            }
            "scope" => plan.optimization_budget.root_budget_scope_id = "other-scope".into(),
            _ => unreachable!(),
        }
        // Every mutation above is a valid plan on its own; the only refusal is
        // the exact root-authorization comparison in the binding gate.
        plan.validate().unwrap_or_else(|error| {
            panic!("{label}: the mutation must pass plan validation first: {error:?}")
        });
        let error = store
            .bind_root_stage_plan(&admin, &bind_request(0, plan, 2))
            .await
            .expect_err(label);
        assert!(
            matches!(&error, Error::Conflict(message) if message == expected),
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
        (BudgetStage::MetaEvaluation, "call-4", "group-4"),
        (BudgetStage::GrayOperations, "call-5", "group-5"),
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
async fn acceptance_stages_require_an_e05_ticket_not_a_stage_label() {
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
    let before = counts(&path).await;
    for (stage, call, group) in [
        (BudgetStage::TaskExecution, "call-1", "group-1"),
        (BudgetStage::FormalEvaluation, "call-2", "group-2"),
    ] {
        let refusal = reserve_planned(
            &store,
            &worker,
            &reservation(call, group, stage, 100, 10),
            &StagePlanAttribution::Unattributed,
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            matches!(
                refusal,
                BudgetPreDispatchRefusal::StagePlan {
                    kind: StagePlanRefusalKind::E05Required,
                    ..
                }
            ),
            "{stage:?}: {refusal:?}"
        );
    }
    assert_eq!(counts(&path).await, before);

    // A witness whose ticket object was never persisted is still refused.
    let missing = StagePlanAttribution::E05Ticket(E05TicketWitness {
        ticket_kind: "evaluation_ticket_v2".into(),
        ticket_id: "ticket-missing".into(),
        ticket_storage_id: ticket_storage_id("ticket-missing"),
        ticket_digest: d("ticket-missing"),
    });
    let refusal = reserve_planned(
        &store,
        &worker,
        &reservation("call-3", "group-3", BudgetStage::TaskExecution, 100, 10),
        &missing,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::E05Required,
            ..
        }
    ));

    // A real persisted ticket object is the storage-side witness. Only
    // TaskExecution has an engine producer today; FormalEvaluation has none and
    // is not claimed. Storage binds the witness to the persisted body digest
    // and to the one formal ticket record kind; the engine separately verified
    // the ticket content and its frozen task view.
    let storage_id = ticket_storage_id("ticket-1");
    let stored_body = serde_json::json!({
        "id": storage_id,
        "schema_version": "rsia.typed_artifact_envelope.v1",
        "record_kind": "evaluation_ticket_v2",
        "payload": {"id": "ticket-1"},
    });
    let stored_digest = evo_core::canonical_fingerprint(&stored_body).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", &storage_id, admin.actor(), &stored_body)
        .await
        .unwrap();
    session.commit().await.unwrap();

    // A witness for the real object with a wrong digest is refused, zero write.
    let tampered = StagePlanAttribution::E05Ticket(E05TicketWitness {
        ticket_kind: "evaluation_ticket_v2".into(),
        ticket_id: "ticket-1".into(),
        ticket_storage_id: storage_id.clone(),
        ticket_digest: d("tampered-ticket-digest"),
    });
    let refusal = reserve_planned(
        &store,
        &worker,
        &reservation("call-3b", "group-3b", BudgetStage::TaskExecution, 100, 10),
        &tampered,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::E05Required,
            ..
        }
    ));

    // Another artifact record kind with the same logical payload id and a
    // matching body digest is still refused: storage pins the ticket kind.
    let decoy_kind = "evaluation_ticket_v1";
    let decoy_storage_id = format!(
        "e05-{}",
        evo_core::fingerprint(&(decoy_kind, "ticket-1")).unwrap()
    );
    let decoy_body = serde_json::json!({
        "id": decoy_storage_id,
        "schema_version": "rsia.typed_artifact_envelope.v1",
        "record_kind": decoy_kind,
        "payload": {"id": "ticket-1"},
    });
    let decoy_digest = evo_core::canonical_fingerprint(&decoy_body).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &decoy_storage_id,
            admin.actor(),
            &decoy_body,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let wrong_kind = StagePlanAttribution::E05Ticket(E05TicketWitness {
        ticket_kind: decoy_kind.into(),
        ticket_id: "ticket-1".into(),
        ticket_storage_id: decoy_storage_id,
        ticket_digest: decoy_digest,
    });
    let refusal = reserve_planned(
        &store,
        &worker,
        &reservation("call-3c", "group-3c", BudgetStage::TaskExecution, 100, 10),
        &wrong_kind,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        refusal,
        BudgetPreDispatchRefusal::StagePlan {
            kind: StagePlanRefusalKind::E05Required,
            ..
        }
    ));
    assert_eq!(counts(&path).await, before);

    // The refusal event API itself refuses acceptance stages: the fixed
    // `stage_plan_e05_ticket_required` code is a return value only and never
    // becomes a `stage_plan_refused` event or a report count.
    let mut session = store.session().await.unwrap();
    let error = session
        .append_stage_plan_refusal(
            &worker,
            &StagePlanRefusal {
                billing_scope: SCOPE.into(),
                stage: BudgetStage::TaskExecution,
                kind: StagePlanRefusalKind::E05Required,
            },
        )
        .await
        .expect_err("acceptance stages do not carry a plan refusal event");
    assert!(matches!(&error, Error::Invalid(message)
        if message == "acceptance stages do not carry a stage plan refusal"));
    drop(session);
    assert_eq!(counts(&path).await, before);

    let ticket = StagePlanAttribution::E05Ticket(E05TicketWitness {
        ticket_kind: "evaluation_ticket_v2".into(),
        ticket_id: "ticket-1".into(),
        ticket_storage_id: storage_id,
        ticket_digest: stored_digest,
    });
    let reserved = reserve_planned(
        &store,
        &worker,
        &reservation("call-4", "group-4", BudgetStage::TaskExecution, 100, 10),
        &ticket,
    )
    .await
    .unwrap();
    assert!(reserved.is_ok(), "{reserved:?}");
    let after = counts(&path).await;
    assert_eq!(after.0, before.0 + 1);
    assert_eq!(after.1, before.1 + 1);
    // Acceptance stages never consume a stage allocation or relation.
    assert_eq!(after.3, before.3);
    assert_eq!(after.4, before.4 + 100);
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
    let before_clock = evo_core::now().unwrap();
    for _ in [10, 11] {
        let mut session = store.session().await.unwrap();
        session
            .append_stage_plan_refusal(
                &worker,
                &StagePlanRefusal {
                    billing_scope: SCOPE.into(),
                    stage: BudgetStage::Practice,
                    kind: StagePlanRefusalKind::Unallocated,
                },
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    let after_clock = evo_core::now().unwrap();
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    assert_eq!(report.refusals.len(), 1);
    assert_eq!(report.refusals[0].stage, BudgetStage::Practice);
    assert_eq!(report.refusals[0].reason, "stage_plan_unallocated");
    assert_eq!(report.refusals[0].count, 2);
    assert_eq!(report.enforcement, RootStagePlanEnforcement::Enforced);
    assert_eq!(report.schema_version, "rsia.root_stage_plan_report.v1");
    assert_eq!(report.binding.as_ref().unwrap().seq, 1);
    assert_eq!(report.course_occupied_micros, 0);
    // `stage_plan_refused.event_at` uses the same-Session trusted clock, never
    // the fixture's now=10/11 and never the caller's time.
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, SCOPE)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let refusals: Vec<_> = snapshot
        .ledger_events
        .iter()
        .filter(|event| event.event_kind == "stage_plan_refused")
        .collect();
    assert_eq!(refusals.len(), 2);
    for event in refusals {
        assert!(
            event.event_at >= before_clock && event.event_at <= after_clock,
            "event_at {} is outside the trusted interval [{before_clock}, {after_clock}]",
            event.event_at
        );
        assert_ne!(event.event_at, 10);
        assert_ne!(event.event_at, 11);
    }
    store.close().await;
}

#[tokio::test]
async fn binding_and_relations_use_the_trusted_clock_not_the_fixture_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    let before = evo_core::now().unwrap();
    let binding = store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    let after = evo_core::now().unwrap();
    assert!(binding.created_at >= before && binding.created_at <= after);
    assert_ne!(binding.created_at, 2);
    let before = evo_core::now().unwrap();
    let mut session = store.session().await.unwrap();
    let relation = session
        .record_root_stage_plan_control(
            &admin,
            SCOPE,
            CONTROL_KIND,
            "control-1",
            &d("control-control-1"),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let after = evo_core::now().unwrap();
    assert!(relation.created_at >= before && relation.created_at <= after);
    assert_ne!(relation.created_at, 2);
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    reserve_planned(
        &store,
        &worker,
        &reservation(
            "call-1",
            "group-1",
            BudgetStage::DevelopmentExecution,
            1_000,
            10,
        ),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap()
    .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let created_at: i64 = sqlx::query_scalar(
        "SELECT created_at FROM root_budget_stage_plan_calls WHERE billing_scope=? AND call_id=?",
    )
    .bind(SCOPE)
    .bind("call-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    assert!(created_at >= before && created_at <= after);
    assert_ne!(created_at, 10);
    store.close().await;
}

#[tokio::test]
async fn binding_digest_recomputes_from_the_full_stored_plan() {
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
    let version = store
        .root_stage_plan_binding_version(&admin, SCOPE, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version.plan.digest().unwrap(), version.plan_digest);
    store.close().await;
}

#[tokio::test]
async fn concurrent_binding_transactions_have_exactly_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    let mut other = plan_with("0.01", Some(("0.002", "0.006")));
    other.curriculum = Some(evo_core::evaluation::StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.001"),
    });
    other.validate().unwrap();
    let first_request = bind_request(0, plan_with("0.01", Some(("0.002", "0.006"))), 2);
    let second_request = bind_request(0, other, 2);
    let first = store.bind_root_stage_plan(&admin, &first_request);
    let second = store.bind_root_stage_plan(&admin, &second_request);
    let (first, second) = tokio::join!(first, second);
    let winners = usize::from(first.is_ok()) + usize::from(second.is_ok());
    assert_eq!(winners, 1, "first={first:?} second={second:?}");
    let versions = store
        .root_stage_plan_binding_versions(&admin, SCOPE)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    store.close().await;
}

/// Two truly independent Stores (each a separate SQLite pool/connection on the
/// same file) race on the same `(billing_scope, seq)` key. Exactly one append
/// wins and the loser writes nothing. Which fixed storage refusal the loser
/// sees depends on interleaving: the unique-key CAS conflict, the already-bound
/// version when it reads after the winner commits, the expected-seq mismatch, or
/// SQLite's WAL snapshot/busy refusal when it upgrades a stale read transaction.
/// The deterministic unique-branch mapping is pinned by the trigger test below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_stores_have_exactly_one_winner_and_no_partial_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let first_store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&first_store, &admin, 10_000).await;
    let before = counts(&path).await;
    let second_store = Store::open(&path).await.unwrap();
    let mut other = plan_with("0.01", Some(("0.002", "0.006")));
    other.curriculum = Some(evo_core::evaluation::StageMoneyCaps {
        per_call_limit: money("0.001"),
        experiment_total: money("0.001"),
    });
    other.validate().unwrap();
    let first_request = bind_request(0, plan_with("0.01", Some(("0.002", "0.006"))), 2);
    let second_request = bind_request(0, other, 2);
    let (first, second) = tokio::join!(
        first_store.bind_root_stage_plan(&admin, &first_request),
        second_store.bind_root_stage_plan(&admin, &second_request)
    );
    let winners = usize::from(first.is_ok()) + usize::from(second.is_ok());
    assert_eq!(winners, 1, "first={first:?} second={second:?}");
    let loser = if first.is_err() { &first } else { &second };
    assert!(
        matches!(
            loser,
            Err(Error::Conflict(message))
                if message == "root_stage_plan_seq_cas_conflict"
                    || message == "root_stage_plan_version_already_bound"
                    || message == "root_stage_plan_expected_seq_mismatch"
        ) || matches!(loser, Err(Error::Internal)),
        "{loser:?}"
    );
    let versions = first_store
        .root_stage_plan_binding_versions(&admin, SCOPE)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].seq, 1);
    // The failed append left no group, call, ledger event, stage-plan relation,
    // reservation or call reference behind.
    assert_eq!(counts(&path).await, before);
    first_store.close().await;
    second_store.close().await;
}

/// Deterministically reaches the `is_unique_violation` branch at the binding
/// INSERT. A temporary BEFORE INSERT trigger inserts a row with the same
/// `(billing_scope, seq)` as the statement's own row. The outer INSERT then
/// raises SQLITE_CONSTRAINT_UNIQUE, which storage must map to the fixed
/// `root_stage_plan_seq_cas_conflict` error. The statement rollback also removes
/// the trigger's row, so the failed attempt leaves no partial binding or other
/// root-budget write.
#[tokio::test]
async fn binding_unique_conflict_maps_to_the_fixed_cas_error_without_partial_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER inject_binding_conflict
         BEFORE INSERT ON root_budget_stage_plan_bindings
         BEGIN
           INSERT INTO root_budget_stage_plan_bindings(
             billing_scope,seq,plan_schema,plan_digest,plan_body,payer_id,
             root_total_micros,currency,pricing_version,authorization_receipt_digest,
             authorizing_namespace,bound_by_actor,created_at)
           VALUES(
             NEW.billing_scope,NEW.seq,NEW.plan_schema,NEW.plan_digest,NEW.plan_body,
             NEW.payer_id,NEW.root_total_micros,NEW.currency,NEW.pricing_version,
             NEW.authorization_receipt_digest,NEW.authorizing_namespace,
             NEW.bound_by_actor,NEW.created_at);
         END",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let before = counts(&path).await;
    let error = store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.002", "0.006"))), 2),
        )
        .await
        .expect_err("the injected duplicate must lose the CAS");
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "root_stage_plan_seq_cas_conflict"),
        "{error:?}"
    );
    assert!(
        store
            .root_stage_plan_binding(&admin, SCOPE)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(counts(&path).await, before);
    store.close().await;
}

#[tokio::test]
async fn caller_cannot_authorize_a_dispatch_with_a_claimed_control() {
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
    let before = counts(&path).await;
    for (namespace, control) in [("n", "ghost"), ("n2", "control-1")] {
        let refusal = reserve_planned(
            &store,
            &worker,
            &reservation(
                "call-1",
                "group-1",
                BudgetStage::DevelopmentExecution,
                1,
                10,
            ),
            &attribution(namespace, control),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            matches!(
                refusal,
                BudgetPreDispatchRefusal::StagePlan {
                    kind: StagePlanRefusalKind::Unknown,
                    ..
                }
            ),
            "{namespace}/{control}: {refusal:?}"
        );
    }
    assert_eq!(counts(&path).await, before);
    store.close().await;
}

#[tokio::test]
async fn control_relation_is_immutable_after_a_plan_switch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(1, plan_with("0.01", Some(("0.003", "0.006"))), 3),
        )
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let error = session
        .record_root_stage_plan_control(
            &admin,
            SCOPE,
            CONTROL_KIND,
            "control-1",
            &d("control-control-1"),
        )
        .await
        .expect_err("a stale relation must not follow the new active version");
    assert!(
        matches!(&error, Error::Conflict(message) if message == "root_stage_plan_control_ref_is_immutable"),
        "{error:?}"
    );
    let relation = session
        .root_stage_plan_control_ref(&admin, CONTROL_KIND, "control-1")
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(relation.binding_seq, 1);
    store.close().await;
}

#[tokio::test]
async fn course_producer_ref_has_no_writer_and_is_not_fabricated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    authorize(&store, &admin, 10_000).await;
    let mut plan = plan_with("0.01", None);
    plan.curriculum = Some(evo_core::evaluation::StageMoneyCaps {
        per_call_limit: money("0.002"),
        experiment_total: money("0.004"),
    });
    plan.validate().unwrap();
    store
        .bind_root_stage_plan(&admin, &bind_request(0, plan, 2))
        .await
        .unwrap();
    record_control(&store, &admin, "control-1").await;
    let reserved = reserve_planned(
        &store,
        &worker,
        &reservation("call-1", "group-1", BudgetStage::Curriculum, 2_000, 10),
        &attribution("n", "control-1"),
    )
    .await
    .unwrap();
    assert!(reserved.is_ok());
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let course_ref: Option<String> = sqlx::query_scalar(
        "SELECT course_producer_ref FROM root_budget_stage_plan_calls WHERE billing_scope=? AND call_id=?",
    )
    .bind(SCOPE)
    .bind("call-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    assert!(course_ref.is_none());
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    assert_eq!(report.course_share_bps, 2_000);
    assert!(!report.course_producer_wired);
    assert_eq!(report.course_occupied_micros, 0);
    store.close().await;
}

#[tokio::test]
async fn binding_versions_are_readable_and_old_versions_stay_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    for seq in 0..3 {
        store
            .bind_root_stage_plan(
                &admin,
                &bind_request(seq, plan_with("0.01", Some(("0.003", "0.006"))), 2),
            )
            .await
            .unwrap();
    }
    let versions = store
        .root_stage_plan_binding_versions(&admin, SCOPE)
        .await
        .unwrap();
    assert_eq!(
        versions
            .iter()
            .map(|version| version.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let first = store
        .root_stage_plan_binding_version(&admin, SCOPE, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.seq, 1);
    assert_eq!(first.plan.digest().unwrap(), first.plan_digest);
    store.close().await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE root_budget_stage_plan_bindings SET plan_digest=? WHERE billing_scope=? AND seq=2",
    )
    .bind(d("tampered"))
    .bind(SCOPE)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let store = Store::open(&path).await.unwrap();
    let error = store
        .root_stage_plan_binding_version(&admin, SCOPE, 2)
        .await
        .expect_err("a tampered old version must not read as valid");
    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    store.close().await;
}

#[tokio::test]
async fn undispatched_release_preserves_existing_accounting_rules() {
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
        now: 11,
    };
    let released = store
        .release_undispatched_budget_call(&worker, &fence, "test_release")
        .await
        .unwrap();
    assert_eq!(
        released.state,
        evo_storage::budget::BudgetCallState::Released
    );
    let report = store.root_stage_plan_report(&admin, SCOPE).await.unwrap();
    let development = report
        .stages
        .iter()
        .find(|stage| stage.stage == BudgetStage::DevelopmentExecution)
        .unwrap();
    assert_eq!(development.occupied_micros, 0);
    let facts = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(table_len(&facts, "root_budget_stage_plan_calls"), 1);
    store.close().await;
}

#[tokio::test]
async fn control_relation_follows_new_content_only_within_the_same_active_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    authorize(&store, &admin, 10_000).await;
    store
        .bind_root_stage_plan(
            &admin,
            &bind_request(0, plan_with("0.01", Some(("0.003", "0.006"))), 2),
        )
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .record_root_stage_plan_control(&admin, SCOPE, CONTROL_KIND, "control-1", &d("first"))
        .await
        .unwrap();
    let updated = session
        .record_root_stage_plan_control(&admin, SCOPE, CONTROL_KIND, "control-1", &d("second"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(updated.control_digest, d("second"));
    assert_eq!(updated.binding_seq, 1);
    store.close().await;
}
