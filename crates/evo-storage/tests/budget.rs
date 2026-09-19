use evo_core::evaluation::OptimizationStage;
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRefence, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};

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
