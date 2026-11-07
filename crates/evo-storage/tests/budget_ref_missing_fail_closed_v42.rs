//! AG-107 (E03 L1229, E08 L1299; plan §11 L1077, §11.3 L1110, §11.5 L1132,
//! §7.2.1 L831; V096(a) L1766 sub-scope): a legacy budget call whose budget call
//! reference is missing is not checked against an empty source closure.
//!
//! The dispatch gate and the model settlement of a legacy call read the sources of
//! the call from its budget call reference (`budget-ref-<digest>`, written when the
//! call is reserved with a source list). A reference that was lost or deleted used
//! to read as an empty closure, which no revocation matches: a call over a revoked
//! run was dispatched, and its late output was stored as usable. Plan §7.2.1 has a
//! revocation recheck a queued action before dispatch; §11 says a late output forms
//! no usable candidate while its real cost is still booked; §11.5 says the late
//! result of a dispatched call cannot regain validity while its cost is reconciled.
//!
//! With the reference missing, the closure is read from the call's persisted
//! request by the reader the revocation cleanup uses (`budget_request_source_ids`):
//!
//! * a closure that names a revoked run refuses the dispatch (`SourceRevoked`, the
//!   historical `Cancelled`; the reservation is kept) and blocks the late response
//!   (`source_revoked`; the charge is booked and the transport redacted);
//! * a request that cannot be read for a closure (a schema the reader does not
//!   read, a closure that is not a list of ids, a body that is not strict JSON, a
//!   redaction) fails closed: the dispatch is refused with
//!   `Conflict("budget_sources_unverifiable")` and writes nothing, and the late
//!   response is booked and blocked under `budget_sources_unverifiable`, its
//!   transport redacted;
//! * a request that names live runs only, a request that names no source and a
//!   call that carries no request are dispatched as before (guard cases).
//!
//! Real SQLite store and real budget and lifecycle APIs; no model, no provider, and
//! the only money is the fixture charge of a settlement.

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetPreDispatchRefusal, BudgetStage, ModelCallSettlementEvidence,
    RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use sqlx::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SCOPE: &str = "scope-1";
const GROUP: &str = "group-1";
/// The run the tests revoke.
const SOURCE: &str = "source-a";
/// A run that is never revoked.
const LIVE: &str = "run-live";
const REASON: &str = "source revoked";
const MODEL_REQUEST_SCHEMA: &str = "rsia.model_request.artifact.v1";
/// The schema the broker stores a formal task request under; the closure reader
/// does not read it.
const FORMAL_TASK_REQUEST_SCHEMA: &str = "rsia.formal_task_request.v1";
const TRANSPORT_SCHEMA: &str = "rsia.model_transport.artifact.v1";
const RESPONSE_SCHEMA: &str = "rsia.model_response.artifact.v1";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
const UNVERIFIABLE: &str = "budget_sources_unverifiable";
/// The model output of the late response. It appears nowhere else.
const OUTPUT: &str = "AG107-LATE-MODEL-OUTPUT-5d2e81c4";

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}

fn host() -> Context {
    Context::new("n", "broker", Role::Host).unwrap()
}

async fn database() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    (dir, path, store)
}

async fn raw_put(store: &Store, kind: &str, id: &str, body: &Value) {
    let admin = admin();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, kind, id, admin.actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// A trusted run row, as `put_source` in `tests/lifecycle.rs` writes it.
async fn put_run(store: &Store, id: &str) {
    raw_put(
        store,
        "run",
        id,
        &json!({"id": id, "schema_version": "rsia.optimization.source.v1", "body": "run-body"}),
    )
    .await;
}

/// `begin_revoke` only: the tombstone and the watermark are committed, no cleanup
/// step has run.
async fn revoke(store: &Store, kind: &str, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        REASON,
        5,
    )
    .await
    .unwrap()
}

/// Steps the cleanup of `job_id` until it is `Complete`.
async fn complete_cleanup(store: &Store, job_id: &str) {
    for now in 6..2_000 {
        let status = LifecycleStore::cleanup_step(&admin(), store, job_id, 8, now)
            .await
            .unwrap();
        match status.state {
            CleanupState::Complete => return,
            CleanupState::Failed => panic!("the cleanup failed: {status:?}"),
            _ => {}
        }
    }
    panic!("the cleanup did not finish");
}

async fn authorize(store: &Store) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec!["n".into()],
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

/// A model request whose source closure is `sources`.
fn model_request(call_id: &str, sources: &[&str]) -> BudgetArtifact {
    let closure: Vec<Value> = sources
        .iter()
        .map(|id| json!({"id": id, "digest": hash(id.as_bytes())}))
        .collect();
    BudgetArtifact::from_serializable(
        MODEL_REQUEST_SCHEMA,
        &json!({
            "request_id": call_id,
            "source_closure": closure,
            "input": [{"content": format!("BUDGET-SECRET-{call_id}")}],
        }),
    )
    .unwrap()
}

/// An artifact of exactly `body` (no reserialization).
fn raw_artifact(schema: &str, body: String) -> BudgetArtifact {
    BudgetArtifact {
        schema_version: schema.into(),
        digest: hash(body.as_bytes()),
        body,
    }
}

/// Requests the budget API accepts but whose source closure the closure reader
/// cannot read. Each one names `SOURCE` somewhere.
fn unreadable_requests(call_id: &str) -> Vec<(&'static str, BudgetArtifact)> {
    let digest = hash(SOURCE.as_bytes());
    vec![
        (
            "a schema the reader does not read (the formal task request)",
            BudgetArtifact::from_serializable(
                FORMAL_TASK_REQUEST_SCHEMA,
                &json!({
                    "task": {"request_digest": hash(call_id.as_bytes()), "ticket_id": "ticket-1"},
                    "sources": [{"kind": "run", "id": SOURCE}],
                    "input": "BUDGET-SECRET",
                }),
            )
            .unwrap(),
        ),
        (
            "a model request whose closure is not a list",
            BudgetArtifact::from_serializable(
                MODEL_REQUEST_SCHEMA,
                &json!({
                    "request_id": call_id,
                    "source_closure": SOURCE,
                    "input": "BUDGET-SECRET",
                }),
            )
            .unwrap(),
        ),
        (
            // `serde_json::Value` keeps the last key (an empty closure), so the
            // reservation accepts the body; the strict reader refuses it.
            "a model request whose closure key is repeated",
            raw_artifact(
                MODEL_REQUEST_SCHEMA,
                format!(
                    r#"{{"request_id":"{call_id}","source_closure":[{{"id":"{SOURCE}","digest":"{digest}"}}],"source_closure":[],"input":"BUDGET-SECRET"}}"#
                ),
            ),
        ),
    ]
}

fn reservation(call_id: &str, request: Option<BudgetArtifact>) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: call_id.into(),
        dispatch_group_id: GROUP.into(),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: request,
        max_cost_micros: 20,
        lease_token: format!("lease-{call_id}"),
        lease_until: 100,
        now: 10,
    }
}

fn owned(sources: &[&str]) -> Vec<String> {
    sources.iter().map(|id| (*id).to_string()).collect()
}

/// Reserves `call_id` with `request` and registers `sources` as its closure (a
/// budget call reference with one edge to each run).
async fn reserve(
    store: &Store,
    call_id: &str,
    request: BudgetArtifact,
    sources: &[&str],
) -> BudgetCallRecord {
    store
        .reserve_budget_call_with_sources(
            &host(),
            &reservation(call_id, Some(request)),
            &owned(sources),
        )
        .await
        .unwrap()
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

async fn dispatch(store: &Store, call: &BudgetCallRecord, now: i64) -> BudgetCallRecord {
    let decision = store
        .begin_budget_dispatch(&host(), &fence(call, now))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    assert_eq!(decision.call.state, BudgetCallState::Dispatched);
    decision.call
}

/// The id of the budget call reference of `call_id`, derived as the store derives it.
fn ref_id(call_id: &str) -> String {
    let digest = fingerprint(&("rsia.budget_call_ref.v1", "n", SCOPE, call_id)).unwrap();
    format!("budget-ref-{}", &digest[..32])
}

/// Loses the budget call reference of `call_id`: the object is deleted the way
/// `e16_budget_binding_v42.rs` deletes it (its edges stay).
async fn delete_ref(store: &Store, call_id: &str) {
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .get::<Value>(&admin(), "artifact", &ref_id(call_id))
            .await
            .unwrap()
            .is_some(),
        "{call_id} has no budget call reference to lose"
    );
    session
        .delete(&admin(), "artifact", &ref_id(call_id))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn call_of(store: &Store, call_id: &str) -> BudgetCallRecord {
    store
        .budget_call(&host(), SCOPE, call_id)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{call_id} has no budget row"))
}

async fn micros(store: &Store) -> (i64, i64) {
    let root = store.root_budget(&admin(), SCOPE).await.unwrap().unwrap();
    (root.reserved_micros, root.spent_micros)
}

async fn read_only(path: &Path) -> sqlx::SqliteConnection {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

/// Every row of every table (the FTS shadow tables and the migration ledger
/// included), as text, through a second read-only connection.
async fn all_rows(path: &Path) -> BTreeMap<String, Vec<String>> {
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

fn assert_nothing_written(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
    what: &str,
) {
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(changed.is_empty(), "{what}: tables written: {changed:?}");
}

/// Where `needle` can be read in the database at `path`: in a row of a table, or
/// in the raw bytes of the database file or its write-ahead log (which also hold
/// pages that were written and overwritten). Empty when it is nowhere.
async fn found_in(path: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (table, rows) in all_rows(path).await {
        let hits = rows.iter().filter(|row| row.contains(needle)).count();
        if hits > 0 {
            found.push(format!("table {table}: {hits} row(s)"));
        }
    }
    for (suffix, label) in [("", "file: database"), ("-wal", "file: write-ahead log")] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        if let Ok(bytes) = std::fs::read(PathBuf::from(name))
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(label.to_string());
        }
    }
    found
}

/// The `details` of the events of `call_id` of one kind, in write order.
async fn events_of(path: &Path, call_id: &str, kind: &str) -> Vec<Value> {
    let mut connection = read_only(path).await;
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT details FROM root_budget_events WHERE call_id=? AND event_kind=? ORDER BY seq",
    )
    .bind(call_id)
    .bind(kind)
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    details
        .iter()
        .map(|text| serde_json::from_str(text).unwrap())
        .collect()
}

/// The charge of the late response of `call_id`: what the provider billed.
fn late_charge(call_id: &str, amount: i64) -> UsageCharge {
    UsageCharge {
        amount_micros: amount,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{call_id}"),
        usage_record_id: format!("usage-{call_id}"),
        output_digest: hash(OUTPUT.as_bytes()),
    }
}

/// A late response in the shapes the persistent broker stores: a transport body
/// and a usable response that both carry the model output, and a blocked response
/// that names none.
fn late_evidence(call_id: &str) -> ModelCallSettlementEvidence {
    ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            TRANSPORT_SCHEMA,
            &json!({
                "response_id": format!("response-{call_id}"),
                "provider_request_id": format!("provider-{call_id}"),
                "usage_record_id": format!("usage-{call_id}"),
                "actual_model_digest": hash(b"model"),
                "output": OUTPUT,
                "actual_cost_micros": 9,
                "currency": "USD",
                "pricing_version": "price-v1",
            }),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                RESPONSE_SCHEMA,
                &json!({
                    "status": "completed",
                    "request_id": call_id,
                    "output": OUTPUT,
                    "output_digest": hash(OUTPUT.as_bytes()),
                }),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            RESPONSE_SCHEMA,
            &json!({
                "status": "cancelled_after_dispatch",
                "request_id": call_id,
                "reason": "dispatch became ineligible after provider execution",
            }),
        )
        .unwrap(),
        forced_block_reason: None,
    }
}

/// The stored transport is the `rsia.redacted.v1` redaction of the transport the
/// provider sent, made for `reason`.
fn assert_transport_redacted(
    call: &BudgetCallRecord,
    evidence: &ModelCallSettlementEvidence,
    reason: &str,
    what: &str,
) {
    let transport = call
        .transport_artifact
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: no transport stored"));
    assert_eq!(transport.schema_version, REDACTED_SCHEMA, "{what}");
    assert!(!transport.body.contains(OUTPUT), "{what}");
    let body: Value = serde_json::from_str(&transport.body).unwrap();
    assert_eq!(body["state"], "source_revoked", "{what}: {body}");
    assert_eq!(body["reason"], reason, "{what}: {body}");
    assert_eq!(body["original_schema"], TRANSPORT_SCHEMA, "{what}: {body}");
    assert_eq!(
        body["original_digest"], evidence.transport_artifact.digest,
        "{what}: {body}"
    );
}

fn assert_unverifiable<T: std::fmt::Debug>(result: evo_core::Result<T>, what: &str) {
    assert!(
        matches!(&result, Err(Error::Conflict(code)) if code == UNVERIFIABLE),
        "{what}: expected Conflict({UNVERIFIABLE}), got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// T1/T2: the dispatch gate
// ---------------------------------------------------------------------------

/// T1. Reserved over `source-a` with a registered closure; the reference is then
/// lost and `source-a` revoked (`begin_revoke`, no cleanup step). The dispatch reads
/// the closure the persisted request names and refuses the call with the decision a
/// caller releases on (`SourceRevoked`; `Cancelled` on the historical API and in a
/// caller's transaction); nothing is written and the reservation is kept until it is
/// released. Baseline: dispatched (`new_dispatch: true`).
#[tokio::test]
async fn t1_dispatch_over_a_revoked_source_is_refused_when_the_reference_is_lost() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = reserve(
        &store,
        "call-1",
        model_request("call-1", &[SOURCE]),
        &[SOURCE],
    )
    .await;
    delete_ref(&store, "call-1").await;
    revoke(&store, "run", SOURCE).await;
    let before = all_rows(&path).await;

    let typed = store
        .begin_budget_dispatch_typed(&host(), &fence(&call, 11))
        .await
        .unwrap();
    assert_eq!(
        typed,
        Err(BudgetPreDispatchRefusal::SourceRevoked),
        "a lost reference dispatched a call over a revoked source"
    );
    let historical = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await;
    assert!(
        matches!(historical, Err(Error::Cancelled)),
        "historical API: {historical:?}"
    );
    let mut session = store.session().await.unwrap();
    let in_session = session
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await;
    assert!(
        matches!(in_session, Err(Error::Cancelled)),
        "session: {in_session:?}"
    );
    drop(session);
    assert_nothing_written(&before, &all_rows(&path).await, "refused dispatch");
    let still = call_of(&store, "call-1").await;
    assert_eq!(still.state, BudgetCallState::Reserved);
    assert_eq!(still.dispatch_id, None);
    assert_eq!(still.dispatched_at, None);
    assert_eq!(micros(&store).await, (20, 0));

    let released = store
        .release_undispatched_budget_call(&host(), &fence(&call, 12), "source_revoked")
        .await
        .unwrap();
    assert_eq!(released.state, BudgetCallState::Released);
    assert_eq!(released.dispatch_id, None);
    assert_eq!(micros(&store).await, (0, 0));
}

/// T2. Reserved over the live `source-a` with a registered closure and a request the
/// closure reader cannot read; the reference is then lost. Whether the call is free
/// of a revoked source cannot be told, so the dispatch fails closed with
/// `Conflict("budget_sources_unverifiable")` on every entry point, writes nothing,
/// and the reservation is kept until it is released. Baseline: dispatched.
#[tokio::test]
async fn t2_dispatch_fails_closed_when_the_reference_is_lost_and_the_request_cannot_be_read() {
    for (case, request) in unreadable_requests("call-1") {
        let (_dir, path, store) = database().await;
        put_run(&store, SOURCE).await;
        authorize(&store).await;
        let call = reserve(&store, "call-1", request, &[SOURCE]).await;
        delete_ref(&store, "call-1").await;
        let before = all_rows(&path).await;

        assert_unverifiable(
            store
                .begin_budget_dispatch_typed(&host(), &fence(&call, 11))
                .await,
            case,
        );
        assert_unverifiable(
            store
                .begin_budget_dispatch(&host(), &fence(&call, 11))
                .await,
            case,
        );
        let mut session = store.session().await.unwrap();
        assert_unverifiable(
            session
                .begin_budget_dispatch(&host(), &fence(&call, 11))
                .await,
            case,
        );
        drop(session);
        assert_nothing_written(&before, &all_rows(&path).await, case);
        let still = call_of(&store, "call-1").await;
        assert_eq!(still.state, BudgetCallState::Reserved, "{case}");
        assert_eq!(still.dispatch_id, None, "{case}");
        assert_eq!(micros(&store).await, (20, 0), "{case}");

        let released = store
            .release_undispatched_budget_call(&host(), &fence(&call, 12), UNVERIFIABLE)
            .await
            .unwrap();
        assert_eq!(released.state, BudgetCallState::Released, "{case}");
        assert_eq!(micros(&store).await, (0, 0), "{case}");
    }
}

/// T2, after the cleanup. The cleanup of `source-a` completes: it redacts the request
/// of the reserved call and keeps its reference, so the dispatch is refused. The
/// reference is then lost. A redaction names no closure, so the dispatch still fails
/// closed and writes nothing. Baseline: the revoked call is dispatched.
#[tokio::test]
async fn t2_dispatch_fails_closed_after_the_cleanup_when_the_reference_is_lost() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = reserve(
        &store,
        "call-1",
        model_request("call-1", &[SOURCE]),
        &[SOURCE],
    )
    .await;
    let status = revoke(&store, "run", SOURCE).await;
    complete_cleanup(&store, &status.job_id).await;
    let redacted = call_of(&store, "call-1").await;
    assert_eq!(
        redacted.request_artifact.as_ref().unwrap().schema_version,
        REDACTED_SCHEMA
    );
    assert!(matches!(
        store
            .begin_budget_dispatch(&host(), &fence(&call, 20))
            .await,
        Err(Error::Cancelled)
    ));
    delete_ref(&store, "call-1").await;
    let before = all_rows(&path).await;

    assert_unverifiable(
        store
            .begin_budget_dispatch(&host(), &fence(&call, 21))
            .await,
        "redacted request",
    );
    assert_nothing_written(&before, &all_rows(&path).await, "redacted request");
    let still = call_of(&store, "call-1").await;
    assert_eq!(still.state, BudgetCallState::Reserved);
    assert_eq!(still.dispatch_id, None);
    assert_eq!(micros(&store).await, (20, 0));
}

// ---------------------------------------------------------------------------
// T3: the settlement twin
// ---------------------------------------------------------------------------

/// T3. Dispatched over `source-a` while it was live; the reference is then lost and
/// `source-a` revoked; then the late response arrives. The settlement reads the
/// closure the persisted request names: the charge is booked (finalized, spent),
/// the response is blocked as `source_revoked`, the transport is stored as its
/// redaction and the output is stored nowhere. Once the execution is closed, as the
/// broker closes it after a settlement, the root dispatches its next call.
/// Baseline: the late output is stored as usable.
#[tokio::test]
async fn t3_late_response_over_a_revoked_source_is_booked_and_blocked_when_the_ref_is_lost() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let call = reserve(
        &store,
        "call-1",
        model_request("call-1", &[SOURCE]),
        &[SOURCE],
    )
    .await;
    let dispatched = dispatch(&store, &call, 11).await;
    delete_ref(&store, "call-1").await;
    revoke(&store, "run", SOURCE).await;
    let evidence = late_evidence("call-1");

    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 9),
            &evidence,
        )
        .await
        .unwrap();
    assert!(
        !settled.response_usable,
        "a lost reference left a late output over a revoked source usable"
    );
    assert_eq!(
        settled.response_block_reason.as_deref(),
        Some("source_revoked")
    );
    let stored = call_of(&store, "call-1").await;
    assert_eq!(settled.call, stored);
    assert_eq!(stored.state, BudgetCallState::Finalized);
    assert_eq!(stored.actual_cost_micros, Some(9));
    assert_eq!(stored.usage_record_id.as_deref(), Some("usage-call-1"));
    assert_eq!(stored.dispatch_id, dispatched.dispatch_id);
    assert_eq!(micros(&store).await, (0, 9));
    assert_eq!(
        stored.response_artifact.as_ref(),
        Some(&evidence.blocked_response)
    );
    assert_transport_redacted(&stored, &evidence, "source_revoked", "T3");
    assert!(
        found_in(&path, OUTPUT).await.is_empty(),
        "the output is readable in {:?}",
        found_in(&path, OUTPUT).await
    );
    let persisted = events_of(&path, "call-1", "model_response_persisted").await;
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0]["source_revoked"], true);
    assert_eq!(persisted[0]["response_usable"], false);

    store
        .close_budget_call_execution(
            &host(),
            SCOPE,
            "call-1",
            dispatched.dispatch_id.as_deref().unwrap(),
            "transport_completion_persisted",
            21,
        )
        .await
        .unwrap();
    let next = reserve(&store, "call-2", model_request("call-2", &[LIVE]), &[LIVE]).await;
    let decision = store
        .begin_budget_dispatch(&host(), &fence(&next, 22))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    assert_eq!(decision.call.state, BudgetCallState::Dispatched);
}

/// T3, unverifiable closure. Dispatched over the live `source-a` (its reference was
/// intact) with a request the closure reader cannot read; the reference is then
/// lost; then the late response arrives. The charge is booked, the response is
/// blocked as `budget_sources_unverifiable`, the transport is stored as its
/// redaction and the output is stored nowhere. A replay of the same settlement is
/// the same settlement and writes nothing. Baseline: the late output is usable.
#[tokio::test]
async fn t3_late_response_with_a_lost_ref_and_an_unreadable_request_is_booked_and_blocked() {
    for (case, request) in unreadable_requests("call-1") {
        let (_dir, path, store) = database().await;
        put_run(&store, SOURCE).await;
        authorize(&store).await;
        let call = reserve(&store, "call-1", request, &[SOURCE]).await;
        let dispatched = dispatch(&store, &call, 11).await;
        delete_ref(&store, "call-1").await;
        let evidence = late_evidence("call-1");

        let settled = store
            .settle_model_budget_call(
                &host(),
                &fence(&dispatched, 20),
                &late_charge("call-1", 9),
                &evidence,
            )
            .await
            .unwrap();
        assert!(!settled.response_usable, "{case}");
        assert_eq!(
            settled.response_block_reason.as_deref(),
            Some(UNVERIFIABLE),
            "{case}"
        );
        let stored = call_of(&store, "call-1").await;
        assert_eq!(settled.call, stored, "{case}");
        assert_eq!(stored.state, BudgetCallState::Finalized, "{case}");
        assert_eq!(stored.actual_cost_micros, Some(9), "{case}");
        assert_eq!(micros(&store).await, (0, 9), "{case}");
        assert_eq!(
            stored.response_artifact.as_ref(),
            Some(&evidence.blocked_response),
            "{case}"
        );
        assert_transport_redacted(&stored, &evidence, UNVERIFIABLE, case);
        assert!(
            found_in(&path, OUTPUT).await.is_empty(),
            "{case}: the output is readable in {:?}",
            found_in(&path, OUTPUT).await
        );
        let persisted = events_of(&path, "call-1", "model_response_persisted").await;
        assert_eq!(persisted.len(), 1, "{case}");
        assert_eq!(persisted[0][UNVERIFIABLE], true, "{case}");
        assert_eq!(persisted[0]["response_usable"], false, "{case}");

        let before = all_rows(&path).await;
        let replayed = store
            .settle_model_budget_call(
                &host(),
                &fence(&dispatched, 21),
                &late_charge("call-1", 9),
                &evidence,
            )
            .await
            .unwrap();
        assert_eq!(replayed, settled, "{case}: replay");
        assert_nothing_written(&before, &all_rows(&path).await, case);
    }
}

// ---------------------------------------------------------------------------
// T4: guard cases (they pass at the baseline and after the change)
// ---------------------------------------------------------------------------

/// T4 guard. The reference is intact and every source is live: the call is
/// dispatched and its response is usable and stored as sent.
#[tokio::test]
async fn t4_guard_an_intact_reference_over_live_sources_dispatches_and_settles_usable() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = reserve(
        &store,
        "call-1",
        model_request("call-1", &[SOURCE]),
        &[SOURCE],
    )
    .await;
    let dispatched = dispatch(&store, &call, 11).await;
    let evidence = late_evidence("call-1");
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 9),
            &evidence,
        )
        .await
        .unwrap();
    assert!(settled.response_usable);
    assert_eq!(settled.response_block_reason, None);
    assert_eq!(
        settled.call.transport_artifact.as_ref(),
        Some(&evidence.transport_artifact)
    );
    assert_eq!(micros(&store).await, (0, 9));
}

/// T4 guard. The reference is lost but the request names live runs only: nothing
/// is revoked, so the call is dispatched and its response is usable.
#[tokio::test]
async fn t4_guard_a_lost_reference_over_live_sources_dispatches_and_settles_usable() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let call = reserve(
        &store,
        "call-1",
        model_request("call-1", &[LIVE, SOURCE]),
        &[LIVE, SOURCE],
    )
    .await;
    delete_ref(&store, "call-1").await;
    let dispatched = dispatch(&store, &call, 11).await;
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 9),
            &late_evidence("call-1"),
        )
        .await
        .unwrap();
    assert!(settled.response_usable);
    assert_eq!(settled.response_block_reason, None);
    assert_eq!(micros(&store).await, (0, 9));
}

/// T4 guard. A call reserved without a source list whose request is of a schema
/// the reader reads and names no source (a library-level call with no closure;
/// production reservations always carry one) is dispatched and settled as before,
/// even with a revocation in its namespace.
#[tokio::test]
async fn t4_guard_a_bare_call_whose_readable_request_names_no_source_is_dispatched() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    revoke(&store, "run", SOURCE).await;
    let call = store
        .reserve_budget_call(
            &host(),
            &reservation("call-1", Some(model_request("call-1", &[]))),
        )
        .await
        .unwrap();
    let dispatched = dispatch(&store, &call, 11).await;
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 9),
            &late_evidence("call-1"),
        )
        .await
        .unwrap();
    assert!(settled.response_usable);
    assert_eq!(micros(&store).await, (0, 9));
}

/// T4 guard. A call reserved without a source list and without a request (the
/// library-level bare reservation, e.g. `budget_revoke_gate.rs`) carries no
/// closure to read and is dispatched as before.
#[tokio::test]
async fn t4_guard_a_bare_call_without_a_request_is_dispatched() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    revoke(&store, "run", SOURCE).await;
    let call = store
        .reserve_budget_call(&host(), &reservation("call-1", None))
        .await
        .unwrap();
    dispatch(&store, &call, 11).await;
    assert_eq!(micros(&store).await, (20, 0));
}
