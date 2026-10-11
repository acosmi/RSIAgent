//! Storage boundaries for formal authority. These synthetic ledger fixtures
//! prove accounting and transaction behavior; none is an ordinary approved P.

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallRef, BudgetCallReservation,
    BudgetCallState, BudgetExecutionProvenance, BudgetPreDispatchRefusal, BudgetStage,
    ModelCallSettlementEvidence, RootBudgetAuthorization, RootBudgetUnresolvedReason, UsageCharge,
};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use std::sync::Arc;
use tokio::sync::oneshot;

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn authorization() -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: "root".into(),
        billing_scope: "scope".into(),
        allowed_namespaces: vec!["ns-b".into(), "ns-a".into()],
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "payer".into(),
        authorization_receipt_digest: hash(b"trusted-admin-authorization"),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 1,
    }
}

fn reservation(id: &str, stage: BudgetStage, now: i64) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "scope".into(),
        call_id: id.into(),
        dispatch_group_id: format!("group-{id}"),
        stage,
        actual_input_digest: hash(id.as_bytes()),
        request_artifact: None,
        max_cost_micros: 10,
        lease_token: format!("lease-{id}"),
        lease_until: now + 10_000,
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

fn charge(id: &str, amount_micros: i64) -> UsageCharge {
    UsageCharge {
        amount_micros,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{id}"),
        usage_record_id: format!("usage-{id}"),
        output_digest: hash(id.as_bytes()),
    }
}

async fn setup() -> (tempfile::TempDir, Store, Context, Context, Context) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker_a = ctx("ns-a", "worker-a", Role::Worker);
    let worker_b = ctx("ns-b", "worker-b", Role::Worker);
    store
        .authorize_root_budget(&admin, &authorization())
        .await
        .unwrap();
    (dir, store, admin, worker_a, worker_b)
}

async fn pay_and_close(
    store: &Store,
    worker: &Context,
    request: &BudgetCallReservation,
    amount: i64,
) {
    let call = store.reserve_budget_call(worker, request).await.unwrap();
    let dispatch = store
        .begin_budget_dispatch(worker, &fence(&call, request.now + 1))
        .await
        .unwrap();
    assert!(dispatch.new_dispatch);
    store
        .finalize_budget_call(
            worker,
            &fence(&dispatch.call, request.now + 2),
            &charge(&call.call_id, amount),
        )
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            worker,
            "scope",
            &call.call_id,
            dispatch.call.dispatch_id.as_deref().unwrap(),
            "execution completed",
            request.now + 3,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn whole_root_reads_all_namespaces_siblings_and_seventeen_stages_without_claiming_episode_cost()
 {
    let (_dir, store, admin, worker_a, worker_b) = setup().await;
    for (index, stage) in BudgetStage::ALL.into_iter().enumerate() {
        let worker = if index % 2 == 0 { &worker_a } else { &worker_b };
        let mut request = reservation(&format!("call-{index:02}"), stage, 10 + index as i64 * 10);
        request.dispatch_group_id = format!("sibling-{}", worker.namespace());
        pay_and_close(&store, worker, &request, 2).await;
    }
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert_eq!(snapshot.calls.len(), 17);
    assert_eq!(snapshot.dispatch_groups.len(), 2);
    assert_eq!(snapshot.root.allowed_namespaces, ["ns-a", "ns-b"]);
    assert_eq!(snapshot.known_actual_micros, "34");
    assert_eq!(snapshot.finalized_actual_micros, "34");
    assert_eq!(snapshot.held_reservation_micros, "0");
    assert_eq!(snapshot.root.spent_micros, 34);
    assert_eq!(snapshot.stage_totals.len(), 17);
    for total in &snapshot.stage_totals {
        assert_eq!(total.call_count, 1);
        assert_eq!(total.known_actual_micros, "2");
        assert_eq!(total.finalized_actual_micros, "2");
        assert_eq!(total.held_reservation_micros, "0");
        assert_eq!(total.unknown_actual_call_count, 0);
    }
    assert!(!snapshot.monetary_complete);
    assert!(!snapshot.complete_episode_cost);
    assert!(!snapshot.ordinary_p_eligible);
    let sealed = session
        .seal_root_budget(&admin, "scope", "all calls closed", 1_000)
        .await
        .unwrap();
    assert!(sealed.monetary_complete);
    assert!(!sealed.complete_episode_cost);
    assert!(!sealed.ordinary_p_eligible);
    assert!(sealed.unresolved.is_empty());
    session.commit().await.unwrap();
    let evaluator = ctx("ns-b", "independent-evaluator", Role::Evaluator);
    let mut session = store.session().await.unwrap();
    let read = session
        .sealed_root_budget_evidence(&evaluator, "scope")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read, sealed);
    let value = serde_json::to_value(&read).unwrap();
    assert!(value.get("calls").is_none());
    assert!(value.get("ledger_events").is_none());
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("lease_token")
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn fixture_money_never_becomes_ordinary_p_and_incompatible_charge_keeps_its_denomination() {
    for mismatched_currency in [false, true] {
        let (_dir, store, admin, _, _) = setup().await;
        let host = ctx("ns-b", "trusted-test-host", Role::Host);
        let mut request = reservation("fixture", BudgetStage::FormalEvaluation, 2);
        // AG-107 fixture (independent review: plan §7.2.1 L831 rechecks a queued
        // call before dispatch, §11.3 L1112 isolates what is missing, and the AG-107
        // card): a call reserved without a source list is judged at dispatch by the
        // closure its request names, and a request the closure reader cannot read
        // (this case used `test.model_request.v1`) now fails closed. The fixture
        // money tested here does not depend on that schema, so the request is a
        // model request artifact that names no source. Replacement coverage for the
        // unreadable request: `budget_ref_missing_fail_closed_v42.rs` (T2).
        request.request_artifact = Some(
            BudgetArtifact::from_serializable(
                "rsia.model_request.artifact.v1",
                &json!({"fixture":true,"source_closure":[],"input":"frozen"}),
            )
            .unwrap(),
        );
        let call = store.reserve_budget_call(&host, &request).await.unwrap();
        let dispatch = store
            .begin_budget_dispatch(&host, &fence(&call, 3))
            .await
            .unwrap();
        let evidence = ModelCallSettlementEvidence {
            provenance: BudgetExecutionProvenance::Fixture,
            actual_model_digest: Some(hash(b"fixture-model")),
            transport_artifact: BudgetArtifact::from_serializable(
                "test.transport.v1",
                &json!({"fixture":true}),
            )
            .unwrap(),
            usable_response: Some(
                BudgetArtifact::from_serializable(
                    "test.response.v1",
                    &json!({"status":"completed"}),
                )
                .unwrap(),
            ),
            blocked_response: BudgetArtifact::from_serializable(
                "test.response.v1",
                &json!({"status":"blocked"}),
            )
            .unwrap(),
            forced_block_reason: None,
        };
        let mut actual = charge("fixture", 3);
        if mismatched_currency {
            actual.currency = "EUR".into();
        }
        let settlement = store
            .settle_model_budget_call(&host, &fence(&dispatch.call, 4), &actual, &evidence)
            .await
            .unwrap();
        assert_eq!(
            settlement.call.execution_provenance,
            Some(BudgetExecutionProvenance::Fixture)
        );
        assert_eq!(settlement.response_usable, !mismatched_currency);
        store
            .close_budget_call_execution(
                &host,
                "scope",
                "fixture",
                dispatch.call.dispatch_id.as_deref().unwrap(),
                "fixture execution closed",
                5,
            )
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        let sealed = session
            .seal_root_budget(&admin, "scope", "fixture monetary accounting only", 6)
            .await
            .unwrap();
        assert_eq!(sealed.monetary_complete, !mismatched_currency);
        assert!(!sealed.complete_episode_cost);
        assert!(!sealed.ordinary_p_eligible);
        if mismatched_currency {
            assert_eq!(sealed.held_reservation_micros, "10");
            assert!(sealed.unresolved.iter().any(|issue| issue.reason
                == RootBudgetUnresolvedReason::ChargeIdentityMismatch
                && issue.actual_cost_micros == Some(3)
                && issue.actual_currency.as_deref() == Some("EUR")));
            let snapshot = session
                .root_budget_monetary_snapshot(&admin, "scope")
                .await
                .unwrap();
            assert_eq!(snapshot.calls[0].actual_cost_micros, Some(3));
            assert_eq!(snapshot.calls[0].actual_currency.as_deref(), Some("EUR"));
        } else {
            assert_eq!(sealed.known_actual_micros, "3");
        }
        session.commit().await.unwrap();
        store.close().await;
    }
}

#[tokio::test]
async fn reservation_candidate_proposal_edges_and_audit_share_one_commit_or_rollback() {
    let (_dir, store, admin, worker, _) = setup().await;
    for commit in [false, true] {
        let mut session = store.session().await.unwrap();
        session
            .reserve_budget_call_with_sources_typed(
                &worker,
                &reservation("atomic", BudgetStage::CandidateGeneration, 2),
                &["source-run".into()],
            )
            .await
            .unwrap()
            .unwrap();
        for (kind, id) in [("candidate", "candidate"), ("artifact", "proposal")] {
            session
                .put(
                    &worker,
                    kind,
                    id,
                    worker.actor(),
                    &json!({"fixture":true,"ordinary_p":false}),
                )
                .await
                .unwrap();
        }
        session
            .put_edge(&worker, "candidate", "candidate", "artifact", "proposal")
            .await
            .unwrap();
        session
            .put_edge(&worker, "artifact", "proposal", "run", "source-run")
            .await
            .unwrap();
        session
            .audit(&worker, "test.atomic.proposal", "candidate")
            .await
            .unwrap();
        if commit {
            session.commit().await.unwrap();
        } else {
            drop(session);
        }
        let mut session = store.session().await.unwrap();
        let snapshot = session
            .root_budget_monetary_snapshot(&admin, "scope")
            .await
            .unwrap();
        assert_eq!(snapshot.calls.len(), usize::from(commit));
        assert_eq!(snapshot.dispatch_groups.len(), usize::from(commit));
        assert_eq!(snapshot.root.reserved_micros, if commit { 10 } else { 0 });
        assert_eq!(
            session
                .get::<Value>(&worker, "candidate", "candidate")
                .await
                .unwrap()
                .is_some(),
            commit
        );
        assert_eq!(
            session
                .get::<Value>(&worker, "artifact", "proposal")
                .await
                .unwrap()
                .is_some(),
            commit
        );
        assert_eq!(
            session
                .dependency_edges(&worker, "candidate", "candidate")
                .await
                .unwrap(),
            if commit {
                vec![("artifact".into(), "proposal".into())]
            } else {
                vec![]
            }
        );
        session.commit().await.unwrap();
        assert_eq!(
            store.verify_audit(&admin).await.unwrap(),
            usize::from(commit)
        );
    }
    store.close().await;
}

#[tokio::test]
async fn typed_capacity_refusal_rolls_back_tentative_group_even_when_caller_commits() {
    let (_dir, store, admin, worker, _) = setup().await;
    for index in 0..10 {
        let mut request = reservation(&format!("held-{index}"), BudgetStage::Reflection, 2);
        request.max_cost_micros = 1;
        store.reserve_budget_call(&worker, &request).await.unwrap();
    }
    let mut session = store.session().await.unwrap();
    let result = session
        .reserve_budget_call_typed(&worker, &reservation("refused", BudgetStage::Practice, 3))
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(BudgetPreDispatchRefusal::ActiveLeaseCapacity {
            used: 10,
            limit: 10
        })
    ));
    session
        .put(
            &worker,
            "artifact",
            "after-refusal",
            worker.actor(),
            &json!({"ordinary_p":false}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert_eq!(snapshot.calls.len(), 10);
    assert_eq!(snapshot.dispatch_groups.len(), 10);
    assert_eq!(snapshot.root.reserved_micros, 10);
    assert!(
        !snapshot
            .ledger_events
            .iter()
            .any(|event| event.details.contains("refused"))
    );
    assert!(
        session
            .get::<Value>(&worker, "artifact", "after-refusal")
            .await
            .unwrap()
            .is_some()
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn late_source_closure_refusal_does_not_leave_call_amount_group_or_event() {
    let (_dir, store, admin, worker, _) = setup().await;
    let request = reservation("collision", BudgetStage::CandidateGeneration, 2);
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        worker.namespace(),
        &request.billing_scope,
        &request.call_id,
    ))
    .unwrap();
    let reference = BudgetCallRef {
        id: format!("budget-ref-{}", &digest[..32]),
        schema_version: "rsia.budget_call_ref.v1".into(),
        namespace: worker.namespace().into(),
        billing_scope: "scope".into(),
        call_id: request.call_id.clone(),
        source_ids: vec!["different-source".into()],
    };
    let mut session = store.session().await.unwrap();
    session
        .put(
            &worker,
            "artifact",
            &reference.id,
            worker.actor(),
            &reference,
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .reserve_budget_call_with_sources_typed(&worker, &request, &["source-run".into()])
            .await
            .unwrap()
            .unwrap_err(),
        BudgetPreDispatchRefusal::SourceClosureChanged
    );
    session
        .audit(&worker, "test.refusal.kept-caller-work", &reference.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert!(snapshot.calls.is_empty());
    assert!(snapshot.dispatch_groups.is_empty());
    assert_eq!(snapshot.root.reserved_micros, 0);
    assert!(
        !snapshot
            .ledger_events
            .iter()
            .any(|event| event.call_id.as_deref() == Some("collision"))
    );
    assert_eq!(
        session
            .need::<BudgetCallRef>(&worker, "artifact", &reference.id)
            .await
            .unwrap(),
        reference
    );
    session.commit().await.unwrap();
    assert_eq!(store.verify_audit(&admin).await.unwrap(), 1);
    store.close().await;
}

#[tokio::test]
async fn only_real_scoped_admin_seals_and_public_artifacts_cannot_mint_evidence() {
    let (_dir, store, admin, worker, worker_b) = setup().await;
    let foreign_admin = ctx("foreign", "admin", Role::Admin);
    let agent = ctx("ns-a", "agent", Role::Agent);
    let mut session = store.session().await.unwrap();
    session.put(&worker, "artifact", "fake-seal", worker.actor(),
        &json!({"schema_version":"rsia.root_budget_monetary_seal.v1","monetary_complete":true,"ordinary_p_eligible":true}))
        .await.unwrap();
    assert!(
        session
            .sealed_root_budget_evidence(&worker, "scope")
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        session
            .seal_root_budget(&worker, "scope", "forged role", 2)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        session
            .root_budget_monetary_snapshot(&worker_b, "scope")
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        session
            .seal_root_budget(&foreign_admin, "scope", "foreign scope", 2)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        session.sealed_root_budget_evidence(&agent, "scope").await,
        Err(Error::Forbidden)
    ));
    let sealed = session
        .seal_root_budget(&admin, "scope", "empty monetary root", 3)
        .await
        .unwrap();
    assert!(sealed.monetary_complete);
    assert!(!sealed.complete_episode_cost);
    assert!(!sealed.ordinary_p_eligible);
    assert_eq!(sealed.call_count, 0);
    assert!(
        sealed
            .stage_totals
            .iter()
            .all(|stage| stage.call_count == 0)
    );
    assert_eq!(
        session
            .sealed_root_budget_evidence(&worker_b, "scope")
            .await
            .unwrap(),
        Some(sealed)
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn reserved_uncertain_unknown_and_late_paid_siblings_survive_sealing_and_require_reseal() {
    let (_dir, store, admin, worker_a, worker_b) = setup().await;
    pay_and_close(
        &store,
        &worker_a,
        &reservation("paid-sibling", BudgetStage::FormalEvaluation, 2),
        4,
    )
    .await;
    let pending = store
        .reserve_budget_call(
            &worker_b,
            &reservation("uncertain", BudgetStage::TaskExecution, 10),
        )
        .await
        .unwrap();
    let dispatch = store
        .begin_budget_dispatch(&worker_b, &fence(&pending, 11))
        .await
        .unwrap();
    store
        .mark_budget_call_uncertain(
            &worker_b,
            &fence(&dispatch.call, 12),
            "provider usage unknown",
        )
        .await
        .unwrap();
    let reserved = store
        .reserve_budget_call(
            &worker_a,
            &reservation("reserved-sibling", BudgetStage::HumanReview, 13),
        )
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let first = session
        .seal_root_budget(&admin, "scope", "stop with unresolved siblings", 20)
        .await
        .unwrap();
    assert_eq!(first.call_count, 3);
    assert_eq!(first.known_actual_micros, "4");
    assert_eq!(first.held_reservation_micros, "20");
    assert!(!first.monetary_complete);
    assert!(first.unresolved.iter().any(|issue| issue.reason
        == RootBudgetUnresolvedReason::Uncertain
        && issue.call_id.as_deref() == Some("uncertain")
        && issue.actual_cost_micros.is_none()));
    assert!(
        first
            .unresolved
            .iter()
            .any(|issue| issue.reason == RootBudgetUnresolvedReason::Reserved)
    );
    assert_eq!(
        session
            .begin_budget_dispatch_typed(&worker_a, &fence(&reserved, 21))
            .await
            .unwrap()
            .unwrap_err(),
        BudgetPreDispatchRefusal::RootStopped
    );
    session.commit().await.unwrap();
    store
        .reconcile_budget_call_cost(&admin, "scope", "uncertain", &charge("uncertain", 6), 22)
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &worker_b,
            "scope",
            "uncertain",
            dispatch.call.dispatch_id.as_deref().unwrap(),
            "late execution closed",
            23,
        )
        .await
        .unwrap();
    store
        .release_undispatched_budget_call(&worker_a, &fence(&reserved, 24), "not dispatched")
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    assert!(matches!(
        session
            .sealed_root_budget_evidence(&worker_b, "scope")
            .await,
        Err(Error::Conflict(_))
    ));
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert_eq!(snapshot.root.spent_micros, 10);
    assert_eq!(snapshot.root.reserved_micros, 0);
    assert_eq!(snapshot.known_actual_micros, "10");
    assert!(
        snapshot
            .ledger_events
            .iter()
            .any(|event| event.event_kind == "usage_uncertain"
                && event.details.contains("provider usage unknown"))
    );
    assert!(
        snapshot
            .ledger_events
            .iter()
            .any(|event| event.event_kind == "cost_reconciled")
    );
    let second = session
        .seal_root_budget(&admin, "scope", "late costs reconciled", 25)
        .await
        .unwrap();
    assert!(second.monetary_complete);
    assert_ne!(second.snapshot_digest, first.snapshot_digest);
    assert_eq!(second.root.stop_reason, first.root.stop_reason);
    assert_eq!(second.root.stop_committed_at, first.root.stop_committed_at);
    assert!(!second.complete_episode_cost);
    assert!(!second.ordinary_p_eligible);
    assert_eq!(
        session
            .sealed_root_budget_evidence(&worker_a, "scope")
            .await
            .unwrap(),
        Some(second)
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn same_session_seal_fences_new_reservation_and_dispatch_in_both_orders() {
    for seal_first in [true, false] {
        let (_dir, store, admin, worker, _) = setup().await;
        let request = reservation("ordered", BudgetStage::Practice, 2);
        let mut session = store.session().await.unwrap();
        if seal_first {
            let seal = session
                .seal_root_budget(&admin, "scope", "seal before reserve", 3)
                .await
                .unwrap();
            assert!(seal.monetary_complete);
            assert_eq!(
                session
                    .reserve_budget_call_typed(&worker, &request)
                    .await
                    .unwrap()
                    .unwrap_err(),
                BudgetPreDispatchRefusal::RootStopped
            );
        } else {
            let call = session
                .reserve_budget_call_typed(&worker, &request)
                .await
                .unwrap()
                .unwrap();
            let seal = session
                .seal_root_budget(&admin, "scope", "reserve before seal", 3)
                .await
                .unwrap();
            assert!(!seal.monetary_complete);
            assert_eq!(seal.held_reservation_micros, "10");
            assert_eq!(
                session
                    .begin_budget_dispatch_typed(&worker, &fence(&call, 4))
                    .await
                    .unwrap()
                    .unwrap_err(),
                BudgetPreDispatchRefusal::RootStopped
            );
            // Historical idempotent reads remain possible; they do not dispatch.
            assert_eq!(
                session
                    .reserve_budget_call_typed(&worker, &request)
                    .await
                    .unwrap()
                    .unwrap(),
                call
            );
        }
        session.commit().await.unwrap();
        let root = store.root_budget(&admin, "scope").await.unwrap().unwrap();
        assert!(root.stopped);
        assert_eq!(root.reserved_micros, if seal_first { 0 } else { 10 });
        store.close().await;
    }
}

#[tokio::test]
async fn root_stop_and_seal_rollback_with_the_caller_session_instead_of_committing_elsewhere() {
    let (_dir, store, admin, worker, _) = setup().await;
    let mut session = store.session().await.unwrap();
    session
        .stop_root_budget(&admin, "scope", "tentative stop", 2)
        .await
        .unwrap();
    assert!(
        session
            .root_budget(&worker, "scope")
            .await
            .unwrap()
            .unwrap()
            .stopped
    );
    drop(session);
    let mut session = store.session().await.unwrap();
    assert!(
        !session
            .root_budget(&worker, "scope")
            .await
            .unwrap()
            .unwrap()
            .stopped
    );
    assert!(
        session
            .sealed_root_budget_evidence(&worker, "scope")
            .await
            .unwrap()
            .is_none()
    );
    let tentative = session
        .seal_root_budget(&admin, "scope", "tentative seal", 3)
        .await
        .unwrap();
    assert!(tentative.monetary_complete);
    drop(session);
    let mut session = store.session().await.unwrap();
    assert!(
        !session
            .root_budget(&worker, "scope")
            .await
            .unwrap()
            .unwrap()
            .stopped
    );
    assert!(
        session
            .sealed_root_budget_evidence(&worker, "scope")
            .await
            .unwrap()
            .is_none()
    );
    session
        .reserve_budget_call_typed(
            &worker,
            &reservation("after-rollback", BudgetStage::Reflection, 4),
        )
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        10
    );
    store.close().await;
}

#[tokio::test]
async fn concurrent_sessions_serialize_seal_versus_reserve_in_both_orders() {
    for seal_first in [true, false] {
        let (_dir, store, admin, worker, _) = setup().await;
        let store = Arc::new(store);
        let request = reservation("concurrent-reserve", BudgetStage::Practice, 2);
        let mut first = store.session().await.unwrap();
        if seal_first {
            first
                .seal_root_budget(&admin, "scope", "first seal", 4)
                .await
                .unwrap();
        } else {
            first
                .reserve_budget_call_typed(&worker, &request)
                .await
                .unwrap()
                .unwrap();
        }
        let (entered_tx, entered_rx) = oneshot::channel();
        let other_store = Arc::clone(&store);
        let other_admin = admin.clone();
        let other_worker = worker.clone();
        let second = tokio::spawn(async move {
            entered_tx.send(()).unwrap();
            // This acquire overlaps the first owned Session and must wait for it.
            let mut session = other_store.session().await.unwrap();
            if seal_first {
                assert_eq!(
                    session
                        .reserve_budget_call_typed(&other_worker, &request)
                        .await
                        .unwrap()
                        .unwrap_err(),
                    BudgetPreDispatchRefusal::RootStopped
                );
            } else {
                let sealed = session
                    .seal_root_budget(&other_admin, "scope", "second seal", 4)
                    .await
                    .unwrap();
                assert!(!sealed.monetary_complete);
                assert_eq!(sealed.held_reservation_micros, "10");
            }
            session.commit().await.unwrap();
        });
        entered_rx.await.unwrap();
        first.commit().await.unwrap();
        second.await.unwrap();
        assert!(
            store
                .root_budget(&admin, "scope")
                .await
                .unwrap()
                .unwrap()
                .stopped
        );
        assert_eq!(
            store
                .budget_call(&worker, "scope", "concurrent-reserve")
                .await
                .unwrap()
                .is_some(),
            !seal_first
        );
        store.close().await;
    }
}

#[tokio::test]
async fn concurrent_sessions_serialize_seal_versus_dispatch_without_forgetting_inflight_usage() {
    for seal_first in [true, false] {
        let (_dir, store, admin, worker, _) = setup().await;
        let store = Arc::new(store);
        let call = store
            .reserve_budget_call(
                &worker,
                &reservation("concurrent-dispatch", BudgetStage::TaskExecution, 2),
            )
            .await
            .unwrap();
        let dispatch_fence = fence(&call, 3);
        let mut first = store.session().await.unwrap();
        if seal_first {
            first
                .seal_root_budget(&admin, "scope", "first seal", 4)
                .await
                .unwrap();
        } else {
            assert!(
                first
                    .begin_budget_dispatch_typed(&worker, &dispatch_fence)
                    .await
                    .unwrap()
                    .unwrap()
                    .new_dispatch
            );
        }
        let (entered_tx, entered_rx) = oneshot::channel();
        let other_store = Arc::clone(&store);
        let other_admin = admin.clone();
        let other_worker = worker.clone();
        let second = tokio::spawn(async move {
            entered_tx.send(()).unwrap();
            let mut session = other_store.session().await.unwrap();
            if seal_first {
                assert_eq!(
                    session
                        .begin_budget_dispatch_typed(&other_worker, &dispatch_fence)
                        .await
                        .unwrap()
                        .unwrap_err(),
                    BudgetPreDispatchRefusal::RootStopped
                );
            } else {
                let sealed = session
                    .seal_root_budget(&other_admin, "scope", "second seal", 4)
                    .await
                    .unwrap();
                assert!(!sealed.monetary_complete);
                assert!(sealed.unresolved.iter().any(|issue| issue.reason
                    == RootBudgetUnresolvedReason::Dispatched
                    && issue.actual_cost_micros.is_none()));
            }
            session.commit().await.unwrap();
        });
        entered_rx.await.unwrap();
        first.commit().await.unwrap();
        second.await.unwrap();
        let persisted = store
            .budget_call(&worker, "scope", "concurrent-dispatch")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.state,
            if seal_first {
                BudgetCallState::Reserved
            } else {
                BudgetCallState::Dispatched
            }
        );
        assert_eq!(persisted.actual_cost_micros, None);
        assert_eq!(persisted.execution_closed, seal_first);
        assert_eq!(
            store
                .root_budget(&admin, "scope")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            10
        );
        store.close().await;
    }
}

#[tokio::test]
async fn overflow_keeps_exact_known_charge_and_original_accounting_without_unknown_zero() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let worker = ctx("ns-b", "worker", Role::Worker);
    let mut auth = authorization();
    auth.per_call_cap_micros = i64::MAX;
    auth.total_limit_micros = i64::MAX;
    store.authorize_root_budget(&admin, &auth).await.unwrap();
    let mut first = reservation("large-paid", BudgetStage::FormalEvaluation, 2);
    first.max_cost_micros = i64::MAX - 1;
    pay_and_close(&store, &worker, &first, i64::MAX - 1).await;
    let mut second = reservation("overflow-paid", BudgetStage::TaskExecution, 10);
    second.max_cost_micros = 1;
    let call = store.reserve_budget_call(&worker, &second).await.unwrap();
    let dispatch = store
        .begin_budget_dispatch(&worker, &fence(&call, 11))
        .await
        .unwrap();
    assert!(matches!(
        store
            .reconcile_budget_call_cost(
                &admin,
                "scope",
                &call.call_id,
                &charge("overflow-paid", i64::MAX),
                12
            )
            .await,
        Err(Error::Conflict(_))
    ));
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert_eq!(
        snapshot.known_actual_micros,
        (i128::from(i64::MAX) * 2 - 1).to_string()
    );
    assert_eq!(snapshot.finalized_actual_micros, (i64::MAX - 1).to_string());
    assert_eq!(snapshot.root.spent_micros, i64::MAX - 1);
    assert_eq!(snapshot.root.reserved_micros, 1);
    assert_eq!(snapshot.held_reservation_micros, "1");
    let uncertain = snapshot
        .calls
        .iter()
        .find(|call| call.call_id == dispatch.call.call_id)
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);
    assert_eq!(uncertain.actual_cost_micros, Some(i64::MAX));
    assert_eq!(uncertain.actual_currency.as_deref(), Some("USD"));
    assert!(!snapshot.monetary_complete);
    assert!(
        snapshot
            .unresolved
            .iter()
            .any(|issue| issue.reason == RootBudgetUnresolvedReason::AuthorizationLimitExceeded)
    );
    let sealed = session
        .seal_root_budget(&admin, "scope", "preserve overflow", 13)
        .await
        .unwrap();
    assert!(!sealed.monetary_complete);
    assert!(!sealed.ordinary_p_eligible);
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn whole_root_pagination_does_not_stop_at_first_thousand_calls_groups_or_events() {
    let (_dir, store, admin, worker, _) = setup().await;
    let mut session = store.session().await.unwrap();
    for index in 0..1_001 {
        let request = reservation(
            &format!("page-{index:04}"),
            BudgetStage::HistoryCollection,
            2,
        );
        let call = session
            .reserve_budget_call_typed(&worker, &request)
            .await
            .unwrap()
            .unwrap();
        session
            .release_undispatched_budget_call(&worker, &fence(&call, 3), "not dispatched")
            .await
            .unwrap();
    }
    let snapshot = session
        .root_budget_monetary_snapshot(&admin, "scope")
        .await
        .unwrap();
    assert_eq!(snapshot.calls.len(), 1_001);
    assert_eq!(snapshot.dispatch_groups.len(), 1_001);
    assert!(snapshot.ledger_events.len() > 3_000);
    assert_eq!(snapshot.calls.last().unwrap().call_id, "page-1000");
    assert_eq!(snapshot.known_actual_micros, "0");
    assert_eq!(snapshot.held_reservation_micros, "0");
    assert!(
        snapshot
            .calls
            .iter()
            .all(|call| call.state == BudgetCallState::Released)
    );
    let sealed = session
        .seal_root_budget(&admin, "scope", "pagination complete", 4)
        .await
        .unwrap();
    assert!(sealed.monetary_complete);
    assert!(!sealed.complete_episode_cost);
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn outbound_dependency_edges_are_typed_sorted_scoped_and_never_truncated() {
    let (_dir, store, _, worker, worker_b) = setup().await;
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&worker, "artifact", "source", "run", "z-run")
        .await
        .unwrap();
    session
        .put_edge(&worker, "artifact", "source", "artifact", "a-artifact")
        .await
        .unwrap();
    session
        .put_edge(&worker, "run", "source", "candidate", "other-kind")
        .await
        .unwrap();
    session
        .put_edge(&worker_b, "artifact", "source", "run", "foreign")
        .await
        .unwrap();
    assert_eq!(
        session
            .dependency_edges(&worker, "artifact", "source")
            .await
            .unwrap(),
        vec![
            ("artifact".into(), "a-artifact".into()),
            ("run".into(), "z-run".into())
        ]
    );
    assert_eq!(
        session
            .dependency_edges(&worker_b, "artifact", "source")
            .await
            .unwrap(),
        vec![("run".into(), "foreign".into())]
    );
    assert!(matches!(
        session
            .dependency_edges(&worker, "bad kind", "source")
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        session
            .dependency_edges(&worker, "artifact", "bad id")
            .await,
        Err(Error::Invalid(_))
    ));
    for index in 0..10_000 {
        session
            .put_edge(
                &worker,
                "artifact",
                "bounded",
                "run",
                &format!("edge-{index:05}"),
            )
            .await
            .unwrap();
    }
    let edges = session
        .dependency_edges(&worker, "artifact", "bounded")
        .await
        .unwrap();
    assert_eq!(edges.len(), 10_000);
    assert_eq!(edges.last().unwrap().1, "edge-09999");
    session
        .put_edge(&worker, "artifact", "bounded", "run", "edge-10000")
        .await
        .unwrap();
    assert!(matches!(
        session
            .dependency_edges(&worker, "artifact", "bounded")
            .await,
        Err(Error::Conflict(_))
    ));
    session.commit().await.unwrap();
    store.close().await;
}

fn content_operation(namespace: &str, kind: &str, id: &str) -> String {
    format!(
        "formal.admin.content.v1.{}",
        fingerprint(&(namespace, kind, id)).unwrap()
    )
}

fn content_entity(namespace: &str, kind: &str, id: &str, body: &Value) -> String {
    fingerprint(&(namespace, kind, id, fingerprint(body).unwrap())).unwrap()
}

#[tokio::test]
async fn private_content_witness_requires_actual_role_and_ignores_public_artifacts() {
    let (_dir, store, admin, worker, _) = setup().await;
    let body = json!({"schema_version":"test.registry.v1","id":"host","authorized_by":"admin"});
    let operation = content_operation("ns-a", "artifact", "host");
    let entity = content_entity("ns-a", "artifact", "host", &body);
    let mut session = store.session().await.unwrap();
    session
        .put(&worker, "artifact", "host", "admin", &body)
        .await
        .unwrap();
    session
        .put(
            &worker,
            "artifact",
            "forged-witness",
            "admin",
            &json!({"role":"admin","operation":operation,"entity":entity,"issuer":"admin"}),
        )
        .await
        .unwrap();
    assert!(
        !session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
            .await
            .unwrap()
    );
    let impersonating_actor = ctx("ns-a", "admin", Role::Worker);
    session
        .audit(&impersonating_actor, &operation, &entity)
        .await
        .unwrap();
    assert!(
        !session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
            .await
            .unwrap()
    );
    assert!(
        session
            .has_role_audit_witness(&worker, Role::Worker, &operation, &entity)
            .await
            .unwrap()
    );
    session.audit(&admin, &operation, &entity).await.unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
            .await
            .unwrap()
    );
    for role in [Role::Host, Role::Evaluator] {
        let issuer = ctx("ns-a", "admin", role);
        let other = hash(format!("actual-{role:?}-body").as_bytes());
        session.audit(&issuer, &operation, &other).await.unwrap();
        assert!(
            session
                .has_role_audit_witness(&worker, role, &operation, &other)
                .await
                .unwrap()
        );
        assert!(
            !session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &other)
                .await
                .unwrap()
        );
        assert!(
            session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
                .await
                .unwrap()
        );
    }
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn private_content_witness_is_part_of_the_same_session_commit_and_rollback() {
    let (_dir, store, admin, worker, _) = setup().await;
    let body = json!({"id":"profile","revision":1});
    let operation = content_operation("ns-a", "artifact", "profile");
    let entity = content_entity("ns-a", "artifact", "profile", &body);
    for commit in [false, true] {
        let mut session = store.session().await.unwrap();
        assert!(
            !session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
                .await
                .unwrap()
        );
        session
            .put(&admin, "artifact", "profile", admin.actor(), &body)
            .await
            .unwrap();
        session.audit(&admin, &operation, &entity).await.unwrap();
        assert!(
            session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
                .await
                .unwrap()
        );
        if commit {
            session.commit().await.unwrap();
        } else {
            drop(session);
        }
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
                .await
                .unwrap(),
            commit
        );
        assert_eq!(
            session
                .get::<Value>(&worker, "artifact", "profile")
                .await
                .unwrap()
                .is_some(),
            commit
        );
        session.commit().await.unwrap();
        assert_eq!(
            store.verify_audit(&admin).await.unwrap(),
            usize::from(commit)
        );
    }
    store.close().await;
}

#[tokio::test]
async fn latest_genuine_selection_rejects_old_pointer_and_complete_body_replay() {
    let (_dir, store, admin, worker, _) = setup().await;
    let first = json!({"schema_version":"test.selection.v1","id":"selected-profile","registration":"profile-a","authorized_by":"admin"});
    let second = json!({"schema_version":"test.selection.v1","id":"selected-profile","registration":"profile-b","authorized_by":"admin"});
    let operation = content_operation("ns-a", "artifact", "selected-profile");
    let first_entity = content_entity("ns-a", "artifact", "selected-profile", &first);
    let second_entity = content_entity("ns-a", "artifact", "selected-profile", &second);
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "selected-profile",
            admin.actor(),
            &first,
        )
        .await
        .unwrap();
    session
        .audit(&admin, &operation, &first_entity)
        .await
        .unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &first_entity)
            .await
            .unwrap()
    );
    session
        .put(
            &admin,
            "artifact",
            "selected-profile",
            admin.actor(),
            &second,
        )
        .await
        .unwrap();
    session
        .audit(&admin, &operation, &second_entity)
        .await
        .unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &second_entity)
            .await
            .unwrap()
    );
    assert!(
        !session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &first_entity)
            .await
            .unwrap()
    );
    session
        .put(&worker, "artifact", "selected-profile", "admin", &first)
        .await
        .unwrap();
    session
        .audit(&worker, &operation, &first_entity)
        .await
        .unwrap();
    let replayed: Value = session
        .need(&worker, "artifact", "selected-profile")
        .await
        .unwrap();
    let replayed_entity = content_entity("ns-a", "artifact", "selected-profile", &replayed);
    assert!(
        !session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &replayed_entity)
            .await
            .unwrap()
    );
    let mut changed = second.clone();
    changed["injected"] = json!({"role":"admin","ordinary_p":true});
    session
        .put(&worker, "artifact", "selected-profile", "admin", &changed)
        .await
        .unwrap();
    assert!(
        !session
            .has_role_audit_witness(
                &worker,
                Role::Admin,
                &operation,
                &content_entity("ns-a", "artifact", "selected-profile", &changed)
            )
            .await
            .unwrap()
    );
    // An unrelated genuine Admin subject also cannot override this selection.
    session
        .audit(
            &admin,
            &content_operation("ns-a", "artifact", "other-profile"),
            &first_entity,
        )
        .await
        .unwrap();
    session
        .put(
            &admin,
            "artifact",
            "selected-profile",
            admin.actor(),
            &second,
        )
        .await
        .unwrap();
    let current: Value = session
        .need(&worker, "artifact", "selected-profile")
        .await
        .unwrap();
    assert!(
        session
            .has_role_audit_witness(
                &worker,
                Role::Admin,
                &operation,
                &content_entity("ns-a", "artifact", "selected-profile", &current)
            )
            .await
            .unwrap()
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn private_content_witness_namespace_is_structural_and_logical_subject_is_bound() {
    let (_dir, store, admin_a, worker_a, worker_b) = setup().await;
    let admin_b = ctx("ns-b", "admin", Role::Admin);
    let body = json!({"id":"host","namespace":"ns-a","authorized_by":"admin"});
    let operation_a = content_operation("ns-a", "artifact", "host");
    let entity_a = content_entity("ns-a", "artifact", "host", &body);
    let operation_b = content_operation("ns-b", "artifact", "host");
    let entity_b = content_entity("ns-b", "artifact", "host", &body);
    let mut session = store.session().await.unwrap();
    // Payloads can claim ns-a, but actual audit namespace comes only from ctx.
    session
        .audit(&admin_b, &operation_a, &entity_a)
        .await
        .unwrap();
    assert!(
        !session
            .has_role_audit_witness(&worker_a, Role::Admin, &operation_a, &entity_a)
            .await
            .unwrap()
    );
    session
        .audit(&admin_b, &operation_b, &entity_b)
        .await
        .unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker_b, Role::Admin, &operation_b, &entity_b)
            .await
            .unwrap()
    );
    session
        .audit(&admin_a, &operation_a, &entity_a)
        .await
        .unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker_a, Role::Admin, &operation_a, &entity_a)
            .await
            .unwrap()
    );
    assert!(
        !session
            .has_role_audit_witness(&worker_b, Role::Admin, &operation_b, &entity_a)
            .await
            .unwrap()
    );
    assert!(
        !session
            .has_role_audit_witness(
                &worker_a,
                Role::Admin,
                &content_operation("ns-a", "candidate", "host"),
                &entity_a
            )
            .await
            .unwrap()
    );
    assert!(
        !session
            .has_role_audit_witness(
                &worker_a,
                Role::Admin,
                &content_operation("ns-a", "artifact", "alias"),
                &entity_a
            )
            .await
            .unwrap()
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn role_witness_checks_history_past_ten_thousand_and_chunked_legacy_payloads() {
    let (_dir, store, admin, worker, _) = setup().await;
    let operation = content_operation("ns-a", "artifact", "profile");
    let first = content_entity("ns-a", "artifact", "profile", &json!({"version":1}));
    let latest = content_entity("ns-a", "artifact", "profile", &json!({"version":2}));
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &operation, &first).await.unwrap();
    for index in 0..10_001 {
        if index == 5_000 {
            session.audit(&admin, &operation, &latest).await.unwrap();
        }
        session
            .audit(
                &worker,
                "legacy.background",
                &format!("business-event-{index}"),
            )
            .await
            .unwrap();
    }
    // Original audit permits free-form strings. Neither UTF-8 chunk boundaries
    // nor a large unrelated escaped payload introduces a new audit limit.
    session
        .audit(
            &worker,
            "legacy operation with spaces",
            &"历史\"\\\n".repeat(4_000),
        )
        .await
        .unwrap();
    session.audit(&worker, &operation, &first).await.unwrap();
    assert!(
        session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &latest)
            .await
            .unwrap()
    );
    assert!(
        !session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &first)
            .await
            .unwrap()
    );
    session.commit().await.unwrap();
    assert_eq!(store.verify_audit(&admin).await.unwrap(), 10_005);
    store.close().await;
}

#[tokio::test]
async fn role_witness_rejects_a_later_unrelated_broken_chain_instead_of_returning_early() {
    let (dir, store, admin, worker, _) = setup().await;
    let operation = content_operation("ns-a", "artifact", "host");
    let entity = content_entity("ns-a", "artifact", "host", &json!({"version":1}));
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &operation, &entity).await.unwrap();
    session
        .audit(&worker, "unrelated.operation", "unrelated-subject")
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    // Deliberate corruption of this test's disposable database only. The
    // helper must validate later unrelated rows before returning any authority.
    let mut connection = SqliteConnection::connect(&format!(
        "sqlite://{}",
        dir.path().join("rsia.sqlite3").display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE audit SET previous_hash=? WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)")
        .bind(hash(b"wrong previous link")).bind("ns-a").bind("ns-a")
        .execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let mut session = store.session().await.unwrap();
    assert!(matches!(
        session
            .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
            .await,
        Err(Error::Conflict(_))
    ));
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn role_witness_rejects_hash_valid_malformed_private_audit_payloads() {
    let operation = content_operation("ns-a", "artifact", "host");
    let entity = content_entity("ns-a", "artifact", "host", &json!({"version":1}));
    let malformed = vec![
        json!(["admin", "admin", operation, entity, 1]).to_string(),
        json!({"actor":"admin","role":"super_admin","operation":operation,"entity":entity,"timestamp":1}).to_string(),
        json!({"actor":"admin","role":"admin","operation":operation,"entity":42,"timestamp":1}).to_string(),
        json!({"actor":"admin","role":"admin","operation":operation,"entity":entity,"timestamp":-1}).to_string(),
        json!({"actor":"bad actor","role":"admin","operation":operation,"entity":entity,"timestamp":1}).to_string(),
        json!({"actor":"admin","role":"admin","operation":operation,"entity":entity,"timestamp":1,"extra":true}).to_string(),
        format!("{{\"actor\":\"admin\",\"role\":\"admin\",\"role\":\"worker\",\"operation\":{},\"entity\":{},\"timestamp\":1}}",
            serde_json::to_string(&operation).unwrap(), serde_json::to_string(&entity).unwrap()),
    ];
    for payload in malformed {
        let (dir, store, admin, worker, _) = setup().await;
        let mut session = store.session().await.unwrap();
        session.audit(&admin, &operation, &entity).await.unwrap();
        session.commit().await.unwrap();
        store.close().await;
        let mut connection = SqliteConnection::connect(&format!(
            "sqlite://{}",
            dir.path().join("rsia.sqlite3").display()
        ))
        .await
        .unwrap();
        // The chain hash is internally consistent, so typed shape/role/actor
        // validation must still reject the malformed row rather than credit it.
        sqlx::query("UPDATE audit SET payload=?,digest=? WHERE namespace=?")
            .bind(&payload)
            .bind(hash(format!("\n{payload}").as_bytes()))
            .bind("ns-a")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
        let mut session = store.session().await.unwrap();
        assert!(matches!(
            session
                .has_role_audit_witness(&worker, Role::Admin, &operation, &entity)
                .await,
            Err(Error::Conflict(_))
        ));
        session.commit().await.unwrap();
        store.close().await;
    }
}
