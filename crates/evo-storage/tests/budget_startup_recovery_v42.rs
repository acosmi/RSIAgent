//! AG-106 (E08 L1299, E04 L1245; plan §11.5 L1134, §6.6 L705/L709, §7.2.1 L825 by
//! analogy; V096(a) L1766 sub-scope): at startup, a finalized budget call whose
//! execution close was lost is closed when it carries its persisted response, and
//! is listed and left open when it carries none.
//!
//! A model call is settled (its charge booked, its response persisted) and its
//! execution closed in two commits, so a crash between them leaves the call
//! finalized with its execution open. Only a replay of the same request closed such
//! a call. One that is never replayed holds its billing root: every later call of
//! the root is refused at the dispatch gate (`RootConcurrencyLimit`), however long
//! ago its lease ended, and the monetary snapshot reports it as `ExecutionOpen`
//! (B0, which uses baseline APIs only).
//!
//! `Store::recover_settled_open_executions`, which `rsia serve` runs at startup as
//! its service Admin, reads the finalized calls whose execution is open, page by
//! page, in the billing scopes the Admin is a member of:
//!
//! * a call with a persisted response (usable, blocked or redacted) is closed
//!   through `close_budget_call_execution` with
//!   `recovered_persisted_response_at_startup`: its three execution columns and one
//!   `execution_closed` event, no amount, no state, no response, and its root
//!   dispatches again (T1, T1b);
//! * a call without a response is listed (`open_without_artifact`) and left open:
//!   its row and its events do not change (T2);
//! * dispatched calls, uncertain calls (one with a persisted response included) and
//!   closed calls are not touched, and a second run writes nothing (T3);
//! * more than one page of calls is recovered whole (T4);
//! * a root the Admin is not a member of is neither read nor listed, and only an
//!   Admin runs the recovery (T5).
//!
//! Real SQLite store and real budget APIs; no model, no provider, and the only
//! money is the fixture charge of a settlement.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallKey, BudgetCallRecord, BudgetCallReservation,
    BudgetCallState, BudgetExecutionProvenance, BudgetPreDispatchRefusal, BudgetStage,
    ModelCallSettlementEvidence, RootBudgetAuthorization, RootBudgetRecord,
    RootBudgetUnresolvedReason, SettledOpenRecovery, UsageCharge,
};
use serde_json::{Value, json};
use sqlx::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The reason the recovery closes an execution with.
const RECOVERED: &str = "recovered_persisted_response_at_startup";
/// The namespace `rsia serve` runs in, and so the namespace of its service Admin.
const NS_A: &str = "ns-a";
/// The other namespace of the same roots.
const NS_B: &str = "ns-b";
/// The namespace of roots the service Admin is not a member of.
const NS_C: &str = "ns-c";
/// The lease of a call. It ends before the next call of a test is reserved, so the
/// instance-wide count of active leases never fills up.
const LEASE: i64 = 5;
const MODEL_REQUEST_SCHEMA: &str = "rsia.model_request.artifact.v1";
const TRANSPORT_SCHEMA: &str = "rsia.model_transport.artifact.v1";
const RESPONSE_SCHEMA: &str = "rsia.model_response.artifact.v1";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// The service Admin of `rsia serve --namespace ns-a`.
fn admin() -> Context {
    Context::new(NS_A, "service-admin", Role::Admin).unwrap()
}

fn admin_of(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

/// The broker of `namespace`.
fn host(namespace: &str) -> Context {
    Context::new(namespace, "broker", Role::Host).unwrap()
}

async fn database() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    (dir, path, store)
}

/// Authorizes the root of `scope` for `namespaces`, by an Admin of the first one.
async fn authorize(store: &Store, scope: &str, namespaces: &[&str]) {
    store
        .authorize_root_budget(
            &admin_of(namespaces[0]),
            &RootBudgetAuthorization {
                root_budget_id: format!("root-{scope}"),
                billing_scope: scope.into(),
                allowed_namespaces: namespaces
                    .iter()
                    .map(|namespace| (*namespace).to_string())
                    .collect(),
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

/// A model request that names no source: the dispatch reads its closure (the call
/// has no budget call reference) and finds no revoked run in it.
fn model_request(call_id: &str) -> BudgetArtifact {
    BudgetArtifact::from_serializable(
        MODEL_REQUEST_SCHEMA,
        &json!({
            "request_id": call_id,
            "source_closure": [],
            "input": format!("input of {call_id}"),
        }),
    )
    .unwrap()
}

fn reservation(scope: &str, call_id: &str, now: i64) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: scope.into(),
        call_id: call_id.into(),
        dispatch_group_id: format!("group-{call_id}"),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(model_request(call_id)),
        max_cost_micros: 20,
        lease_token: format!("lease-{call_id}"),
        lease_until: now + LEASE,
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

/// What the provider billed for `call_id`.
fn charge(call_id: &str) -> UsageCharge {
    UsageCharge {
        amount_micros: 9,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{call_id}"),
        usage_record_id: format!("usage-{call_id}"),
        output_digest: hash(format!("output-{call_id}").as_bytes()),
    }
}

/// A response in the shapes the persistent broker stores: a transport body and a
/// usable response that carry the output, and a blocked response that does not.
fn evidence(call_id: &str) -> ModelCallSettlementEvidence {
    ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            TRANSPORT_SCHEMA,
            &json!({
                "response_id": format!("response-{call_id}"),
                "output": format!("output of {call_id}"),
            }),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                RESPONSE_SCHEMA,
                &json!({
                    "status": "completed",
                    "request_id": call_id,
                    "output": format!("output of {call_id}"),
                }),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            RESPONSE_SCHEMA,
            &json!({"status": "cancelled_after_dispatch", "request_id": call_id}),
        )
        .unwrap(),
        forced_block_reason: None,
    }
}

async fn reserve(
    store: &Store,
    namespace: &str,
    scope: &str,
    call_id: &str,
    now: i64,
) -> BudgetCallRecord {
    store
        .reserve_budget_call(&host(namespace), &reservation(scope, call_id, now))
        .await
        .unwrap()
}

/// Begins `call` at `now`: `Ok(true)` when it is dispatched, or the refusal of the
/// dispatch gate.
async fn begin(
    store: &Store,
    call: &BudgetCallRecord,
    now: i64,
) -> Result<bool, BudgetPreDispatchRefusal> {
    store
        .begin_budget_dispatch_typed(&host(&call.namespace), &fence(call, now))
        .await
        .unwrap()
        .map(|decision| decision.new_dispatch)
}

/// Reserves `call_id` at `now` and dispatches it.
async fn dispatched(
    store: &Store,
    namespace: &str,
    scope: &str,
    call_id: &str,
    now: i64,
) -> BudgetCallRecord {
    let call = reserve(store, namespace, scope, call_id, now).await;
    let decision = store
        .begin_budget_dispatch(&host(namespace), &fence(&call, now))
        .await
        .unwrap();
    assert!(decision.new_dispatch, "{call_id} was not dispatched");
    decision.call
}

/// What a crash between the two commits of a model call leaves: the settlement
/// (the charge booked, the usable response persisted) is committed and the close is
/// not.
async fn settled_open(
    store: &Store,
    namespace: &str,
    scope: &str,
    call_id: &str,
    now: i64,
) -> BudgetCallRecord {
    let call = dispatched(store, namespace, scope, call_id, now).await;
    let settlement = store
        .settle_model_budget_call(
            &host(namespace),
            &fence(&call, now),
            &charge(call_id),
            &evidence(call_id),
        )
        .await
        .unwrap();
    assert!(settlement.response_usable, "{call_id}");
    let call = settlement.call;
    assert_eq!(call.state, BudgetCallState::Finalized, "{call_id}");
    assert!(call.response_artifact.is_some(), "{call_id}");
    assert!(!call.execution_closed, "{call_id}");
    call
}

/// The call as `ctx` reads it.
async fn call_as(store: &Store, ctx: &Context, scope: &str, call_id: &str) -> BudgetCallRecord {
    store
        .budget_call(ctx, scope, call_id)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{call_id} has no budget row"))
}

/// The call as the service Admin reads it.
async fn call_of(store: &Store, scope: &str, call_id: &str) -> BudgetCallRecord {
    call_as(store, &admin(), scope, call_id).await
}

/// The calls of `scope` its monetary snapshot reports as `ExecutionOpen`.
async fn execution_open(store: &Store, scope: &str) -> Vec<String> {
    let mut session = store.session().await.unwrap();
    let snapshot = session
        .root_budget_monetary_snapshot(&admin(), scope)
        .await
        .unwrap();
    session.commit().await.unwrap();
    snapshot
        .unresolved
        .into_iter()
        .filter(|issue| issue.reason == RootBudgetUnresolvedReason::ExecutionOpen)
        .map(|issue| issue.call_id.unwrap())
        .collect()
}

// ---------------------------------------------------------------------------
// B0: the held root (baseline APIs only)
// ---------------------------------------------------------------------------

/// B0. A call settled with its close lost, and never replayed, holds its root: a
/// later call of the root, of either namespace and after the lease of the held call
/// ended, is refused (`RootConcurrencyLimit`; `Conflict("root_budget_concurrency_limit")`
/// on the historical API), the snapshot reports the held call as `ExecutionOpen`,
/// and nothing closes it.
#[tokio::test]
async fn b0_a_settled_call_left_open_holds_its_root_until_it_is_replayed() {
    let (_dir, _path, store) = database().await;
    authorize(&store, "scope-a", &[NS_A, NS_B]).await;
    let held = settled_open(&store, NS_A, "scope-a", "call-held", 10).await;
    assert_eq!(execution_open(&store, "scope-a").await, ["call-held"]);
    for (namespace, call_id, now) in [(NS_A, "next-a", 20), (NS_B, "next-b", 30)] {
        assert!(
            now > held.lease_until,
            "the lease of the held call has ended"
        );
        let next = reserve(&store, namespace, "scope-a", call_id, now).await;
        assert_eq!(
            begin(&store, &next, now).await,
            Err(BudgetPreDispatchRefusal::RootConcurrencyLimit),
            "{call_id}"
        );
        let historical = store
            .begin_budget_dispatch(&host(namespace), &fence(&next, now))
            .await;
        assert!(
            matches!(&historical, Err(Error::Conflict(code)) if code == "root_budget_concurrency_limit"),
            "{call_id}: {historical:?}"
        );
    }
    assert_eq!(call_of(&store, "scope-a", "call-held").await, held);
    assert_eq!(execution_open(&store, "scope-a").await, ["call-held"]);
}

// ---------------------------------------------------------------------------
// T1-T5: the startup recovery
// ---------------------------------------------------------------------------

/// Every row of every table, as text.
type Rows = BTreeMap<String, Vec<String>>;

fn key(billing_scope: &str, call_id: &str) -> BudgetCallKey {
    BudgetCallKey {
        billing_scope: billing_scope.into(),
        call_id: call_id.into(),
    }
}

fn key_of(call: &BudgetCallRecord) -> BudgetCallKey {
    key(&call.billing_scope, &call.call_id)
}

/// How a finalized call that carries no response had its charge booked.
#[derive(Clone, Copy)]
enum Booked {
    /// `finalize_budget_call`, by its broker.
    Finalize,
    /// `reconcile_budget_call_cost`, by an Admin, once its usage was uncertain.
    Reconcile,
}

/// A finalized call whose execution is open and that carries no response.
async fn finalized_without_response(
    store: &Store,
    namespace: &str,
    scope: &str,
    call_id: &str,
    now: i64,
    booked: Booked,
) -> BudgetCallRecord {
    let call = dispatched(store, namespace, scope, call_id, now).await;
    let call = match booked {
        Booked::Finalize => store
            .finalize_budget_call(&host(namespace), &fence(&call, now), &charge(call_id))
            .await
            .unwrap(),
        Booked::Reconcile => {
            store
                .mark_budget_call_uncertain(
                    &host(namespace),
                    &fence(&call, now),
                    "provider receipt pending",
                )
                .await
                .unwrap();
            store
                .reconcile_budget_call_cost(
                    &admin_of(namespace),
                    scope,
                    call_id,
                    &charge(call_id),
                    now,
                )
                .await
                .unwrap()
        }
    };
    assert_eq!(call.state, BudgetCallState::Finalized, "{call_id}");
    assert!(call.response_artifact.is_none(), "{call_id}");
    assert!(!call.execution_closed, "{call_id}");
    call
}

async fn root_of(store: &Store, scope: &str) -> RootBudgetRecord {
    store.root_budget(&admin(), scope).await.unwrap().unwrap()
}

async fn read_only(path: &Path) -> sqlx::SqliteConnection {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

/// Every row of every table (the FTS shadow tables and the migration ledger
/// included), as text, through a second read-only connection.
async fn all_rows(path: &Path) -> Rows {
    let mut connection = read_only(path).await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type='table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let mut out = BTreeMap::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
        ))
        .fetch_all(&mut connection)
        .await
        .unwrap();
        let row = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let rows: Vec<String> =
            sqlx::query_scalar(&format!("SELECT {row} FROM \"{table}\" ORDER BY 1"))
                .fetch_all(&mut connection)
                .await
                .unwrap();
        out.insert(table, rows);
    }
    connection.close().await.unwrap();
    out
}

fn assert_nothing_written(before: &Rows, after: &Rows, what: &str) {
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(changed.is_empty(), "{what}: tables written: {changed:?}");
}

/// The `(event_at, details)` of the events of one kind of `call_id`, in write order.
async fn events_of(path: &Path, call_id: &str, kind: &str) -> Vec<(i64, Value)> {
    let mut connection = read_only(path).await;
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT event_at,details FROM root_budget_events
         WHERE call_id=? AND event_kind=? ORDER BY seq",
    )
    .bind(call_id)
    .bind(kind)
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    rows.into_iter()
        .map(|(at, details)| (at, serde_json::from_str(&details).unwrap()))
        .collect()
}

/// `after` is `before` with its execution closed by the recovery at `now` and
/// nothing else changed, and the call has one `execution_closed` event, the
/// recovery's.
async fn assert_closed_by_recovery(
    path: &Path,
    before: &BudgetCallRecord,
    after: &BudgetCallRecord,
    now: i64,
) {
    let mut expected = before.clone();
    expected.execution_closed = true;
    expected.execution_closed_at = Some(now);
    expected.execution_close_reason = Some(RECOVERED.into());
    assert_eq!(
        after, &expected,
        "{}: the recovery changed more than the execution close",
        before.call_id
    );
    assert_eq!(
        events_of(path, &before.call_id, "execution_closed").await,
        [(
            now,
            json!({"dispatch_id": before.dispatch_id, "reason": RECOVERED})
        )],
        "{}",
        before.call_id
    );
}

/// From `before` to `after`, a recovery that closed `closed` wrote their call rows
/// and one event row for each of them, and nothing else: every other table and
/// every other call row is as it was, and no event row changed or disappeared. What
/// changed in the row of a closed call is checked by `assert_closed_by_recovery`.
fn assert_only_closes_written(before: &Rows, after: &Rows, closed: &[&BudgetCallRecord]) {
    for table in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        if table != "root_budget_calls" && table != "root_budget_events" {
            assert_eq!(
                before.get(table),
                after.get(table),
                "the recovery wrote {table}"
            );
        }
    }
    let (calls_before, calls_after) = (&before["root_budget_calls"], &after["root_budget_calls"]);
    assert_eq!(
        calls_before.len(),
        calls_after.len(),
        "a call row was added"
    );
    let changed: Vec<&String> = calls_before
        .iter()
        .filter(|row| !calls_after.contains(row))
        .collect();
    assert_eq!(
        changed.len(),
        closed.len(),
        "call rows changed: {changed:?}"
    );
    for call in closed {
        let prefix = format!("'{}'|'{}'|", call.billing_scope, call.call_id);
        assert!(
            changed.iter().any(|row| row.starts_with(&prefix)),
            "{} was not written",
            call.call_id
        );
    }
    let (events_before, events_after) =
        (&before["root_budget_events"], &after["root_budget_events"]);
    assert!(
        events_before.iter().all(|row| events_after.contains(row)),
        "an event row changed or disappeared"
    );
    assert_eq!(
        events_after.len(),
        events_before.len() + closed.len(),
        "one event row per close"
    );
}

/// T1. A call of each namespace was settled with its close lost, and each holds its
/// root. The recovery closes both: the three execution columns and one
/// `execution_closed` event each, no amount, state or response changed; the
/// snapshots report no open execution, and the calls each root refused are
/// dispatched.
#[tokio::test]
async fn t1_settled_open_calls_of_two_namespaces_are_closed_and_their_roots_dispatch() {
    let (_dir, path, store) = database().await;
    authorize(&store, "scope-a", &[NS_A, NS_B]).await;
    authorize(&store, "scope-b", &[NS_A, NS_B]).await;
    let call_a = settled_open(&store, NS_A, "scope-a", "call-a", 10).await;
    let call_b = settled_open(&store, NS_B, "scope-b", "call-b", 20).await;
    let next_a = reserve(&store, NS_A, "scope-a", "next-a", 30).await;
    let next_b = reserve(&store, NS_B, "scope-b", "next-b", 30).await;
    for next in [&next_a, &next_b] {
        assert_eq!(
            begin(&store, next, 30).await,
            Err(BudgetPreDispatchRefusal::RootConcurrencyLimit),
            "{}",
            next.call_id
        );
    }
    assert_eq!(execution_open(&store, "scope-a").await, ["call-a"]);
    assert_eq!(execution_open(&store, "scope-b").await, ["call-b"]);
    let roots = (
        root_of(&store, "scope-a").await,
        root_of(&store, "scope-b").await,
    );
    let before = all_rows(&path).await;

    let recovery = store
        .recover_settled_open_executions(&admin(), 31)
        .await
        .unwrap();

    let after = all_rows(&path).await;
    assert_eq!(
        recovery,
        SettledOpenRecovery {
            closed: vec![key_of(&call_a), key_of(&call_b)],
            open_without_artifact: Vec::new(),
        }
    );
    assert_only_closes_written(&before, &after, &[&call_a, &call_b]);
    for call in [&call_a, &call_b] {
        let closed = call_of(&store, &call.billing_scope, &call.call_id).await;
        assert_closed_by_recovery(&path, call, &closed, 31).await;
    }
    assert_eq!(
        (
            root_of(&store, "scope-a").await,
            root_of(&store, "scope-b").await
        ),
        roots,
        "the amounts of a root changed"
    );
    assert!(execution_open(&store, "scope-a").await.is_empty());
    assert!(execution_open(&store, "scope-b").await.is_empty());
    assert_eq!(begin(&store, &next_a, 32).await, Ok(true));
    assert_eq!(begin(&store, &next_b, 33).await, Ok(true));
}

/// T1b. A persisted response shows that the execution ended whatever it holds: a
/// response blocked at the settlement (the lease had ended) and a response redacted
/// afterwards (as a revocation redacts it) are closed like a usable one.
#[tokio::test]
async fn t1b_a_blocked_or_redacted_response_is_closed_like_a_usable_one() {
    let (_dir, path, store) = database().await;
    authorize(&store, "scope-k", &[NS_A, NS_B]).await;
    authorize(&store, "scope-r", &[NS_A, NS_B]).await;
    let call = dispatched(&store, NS_A, "scope-k", "call-k", 10).await;
    let settlement = store
        .settle_model_budget_call(
            &host(NS_A),
            &fence(&call, 10 + LEASE + 1),
            &charge("call-k"),
            &evidence("call-k"),
        )
        .await
        .unwrap();
    assert!(!settlement.response_usable);
    assert_eq!(
        settlement.response_block_reason.as_deref(),
        Some("lease_expired")
    );
    let blocked = settlement.call;
    assert_eq!(blocked.state, BudgetCallState::Finalized);
    assert!(!blocked.execution_closed);
    settled_open(&store, NS_B, "scope-r", "call-r", 30).await;
    let mut session = store.session().await.unwrap();
    let redacted = session
        .redact_budget_call_content(&host(NS_B), "scope-r", "call-r", "source revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        redacted
            .response_artifact
            .as_ref()
            .map(|artifact| artifact.schema_version.as_str()),
        Some(REDACTED_SCHEMA)
    );
    assert!(!redacted.execution_closed);
    let before = all_rows(&path).await;

    let recovery = store
        .recover_settled_open_executions(&admin(), 40)
        .await
        .unwrap();

    assert_eq!(
        recovery,
        SettledOpenRecovery {
            closed: vec![key_of(&blocked), key_of(&redacted)],
            open_without_artifact: Vec::new(),
        }
    );
    assert_only_closes_written(&before, &all_rows(&path).await, &[&blocked, &redacted]);
    for call in [&blocked, &redacted] {
        let closed = call_of(&store, &call.billing_scope, &call.call_id).await;
        assert_closed_by_recovery(&path, call, &closed, 40).await;
    }
}

/// T2. A finalized call whose execution is open and that carries no response, its
/// charge booked by `finalize_budget_call` or reconciled by an Admin, shows nothing
/// of its execution's end: it is listed, not closed, and its row and its events do
/// not change. It still holds its root, for an Admin to close or reconcile.
#[tokio::test]
async fn t2_finalized_open_calls_without_a_response_are_listed_and_left_open() {
    let (_dir, path, store) = database().await;
    authorize(&store, "scope-f", &[NS_A, NS_B]).await;
    authorize(&store, "scope-r", &[NS_A, NS_B]).await;
    let finalized =
        finalized_without_response(&store, NS_B, "scope-f", "call-f", 10, Booked::Finalize).await;
    let reconciled =
        finalized_without_response(&store, NS_A, "scope-r", "call-r", 20, Booked::Reconcile).await;
    let before = all_rows(&path).await;

    let recovery = store
        .recover_settled_open_executions(&admin(), 30)
        .await
        .unwrap();

    assert_eq!(
        recovery,
        SettledOpenRecovery {
            closed: Vec::new(),
            open_without_artifact: vec![key_of(&finalized), key_of(&reconciled)],
        }
    );
    assert_nothing_written(&before, &all_rows(&path).await, "listing calls");
    assert_eq!(call_of(&store, "scope-f", "call-f").await, finalized);
    assert_eq!(call_of(&store, "scope-r", "call-r").await, reconciled);
    assert_eq!(execution_open(&store, "scope-f").await, ["call-f"]);
    assert_eq!(execution_open(&store, "scope-r").await, ["call-r"]);
    let next = reserve(&store, NS_A, "scope-r", "next-r", 40).await;
    assert_eq!(
        begin(&store, &next, 40).await,
        Err(BudgetPreDispatchRefusal::RootConcurrencyLimit)
    );
}

/// T3. Only finalized calls whose execution is open are read. A dispatched call, an
/// uncertain call without a response, an uncertain call with a persisted response
/// (its charge came in a pricing identity the root does not hold) and a call its
/// broker closed are untouched. A second run closes nothing and writes nothing.
#[tokio::test]
async fn t3_dispatched_uncertain_and_closed_calls_are_untouched_and_a_second_run_writes_nothing() {
    let (_dir, path, store) = database().await;
    for scope in [
        "scope-c", "scope-d", "scope-f", "scope-p", "scope-s", "scope-u",
    ] {
        authorize(&store, scope, &[NS_A, NS_B]).await;
    }
    let settled = settled_open(&store, NS_A, "scope-c", "call-c", 10).await;
    let closed = store
        .close_budget_call_execution(
            &host(NS_A),
            "scope-c",
            "call-c",
            settled.dispatch_id.as_deref().unwrap(),
            "transport_completion_persisted",
            10,
        )
        .await
        .unwrap();
    assert!(closed.execution_closed);
    let in_flight = dispatched(&store, NS_B, "scope-d", "call-d", 20).await;
    let uncertain = dispatched(&store, NS_A, "scope-u", "call-u", 30).await;
    let uncertain = store
        .mark_budget_call_uncertain(
            &host(NS_A),
            &fence(&uncertain, 30),
            "provider receipt pending",
        )
        .await
        .unwrap();
    let mispriced = dispatched(&store, NS_B, "scope-p", "call-p", 40).await;
    let mispriced = store
        .settle_model_budget_call(
            &host(NS_B),
            &fence(&mispriced, 40),
            &UsageCharge {
                currency: "EUR".into(),
                ..charge("call-p")
            },
            &evidence("call-p"),
        )
        .await
        .unwrap()
        .call;
    assert_eq!(mispriced.state, BudgetCallState::Uncertain);
    assert!(mispriced.response_artifact.is_some());
    assert!(!mispriced.execution_closed);
    let unanswered =
        finalized_without_response(&store, NS_A, "scope-f", "call-f", 50, Booked::Finalize).await;
    let held = settled_open(&store, NS_B, "scope-s", "call-s", 60).await;
    let before = all_rows(&path).await;

    let first = store
        .recover_settled_open_executions(&admin(), 70)
        .await
        .unwrap();

    assert_eq!(
        first,
        SettledOpenRecovery {
            closed: vec![key_of(&held)],
            open_without_artifact: vec![key_of(&unanswered)],
        }
    );
    assert_only_closes_written(&before, &all_rows(&path).await, &[&held]);
    for call in [&closed, &in_flight, &uncertain, &mispriced, &unanswered] {
        assert_eq!(
            &call_of(&store, &call.billing_scope, &call.call_id).await,
            call,
            "{} was touched",
            call.call_id
        );
    }
    let before = all_rows(&path).await;
    let second = store
        .recover_settled_open_executions(&admin(), 80)
        .await
        .unwrap();
    assert_eq!(
        second,
        SettledOpenRecovery {
            closed: Vec::new(),
            open_without_artifact: vec![key_of(&unanswered)],
        }
    );
    assert_nothing_written(&before, &all_rows(&path).await, "the second run");
    assert_eq!(
        events_of(&path, "call-s", "execution_closed").await.len(),
        1
    );
}

/// T4. More calls than a page holds (130 against 128), in 130 roots of both
/// namespaces, with and without a response in turn in key order. The closes of the
/// first page take its calls out of what the next page reads, and the run still
/// reaches every call. A second run lists the same calls and writes nothing.
#[tokio::test]
async fn t4_more_than_one_page_of_calls_is_recovered_whole() {
    const CALLS: i64 = 130;
    let (_dir, path, store) = database().await;
    let (mut answered, mut unanswered) = (Vec::new(), Vec::new());
    for index in 0..CALLS {
        let (scope, call_id) = (format!("scope-{index:03}"), format!("call-{index:03}"));
        let namespace = if index % 3 == 0 { NS_B } else { NS_A };
        let now = 10 + 10 * index;
        authorize(&store, &scope, &[NS_A, NS_B]).await;
        if index % 2 == 0 {
            answered.push(settled_open(&store, namespace, &scope, &call_id, now).await);
        } else {
            unanswered.push(
                finalized_without_response(
                    &store,
                    namespace,
                    &scope,
                    &call_id,
                    now,
                    Booked::Finalize,
                )
                .await,
            );
        }
    }
    let now = 10 + 10 * CALLS;
    let before = all_rows(&path).await;

    let first = store
        .recover_settled_open_executions(&admin(), now)
        .await
        .unwrap();

    assert_eq!(
        first.closed,
        answered.iter().map(key_of).collect::<Vec<_>>()
    );
    assert_eq!(
        first.open_without_artifact,
        unanswered.iter().map(key_of).collect::<Vec<_>>()
    );
    assert_only_closes_written(
        &before,
        &all_rows(&path).await,
        &answered.iter().collect::<Vec<_>>(),
    );
    for call in &answered {
        let closed = call_of(&store, &call.billing_scope, &call.call_id).await;
        assert_closed_by_recovery(&path, call, &closed, now).await;
    }
    for call in &unanswered {
        assert_eq!(
            &call_of(&store, &call.billing_scope, &call.call_id).await,
            call
        );
    }
    let before = all_rows(&path).await;
    let second = store
        .recover_settled_open_executions(&admin(), now + 10)
        .await
        .unwrap();
    assert_eq!(
        second,
        SettledOpenRecovery {
            closed: Vec::new(),
            open_without_artifact: first.open_without_artifact.clone(),
        }
    );
    assert_nothing_written(&before, &all_rows(&path).await, "the second run");
}

/// T5. Only an Admin runs the recovery, and it reads the roots its namespace is a
/// member of: the service Admin of `ns-a` recovers its root and neither closes nor
/// lists the calls of the roots of `ns-c` alone, which an Admin of `ns-c` recovers.
#[tokio::test]
async fn t5_only_an_admin_recovers_and_only_the_roots_it_is_a_member_of() {
    let (_dir, path, store) = database().await;
    authorize(&store, "scope-x", &[NS_C]).await;
    authorize(&store, "scope-y", &[NS_C]).await;
    authorize(&store, "scope-a", &[NS_A, NS_B]).await;
    let outside = settled_open(&store, NS_C, "scope-x", "call-x", 10).await;
    let outside_unanswered =
        finalized_without_response(&store, NS_C, "scope-y", "call-y", 20, Booked::Finalize).await;
    let inside = settled_open(&store, NS_A, "scope-a", "call-a", 30).await;
    let before = all_rows(&path).await;
    for role in [Role::Host, Role::Evaluator, Role::Worker, Role::Agent] {
        let caller = Context::new(NS_A, "caller", role).unwrap();
        let refused = store.recover_settled_open_executions(&caller, 40).await;
        assert!(
            matches!(refused, Err(Error::Forbidden)),
            "{role:?}: {refused:?}"
        );
    }
    let invalid = store.recover_settled_open_executions(&admin(), -1).await;
    assert!(matches!(invalid, Err(Error::Invalid(_))), "{invalid:?}");
    assert_nothing_written(&before, &all_rows(&path).await, "a refused recovery");

    let recovery = store
        .recover_settled_open_executions(&admin(), 40)
        .await
        .unwrap();
    assert_eq!(
        recovery,
        SettledOpenRecovery {
            closed: vec![key_of(&inside)],
            open_without_artifact: Vec::new(),
        }
    );
    let outsider = admin_of(NS_C);
    assert_eq!(
        call_as(&store, &outsider, "scope-x", "call-x").await,
        outside
    );
    assert_eq!(
        call_as(&store, &outsider, "scope-y", "call-y").await,
        outside_unanswered
    );
    assert!(
        events_of(&path, "call-x", "execution_closed")
            .await
            .is_empty()
    );

    let recovery = store
        .recover_settled_open_executions(&outsider, 50)
        .await
        .unwrap();
    assert_eq!(
        recovery,
        SettledOpenRecovery {
            closed: vec![key_of(&outside)],
            open_without_artifact: vec![key_of(&outside_unanswered)],
        }
    );
}
