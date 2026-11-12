//! AG-146 real dispatcher controls for the four budget management operations.
//! Every test uses the real submit/claim/status path and a real store; no
//! provider or model call is produced by any operation.
use evo_core::evaluation::{
    Money, OptimizationBudgetPlan, OptimizationStage, RootStageBudgetPlanV1, StageMoneyBudget,
};
use evo_core::{Context, Error, Role, hash};
use evo_engine::dispatch::{
    ManagementBudgetResultV2, ManagementCommittedEffect, ManagementDispatcher,
    ManagementFeeKnowledge, ManagementJobState, ManagementJobStatusV1, ManagementResultV2,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use serde_json::{Value, json};
use std::time::Duration;

const SCOPE: &str = "scope-1";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    worker: Context,
    evaluator: Context,
    dispatcher: ManagementDispatcher,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    let worker = Context::new("ns-b", "worker", Role::Worker).unwrap();
    let evaluator = Context::new("ns-a", "evaluator", Role::Evaluator).unwrap();
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    Fixture {
        _dir: dir,
        store,
        admin,
        worker,
        evaluator,
        dispatcher,
    }
}

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn authorization(root: &str, scope: &str) -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: root.into(),
        billing_scope: scope.into(),
        allowed_namespaces: vec!["ns-b".into(), "ns-a".into()],
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        authorization_receipt_digest: d("receipt"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn authorize_payload(key: &str, root: &str, scope: &str) -> Value {
    json!({
        "schema_version": "rsia.management.budget_root_authorize.v1",
        "request_key": key,
        "root_budget_id": root,
        "billing_scope": scope,
        "allowed_namespaces": ["ns-b", "ns-a"],
        "currency": "USD",
        "pricing_version": "price-v1",
        "payment_subject": "payer-1",
        "authorization_receipt_digest": d("receipt"),
        "per_call_cap_micros": 100,
        "total_limit_micros": 1000,
    })
}

fn reservation(scope: &str, call_id: &str, amount: i64, at: i64) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: scope.into(),
        call_id: call_id.into(),
        dispatch_group_id: format!("group-{call_id}"),
        stage: BudgetStage::Reflection,
        actual_input_digest: d(&format!("input-{call_id}")),
        request_artifact: None,
        max_cost_micros: amount,
        lease_token: format!("lease-{call_id}"),
        lease_until: at + 100,
        now: at,
    }
}

fn fence(call: &BudgetCallRecord, at: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: at,
    }
}

fn charge(amount: i64, suffix: &str) -> UsageCharge {
    UsageCharge {
        amount_micros: amount,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{suffix}"),
        usage_record_id: format!("usage-{suffix}"),
        output_digest: Some(d(&format!("output-{suffix}"))),
    }
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
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

fn plan(
    scope: &str,
    payer: &str,
    root_amount: &str,
    stage_limit: Option<(&str, &str)>,
) -> RootStageBudgetPlanV1 {
    let mut budget = OptimizationBudgetPlan::unfunded(scope, payer, "USD").unwrap();
    budget.root_total = money(root_amount);
    budget.admin_authorization_receipt_digest = Some(d("receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit = money("0");
        stage.experiment_total = money("0");
    }
    if let Some((per_call, total)) = stage_limit {
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

fn bind_payload(key: &str, scope: &str, expected_seq: i64, plan: RootStageBudgetPlanV1) -> Value {
    json!({
        "schema_version": "rsia.management.budget_stage_plan_bind.v1",
        "request_key": key,
        "billing_scope": scope,
        "expected_seq": expected_seq,
        "plan": plan,
    })
}

fn list_payload(key: &str, scope: &str, page_size: u16, cursor: Option<&str>) -> Value {
    json!({
        "schema_version": "rsia.management.budget_calls_list_open.v1",
        "request_key": key,
        "billing_scope": scope,
        "page_size": page_size,
        "cursor": cursor,
    })
}

fn reconcile_payload(key: &str, scope: &str, call_id: &str, charge: &UsageCharge) -> Value {
    json!({
        "schema_version": "rsia.management.budget_call_reconcile.v1",
        "request_key": key,
        "billing_scope": scope,
        "call_id": call_id,
        "provider_request_id": charge.provider_request_id,
        "usage_record_id": charge.usage_record_id,
        "output_digest": charge.output_digest,
        "amount_micros": charge.amount_micros,
        "currency": charge.currency,
        "pricing_version": charge.pricing_version,
    })
}

fn terminal(state: ManagementJobState) -> bool {
    matches!(
        state,
        ManagementJobState::Succeeded
            | ManagementJobState::Failed
            | ManagementJobState::Cancelled
            | ManagementJobState::Blocked
    )
}

async fn wait(dispatcher: &ManagementDispatcher, ctx: &Context, id: &str) -> ManagementJobStatusV1 {
    for _ in 0..2_000 {
        let status = dispatcher.status_view(ctx, id).await.unwrap();
        if terminal(status.state) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("management job {id} did not finish");
}

async fn submit_wait(fixture: &Fixture, operation: &str, payload: Value) -> ManagementJobStatusV1 {
    let queued = fixture
        .dispatcher
        .submit(&fixture.admin, operation, payload)
        .await
        .unwrap();
    wait(&fixture.dispatcher, &fixture.admin, queued.identity().id).await
}

fn budget_result(status: &ManagementJobStatusV1) -> ManagementBudgetResultV2 {
    match status.result.clone() {
        Some(ManagementResultV2::Budget(result)) => result,
        other => panic!("expected a budget result, got {other:?}"),
    }
}

async fn rows(store: &Store, ctx: &Context) -> (Vec<String>, Vec<String>) {
    let mut session = store.session().await.unwrap();
    let jobs = session.list::<Value>(ctx, "job").await.unwrap();
    let artifacts = session.list::<Value>(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    (
        jobs.iter().map(Value::to_string).collect(),
        artifacts.iter().map(Value::to_string).collect(),
    )
}

async fn authorize_root(fixture: &Fixture) {
    fixture
        .store
        .authorize_root_budget(&fixture.admin, &authorization("root-1", SCOPE))
        .await
        .unwrap();
}

#[tokio::test]
async fn authorize_creates_a_real_root_and_no_call() {
    let fixture = fixture().await;
    let status = submit_wait(
        &fixture,
        "budget.root.authorize",
        authorize_payload("authorize-1", "root-1", SCOPE),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "root_authorized");
    let ManagementBudgetResultV2::RootAuthorized { root } = budget_result(&status) else {
        panic!("expected root_authorized");
    };
    assert_eq!(root.root_budget_id, "root-1");
    assert_eq!(root.billing_scope, SCOPE);
    assert_eq!(root.authorizing_namespace, "ns-a");
    assert_eq!(root.allowed_namespaces, vec!["ns-a", "ns-b"]);
    assert_eq!(root.spent_micros, 0);
    assert_eq!(root.reserved_micros, 0);
    assert!(!root.stopped);
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .created_at,
        root.created_at
    );
    let mut session = fixture.store.session().await.unwrap();
    let page = session
        .list_open_budget_calls(&fixture.admin, SCOPE, 50, None)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(page.entries.is_empty());
    assert!(matches!(
        status.committed_side_effects.as_deref(),
        Some([ManagementCommittedEffect::BudgetRoot { .. }])
    ));
    assert!(matches!(
        status.fee_knowledge,
        ManagementFeeKnowledge::KnownNone
    ));
    fixture.store.close().await;
}

#[tokio::test]
async fn authorize_same_key_replay_returns_the_same_job_without_new_rows() {
    let fixture = fixture().await;
    let payload = authorize_payload("authorize-replay", "root-1", SCOPE);
    let first = submit_wait(&fixture, "budget.root.authorize", payload.clone()).await;
    let (jobs_before, artifacts_before) = rows(&fixture.store, &fixture.admin).await;
    let second = submit_wait(&fixture, "budget.root.authorize", payload).await;
    assert_eq!(first.id, second.id);
    assert_eq!(
        serde_json::to_value(&first.result).unwrap(),
        serde_json::to_value(&second.result).unwrap()
    );
    let (jobs_after, artifacts_after) = rows(&fixture.store, &fixture.admin).await;
    assert_eq!(jobs_after, jobs_before);
    assert_eq!(artifacts_after, artifacts_before);
    fixture.store.close().await;
}

#[tokio::test]
async fn authorize_same_scope_different_authorization_is_refused_without_a_second_root() {
    let fixture = fixture().await;
    let first = submit_wait(
        &fixture,
        "budget.root.authorize",
        authorize_payload("authorize-a", "root-1", SCOPE),
    )
    .await;
    assert_eq!(first.state, ManagementJobState::Succeeded);
    let mut changed = authorize_payload("authorize-b", "root-1", SCOPE);
    changed["total_limit_micros"] = json!(2_000);
    let refused = submit_wait(&fixture, "budget.root.authorize", changed).await;
    assert_eq!(refused.state, ManagementJobState::Failed, "{refused:?}");
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));
    assert!(refused.result.is_none());
    let root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.total_limit_micros, 1_000);
    assert_eq!(root.root_budget_id, "root-1");
    fixture.store.close().await;
}

#[tokio::test]
async fn authorize_rejects_non_admin_and_malformed_before_creating_a_job() {
    let fixture = fixture().await;
    let before = rows(&fixture.store, &fixture.admin).await;
    for payload in [
        json!({}),
        json!({"schema_version":"rsia.management.budget_root_authorize.v2","request_key":"bad-schema"}),
        json!({"schema_version":"rsia.management.budget_root_authorize.v1","request_key":"bad","root_budget_id":"r","billing_scope":"s","allowed_namespaces":["ns-a"],"currency":"USD","pricing_version":"p","payment_subject":"x","authorization_receipt_digest":d("receipt"),"per_call_cap_micros":1.5,"total_limit_micros":10}),
        json!({"schema_version":"rsia.management.budget_root_authorize.v1","request_key":"bad-extra","root_budget_id":"r","billing_scope":"s","allowed_namespaces":["ns-a"],"currency":"USD","pricing_version":"p","payment_subject":"x","authorization_receipt_digest":d("receipt"),"per_call_cap_micros":1,"total_limit_micros":10,"actor":"forged"}),
    ] {
        let error = fixture
            .dispatcher
            .submit(&fixture.admin, "budget.root.authorize", payload)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
    }
    let error = fixture
        .dispatcher
        .submit(
            &fixture.evaluator,
            "budget.root.authorize",
            authorize_payload("evaluator", "root-1", SCOPE),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Forbidden), "{error:?}");
    assert_eq!(rows(&fixture.store, &fixture.admin).await, before);
    fixture.store.close().await;
}

#[tokio::test]
async fn stage_plan_bind_binds_the_current_root_plan() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let status = submit_wait(
        &fixture,
        "budget.stage_plan.bind",
        bind_payload(
            "bind-1",
            SCOPE,
            0,
            plan(SCOPE, "payer-1", "0.001", Some(("0.00005", "0.0005"))),
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "stage_plan_bound");
    let ManagementBudgetResultV2::StagePlanBound { binding } = budget_result(&status) else {
        panic!("expected stage_plan_bound");
    };
    assert_eq!(binding.seq, 1);
    assert_eq!(binding.billing_scope, SCOPE);
    assert_eq!(binding.payer_id, "payer-1");
    assert_eq!(binding.root_total_micros, 1_000);
    assert_eq!(binding.bound_by_actor, "admin");
    assert_eq!(
        serde_json::to_value(
            fixture
                .store
                .root_stage_plan_binding(&fixture.admin, SCOPE)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&binding).unwrap()
    );
    assert!(matches!(
        status.committed_side_effects.as_deref(),
        Some([ManagementCommittedEffect::BudgetBinding { binding_seq: 1, .. }])
    ));
    fixture.store.close().await;
}

#[tokio::test]
async fn stage_plan_bind_replay_does_not_append_a_second_version() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let payload = bind_payload(
        "bind-replay",
        SCOPE,
        0,
        plan(SCOPE, "payer-1", "0.001", Some(("0.00005", "0.0005"))),
    );
    let first = submit_wait(&fixture, "budget.stage_plan.bind", payload.clone()).await;
    assert_eq!(first.state, ManagementJobState::Succeeded);
    let (jobs_before, artifacts_before) = rows(&fixture.store, &fixture.admin).await;
    let second = submit_wait(&fixture, "budget.stage_plan.bind", payload).await;
    assert_eq!(first.id, second.id);
    assert_eq!(
        serde_json::to_value(&first.result).unwrap(),
        serde_json::to_value(&second.result).unwrap()
    );
    let versions = fixture
        .store
        .root_stage_plan_binding_versions(&fixture.admin, SCOPE)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(
        rows(&fixture.store, &fixture.admin).await,
        (jobs_before, artifacts_before)
    );

    // A later expected_seq binds the next append-only version.
    let next = submit_wait(
        &fixture,
        "budget.stage_plan.bind",
        bind_payload(
            "bind-next",
            SCOPE,
            1,
            plan(SCOPE, "payer-1", "0.001", Some(("0.00006", "0.0006"))),
        ),
    )
    .await;
    assert_eq!(next.state, ManagementJobState::Succeeded, "{next:?}");
    let ManagementBudgetResultV2::StagePlanBound { binding } = budget_result(&next) else {
        panic!("expected stage_plan_bound");
    };
    assert_eq!(binding.seq, 2);
    assert_eq!(
        fixture
            .store
            .root_stage_plan_binding_versions(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .len(),
        2
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn stage_plan_bind_refuses_stale_seq_and_wrong_root_fields_without_a_version() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let first = submit_wait(
        &fixture,
        "budget.stage_plan.bind",
        bind_payload(
            "bind-first",
            SCOPE,
            0,
            plan(SCOPE, "payer-1", "0.001", Some(("0.00005", "0.0005"))),
        ),
    )
    .await;
    assert_eq!(first.state, ManagementJobState::Succeeded);

    let wrong_payer = submit_wait(
        &fixture,
        "budget.stage_plan.bind",
        bind_payload(
            "bind-wrong-payer",
            SCOPE,
            1,
            plan(SCOPE, "payer-2", "0.001", Some(("0.00005", "0.0005"))),
        ),
    )
    .await;
    assert_eq!(wrong_payer.state, ManagementJobState::Failed);
    assert_eq!(wrong_payer.error_code.as_deref(), Some("conflict"));

    let stale = submit_wait(
        &fixture,
        "budget.stage_plan.bind",
        bind_payload(
            "bind-stale",
            SCOPE,
            5,
            plan(SCOPE, "payer-1", "0.001", Some(("0.00005", "0.0005"))),
        ),
    )
    .await;
    assert_eq!(stale.state, ManagementJobState::Failed);
    assert_eq!(stale.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        fixture
            .store
            .root_stage_plan_binding_versions(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .len(),
        1
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn list_open_projects_cross_namespace_open_calls_without_bodies() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    // An uncertain call whose execution is already closed stays open.
    let uncertain = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &reservation(SCOPE, "call-uncertain", 20, 2),
        )
        .await
        .unwrap();
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&uncertain, 3))
        .await
        .unwrap();
    fixture
        .store
        .mark_budget_call_uncertain(&fixture.worker, &fence(&uncertain, 4), "timeout")
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            SCOPE,
            "call-uncertain",
            dispatched.call.dispatch_id.as_deref().unwrap(),
            "process_confirmed_stopped",
            5,
        )
        .await
        .unwrap();
    let mut reserved = reservation(SCOPE, "call-reserved", 10, 6);
    reserved.request_artifact = Some(
        BudgetArtifact::from_serializable(
            "rsia.test.request.v1",
            &json!({"secret": "SECRET-MANAGEMENT-BODY"}),
        )
        .unwrap(),
    );
    fixture
        .store
        .reserve_budget_call(&fixture.worker, &reserved)
        .await
        .unwrap();
    // Another root's call must never be named in this page.
    let mut other = authorization("root-2", "scope-2");
    other.allowed_namespaces = vec!["ns-a".into()];
    fixture
        .store
        .authorize_root_budget(&fixture.admin, &other)
        .await
        .unwrap();
    let worker_a = Context::new("ns-a", "worker-a", Role::Worker).unwrap();
    fixture
        .store
        .reserve_budget_call(&worker_a, &reservation("scope-2", "other-root-call", 10, 2))
        .await
        .unwrap();

    let status = submit_wait(
        &fixture,
        "budget.calls.list_open",
        list_payload("list-cross-ns", SCOPE, 50, None),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "open_calls_listed");
    let ManagementBudgetResultV2::OpenCallsListed { page } = budget_result(&status) else {
        panic!("expected open_calls_listed");
    };
    let ids: Vec<_> = page
        .entries
        .iter()
        .map(|entry| entry.call_id.as_str())
        .collect();
    assert_eq!(ids, vec!["call-reserved", "call-uncertain"]);
    assert_eq!(page.entries[0].namespace, "ns-b");
    assert_eq!(page.entries[0].state, BudgetCallState::Reserved);
    assert_eq!(page.entries[0].actual_cost_micros, None);
    assert_eq!(page.entries[1].state, BudgetCallState::Uncertain);
    assert!(page.entries[1].execution_closed);
    assert_eq!(page.entries[1].actual_cost_micros, None);
    assert!(!ids.contains(&"other-root-call"));
    let serialized = serde_json::to_string(&status.result).unwrap();
    assert!(!serialized.contains("SECRET-MANAGEMENT-BODY"));
    assert!(!serialized.contains("request_artifact"));
    assert!(!serialized.contains("response_artifact"));
    // A listing is a read: it reports no business side effect and no fee.
    assert!(matches!(status.committed_side_effects.as_deref(), Some([])));
    assert!(matches!(
        status.fee_knowledge,
        ManagementFeeKnowledge::KnownNone
    ));
    fixture.store.close().await;
}

#[tokio::test]
async fn list_open_paginates_with_a_real_anchor_and_refuses_a_stale_cursor() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    for (index, call_id) in ["call-a", "call-b", "call-c"].iter().enumerate() {
        fixture
            .store
            .reserve_budget_call(
                &fixture.worker,
                &reservation(SCOPE, call_id, 10, 2 + index as i64),
            )
            .await
            .unwrap();
    }
    let call_a = fixture
        .store
        .budget_call(&fixture.worker, SCOPE, "call-a")
        .await
        .unwrap()
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&call_a, 6))
        .await
        .unwrap();
    fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-d", 10, 6))
        .await
        .unwrap();

    let first = submit_wait(
        &fixture,
        "budget.calls.list_open",
        list_payload("list-page-1", SCOPE, 2, None),
    )
    .await;
    assert_eq!(first.state, ManagementJobState::Succeeded, "{first:?}");
    let ManagementBudgetResultV2::OpenCallsListed { page } = budget_result(&first) else {
        panic!("expected open_calls_listed");
    };
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b"]
    );
    assert_eq!(page.entries[0].state, BudgetCallState::Dispatched);
    assert!(!page.complete);
    let cursor = page.next_cursor.clone().unwrap();

    let second = submit_wait(
        &fixture,
        "budget.calls.list_open",
        list_payload("list-page-2", SCOPE, 2, Some(&cursor)),
    )
    .await;
    assert_eq!(second.state, ManagementJobState::Succeeded, "{second:?}");
    let ManagementBudgetResultV2::OpenCallsListed { page: page_two } = budget_result(&second)
    else {
        panic!("expected open_calls_listed");
    };
    assert_eq!(
        page_two
            .entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-c", "call-d"]
    );
    assert!(page_two.complete);
    assert!(page_two.next_cursor.is_none());
    assert_eq!(page.anchor, page_two.anchor);

    // A changed view invalidates the old position instead of splicing pages.
    fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-e", 10, 7))
        .await
        .unwrap();
    let stale = submit_wait(
        &fixture,
        "budget.calls.list_open",
        list_payload("list-stale", SCOPE, 2, Some(&cursor)),
    )
    .await;
    assert_eq!(stale.state, ManagementJobState::Failed);
    assert_eq!(stale.error_code.as_deref(), Some("conflict"));
    assert!(stale.result.is_none());
    fixture.store.close().await;
}

#[tokio::test]
async fn list_open_refuses_a_non_member_admin_and_discloses_no_other_root() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    // A second root the ns-a Admin is a member of, with one real call.
    let mut other = authorization("root-2", "scope-2");
    other.allowed_namespaces = vec!["ns-a".into()];
    fixture
        .store
        .authorize_root_budget(&fixture.admin, &other)
        .await
        .unwrap();
    let worker_a = Context::new("ns-a", "worker-a", Role::Worker).unwrap();
    fixture
        .store
        .reserve_budget_call(&worker_a, &reservation("scope-2", "call-other", 10, 2))
        .await
        .unwrap();

    let own = submit_wait(
        &fixture,
        "budget.calls.list_open",
        list_payload("list-own", "scope-2", 50, None),
    )
    .await;
    let ManagementBudgetResultV2::OpenCallsListed { page } = budget_result(&own) else {
        panic!("expected open_calls_listed");
    };
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-other"]
    );

    // A non-member Admin of scope-1 gets no page at all, not an empty one.
    let admin_c = Context::new("ns-c", "admin-c", Role::Admin).unwrap();
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![admin_c.clone()]).unwrap();
    let denied = {
        let queued = dispatcher
            .submit(
                &admin_c,
                "budget.calls.list_open",
                list_payload("list-denied", SCOPE, 50, None),
            )
            .await
            .unwrap();
        wait(&dispatcher, &admin_c, queued.identity().id).await
    };
    assert_eq!(denied.state, ManagementJobState::Failed);
    assert_eq!(denied.error_code.as_deref(), Some("not_found"));
    assert!(denied.result.is_none());
    fixture.store.close().await;
}

#[tokio::test]
async fn reconcile_matches_an_owned_call_and_persists_the_actual_charge() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let call = fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-1", 30, 2))
        .await
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&call, 3))
        .await
        .unwrap();
    let payload = reconcile_payload("reconcile-1", SCOPE, "call-1", &charge(12, "actual"));
    let status = submit_wait(&fixture, "budget.call.reconcile", payload.clone()).await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "call_reconciled");
    let ManagementBudgetResultV2::CallReconciled {
        call: view,
        accounting_overflow,
        provider_source_verified,
    } = budget_result(&status)
    else {
        panic!("expected call_reconciled");
    };
    assert_eq!(view.state, BudgetCallState::Finalized);
    assert_eq!(view.actual_cost_micros, Some(12));
    assert_eq!(view.provider_request_id.as_deref(), Some("provider-actual"));
    assert_eq!(view.usage_record_id.as_deref(), Some("usage-actual"));
    assert!(!view.execution_closed);
    assert!(!view.root_stopped);
    assert!(!accounting_overflow);
    // No provider carrier exists: the result never claims a supplier check.
    assert!(!provider_source_verified);
    let stored = fixture
        .store
        .budget_call(&fixture.admin, SCOPE, "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, BudgetCallState::Finalized);
    assert_eq!(stored.actual_cost_micros, Some(12));
    let root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 12);
    assert_eq!(root.reserved_micros, 0);
    assert!(matches!(
        status.committed_side_effects.as_deref(),
        Some([ManagementCommittedEffect::BudgetReconciliation {
            accounting_overflow: false,
            ..
        }])
    ));

    // The same request key and payload replay the same committed job and do
    // not charge the root a second time.
    let replay = submit_wait(&fixture, "budget.call.reconcile", payload).await;
    assert_eq!(replay.id, status.id);
    assert_eq!(
        serde_json::to_value(&replay.result).unwrap(),
        serde_json::to_value(&status.result).unwrap()
    );
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        12
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn reconcile_finalized_same_charge_is_idempotent_and_different_charge_is_refused() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let call = fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-1", 30, 2))
        .await
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&call, 3))
        .await
        .unwrap();
    let actual = charge(12, "actual");
    let first = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-a", SCOPE, "call-1", &actual),
    )
    .await;
    assert_eq!(first.state, ManagementJobState::Succeeded);

    // A different request key with the exact same charge is the same business
    // fact: the call is already finalized and nothing is charged again.
    let same = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-b", SCOPE, "call-1", &actual),
    )
    .await;
    assert_eq!(same.state, ManagementJobState::Succeeded, "{same:?}");
    assert_eq!(same.step, "call_already_reconciled");
    let ManagementBudgetResultV2::CallReconciled {
        call: view,
        accounting_overflow: false,
        ..
    } = budget_result(&same)
    else {
        panic!("expected an idempotent call_reconciled");
    };
    assert_eq!(view.actual_cost_micros, Some(12));
    // The replay wrote nothing new: no committed business effect is claimed.
    assert!(matches!(same.committed_side_effects.as_deref(), Some([])));

    let mut changed = actual.clone();
    changed.amount_micros = 13;
    let refused = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-c", SCOPE, "call-1", &changed),
    )
    .await;
    assert_eq!(refused.state, ManagementJobState::Failed);
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));
    let stored = fixture
        .store
        .budget_call(&fixture.admin, SCOPE, "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.actual_cost_micros, Some(12));
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        12
    );
    fixture.store.close().await;
}

/// AG-206: a crash-left uncertain call whose bill is known but whose output
/// was never received is reconciled without an output digest (absent or
/// `null` on the wire). Nothing is invented: the stored and shown output stays
/// unknown, the amount is charged once, and replays compare it as an Option.
#[tokio::test]
async fn reconcile_without_output_charges_the_bill_and_keeps_the_output_unknown() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let call = fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-1", 30, 2))
        .await
        .unwrap();
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&call, 3))
        .await
        .unwrap()
        .call;
    let uncertain = fixture
        .store
        .cancel_budget_call(&fixture.worker, &fence(&dispatched, 4), "process_lost")
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);

    let mut billed = charge(12, "billed");
    billed.output_digest = None;
    let mut absent = reconcile_payload("reconcile-billed", SCOPE, "call-1", &billed);
    absent.as_object_mut().unwrap().remove("output_digest");
    let status = submit_wait(&fixture, "budget.call.reconcile", absent).await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "call_reconciled");
    let ManagementBudgetResultV2::CallReconciled {
        call: view,
        accounting_overflow: false,
        provider_source_verified: false,
    } = budget_result(&status)
    else {
        panic!("expected call_reconciled");
    };
    assert_eq!(view.state, BudgetCallState::Finalized);
    assert_eq!(view.actual_cost_micros, Some(12));
    assert_eq!(view.provider_request_id.as_deref(), Some("provider-billed"));
    assert_eq!(view.output_digest, None);
    assert!(!view.execution_closed);
    assert!(matches!(
        status.committed_side_effects.as_deref(),
        Some([ManagementCommittedEffect::BudgetReconciliation {
            accounting_overflow: false,
            ..
        }])
    ));
    let stored = fixture
        .store
        .budget_call(&fixture.admin, SCOPE, "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, BudgetCallState::Finalized);
    assert_eq!(stored.output_digest, None);
    assert_eq!(stored.actual_cost_micros, Some(12));
    let spent = || async {
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .spent_micros
    };
    assert_eq!(spent().await, 12);

    // An explicit `null` is the same request: the same key replays the
    // original job.
    let null = reconcile_payload("reconcile-billed", SCOPE, "call-1", &billed);
    assert_eq!(null["output_digest"], Value::Null);
    let replay = submit_wait(&fixture, "budget.call.reconcile", null).await;
    assert_eq!(replay.id, status.id);

    // A new key with the same no-output charge is the already finalized fact:
    // it succeeds, charges nothing and claims no committed effect.
    let again = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-billed-2", SCOPE, "call-1", &billed),
    )
    .await;
    assert_eq!(again.state, ManagementJobState::Succeeded, "{again:?}");
    assert_eq!(again.step, "call_already_reconciled");
    assert!(matches!(again.committed_side_effects.as_deref(), Some([])));

    // Adding an output afterwards is a different charge.
    let refused = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-billed-3", SCOPE, "call-1", &charge(12, "billed")),
    )
    .await;
    assert_eq!(refused.state, ManagementJobState::Failed, "{refused:?}");
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));

    // The reverse order is refused too: a call reconciled with its output
    // cannot be replayed as "output not received".
    let other = fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-2", 30, 5))
        .await
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&other, 6))
        .await
        .unwrap();
    let with_output = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-out", SCOPE, "call-2", &charge(5, "out")),
    )
    .await;
    assert_eq!(with_output.state, ManagementJobState::Succeeded);
    let mut no_output = charge(5, "out");
    no_output.output_digest = None;
    let refused = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-out-2", SCOPE, "call-2", &no_output),
    )
    .await;
    assert_eq!(refused.state, ManagementJobState::Failed, "{refused:?}");
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));
    assert_eq!(spent().await, 17);
    fixture.store.close().await;
}

/// AG-206: the dispatcher reconciles on the real clock, so a dispatched call
/// whose worker lease is still live is refused by the kernel: the job fails
/// with a conflict, claims no committed effect and changes no budget fact.
#[tokio::test]
async fn reconcile_of_a_live_dispatch_lease_fails_without_side_effects() {
    let fixture = fixture().await;
    authorize_root(&fixture).await;
    let at = evo_core::now().unwrap();
    let call = fixture
        .store
        .reserve_budget_call(&fixture.worker, &reservation(SCOPE, "call-live", 30, at))
        .await
        .unwrap();
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&call, at))
        .await
        .unwrap()
        .call;
    let root_before = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    let status = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload("reconcile-live", SCOPE, "call-live", &charge(12, "live")),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Failed, "{status:?}");
    assert_eq!(status.error_code.as_deref(), Some("conflict"));
    assert!(status.result.is_none());
    assert!(matches!(status.committed_side_effects.as_deref(), Some([])));
    let stored = fixture
        .store
        .budget_call(&fixture.admin, SCOPE, "call-live")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, dispatched);
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap(),
        root_before
    );
    fixture.store.close().await;
}

/// Two calls make the second reconciliation overflow using only the real store
/// API: the first is finalized at `i64::MAX - 1000` without overrun, its
/// execution is closed, and the second reserved call is dispatched afterwards.
async fn overflow_ready(fixture: &Fixture) -> (BudgetCallRecord, UsageCharge) {
    let scope = "scope-max";
    let mut authorization = authorization("root-max", scope);
    authorization.allowed_namespaces = vec!["ns-b".into(), "ns-a".into()];
    authorization.per_call_cap_micros = i64::MAX;
    authorization.total_limit_micros = i64::MAX;
    fixture
        .store
        .authorize_root_budget(&fixture.admin, &authorization)
        .await
        .unwrap();
    let first = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &reservation(scope, "call-first", i64::MAX - 1_000, 2),
        )
        .await
        .unwrap();
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&first, 3))
        .await
        .unwrap();
    let second = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &reservation(scope, "call-overflow", 1_000, 3),
        )
        .await
        .unwrap();
    let finalized = fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &fence(&first, 4),
            &charge(i64::MAX - 1_000, "first"),
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            scope,
            "call-first",
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            5,
        )
        .await
        .unwrap();
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &fence(&second, 6))
        .await
        .unwrap();
    (dispatched.call, charge(2_000, "overflow"))
}

#[tokio::test]
async fn reconcile_overflow_commits_preserved_facts_and_reports_blocked() {
    let fixture = fixture().await;
    let (overflow_call, overflow_charge) = overflow_ready(&fixture).await;
    let status = submit_wait(
        &fixture,
        "budget.call.reconcile",
        reconcile_payload(
            "reconcile-overflow",
            &overflow_call.billing_scope,
            &overflow_call.call_id,
            &overflow_charge,
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Blocked, "{status:?}");
    assert_eq!(status.step, "cost_accounting_overflow");
    assert_eq!(
        status.error_code.as_deref(),
        Some("cost_accounting_overflow")
    );
    let ManagementBudgetResultV2::CallReconciled {
        call: view,
        accounting_overflow: true,
        provider_source_verified: false,
    } = budget_result(&status)
    else {
        panic!("expected the preserved overflow result");
    };
    assert_eq!(view.state, BudgetCallState::Uncertain);
    assert_eq!(view.actual_cost_micros, Some(2_000));
    assert_eq!(
        view.provider_request_id.as_deref(),
        Some("provider-overflow")
    );
    assert!(view.root_stopped);
    assert_eq!(
        view.root_stop_reason.as_deref(),
        Some("cost_accounting_overflow")
    );
    assert!(!view.execution_closed);
    // The committed cost facts survive the blocked terminal state: no refund,
    // no zeroed amount, no deleted charge, no automatic execution close.
    let stored = fixture
        .store
        .budget_call(
            &fixture.admin,
            &overflow_call.billing_scope,
            "call-overflow",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, BudgetCallState::Uncertain);
    assert_eq!(stored.actual_cost_micros, Some(2_000));
    assert_eq!(stored.usage_record_id.as_deref(), Some("usage-overflow"));
    assert_eq!(
        stored.output_digest.as_deref(),
        Some(d("output-overflow").as_str())
    );
    assert!(!stored.execution_closed);
    let root = fixture
        .store
        .root_budget(&fixture.admin, &overflow_call.billing_scope)
        .await
        .unwrap()
        .unwrap();
    assert!(root.stopped);
    assert_eq!(
        root.stop_reason.as_deref(),
        Some("cost_accounting_overflow")
    );
    assert_eq!(root.spent_micros, i64::MAX - 1_000);
    assert_eq!(root.reserved_micros, 1_000);
    assert!(matches!(
        status.committed_side_effects.as_deref(),
        Some([ManagementCommittedEffect::BudgetReconciliation {
            accounting_overflow: true,
            ..
        }])
    ));
    assert!(matches!(
        status.fee_knowledge,
        ManagementFeeKnowledge::KnownNone
    ));
    fixture.store.close().await;
}
