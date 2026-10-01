use evo_core::evaluation::OptimizationStage;
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRefence, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use sqlx::{Connection, SqliteConnection};

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn authorization(root: &str, scope: &str) -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: root.into(),
        billing_scope: scope.into(),
        allowed_namespaces: vec!["ns-b".into(), "ns-a".into()],
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        authorization_receipt_digest: hash(b"admin-authorization"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn reservation(call_id: &str, amount: i64, now: i64) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "scope-1".into(),
        call_id: call_id.into(),
        dispatch_group_id: format!("group-{call_id}"),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: None,
        max_cost_micros: amount,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + 100,
        now,
    }
}

fn fence(call: &evo_storage::budget::BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

fn charge(amount: i64, suffix: &str) -> UsageCharge {
    UsageCharge {
        amount_micros: amount,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{suffix}"),
        usage_record_id: format!("usage-{suffix}"),
        output_digest: hash(format!("output-{suffix}").as_bytes()),
    }
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    (dir, store)
}

#[tokio::test]
async fn missing_authorization_is_zero_and_billing_scope_cannot_split_by_namespace() {
    let (_dir, store) = store().await;
    let admin_a = ctx("ns-a", "admin-a", Role::Admin);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    let worker_c = ctx("ns-c", "worker-c", Role::Worker);
    assert!(matches!(
        store
            .reserve_budget_call(&worker_a, &reservation("c0", 10, 1))
            .await,
        Err(Error::Budget)
    ));

    let root = store
        .authorize_root_budget(&admin_a, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    assert_eq!(root.allowed_namespaces, vec!["ns-a", "ns-b"]);
    assert_eq!(root.spent_micros, 0);
    assert!(matches!(
        store
            .reserve_budget_call(&worker_c, &reservation("c1", 10, 2))
            .await,
        Err(Error::Forbidden)
    ));
    let scoped_call = store
        .reserve_budget_call(&worker_a, &reservation("scoped-call", 10, 2))
        .await
        .unwrap();
    let admin_c = ctx("ns-c", "admin-c", Role::Admin);
    assert!(matches!(
        store.root_budget(&admin_c, "scope-1").await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .stop_root_budget(&admin_c, "scope-1", "cross_tenant", 3)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .budget_call(&admin_c, "scope-1", &scoped_call.call_id)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .dispatch_group(&admin_c, "scope-1", &scoped_call.dispatch_group_id)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .reconcile_budget_call_cost(
                &admin_c,
                "scope-1",
                &scoped_call.call_id,
                &charge(1, "unauthorized"),
                3,
            )
            .await,
        Err(Error::Forbidden)
    ));

    let admin_b = ctx("ns-b", "admin-b", Role::Admin);
    let conflicting = authorization("root-2", "scope-1");
    assert!(
        store
            .authorize_root_budget(&admin_b, &conflicting)
            .await
            .unwrap_err()
            .to_string()
            .contains("billing_scope_already_bound")
    );
}

#[tokio::test]
async fn reservations_are_idempotent_and_all_namespaces_share_concurrency_one() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    let worker_b = ctx("ns-b", "worker-b", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();

    let request_a = reservation("call-a", 30, 2);
    let call_a = store
        .reserve_budget_call(&worker_a, &request_a)
        .await
        .unwrap();
    let duplicate = store
        .reserve_budget_call(&worker_a, &request_a)
        .await
        .unwrap();
    assert_eq!(duplicate, call_a);
    let mut changed = request_a.clone();
    changed.actual_input_digest = hash(b"changed");
    assert!(
        store
            .reserve_budget_call(&worker_a, &changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("call_id_reused")
    );
    let call_b = store
        .reserve_budget_call(&worker_b, &reservation("call-b", 40, 2))
        .await
        .unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        70
    );

    let dispatch_a = store
        .begin_budget_dispatch(&worker_a, &fence(&call_a, 3))
        .await
        .unwrap();
    assert!(dispatch_a.new_dispatch);
    let duplicate_dispatch = store
        .begin_budget_dispatch(&worker_a, &fence(&call_a, 3))
        .await
        .unwrap();
    assert!(!duplicate_dispatch.new_dispatch);
    assert!(
        store
            .begin_budget_dispatch(&worker_b, &fence(&call_b, 3))
            .await
            .unwrap_err()
            .to_string()
            .contains("concurrency_limit")
    );

    let finalized = store
        .finalize_budget_call(&worker_a, &fence(&call_a, 4), &charge(20, "a"))
        .await
        .unwrap();
    assert_eq!(finalized.state, BudgetCallState::Finalized);
    assert!(!finalized.execution_closed);
    assert!(
        store
            .begin_budget_dispatch(&worker_b, &fence(&call_b, 4))
            .await
            .is_err()
    );
    let dispatch_id = finalized.dispatch_id.as_deref().unwrap();
    store
        .close_budget_call_execution(
            &worker_a,
            "scope-1",
            "call-a",
            dispatch_id,
            "response_complete",
            5,
        )
        .await
        .unwrap();
    assert!(
        store
            .begin_budget_dispatch(&worker_b, &fence(&call_b, 6))
            .await
            .unwrap()
            .new_dispatch
    );
}

#[tokio::test]
async fn uncertain_call_survives_restart_and_is_never_automatically_released_or_resent() {
    let (dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("call-a", 30, 2))
        .await
        .unwrap();
    let dispatch = store
        .begin_budget_dispatch(&worker, &fence(&call, 3))
        .await
        .unwrap();
    let uncertain = store
        .mark_budget_call_uncertain(&worker, &fence(&call, 103), "timeout")
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);
    assert!(
        store
            .release_undispatched_budget_call(&worker, &fence(&call, 104), "retry")
            .await
            .is_err()
    );
    assert!(
        !store
            .begin_budget_dispatch(&worker, &fence(&call, 104))
            .await
            .unwrap()
            .new_dispatch
    );
    store.close().await;

    let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let restored = reopened
        .budget_call(&worker, "scope-1", "call-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.state, BudgetCallState::Uncertain);
    reopened
        .close_budget_call_execution(
            &worker,
            "scope-1",
            "call-a",
            dispatch.call.dispatch_id.as_deref().unwrap(),
            "process_confirmed_stopped",
            105,
        )
        .await
        .unwrap();
    let call_b = reopened
        .reserve_budget_call(&worker, &reservation("call-b", 20, 105))
        .await
        .unwrap();
    assert!(
        reopened
            .begin_budget_dispatch(&worker, &fence(&call_b, 106))
            .await
            .is_err()
    );
    reopened
        .reconcile_budget_call_cost(&admin, "scope-1", "call-a", &charge(12, "late"), 107)
        .await
        .unwrap();
    assert!(
        reopened
            .begin_budget_dispatch(&worker, &fence(&call_b, 108))
            .await
            .unwrap()
            .new_dispatch
    );
}

#[tokio::test]
async fn persistent_stop_blocks_dispatch_but_undispatched_money_can_be_released() {
    let (dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("call-a", 30, 2))
        .await
        .unwrap();
    store
        .stop_root_budget(&admin, "scope-1", "early_stop", 3)
        .await
        .unwrap();
    assert!(matches!(
        store.begin_budget_dispatch(&worker, &fence(&call, 4)).await,
        Err(Error::Cancelled)
    ));
    store
        .release_undispatched_budget_call(&worker, &fence(&call, 4), "stopped_before_dispatch")
        .await
        .unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        0
    );
    store.close().await;

    let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let root = reopened
        .root_budget(&admin, "scope-1")
        .await
        .unwrap()
        .unwrap();
    assert!(root.stopped);
    assert_eq!(root.stop_reason.as_deref(), Some("early_stop"));
    assert!(matches!(
        reopened
            .reserve_budget_call(&worker, &reservation("call-b", 10, 5))
            .await,
        Err(Error::Cancelled)
    ));
}

#[tokio::test]
async fn late_overrun_is_charged_and_never_clears_an_existing_stop() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("call-a", 10, 2))
        .await
        .unwrap();
    store
        .begin_budget_dispatch(&worker, &fence(&call, 3))
        .await
        .unwrap();
    store
        .stop_root_budget(&admin, "scope-1", "early_stop", 4)
        .await
        .unwrap();
    let finalized = store
        .reconcile_budget_call_cost(&admin, "scope-1", "call-a", &charge(15, "overrun"), 5)
        .await
        .unwrap();
    assert_eq!(finalized.actual_cost_micros, Some(15));
    assert_eq!(finalized.terminal_reason.as_deref(), Some("cost_overrun"));
    let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
    assert_eq!(root.spent_micros, 15);
    assert_eq!(root.reserved_micros, 0);
    assert!(root.stopped);
    assert_eq!(root.stop_reason.as_deref(), Some("early_stop"));
}

#[tokio::test]
async fn expired_reserved_lease_can_be_refenced_and_old_worker_is_fenced() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut request = reservation("call-a", 10, 1);
    request.lease_until = 10;
    let old = store.reserve_budget_call(&worker, &request).await.unwrap();
    assert!(matches!(
        store.begin_budget_dispatch(&worker, &fence(&old, 11)).await,
        Err(Error::Cancelled)
    ));
    let fresh = store
        .refence_reserved_budget_call(
            &worker,
            &BudgetCallRefence {
                billing_scope: "scope-1".into(),
                call_id: "call-a".into(),
                expected_epoch: 1,
                new_lease_token: "lease-new".into(),
                new_lease_until: 50,
                now: 11,
            },
        )
        .await
        .unwrap();
    assert_eq!(fresh.lease_epoch, 2);
    assert!(matches!(
        store.begin_budget_dispatch(&worker, &fence(&old, 12)).await,
        Err(Error::Cancelled)
    ));
    assert!(
        store
            .begin_budget_dispatch(&worker, &fence(&fresh, 12))
            .await
            .unwrap()
            .new_dispatch
    );
}

#[tokio::test]
async fn competing_dispatches_have_one_winner_and_cancel_never_refunds_dispatched_usage() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let first = store
        .reserve_budget_call(&worker, &reservation("call-a", 20, 1))
        .await
        .unwrap();
    let second = store
        .reserve_budget_call(&worker, &reservation("call-b", 20, 1))
        .await
        .unwrap();
    let first_store = store.clone();
    let second_store = store.clone();
    let first_worker = worker.clone();
    let second_worker = worker.clone();
    let first_fence = fence(&first, 2);
    let second_fence = fence(&second, 2);
    let (left, right) = tokio::join!(
        first_store.begin_budget_dispatch(&first_worker, &first_fence),
        second_store.begin_budget_dispatch(&second_worker, &second_fence)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let (winner, loser) = if left.is_ok() {
        (first, second)
    } else {
        (second, first)
    };
    let uncertain = store
        .cancel_budget_call(&worker, &fence(&winner, 3), "user_cancelled")
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        40
    );
    let cancelled = store
        .cancel_budget_call(&worker, &fence(&loser, 3), "root_stopped")
        .await
        .unwrap();
    assert_eq!(cancelled.state, BudgetCallState::Cancelled);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        20
    );
}

#[tokio::test]
async fn persistent_group_stop_blocks_old_lease_and_new_call_without_stopping_other_groups() {
    let (dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let evaluator = ctx("ns-a", "evaluator", Role::Evaluator);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut first_request = reservation("call-a", 20, 1);
    first_request.dispatch_group_id = "ticket-1".into();
    let first = store
        .reserve_budget_call(&worker, &first_request)
        .await
        .unwrap();
    let worker_b = ctx("ns-b", "worker-b", Role::Worker);
    let mut cross_namespace = reservation("call-cross", 20, 1);
    cross_namespace.dispatch_group_id = "ticket-1".into();
    assert!(
        store
            .reserve_budget_call(&worker_b, &cross_namespace)
            .await
            .unwrap_err()
            .to_string()
            .contains("different_namespace")
    );
    let evaluator_b = ctx("ns-b", "evaluator-b", Role::Evaluator);
    assert!(matches!(
        store
            .stop_dispatch_group(&evaluator_b, "scope-1", "ticket-1", "wrong_owner", 2)
            .await,
        Err(Error::NotFound)
    ));
    let stopped = store
        .stop_dispatch_group(&evaluator, "scope-1", "ticket-1", "futility", 2)
        .await
        .unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.stop_reason.as_deref(), Some("futility"));
    assert!(matches!(
        store
            .begin_budget_dispatch(&worker, &fence(&first, 3))
            .await,
        Err(Error::Cancelled)
    ));
    let mut bypass = reservation("call-b", 20, 3);
    bypass.dispatch_group_id = "ticket-1".into();
    assert!(matches!(
        store.reserve_budget_call(&worker, &bypass).await,
        Err(Error::Cancelled)
    ));
    let other = store
        .reserve_budget_call(&worker, &reservation("call-c", 20, 3))
        .await
        .unwrap();
    assert!(
        store
            .begin_budget_dispatch(&worker, &fence(&other, 4))
            .await
            .unwrap()
            .new_dispatch
    );
    store.close().await;

    let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let group = reopened
        .dispatch_group(&evaluator, "scope-1", "ticket-1")
        .await
        .unwrap()
        .unwrap();
    assert!(group.stopped);
}

#[tokio::test]
async fn session_can_stop_a_group_before_any_call_in_the_same_transaction() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let evaluator = ctx("ns-a", "evaluator", Role::Evaluator);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let stopped = session
        .stop_dispatch_group(&evaluator, "scope-1", "ticket-before-call", "futility", 2)
        .await
        .unwrap();
    assert!(stopped.stopped);
    session.commit().await.unwrap();

    let mut request = reservation("late-call", 20, 3);
    request.dispatch_group_id = "ticket-before-call".into();
    assert!(matches!(
        store.reserve_budget_call(&worker, &request).await,
        Err(Error::Cancelled)
    ));
}

#[tokio::test]
async fn model_request_charge_and_response_artifacts_settle_atomically_and_idempotently() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "trusted-broker", Role::Host);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut request = reservation("model-call", 20, 1);
    request.request_artifact = Some(
        BudgetArtifact::from_serializable(
            "test.model_request.v1",
            &serde_json::json!({"request_id":"model-call","input":"frozen"}),
        )
        .unwrap(),
    );
    let call = store.reserve_budget_call(&worker, &request).await.unwrap();
    let dispatched = store
        .begin_budget_dispatch(&worker, &fence(&call, 2))
        .await
        .unwrap()
        .call;
    let evidence = ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            "test.transport.v1",
            &serde_json::json!({"provider_request_id":"provider-model"}),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                "test.response.v1",
                &serde_json::json!({"status":"completed"}),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "test.response.v1",
            &serde_json::json!({"status":"cancelled_after_dispatch"}),
        )
        .unwrap(),
        forced_block_reason: None,
    };
    let charge = charge(10, "model");
    let settlement = store
        .settle_model_budget_call(&worker, &fence(&dispatched, 3), &charge, &evidence)
        .await
        .unwrap();
    assert!(settlement.response_usable);
    assert_eq!(settlement.call.state, BudgetCallState::Finalized);
    assert_eq!(settlement.call.request_artifact, request.request_artifact);
    assert_eq!(
        settlement.call.execution_provenance,
        Some(BudgetExecutionProvenance::Fixture)
    );
    assert!(!settlement.call.execution_closed);

    let repeated = store
        .settle_model_budget_call(&worker, &fence(&dispatched, 4), &charge, &evidence)
        .await
        .unwrap();
    assert_eq!(repeated.response_artifact, settlement.response_artifact);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        10
    );
}

#[tokio::test]
async fn session_budget_transitions_share_ticket_transaction_and_rollback_cleanly() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut request = reservation("ticket-call", 20, 1);
    request.dispatch_group_id = "ticket-atomic".into();
    let call = store.reserve_budget_call(&worker, &request).await.unwrap();
    let call_fence = fence(&call, 2);

    let mut rolled_back = store.session().await.unwrap();
    assert!(
        rolled_back
            .begin_budget_dispatch(&worker, &call_fence)
            .await
            .unwrap()
            .new_dispatch
    );
    rolled_back
        .put(
            &worker,
            "artifact",
            "ticket-started",
            worker.actor(),
            &serde_json::json!({"id":"ticket-started","state":"running"}),
        )
        .await
        .unwrap();
    drop(rolled_back);
    let restored = store
        .budget_call(&worker, "scope-1", "ticket-call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.state, BudgetCallState::Reserved);

    let mut committed = store.session().await.unwrap();
    let decision = committed
        .begin_budget_dispatch(&worker, &call_fence)
        .await
        .unwrap();
    committed
        .put(
            &worker,
            "artifact",
            "ticket-started",
            worker.actor(),
            &serde_json::json!({"id":"ticket-started","state":"running"}),
        )
        .await
        .unwrap();
    let calls = committed
        .budget_calls_for_group(&worker, "scope-1", "ticket-atomic")
        .await
        .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].state, BudgetCallState::Dispatched);
    committed.commit().await.unwrap();
    assert!(decision.new_dispatch);

    let mut read = store.session().await.unwrap();
    assert!(
        read.get::<serde_json::Value>(&worker, "artifact", "ticket-started")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        read.budget_call(&worker, "scope-1", "ticket-call")
            .await
            .unwrap()
            .unwrap()
            .state,
        BudgetCallState::Dispatched
    );
    read.commit().await.unwrap();
}

#[tokio::test]
async fn session_release_rolls_back_and_group_stop_fences_old_lease() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let evaluator = ctx("ns-a", "evaluator", Role::Evaluator);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut request = reservation("release-call", 20, 1);
    request.dispatch_group_id = "ticket-stop".into();
    let call = store.reserve_budget_call(&worker, &request).await.unwrap();
    let call_fence = fence(&call, 2);

    let mut rolled_back = store.session().await.unwrap();
    rolled_back
        .release_undispatched_budget_call(&worker, &call_fence, "planned_stop")
        .await
        .unwrap();
    drop(rolled_back);
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "release-call")
            .await
            .unwrap()
            .unwrap()
            .state,
        BudgetCallState::Reserved
    );

    let mut stopped = store.session().await.unwrap();
    stopped
        .stop_dispatch_group(&evaluator, "scope-1", "ticket-stop", "futility", 3)
        .await
        .unwrap();
    assert!(matches!(
        stopped.begin_budget_dispatch(&worker, &call_fence).await,
        Err(Error::Cancelled)
    ));
    stopped
        .release_undispatched_budget_call(&worker, &call_fence, "group_stopped")
        .await
        .unwrap();
    stopped.commit().await.unwrap();
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "release-call")
            .await
            .unwrap()
            .unwrap()
            .state,
        BudgetCallState::Released
    );
}

#[test]
fn all_frozen_cost_stages_are_distinct() {
    let stages = [
        BudgetStage::Reflection,
        BudgetStage::Merge,
        BudgetStage::Ranking,
        BudgetStage::DevelopmentExecution,
        BudgetStage::DevelopmentScoring,
        BudgetStage::Practice,
        BudgetStage::Consolidation,
        BudgetStage::Guidance,
        BudgetStage::TaskExecution,
        BudgetStage::CandidateGeneration,
        BudgetStage::FormalEvaluation,
        BudgetStage::Curriculum,
        BudgetStage::MetaEvaluation,
        BudgetStage::HistoryCollection,
        BudgetStage::StorageCpu,
        BudgetStage::GrayOperations,
        BudgetStage::HumanReview,
    ];
    let names = stages
        .into_iter()
        .map(BudgetStage::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names.len(), 17);

    let plan_stages = [
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
    ];
    let mapped = plan_stages
        .into_iter()
        .map(BudgetStage::from)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(mapped.len(), 11);
}

#[tokio::test]
async fn every_frozen_stage_uses_the_same_root_reserve_dispatch_finalize_path() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let stages = [
        BudgetStage::Reflection,
        BudgetStage::Merge,
        BudgetStage::Ranking,
        BudgetStage::DevelopmentExecution,
        BudgetStage::DevelopmentScoring,
        BudgetStage::Practice,
        BudgetStage::Consolidation,
        BudgetStage::Guidance,
        BudgetStage::TaskExecution,
        BudgetStage::CandidateGeneration,
        BudgetStage::FormalEvaluation,
        BudgetStage::Curriculum,
        BudgetStage::MetaEvaluation,
        BudgetStage::HistoryCollection,
        BudgetStage::StorageCpu,
        BudgetStage::GrayOperations,
        BudgetStage::HumanReview,
    ];
    for (index, stage) in stages.into_iter().enumerate() {
        let id = format!("stage-{index}");
        let mut request = reservation(&id, 1, 10 + index as i64);
        request.stage = stage;
        let call = store.reserve_budget_call(&worker, &request).await.unwrap();
        let dispatched = store
            .begin_budget_dispatch(&worker, &fence(&call, request.now + 1))
            .await
            .unwrap()
            .call;
        let finalized = store
            .finalize_budget_call(
                &worker,
                &fence(&dispatched, request.now + 2),
                &charge(0, &id),
            )
            .await
            .unwrap();
        store
            .close_budget_call_execution(
                &worker,
                "scope-1",
                &id,
                finalized.dispatch_id.as_deref().unwrap(),
                "stage_complete",
                request.now + 3,
            )
            .await
            .unwrap();
    }
    let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
    assert_eq!(root.reserved_micros, 0);
    assert_eq!(root.spent_micros, 0);
}

#[tokio::test]
async fn group_ledger_keyset_scan_returns_all_10001_calls_and_stop_fences_old_lease() {
    let (dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let evaluator = ctx("ns-a", "evaluator", Role::Evaluator);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let mut transaction = connection.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO root_budget_dispatch_groups(
           billing_scope,dispatch_group_id,owner_namespace,created_at
         ) VALUES('scope-1','large-ticket','ns-a',1)",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    for index in 0..10_001 {
        sqlx::query(
            "INSERT INTO root_budget_calls(
               billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,
               reserved_micros,state,lease_token,lease_epoch,lease_until,execution_closed,created_at
             ) VALUES('scope-1',?,'large-ticket','ns-a','task_execution',?,1,'reserved',?,1,1000,1,1)",
        )
        .bind(format!("call-{index:05}"))
        .bind(hash(format!("input-{index}").as_bytes()))
        .bind(format!("lease-{index}"))
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE root_budgets SET reserved_micros=10001 WHERE billing_scope='scope-1'")
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    drop(connection);

    let mut session = store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&worker, "scope-1", "large-ticket")
        .await
        .unwrap();
    assert_eq!(calls.len(), 10_001);
    assert_eq!(calls.first().unwrap().call_id, "call-00000");
    assert_eq!(calls.last().unwrap().call_id, "call-10000");
    let attributed = calls
        .iter()
        .take(10_000)
        .map(|call| call.call_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let unknown = calls
        .iter()
        .filter(|call| !attributed.contains(call.call_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].call_id, "call-10000");
    session
        .stop_dispatch_group(&evaluator, "scope-1", "large-ticket", "early_stop", 2)
        .await
        .unwrap();
    let first = calls.first().unwrap();
    let old_fence = BudgetCallFence {
        billing_scope: first.billing_scope.clone(),
        call_id: first.call_id.clone(),
        actual_input_digest: first.actual_input_digest.clone(),
        lease_token: first.lease_token.clone(),
        lease_epoch: first.lease_epoch,
        now: 3,
    };
    assert!(matches!(
        session.begin_budget_dispatch(&worker, &old_fence).await,
        Err(Error::Cancelled)
    ));
    session.commit().await.unwrap();
}

// ---------------------------------------------------------------------------
// E16.5 AG-014: active lease capacity at the reservation entry point
// ---------------------------------------------------------------------------
#[tokio::test]
async fn eleventh_live_lease_is_refused_and_released_or_expired_leases_do_not_count() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker-a", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let mut calls = Vec::new();
    for index in 0..evo_storage::MVP_MAX_ACTIVE_LEASES {
        calls.push(
            store
                .reserve_budget_call(&worker, &reservation(&format!("lease-{index}"), 10, 1))
                .await
                .unwrap(),
        );
    }
    // Ten leases are live until 101: the eleventh reservation is refused
    // with the typed conflict and writes nothing.
    match store
        .reserve_budget_call(&worker, &reservation("lease-10", 10, 2))
        .await
    {
        Err(Error::Conflict(msg)) => {
            assert_eq!(msg, "MVP capacity exceeded: active leases 10 >= limit 10")
        }
        other => panic!("expected the MVP active lease conflict, got {other:?}"),
    }
    assert!(
        store
            .budget_call(&worker, "scope-1", "lease-10")
            .await
            .unwrap()
            .is_none()
    );
    let root = store.root_budget(&admin, "scope-1").await.unwrap();
    assert_eq!(root.unwrap().reserved_micros, 100);
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session.capacity_usage_v41(2).await.unwrap().active_leases,
        10
    );
    session.commit().await.unwrap();

    // Re-reserving an existing call is idempotent, not a new lease.
    let duplicate = store
        .reserve_budget_call(&worker, &reservation("lease-0", 10, 1))
        .await
        .unwrap();
    assert_eq!(duplicate, calls[0]);

    // A released call holds no lease: the next reservation is admitted.
    let released = store
        .release_undispatched_budget_call(&worker, &fence(&calls[0], 3), "capacity_test")
        .await
        .unwrap();
    assert_eq!(released.state, BudgetCallState::Released);
    let admitted = store
        .reserve_budget_call(&worker, &reservation("lease-10", 10, 3))
        .await
        .unwrap();
    assert_eq!(admitted.state, BudgetCallState::Reserved);

    // Ten live leases again: refused at a clock inside the leases, admitted
    // once the request clock is past every lease_until.
    assert!(matches!(
        store
            .reserve_budget_call(&worker, &reservation("lease-11", 10, 4))
            .await,
        Err(Error::Conflict(_))
    ));
    let after_expiry = store
        .reserve_budget_call(&worker, &reservation("lease-11", 10, 500))
        .await
        .unwrap();
    assert_eq!(after_expiry.state, BudgetCallState::Reserved);
    let mut session = store.session().await.unwrap();
    let usage = session.capacity_usage_v41(4).await.unwrap();
    assert_eq!(usage.active_leases, 11);
    let usage = session.capacity_usage_v41(500).await.unwrap();
    assert_eq!(usage.active_leases, 1);
    session.commit().await.unwrap();
}

/// F24: the lease cap is per instance. Ten live leases taken from ns-a refuse
/// the eleventh lease from ns-b on the same shared billing scope.
#[tokio::test]
async fn lease_capacity_cannot_be_split_across_namespaces() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    let worker_b = ctx("ns-b", "worker-b", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    for index in 0..evo_storage::MVP_MAX_ACTIVE_LEASES {
        store
            .reserve_budget_call(&worker_a, &reservation(&format!("split-a-{index}"), 10, 1))
            .await
            .unwrap();
    }
    match store
        .reserve_budget_call(&worker_b, &reservation("split-b", 10, 2))
        .await
    {
        Err(Error::Conflict(msg)) => {
            assert_eq!(msg, "MVP capacity exceeded: active leases 10 >= limit 10")
        }
        other => panic!("ns-b must not get an eleventh lease, got {other:?}"),
    }
    assert!(
        store
            .budget_call(&worker_b, "scope-1", "split-b")
            .await
            .unwrap()
            .is_none()
    );
    let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
    assert_eq!(root.reserved_micros, 100);
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session.capacity_usage_v41(2).await.unwrap().active_leases,
        10
    );
    session.commit().await.unwrap();
}

// AG-054: added comparisons leave every historical assertion above intact.
fn assert_exact_error(actual: Error, expected: &Error) {
    assert_eq!(
        std::mem::discriminant(&actual),
        std::mem::discriminant(expected)
    );
    assert_eq!(actual.to_string(), expected.to_string());
}

async fn compare_reserve_refusal(
    store: &Store,
    caller: &Context,
    request: &BudgetCallReservation,
    sources: &[String],
    code: &str,
    legacy: Error,
) {
    let refusal = store
        .reserve_budget_call_with_sources_typed(caller, request, sources)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code(), code);
    assert_exact_error(refusal.into_reservation_error(), &legacy);
    assert_exact_error(
        store
            .reserve_budget_call_with_sources(caller, request, sources)
            .await
            .unwrap_err(),
        &legacy,
    );
}

async fn authorized_store() -> (tempfile::TempDir, Store, Context, Context) {
    let (dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    (dir, store, admin, worker)
}

async fn put_budget_gate_object(
    store: &Store,
    admin: &Context,
    kind: &str,
    id: &str,
    body: serde_json::Value,
) {
    let mut session = store.session().await.unwrap();
    session
        .put(admin, kind, id, admin.actor(), &body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn budget_ref_id(call: &evo_storage::budget::BudgetCallRecord) -> String {
    let digest = evo_core::fingerprint(&(
        "rsia.budget_call_ref.v1",
        &call.namespace,
        &call.billing_scope,
        &call.call_id,
    ))
    .unwrap();
    format!("budget-ref-{}", &digest[..32])
}

#[tokio::test]
async fn ag054_reserve_typed_validation_authority_and_money_keep_legacy_errors() {
    let (_dir, missing) = store().await;
    let worker = ctx("ns-a", "worker", Role::Worker);
    compare_reserve_refusal(
        &missing,
        &worker,
        &reservation("missing", 10, 1),
        &[],
        "root_authorization_missing",
        Error::Budget,
    )
    .await;
    let (dir, store, admin, worker) = authorized_store().await;
    let request = reservation("new", 10, 2);
    compare_reserve_refusal(
        &store,
        &ctx("ns-a", "agent", Role::Agent),
        &request,
        &[],
        "unauthorized",
        Error::Forbidden,
    )
    .await;
    compare_reserve_refusal(
        &store,
        &ctx("ns-c", "worker", Role::Worker),
        &request,
        &[],
        "unauthorized",
        Error::Forbidden,
    )
    .await;
    let mut invalid = request.clone();
    invalid.max_cost_micros = 0;
    compare_reserve_refusal(
        &store,
        &worker,
        &invalid,
        &[],
        "invalid_request",
        Error::Invalid("reservation must be positive".into()),
    )
    .await;
    compare_reserve_refusal(
        &store,
        &worker,
        &request,
        &["bad source".into()],
        "invalid_request",
        evo_core::identifier("bad source").unwrap_err(),
    )
    .await;
    compare_reserve_refusal(
        &store,
        &worker,
        &request,
        &["source".into(), "source".into()],
        "duplicate_source",
        Error::Conflict("duplicate budget call source id".into()),
    )
    .await;
    let mut cap = request.clone();
    cap.max_cost_micros = 101;
    compare_reserve_refusal(
        &store,
        &worker,
        &cap,
        &[],
        "per_call_cap_exceeded",
        Error::Budget,
    )
    .await;
    let path = dir.path().join("rsia.sqlite3");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE root_budgets SET spent_micros=999")
        .execute(&mut connection)
        .await
        .unwrap();
    compare_reserve_refusal(
        &store,
        &worker,
        &request,
        &[],
        "root_budget_exhausted",
        Error::Budget,
    )
    .await;
    sqlx::query("UPDATE root_budgets SET spent_micros=?")
        .bind(i64::MAX)
        .execute(&mut connection)
        .await
        .unwrap();
    compare_reserve_refusal(
        &store,
        &worker,
        &request,
        &[],
        "budget_arithmetic_overflow",
        Error::Budget,
    )
    .await;
    assert!(
        store
            .budget_call(&worker, "scope-1", "new")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        0
    );
}

#[tokio::test]
async fn ag054_reserve_root_group_namespace_revocation_and_capacity_keep_legacy_errors() {
    for mode in [
        "root_stopped",
        "group_stopped",
        "group_namespace_mismatch",
        "source_revoked",
        "active_lease_capacity",
    ] {
        let (_dir, store, admin, worker) = authorized_store().await;
        let request = reservation("refused", 10, 2);
        let mut sources = Vec::new();
        match mode {
            "root_stopped" => {
                store
                    .stop_root_budget(&admin, "scope-1", "operator", 2)
                    .await
                    .unwrap();
            }
            "group_stopped" => {
                store
                    .stop_dispatch_group(
                        &admin,
                        "scope-1",
                        &request.dispatch_group_id,
                        "operator",
                        2,
                    )
                    .await
                    .unwrap();
            }
            "group_namespace_mismatch" => {
                let mut other = reservation("other", 10, 2);
                other.dispatch_group_id = request.dispatch_group_id.clone();
                store
                    .reserve_budget_call(&ctx("ns-b", "worker-b", Role::Worker), &other)
                    .await
                    .unwrap();
            }
            "source_revoked" => {
                sources.push("source".into());
                // Malformed tombstone is a determinate fail-closed refusal.
                put_budget_gate_object(
                    &store,
                    &admin,
                    "tombstone",
                    "source",
                    serde_json::json!({}),
                )
                .await;
            }
            "active_lease_capacity" => {
                for index in 0..10 {
                    store
                        .reserve_budget_call(&worker, &reservation(&format!("held-{index}"), 10, 2))
                        .await
                        .unwrap();
                }
                let refusal = store
                    .reserve_budget_call_typed(&worker, &request)
                    .await
                    .unwrap()
                    .unwrap_err();
                assert!(matches!(
                    refusal,
                    evo_storage::budget::BudgetPreDispatchRefusal::ActiveLeaseCapacity {
                        used: 10,
                        limit: 10
                    }
                ));
            }
            _ => unreachable!(),
        }
        let expected = match mode {
            "root_stopped" | "group_stopped" => Error::Cancelled,
            "group_namespace_mismatch" => {
                Error::Conflict("dispatch_group_owned_by_different_namespace".into())
            }
            "source_revoked" => Error::Forbidden,
            "active_lease_capacity" => {
                Error::Conflict("MVP capacity exceeded: active leases 10 >= limit 10".into())
            }
            _ => unreachable!(),
        };
        compare_reserve_refusal(&store, &worker, &request, &sources, mode, expected).await;
        assert!(
            store
                .budget_call(&worker, "scope-1", "refused")
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn ag054_call_reuse_preserves_reserved_finalized_and_uncertain_facts() {
    for state in [
        BudgetCallState::Reserved,
        BudgetCallState::Finalized,
        BudgetCallState::Uncertain,
    ] {
        let (_dir, store, _admin, worker) = authorized_store().await;
        let request = reservation("same", 10, 2);
        let call = store.reserve_budget_call(&worker, &request).await.unwrap();
        if state != BudgetCallState::Reserved {
            store
                .begin_budget_dispatch(&worker, &fence(&call, 3))
                .await
                .unwrap();
            if state == BudgetCallState::Finalized {
                store
                    .finalize_budget_call(&worker, &fence(&call, 4), &charge(7, "same"))
                    .await
                    .unwrap();
            } else {
                store
                    .mark_budget_call_uncertain(&worker, &fence(&call, 4), "unknown usage")
                    .await
                    .unwrap();
            }
        }
        let before = store
            .budget_call(&worker, "scope-1", "same")
            .await
            .unwrap()
            .unwrap();
        let root_before = store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap();
        let mut changed = request.clone();
        changed.actual_input_digest = hash(b"changed");
        compare_reserve_refusal(
            &store,
            &worker,
            &changed,
            &[],
            "call_id_reused",
            Error::Conflict("call_id_reused_with_different_content".into()),
        )
        .await;
        assert_eq!(
            store
                .budget_call(&worker, "scope-1", "same")
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert_eq!(
            store
                .root_budget(&worker, "scope-1")
                .await
                .unwrap()
                .unwrap(),
            root_before
        );
    }
}

#[tokio::test]
async fn ag054_source_closure_changed_is_typed_but_unreadable_ref_stays_internal() {
    let (_dir, store, admin, worker) = authorized_store().await;
    let request = reservation("closure", 10, 2);
    let call = store
        .reserve_budget_call_with_sources(&worker, &request, &["source".into()])
        .await
        .unwrap();
    let reference_id = budget_ref_id(&call);
    let mut session = store.session().await.unwrap();
    let before = session
        .raw_object(&admin, "artifact", &reference_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    compare_reserve_refusal(
        &store,
        &worker,
        &request,
        &["other".into()],
        "source_closure_changed",
        Error::Conflict("budget call source closure changed for fixed call".into()),
    )
    .await;
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .raw_object(&admin, "artifact", &reference_id)
            .await
            .unwrap(),
        before
    );
    session.commit().await.unwrap();
    // Reachable corrupt persisted reference, not a simulated DB/commit failure.
    put_budget_gate_object(
        &store,
        &admin,
        "artifact",
        &reference_id,
        serde_json::json!({}),
    )
    .await;
    assert!(matches!(
        store
            .reserve_budget_call_with_sources_typed(&worker, &request, &["source".into()])
            .await,
        Err(Error::Internal)
    ));
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(&worker, &request, &["source".into()])
            .await,
        Err(Error::Internal)
    ));
    assert!(matches!(
        store
            .begin_budget_dispatch_typed(&worker, &fence(&call, 3))
            .await,
        Err(Error::Internal)
    ));
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "closure")
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        10
    );
}

#[tokio::test]
async fn ag054_begin_causes_are_distinct_and_store_session_legacy_errors_are_exact() {
    for mode in [
        "root_stopped",
        "group_stopped",
        "lease_expired",
        "source_revoked",
        "root_concurrency_limit",
    ] {
        let (_dir, store, admin, worker) = authorized_store().await;
        let request = reservation("pending", 13, 2);
        let call = store
            .reserve_budget_call_with_sources(&worker, &request, &["source".into()])
            .await
            .unwrap();
        let mut pending_fence = fence(&call, 3);
        match mode {
            "root_stopped" => {
                store
                    .stop_root_budget(&admin, "scope-1", "operator", 3)
                    .await
                    .unwrap();
            }
            "group_stopped" => {
                store
                    .stop_dispatch_group(&admin, "scope-1", &call.dispatch_group_id, "operator", 3)
                    .await
                    .unwrap();
            }
            "lease_expired" => pending_fence.now = call.lease_until + 1,
            "source_revoked" => {
                put_budget_gate_object(
                    &store,
                    &admin,
                    "tombstone",
                    "source",
                    serde_json::json!({}),
                )
                .await;
            }
            "root_concurrency_limit" => {
                let active = store
                    .reserve_budget_call(&worker, &reservation("active", 7, 2))
                    .await
                    .unwrap();
                store
                    .begin_budget_dispatch(&worker, &fence(&active, 3))
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let refusal = store
            .begin_budget_dispatch_typed(&worker, &pending_fence)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(refusal.code(), mode);
        let expected = if mode == "root_concurrency_limit" {
            Error::Conflict("root_budget_concurrency_limit".into())
        } else {
            Error::Cancelled
        };
        assert_exact_error(refusal.into_dispatch_error(), &expected);
        assert_exact_error(
            store
                .begin_budget_dispatch(&worker, &pending_fence)
                .await
                .unwrap_err(),
            &expected,
        );
        let mut session = store.session().await.unwrap();
        assert_exact_error(
            session
                .begin_budget_dispatch(&worker, &pending_fence)
                .await
                .unwrap_err(),
            &expected,
        );
        session.commit().await.unwrap();
        assert_eq!(
            store
                .budget_call(&worker, "scope-1", "pending")
                .await
                .unwrap()
                .unwrap(),
            call
        );
    }
}

#[tokio::test]
async fn ag054_old_fence_remains_outer_error_and_cannot_release_new_lease() {
    let (_dir, store, _admin, worker) = authorized_store().await;
    let call = store
        .reserve_budget_call(&worker, &reservation("refenced", 13, 2))
        .await
        .unwrap();
    let old_fence = fence(&call, 103);
    let new_call = store
        .refence_reserved_budget_call(
            &worker,
            &BudgetCallRefence {
                billing_scope: call.billing_scope.clone(),
                call_id: call.call_id.clone(),
                expected_epoch: call.lease_epoch,
                new_lease_token: "new-token".into(),
                new_lease_until: 300,
                now: 103,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.begin_budget_dispatch_typed(&worker, &old_fence).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        store
            .release_undispatched_budget_call(&worker, &old_fence, "must not refund")
            .await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        store
            .refence_reserved_budget_call(
                &worker,
                &BudgetCallRefence {
                    billing_scope: call.billing_scope.clone(),
                    call_id: call.call_id.clone(),
                    expected_epoch: call.lease_epoch,
                    new_lease_token: "stale-token".into(),
                    new_lease_until: 400,
                    now: 301,
                }
            )
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "refenced")
            .await
            .unwrap()
            .unwrap(),
        new_call
    );
    assert_eq!(
        store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        13
    );
}

#[tokio::test]
async fn ag054_typed_access_refusals_preserve_legacy_scope_errors_without_mutation() {
    let (_dir, store, _admin, worker) = authorized_store().await;
    let call = store
        .reserve_budget_call(&worker, &reservation("private", 13, 2))
        .await
        .unwrap();
    for (caller, expected) in [
        (ctx("ns-c", "outsider", Role::Worker), Error::Forbidden),
        (ctx("ns-b", "worker-b", Role::Worker), Error::NotFound),
        (ctx("ns-a", "agent", Role::Agent), Error::Forbidden),
    ] {
        let refusal = store
            .begin_budget_dispatch_typed(&caller, &fence(&call, 3))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(refusal.code(), "unauthorized");
        assert_exact_error(refusal.into_dispatch_error(), &expected);
        assert_exact_error(
            store
                .begin_budget_dispatch(&caller, &fence(&call, 3))
                .await
                .unwrap_err(),
            &expected,
        );
        let refusal = store
            .budget_call_typed(&caller, "scope-1", "private")
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(refusal.code(), "unauthorized");
        assert_exact_error(refusal.into_dispatch_error(), &expected);
        assert_exact_error(
            store
                .budget_call(&caller, "scope-1", "private")
                .await
                .unwrap_err(),
            &expected,
        );
    }
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "private")
            .await
            .unwrap()
            .unwrap(),
        call
    );
}

#[tokio::test]
async fn ag054_r1_empty_source_recovery_preserves_money_refs_edges_and_revocation_gate() {
    let (dir, store, admin, worker) = authorized_store().await;
    put_budget_gate_object(&store, &admin, "run", "source",
        serde_json::json!({"id":"source","schema_version":"rsia.optimization.source.v1","body":"trusted source"})).await;
    let request = reservation("recovery", 13, 2);
    let call = store
        .reserve_budget_call_with_sources(&worker, &request, &["source".into()])
        .await
        .unwrap();
    let root = store
        .root_budget(&worker, "scope-1")
        .await
        .unwrap()
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!(
        "sqlite://{}",
        dir.path().join("rsia.sqlite3").display()
    ))
    .await
    .unwrap();
    let reference: String = sqlx::query_scalar(
        "SELECT body FROM objects WHERE namespace='ns-a' AND kind='artifact' AND id=?",
    )
    .bind(budget_ref_id(&call))
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let edges: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT src_kind,src_id,dst_kind,dst_id FROM dependencies WHERE namespace='ns-a' ORDER BY src_kind,src_id,dst_kind,dst_id"
    ).fetch_all(&mut connection).await.unwrap();
    assert!(!edges.is_empty());
    assert_eq!(
        store
            .reserve_budget_call_with_sources_typed(&worker, &request, &[])
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .reserve_budget_call_with_sources(&worker, &request, &[])
            .await
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .reserve_budget_call_typed(&worker, &request)
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store.reserve_budget_call(&worker, &request).await.unwrap(),
        call
    );
    assert_eq!(
        store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap(),
        root
    );
    let after_ref: String = sqlx::query_scalar(
        "SELECT body FROM objects WHERE namespace='ns-a' AND kind='artifact' AND id=?",
    )
    .bind(budget_ref_id(&call))
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let after_edges: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT src_kind,src_id,dst_kind,dst_id FROM dependencies WHERE namespace='ns-a' ORDER BY src_kind,src_id,dst_kind,dst_id"
    ).fetch_all(&mut connection).await.unwrap();
    assert_eq!(after_ref, reference);
    assert_eq!(after_edges, edges);
    evo_storage::lifecycle::LifecycleStore::begin_revoke(
        &admin,
        &store,
        evo_storage::lifecycle::TypedObjectRef {
            kind: "run".into(),
            id: "source".into(),
        },
        "revoked after recovery",
        3,
    )
    .await
    .unwrap();
    let refused = store
        .begin_budget_dispatch_typed(&worker, &fence(&call, 4))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refused,
        evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked
    );
    assert!(matches!(
        store.begin_budget_dispatch(&worker, &fence(&call, 4)).await,
        Err(Error::Cancelled)
    ));
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", &call.call_id)
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap(),
        root
    );
}

#[tokio::test]
async fn ag054_reservation_validation_branches_keep_exact_legacy_error_text() {
    let (_dir, store, _admin, worker) = authorized_store().await;
    for mode in [
        "scope",
        "call",
        "group",
        "input_digest",
        "token",
        "time",
        "lease",
        "artifact_schema",
        "artifact_digest",
        "artifact_mismatch",
        "artifact_size",
    ] {
        let mut request = reservation("invalid", 10, 2);
        let expected = match mode {
            "scope" => {
                request.billing_scope = "bad id".into();
                evo_core::identifier("bad id").unwrap_err()
            }
            "call" => {
                request.call_id = "bad id".into();
                evo_core::identifier("bad id").unwrap_err()
            }
            "group" => {
                request.dispatch_group_id = "bad id".into();
                evo_core::identifier("bad id").unwrap_err()
            }
            "input_digest" => {
                request.actual_input_digest = "bad".into();
                Error::Invalid("actual_input_digest: expected lowercase sha256 digest".into())
            }
            "token" => {
                request.lease_token = "bad id".into();
                evo_core::identifier("bad id").unwrap_err()
            }
            "time" => {
                request.now = -1;
                Error::Invalid("time must be nonnegative".into())
            }
            "lease" => {
                request.lease_until = request.now;
                Error::Invalid("lease must expire after reservation".into())
            }
            "artifact_schema" => {
                request.request_artifact = Some(BudgetArtifact {
                    schema_version: "bad id".into(),
                    digest: hash(b"{}"),
                    body: "{}".into(),
                });
                evo_core::identifier("bad id").unwrap_err()
            }
            "artifact_digest" => {
                request.request_artifact = Some(BudgetArtifact {
                    schema_version: "test.v1".into(),
                    digest: "bad".into(),
                    body: "{}".into(),
                });
                Error::Invalid("artifact digest: expected lowercase sha256 digest".into())
            }
            "artifact_mismatch" => {
                request.request_artifact = Some(BudgetArtifact {
                    schema_version: "test.v1".into(),
                    digest: hash(b"wrong"),
                    body: "{}".into(),
                });
                Error::Conflict("artifact digest mismatch".into())
            }
            "artifact_size" => {
                request.request_artifact = Some(BudgetArtifact {
                    schema_version: "test.v1".into(),
                    digest: hash(b"{}"),
                    body: " ".repeat(4 * 1024 * 1024 + 1),
                });
                Error::Invalid("budget artifact exceeds 4 MiB".into())
            }
            _ => unreachable!(),
        };
        compare_reserve_refusal(&store, &worker, &request, &[], "invalid_request", expected).await;
    }
    let mut request = reservation("invalid-json", 10, 2);
    request.request_artifact = Some(BudgetArtifact {
        schema_version: "test.v1".into(),
        digest: hash(b"not json"),
        body: "not json".into(),
    });
    assert!(matches!(
        store.reserve_budget_call_typed(&worker, &request).await,
        Err(Error::Internal)
    ));
    assert!(matches!(
        store.reserve_budget_call(&worker, &request).await,
        Err(Error::Internal)
    ));
    assert!(
        store
            .budget_call(&worker, "scope-1", "invalid-json")
            .await
            .unwrap()
            .is_none()
    );
}
