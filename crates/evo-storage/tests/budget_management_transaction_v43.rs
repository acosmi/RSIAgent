//! AG-146 storage transaction controls for the four budget management
//! operations. These are local storage tests: no provider, no model call and no
//! paid dispatch.
use evo_core::evaluation::{OptimizationBudgetPlan, RootStageBudgetPlanV1};
use evo_core::{Context, Error, Role, hash};
use evo_storage::budget::{
    BUDGET_OPEN_CALLS_PAGE_SCHEMA, BudgetArtifact, BudgetCallFence, BudgetCallRecord,
    BudgetCallReservation, BudgetCallSettlementView, BudgetCallState, BudgetOpenCallView,
    BudgetOpenCallsPage, BudgetReconcileOutcome, BudgetStage, RootBudgetAuthorization,
    RootBudgetRecord, RootBudgetUnresolvedReason, RootStagePlanBinding, UsageCharge,
};
use evo_storage::{
    ManagementBudgetResultV2, ManagementJobState, ManagementJobV2, ManagementResultV2, Store,
    VersionedManagementJob, decode_management_job,
};
use serde_json::json;

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
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
        authorization_receipt_digest: d("admin-authorization"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn reservation(scope: &str, call_id: &str, amount: i64, now: i64) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: scope.into(),
        call_id: call_id.into(),
        dispatch_group_id: format!("group-{call_id}"),
        stage: BudgetStage::Reflection,
        actual_input_digest: d(&format!("input-{call_id}")),
        request_artifact: None,
        max_cost_micros: amount,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + 100,
        now,
    }
}

fn fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
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
        output_digest: Some(d(&format!("output-{suffix}"))),
    }
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    (dir, store)
}

/// Two calls that make the second reconciliation overflow: the first is
/// finalized at `i64::MAX - 1000` (not an overrun, so the root stays open), its
/// execution is closed, and the second reserved call is dispatched afterwards.
async fn overflow_ready(
    store: &Store,
    admin: &Context,
    worker: &Context,
) -> (BudgetCallRecord, UsageCharge) {
    let scope = "scope-max";
    let mut authorization = authorization("root-max", scope);
    authorization.allowed_namespaces = vec!["ns-a".into()];
    authorization.per_call_cap_micros = i64::MAX;
    authorization.total_limit_micros = i64::MAX;
    store
        .authorize_root_budget(admin, &authorization)
        .await
        .unwrap();
    let first = store
        .reserve_budget_call(
            worker,
            &reservation(scope, "call-first", i64::MAX - 1_000, 2),
        )
        .await
        .unwrap();
    store
        .begin_budget_dispatch(worker, &fence(&first, 3))
        .await
        .unwrap();
    let second = store
        .reserve_budget_call(worker, &reservation(scope, "call-overflow", 1_000, 3))
        .await
        .unwrap();
    let finalized = store
        .finalize_budget_call(
            worker,
            &fence(&first, 4),
            &charge(i64::MAX - 1_000, "first"),
        )
        .await
        .unwrap();
    assert_eq!(finalized.state, BudgetCallState::Finalized);
    store
        .close_budget_call_execution(
            worker,
            scope,
            "call-first",
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            5,
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(worker, &fence(&second, 6))
        .await
        .unwrap();
    (dispatched.call, charge(2_000, "overflow"))
}

#[tokio::test]
async fn session_authorize_is_one_transaction_and_store_wrapper_keeps_semantics() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);

    // A Session write that never commits leaves no root behind.
    let mut session = store.session().await.unwrap();
    session
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    drop(session);
    assert!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .is_none()
    );

    // The Store wrapper still commits and reads the original record.
    let root = store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    assert_eq!(root.spent_micros, 0);
    assert_eq!(root.reserved_micros, 0);
    assert!(!root.stopped);

    // Same billing scope with identical authorization fields is idempotent,
    // including a different created_at sample (not a conflict field).
    let mut replay = authorization("root-1", "scope-1");
    replay.created_at = 99;
    assert_eq!(
        store.authorize_root_budget(&admin, &replay).await.unwrap(),
        root
    );

    // Any changed authorization field is refused and leaves the root intact.
    let mut changed = authorization("root-1", "scope-1");
    changed.total_limit_micros = 2_000;
    let error = store
        .authorize_root_budget(&admin, &changed)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("billing_scope_already_bound"));
    assert_eq!(
        store.root_budget(&admin, "scope-1").await.unwrap(),
        Some(root)
    );

    // A committed Session write creates the same real root.
    let mut session = store.session().await.unwrap();
    let committed = session
        .authorize_root_budget(&admin, &authorization("root-2", "scope-2"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(committed.billing_scope, "scope-2");
    assert!(
        store
            .root_budget(&admin, "scope-2")
            .await
            .unwrap()
            .is_some()
    );
    store.close().await;
}

#[tokio::test]
async fn session_reconcile_rolls_back_and_store_overflow_keeps_its_committed_facts() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-1", 30, 2))
        .await
        .unwrap();
    store
        .begin_budget_dispatch(&worker, &fence(&call, 3))
        .await
        .unwrap();

    // A Session reconcile that is rolled back changes neither the call nor the
    // root amounts. (AG-206: it runs after the dispatch lease, until 102.)
    let mut session = store.session().await.unwrap();
    let outcome = session
        .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &charge(20, "rollback"), 103)
        .await
        .unwrap();
    assert!(matches!(outcome, BudgetReconcileOutcome::Applied(_)));
    drop(session);
    let unchanged = store
        .budget_call(&worker, "scope-1", "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.state, BudgetCallState::Dispatched);
    assert_eq!(unchanged.actual_cost_micros, None);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        0
    );

    // The Store wrapper preserves the original overflow order: the preserved
    // facts are committed before Err(cost_accounting_overflow) is returned.
    let (overflow_call, overflow_charge) = overflow_ready(&store, &admin, &worker).await;
    let error = store
        .reconcile_budget_call_cost(
            &admin,
            &overflow_call.billing_scope,
            &overflow_call.call_id,
            &overflow_charge,
            // AG-206: past the overflow call's dispatch lease (until 103).
            104,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Conflict(ref reason) if reason == "cost_accounting_overflow"),
        "{error:?}"
    );
    let after = store
        .budget_call(&admin, &overflow_call.billing_scope, &overflow_call.call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.state, BudgetCallState::Uncertain);
    assert_eq!(after.actual_cost_micros, Some(2_000));
    assert_eq!(
        after.provider_request_id.as_deref(),
        Some("provider-overflow")
    );
    let root = store
        .root_budget(&admin, &overflow_call.billing_scope)
        .await
        .unwrap()
        .unwrap();
    assert!(root.stopped);
    assert_eq!(
        root.stop_reason.as_deref(),
        Some("cost_accounting_overflow")
    );
    store.close().await;
}

/// AG-206: a crash-left call whose bill is known but whose output was never
/// received is reconciled by provider_request_id alone. The stored output stays
/// NULL (nothing is invented), the amount is charged, and the finalized call is
/// a valid ledger state. Replays compare the output as an Option.
#[tokio::test]
async fn session_reconcile_without_output_finalizes_the_known_charge_only() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-1", 30, 2))
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(&worker, &fence(&call, 3))
        .await
        .unwrap()
        .call;
    let uncertain = store
        .cancel_budget_call(&worker, &fence(&dispatched, 4), "process_lost")
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);

    // A worker settlement still requires the output it received: it is
    // refused at the boundary and writes nothing.
    // (A separate root: call-1's open execution holds scope-1's dispatch slot.)
    let mut billed = charge(12, "billed");
    billed.output_digest = None;
    store
        .authorize_root_budget(&admin, &authorization("root-2", "scope-2"))
        .await
        .unwrap();
    let mut worker_call = store
        .reserve_budget_call(&worker, &reservation("scope-2", "call-w", 30, 4))
        .await
        .unwrap();
    worker_call = store
        .begin_budget_dispatch(&worker, &fence(&worker_call, 5))
        .await
        .unwrap()
        .call;
    let refused = store
        .finalize_budget_call(&worker, &fence(&worker_call, 6), &billed)
        .await
        .unwrap_err();
    assert!(matches!(refused, Error::Invalid(_)), "{refused:?}");
    let still = store
        .budget_call(&worker, "scope-2", "call-w")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.state, BudgetCallState::Dispatched);
    assert_eq!(still.actual_cost_micros, None);

    let mut session = store.session().await.unwrap();
    let outcome = session
        .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &billed, 7)
        .await
        .unwrap();
    let BudgetReconcileOutcome::Applied(applied) = outcome else {
        panic!("expected an applied reconciliation");
    };
    session.commit().await.unwrap();
    assert_eq!(applied.state, BudgetCallState::Finalized);
    assert_eq!(
        applied.provider_request_id.as_deref(),
        Some("provider-billed")
    );
    assert_eq!(applied.usage_record_id.as_deref(), Some("usage-billed"));
    assert_eq!(applied.actual_cost_micros, Some(12));
    assert_eq!(applied.output_digest, None);
    // Reconciliation does not close the execution it never observed.
    assert!(!applied.execution_closed);
    let stored = store
        .budget_call(&admin, "scope-1", "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, applied);
    let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
    assert_eq!(root.spent_micros, 12);
    assert_eq!(root.reserved_micros, 0);
    let view = BudgetCallSettlementView::new(&stored, &root);
    assert_eq!(view.output_digest, None);
    assert_eq!(view.actual_cost_micros, Some(12));

    // A finalized call without output is a valid ledger state, not an
    // invalid call.
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope-1")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        !snapshot.unresolved.iter().any(|issue| issue.reason
            == RootBudgetUnresolvedReason::InvalidCallState
            && issue.call_id.as_deref() == Some("call-1")),
        "{:?}",
        snapshot.unresolved
    );

    // The same no-output charge is an idempotent replay; adding an output
    // afterwards is a different charge and is refused without writes.
    let mut session = store.session().await.unwrap();
    let replay = session
        .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &billed, 8)
        .await
        .unwrap();
    assert!(
        matches!(replay, BudgetReconcileOutcome::AlreadyFinalized(ref same) if *same == stored)
    );
    let with_output = charge(12, "billed");
    let error = session
        .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &with_output, 8)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Conflict(ref reason) if reason == "finalized_call_reused_with_different_charge"),
        "{error:?}"
    );
    drop(session);

    // The reverse order is refused as well: a call reconciled with an output
    // cannot be replayed as "output not received".
    let finalized_w = store
        // AG-206: past call-w's dispatch lease (until 104).
        .reconcile_budget_call_cost(&admin, "scope-2", "call-w", &charge(5, "w"), 105)
        .await
        .unwrap();
    assert_eq!(
        finalized_w.output_digest.as_deref(),
        Some(d("output-w").as_str())
    );
    let mut without_output = charge(5, "w");
    without_output.output_digest = None;
    let error = store
        .reconcile_budget_call_cost(&admin, "scope-2", "call-w", &without_output, 106)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Conflict(ref reason) if reason == "finalized_call_reused_with_different_charge"),
        "{error:?}"
    );
    for (scope, spent) in [("scope-1", 12), ("scope-2", 5)] {
        assert_eq!(
            store
                .root_budget(&admin, scope)
                .await
                .unwrap()
                .unwrap()
                .spent_micros,
            spent
        );
    }
    store.close().await;
}

/// AG-206: while its dispatch lease is live the worker may still settle a
/// dispatched call, so Admin reconciliation is refused with zero writes; after
/// the lease expires it succeeds. An uncertain call ignores the lease.
#[tokio::test]
async fn reconcile_refuses_a_dispatched_call_whose_lease_is_still_live() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-1", 30, 2))
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(&worker, &fence(&call, 3))
        .await
        .unwrap()
        .call;
    assert_eq!(dispatched.lease_until, 102);
    let snapshot = || async {
        let mut session = store.session().await.unwrap();
        let snapshot = session
            .root_budget_monetary_snapshot(&admin, "scope-1")
            .await
            .unwrap();
        session.commit().await.unwrap();
        snapshot
    };
    let before = snapshot().await;
    // Both the Store wrapper and the Session kernel refuse, up to and
    // including the lease bound itself.
    for now in [4, 102] {
        let error = store
            .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &charge(12, "a"), now)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Conflict(ref reason) if reason == "reconcile_requires_expired_dispatch_lease"),
            "{error:?}"
        );
        let mut session = store.session().await.unwrap();
        let error = session
            .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &charge(12, "a"), now)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Conflict(ref reason) if reason == "reconcile_requires_expired_dispatch_lease"),
            "{error:?}"
        );
        session.commit().await.unwrap();
    }
    // Call row, root ledger and events are unchanged.
    assert_eq!(snapshot().await, before);

    // Once the lease has expired only Admin can close it, and it succeeds.
    let reconciled = store
        .reconcile_budget_call_cost(&admin, "scope-1", "call-1", &charge(12, "a"), 103)
        .await
        .unwrap();
    assert_eq!(reconciled.state, BudgetCallState::Finalized);
    assert_eq!(reconciled.actual_cost_micros, Some(12));
    // The exited worker's execution is closed so the root admits the next call.
    store
        .close_budget_call_execution(
            &worker,
            "scope-1",
            "call-1",
            dispatched.dispatch_id.as_deref().unwrap(),
            "process_confirmed_stopped",
            104,
        )
        .await
        .unwrap();

    // An uncertain call is reconciled regardless of its lease.
    let other = store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-2", 30, 104))
        .await
        .unwrap();
    let other = store
        .begin_budget_dispatch(&worker, &fence(&other, 105))
        .await
        .unwrap()
        .call;
    store
        .cancel_budget_call(&worker, &fence(&other, 106), "process_lost")
        .await
        .unwrap();
    assert!(107 <= other.lease_until);
    let uncertain = store
        .reconcile_budget_call_cost(&admin, "scope-1", "call-2", &charge(7, "b"), 107)
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Finalized);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        19
    );
    store.close().await;
}

#[tokio::test]
async fn session_overflow_outcome_preserves_facts_in_one_commit() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-a", "worker", Role::Worker);
    let (overflow_call, overflow_charge) = overflow_ready(&store, &admin, &worker).await;

    let mut session = store.session().await.unwrap();
    let outcome = session
        .reconcile_budget_call_cost(
            &admin,
            &overflow_call.billing_scope,
            &overflow_call.call_id,
            &overflow_charge,
            // AG-206: past the overflow call's dispatch lease (until 103).
            104,
        )
        .await
        .unwrap();
    let BudgetReconcileOutcome::PreservedAccountingOverflow(call) = outcome else {
        panic!("expected preserved accounting overflow");
    };
    assert_eq!(call.state, BudgetCallState::Uncertain);
    assert_eq!(call.actual_cost_micros, Some(2_000));
    session.commit().await.unwrap();
    let after = store
        .budget_call(&admin, &overflow_call.billing_scope, &overflow_call.call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.state, BudgetCallState::Uncertain);
    assert_eq!(after.actual_cost_micros, Some(2_000));
    assert_eq!(after.actual_currency.as_deref(), Some("USD"));
    assert_eq!(after.usage_record_id.as_deref(), Some("usage-overflow"));
    assert_eq!(
        after.output_digest.as_deref(),
        Some(d("output-overflow").as_str())
    );
    let root = store
        .root_budget(&admin, &overflow_call.billing_scope)
        .await
        .unwrap()
        .unwrap();
    assert!(root.stopped);
    assert_eq!(
        root.stop_reason.as_deref(),
        Some("cost_accounting_overflow")
    );
    // No refund and no automatic execution close: the preserved call keeps its
    // open execution and the root keeps its spent/reserved facts.
    assert!(!after.execution_closed);
    assert_eq!(root.spent_micros, i64::MAX - 1_000);
    assert_eq!(root.reserved_micros, 1_000);
    store.close().await;
}

#[tokio::test]
async fn list_open_projects_cross_namespace_calls_without_private_bodies() {
    let (_dir, store) = store().await;
    let admin_a = ctx("ns-a", "admin-a", Role::Admin);
    let admin_c = ctx("ns-c", "admin-c", Role::Admin);
    let worker_b = ctx("ns-b", "worker-b", Role::Worker);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    store
        .authorize_root_budget(&admin_a, &authorization("root-1", "scope-1"))
        .await
        .unwrap();

    // A released call is not open.
    let released = store
        .reserve_budget_call(&worker_b, &reservation("scope-1", "call-released", 10, 2))
        .await
        .unwrap();
    store
        .release_undispatched_budget_call(&worker_b, &fence(&released, 3), "retry")
        .await
        .unwrap();
    // An uncertain call whose execution is already closed is still open. Its
    // lease expires before the next dispatch, so the root's one concurrent
    // dispatch slot is free without interpreting the uncertain call as exited.
    let uncertain = store
        .reserve_budget_call(&worker_b, &reservation("scope-1", "call-uncertain", 20, 4))
        .await
        .unwrap();
    let uncertain_dispatch = store
        .begin_budget_dispatch(&worker_b, &fence(&uncertain, 5))
        .await
        .unwrap();
    store
        .mark_budget_call_uncertain(&worker_b, &fence(&uncertain, 6), "timeout")
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &worker_b,
            "scope-1",
            "call-uncertain",
            uncertain_dispatch.call.dispatch_id.as_deref().unwrap(),
            "process_confirmed_stopped",
            7,
        )
        .await
        .unwrap();
    // A reserved call stays open. Its private request body carries a marker
    // that must never be projected.
    let mut reserved_with_body = reservation("scope-1", "call-reserved", 10, 8);
    reserved_with_body.request_artifact = Some(
        BudgetArtifact::from_serializable(
            "rsia.test.request.v1",
            &json!({"secret": "SECRET-REQUEST-BODY"}),
        )
        .unwrap(),
    );
    store
        .reserve_budget_call(&worker_b, &reserved_with_body)
        .await
        .unwrap();

    // Another root's call is never named.
    let mut other_authorization = authorization("root-2", "scope-2");
    other_authorization.allowed_namespaces = vec!["ns-c".into()];
    store
        .authorize_root_budget(&admin_c, &other_authorization)
        .await
        .unwrap();
    let worker_c = ctx("ns-c", "worker-c", Role::Worker);
    store
        .reserve_budget_call(&worker_c, &reservation("scope-2", "other-root-call", 10, 2))
        .await
        .unwrap();

    // A third root isolates the finalized-but-execution-open class, which its
    // own root's one-dispatch gate cannot combine with a dispatched call.
    let mut final_open_authorization = authorization("root-3", "scope-3");
    final_open_authorization.allowed_namespaces = vec!["ns-a".into()];
    store
        .authorize_root_budget(&admin_a, &final_open_authorization)
        .await
        .unwrap();
    let final_open = store
        .reserve_budget_call(&worker_a, &reservation("scope-3", "call-final-open", 20, 2))
        .await
        .unwrap();
    store
        .begin_budget_dispatch(&worker_a, &fence(&final_open, 3))
        .await
        .unwrap();
    store
        .finalize_budget_call(&worker_a, &fence(&final_open, 4), &charge(15, "final-open"))
        .await
        .unwrap();

    let mut session = store.session().await.unwrap();
    let page = session
        .list_open_budget_calls(&admin_a, "scope-1", 50, None)
        .await
        .unwrap();
    assert_eq!(page.schema_version, BUDGET_OPEN_CALLS_PAGE_SCHEMA);
    assert_eq!(page.billing_scope, "scope-1");
    assert!(page.complete);
    assert!(page.next_cursor.is_none());
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
    assert!(!page.entries[1].execution_open);
    assert_eq!(page.entries[1].actual_cost_micros, None);
    assert!(!ids.contains(&"call-released"));
    assert!(!ids.contains(&"other-root-call"));
    let serialized = serde_json::to_string(&page).unwrap();
    assert!(!serialized.contains("SECRET-REQUEST-BODY"));
    assert!(!serialized.contains("request_artifact"));
    assert!(!serialized.contains("response_artifact"));

    let final_page = session
        .list_open_budget_calls(&admin_a, "scope-3", 50, None)
        .await
        .unwrap();
    assert_eq!(final_page.entries.len(), 1);
    assert_eq!(final_page.entries[0].state, BudgetCallState::Finalized);
    assert!(final_page.entries[0].execution_open);
    assert_eq!(final_page.entries[0].actual_cost_micros, Some(15));

    // A non-member Admin cannot read this root, with or without a cursor.
    assert!(matches!(
        session
            .list_open_budget_calls(&admin_c, "scope-1", 50, None)
            .await,
        Err(Error::NotFound)
    ));
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn list_open_paginates_by_call_id_with_a_real_view_anchor() {
    let (_dir, store) = store().await;
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-b", "worker", Role::Worker);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization("root-1", "scope-1"))
        .await
        .unwrap();
    for (index, call_id) in ["call-a", "call-b", "call-c"].iter().enumerate() {
        store
            .reserve_budget_call(
                &worker,
                &reservation("scope-1", call_id, 10, 2 + index as i64),
            )
            .await
            .unwrap();
    }
    let dispatched = store
        .budget_call(&worker, "scope-1", "call-a")
        .await
        .unwrap()
        .unwrap();
    store
        .begin_budget_dispatch(&worker, &fence(&dispatched, 6))
        .await
        .unwrap();
    store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-d", 10, 6))
        .await
        .unwrap();

    // A second root lives in the same store; it must never satisfy a scope-1
    // cursor even though the caller is a member of both.
    let mut other = authorization("root-2", "scope-2");
    other.allowed_namespaces = vec!["ns-a".into()];
    store.authorize_root_budget(&admin, &other).await.unwrap();
    store
        .reserve_budget_call(&worker_a, &reservation("scope-2", "call-other", 10, 8))
        .await
        .unwrap();

    let mut session = store.session().await.unwrap();
    let first = session
        .list_open_budget_calls(&admin, "scope-1", 2, None)
        .await
        .unwrap();
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b"]
    );
    assert_eq!(first.entries[0].state, BudgetCallState::Dispatched);
    assert!(!first.complete);
    let cursor = first.next_cursor.clone().unwrap();
    assert!(cursor.starts_with(&format!("v1:{}:", first.anchor)));

    let second = session
        .list_open_budget_calls(&admin, "scope-1", 2, Some(&cursor))
        .await
        .unwrap();
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-c", "call-d"]
    );
    assert!(second.complete);
    assert!(second.next_cursor.is_none());
    assert_eq!(first.anchor, second.anchor);

    // A cursor is a position, not authority: another root's scope is refused
    // before any page can be read, and a non-member Admin is refused too.
    assert!(matches!(
        session
            .list_open_budget_calls(&admin, "scope-2", 2, Some(&cursor))
            .await,
        Err(Error::Conflict(ref reason)) if reason == "budget_list_open_anchor_changed"
    ));
    let admin_c = ctx("ns-c", "admin-c", Role::Admin);
    assert!(matches!(
        session
            .list_open_budget_calls(&admin_c, "scope-1", 2, Some(&cursor))
            .await,
        Err(Error::NotFound)
    ));
    // Out-of-range page sizes are refused before any read.
    assert!(matches!(
        session
            .list_open_budget_calls(&admin, "scope-1", 0, None)
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        session
            .list_open_budget_calls(&admin, "scope-1", 101, None)
            .await,
        Err(Error::Invalid(_))
    ));
    session.commit().await.unwrap();

    // A changed projected view invalidates the old cursor instead of splicing.
    store
        .reserve_budget_call(&worker, &reservation("scope-1", "call-e", 10, 7))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    assert!(matches!(
        session
            .list_open_budget_calls(&admin, "scope-1", 2, Some(&cursor))
            .await,
        Err(Error::Conflict(ref reason)) if reason == "budget_list_open_anchor_changed"
    ));
    session.commit().await.unwrap();
    store.close().await;
}

fn budget_job(operation: &str, result: ManagementBudgetResultV2) -> ManagementJobV2 {
    ManagementJobV2 {
        id: "management-job-budget".into(),
        schema_version: "rsia.management_job.v2".into(),
        operation: operation.into(),
        request_key: "budget-key".into(),
        payload_digest: d("payload"),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Succeeded,
        step: "finished".into(),
        private_input_ref: "management-input-budget".into(),
        result: Some(ManagementResultV2::Budget(result)),
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 1,
        diagnostics: vec![],
        created_at: 1,
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    }
}

fn root_record() -> RootBudgetRecord {
    RootBudgetRecord {
        root_budget_id: "root-1".into(),
        billing_scope: "scope-1".into(),
        authorizing_namespace: "ns-a".into(),
        allowed_namespaces: vec!["ns-a".into()],
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer-1".into(),
        authorization_receipt_digest: d("receipt"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        spent_micros: 0,
        reserved_micros: 0,
        stopped: false,
        stop_reason: None,
        stop_committed_at: None,
        created_at: 1,
    }
}

#[test]
fn budget_results_round_trip_and_keep_their_physical_operation_binding() {
    let root = root_record();
    let binding = RootStagePlanBinding {
        billing_scope: "scope-1".into(),
        seq: 1,
        plan_digest: d("plan"),
        plan: RootStageBudgetPlanV1::from_optimization_budget(
            OptimizationBudgetPlan::unfunded("scope-1", "payer-1", "USD").unwrap(),
        ),
        payer_id: "payer-1".into(),
        root_total_micros: 1_000,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        authorization_receipt_digest: d("receipt"),
        authorizing_namespace: "ns-a".into(),
        bound_by_actor: "admin".into(),
        created_at: 1,
    };
    let page = BudgetOpenCallsPage {
        schema_version: BUDGET_OPEN_CALLS_PAGE_SCHEMA.into(),
        billing_scope: "scope-1".into(),
        anchor: d("anchor"),
        page_size: 50,
        entries: vec![BudgetOpenCallView {
            call_id: "call-1".into(),
            namespace: "ns-a".into(),
            stage: BudgetStage::Reflection,
            state: BudgetCallState::Dispatched,
            dispatch_group_id: "group-1".into(),
            dispatch_id: Some("dispatch-1".into()),
            provider_request_id: None,
            usage_record_id: None,
            output_digest: None,
            reserved_micros: 10,
            actual_cost_micros: None,
            actual_currency: None,
            actual_pricing_version: None,
            execution_closed: false,
            execution_open: true,
            terminal_reason: None,
            created_at: 1,
            dispatched_at: Some(2),
            finalized_at: None,
        }],
        next_cursor: None,
        complete: true,
    };
    let call = BudgetCallSettlementView {
        billing_scope: "scope-1".into(),
        call_id: "call-1".into(),
        state: BudgetCallState::Finalized,
        dispatch_id: Some("dispatch-1".into()),
        provider_request_id: Some("provider-1".into()),
        usage_record_id: Some("usage-1".into()),
        output_digest: Some(d("output")),
        reserved_micros: 10,
        actual_cost_micros: Some(9),
        actual_currency: Some("USD".into()),
        actual_pricing_version: Some("price-v1".into()),
        execution_closed: false,
        execution_closed_at: None,
        execution_close_reason: None,
        terminal_reason: None,
        finalized_at: Some(3),
        root_stopped: false,
        root_stop_reason: None,
    };

    for (operation, result, tag) in [
        (
            "budget.root.authorize",
            ManagementBudgetResultV2::RootAuthorized {
                root: Box::new(root.clone()),
            },
            "root_authorized",
        ),
        (
            "budget.stage_plan.bind",
            ManagementBudgetResultV2::StagePlanBound {
                binding: Box::new(binding.clone()),
            },
            "stage_plan_bound",
        ),
        (
            "budget.calls.list_open",
            ManagementBudgetResultV2::OpenCallsListed {
                page: Box::new(page.clone()),
            },
            "open_calls_listed",
        ),
        (
            "budget.call.reconcile",
            ManagementBudgetResultV2::CallReconciled {
                call: Box::new(call.clone()),
                accounting_overflow: false,
                provider_source_verified: false,
            },
            "call_reconciled",
        ),
    ] {
        let job = budget_job(operation, result);
        let raw = serde_json::to_string(&job).unwrap();
        assert!(raw.contains(&format!("\"result\":\"{tag}\"")));
        let decoded = decode_management_job(&raw).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), raw);

        // The same result under another operation is an invalid physical bind.
        let wrong = budget_job(
            "budget.call.reconcile",
            ManagementBudgetResultV2::RootAuthorized {
                root: Box::new(root.clone()),
            },
        );
        let error = decode_management_job(&serde_json::to_string(&wrong).unwrap()).unwrap_err();
        assert!(
            matches!(error, Error::Conflict(ref reason) if reason == "management_job_invalid_shape")
        );
    }

    // A truncated listing that claims completeness is rejected.
    let mut incomplete = page;
    incomplete.complete = false;
    incomplete.next_cursor = None;
    let error = decode_management_job(
        &serde_json::to_string(&budget_job(
            "budget.calls.list_open",
            ManagementBudgetResultV2::OpenCallsListed {
                page: Box::new(incomplete),
            },
        ))
        .unwrap(),
    )
    .unwrap_err();
    assert!(
        matches!(error, Error::Conflict(ref reason) if reason == "management_job_invalid_shape")
    );

    // Old v1 management jobs stay readable exactly as before.
    let mut original = json!({
        "id":"management-job-old",
        "schema_version":"rsia.management_job.v1",
        "operation":"meta.start",
        "request_key":"old-key",
        "payload_digest":d("old"),
        "owner_actor":"admin",
        "owner_role":"admin",
        "state":"blocked",
        "step":"blocked_feature",
        "private_input_ref":"management-input-old",
        "result":null,
        "error_code":"meta.start_consumer_unavailable",
        "cancel_requested":false,
        "lease_token":null,
        "lease_until":0,
        "generation":0,
        "diagnostics":[],
        "created_at":1
    });
    let decoded = decode_management_job(&serde_json::to_string(&original).unwrap()).unwrap();
    assert!(matches!(decoded, VersionedManagementJob::V1(_)));
    original["result"] = json!({"result":"evaluation_status","ticket_id":"ticket-1","lifecycle":"issued","planned_targets":1,"has_formal_terminal":false});
    assert!(matches!(
        decode_management_job(&serde_json::to_string(&original).unwrap()).unwrap(),
        VersionedManagementJob::V1(_)
    ));
}
