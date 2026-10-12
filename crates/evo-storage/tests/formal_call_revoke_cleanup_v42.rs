//! AG-117 (E08 L1293/L1295/L1299; plan §11 L1077, §11.3 L1110, §11.5 L1132/L1134,
//! L835; V017 L1517 and V018 L1518 sub-scope): the revocation cleanup judges the
//! source closure of a legacy budget call by its registered budget call reference,
//! as the legacy dispatch and settlement gates do, and falls back to the persisted
//! request only when the call has no reference.
//!
//! The broker stores a formal task call's request as `rsia.formal_task_request.v1`,
//! a body that names no source closure; the closure is the candidate's sources,
//! registered as the call's budget call reference when it is reserved. The cleanup
//! used to read request bodies only: every such call of the namespace was of a
//! schema it does not read, so every revocation cleanup of the namespace isolated
//! it (`blocked_unknown_scope`) and ended `Failed`, whatever the revoked source, and
//! every Admin retry failed again; and a formal call that did depend on the revoked
//! source kept its request (the task input and the mandatory context).
//!
//! * A: a formal call whose registered closure does not hold the revoked run is
//!   left as it is, byte for byte, and the cleanup completes.
//! * B: a formal call whose registered closure holds the revoked run has its request
//!   (and transport and response, once settled) redacted; its reference and its
//!   accounting are kept.
//! * C: the same two outcomes for a call written after the budget scan passed (the
//!   recheck `redact_late_budget_calls`).
//! * D: no Admin retry is needed; a job the previous code failed on such a call
//!   completes on its Admin retry.
//! * E (guards): a call without a reference is judged by its request as before:
//!   unreadable (an unknown schema, a formal request) is isolated and fails the job,
//!   a model request is cleaned by the closure it names.
//! * F (gate unchanged): the dispatch gate's answer for a formal call is the same
//!   before the revocation, after it and after its cleanup.
//! * G: a call whose reference is stored but does not decode is isolated by the
//!   cleanup (the rest of the namespace is cleaned, the job fails, every Admin
//!   retry isolates it again), while the dispatch gate refuses it with `Internal`.
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
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const NS: &str = "n";
const SCOPE: &str = "scope-1";
const GROUP: &str = "group-1";
/// The run the tests revoke.
const RUN_A: &str = "run-a";
/// A run that is never revoked.
const RUN_B: &str = "run-b";
const REASON: &str = "source revoked";
const FORMAL_SCHEMA: &str = "rsia.formal_task_request.v1";
const MODEL_REQUEST_SCHEMA: &str = "rsia.model_request.artifact.v1";
const TRANSPORT_SCHEMA: &str = "rsia.model_transport.artifact.v1";
const RESPONSE_SCHEMA: &str = "rsia.model_response.artifact.v1";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
/// The diagnostic the cleanup isolated every formal call under before this change.
const FORMAL_UNKNOWN: &str = "unknown_budget_request_schema:rsia.formal_task_request.v1";
/// Text that appears in a formal request only (its task input and its mandatory
/// context), and in the model output of a settled call only.
const FORMAL_SECRET: &str = "AG117-FORMAL-SECRET";
const OUTPUT: &str = "AG117-FORMAL-MODEL-OUTPUT-9c41e07a";
/// The columns a cleanup redaction may write: the three artifacts, and the
/// usability of a stored response. Every other column is accounting.
const CONTENT_COLUMNS: [&str; 11] = [
    "request_artifact_schema",
    "request_artifact_digest",
    "request_artifact_body",
    "transport_artifact_schema",
    "transport_artifact_digest",
    "transport_artifact_body",
    "response_artifact_schema",
    "response_artifact_digest",
    "response_artifact_body",
    "response_usable",
    "response_block_reason",
];
/// More than any scenario below needs: a cleanup that does not finish within it is
/// stuck, not slow.
const MAX_STEPS: usize = 500;

fn admin() -> Context {
    Context::new(NS, "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NS, "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new(NS, "broker", Role::Host).unwrap()
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
async fn revoke(store: &Store, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: id.into(),
        },
        REASON,
        30,
    )
    .await
    .unwrap()
}

/// Steps the cleanup of `job_id` as Admin until it is `Complete` or `Failed`, one
/// time unit per step. An Admin step on a `Failed` job retries it, so calling this
/// again on a `Failed` job is one explicit Admin retry.
async fn drive(store: &Store, job_id: &str, limit: usize, clock: &mut i64) -> CleanupStatus {
    for _ in 0..MAX_STEPS {
        *clock += 1;
        let status = LifecycleStore::cleanup_step(&admin(), store, job_id, limit, *clock)
            .await
            .unwrap();
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            return status;
        }
    }
    panic!("the cleanup of {job_id} did not finish in {MAX_STEPS} steps");
}

async fn authorize(store: &Store) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec![NS.into()],
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

/// A request in the shape of the broker's `FormalTaskRequest` (field for field;
/// storage cannot depend on the engine's types). It names no source closure: the
/// closure of a formal call is the candidate's sources, registered at reservation.
fn formal_request(call_id: &str) -> BudgetArtifact {
    let input = format!("{FORMAL_SECRET}-task-input-{call_id}");
    BudgetArtifact::from_serializable(
        FORMAL_SCHEMA,
        &json!({
            "task": {
                "ticket_id": "ticket-1",
                "target_id": format!("target-{call_id}"),
                "input_artifact_id": format!("input-{call_id}"),
                "input_digest": hash(input.as_bytes()),
                "side": "candidate",
                "bundle_digest": hash(b"bundle"),
                "environment_digest": hash(b"environment"),
                "request_digest": hash(format!("request-{call_id}").as_bytes()),
            },
            "candidate_id": "candidate-1",
            "profile_registration_id": "profile-1",
            "compiler_binding_digest": hash(b"binding"),
            "model_digest": hash(b"model"),
            "mandatory_context": format!("{FORMAL_SECRET}-mandatory-context-{call_id}"),
            "projection": {"bundle_digest": hash(b"bundle")},
            "task_input_utf8": input,
        }),
    )
    .unwrap()
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

fn reservation(call_id: &str, request: BudgetArtifact) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: call_id.into(),
        dispatch_group_id: GROUP.into(),
        stage: BudgetStage::TaskExecution,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(request),
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
/// budget call reference with one edge to each run), as the broker reserves a
/// formal call over its candidate's sources.
async fn reserve(
    store: &Store,
    call_id: &str,
    request: BudgetArtifact,
    sources: &[&str],
) -> BudgetCallRecord {
    store
        .reserve_budget_call_with_sources(&host(), &reservation(call_id, request), &owned(sources))
        .await
        .unwrap()
}

/// Reserves `call_id` with `request` and no closure: the call has no reference.
async fn reserve_bare(store: &Store, call_id: &str, request: BudgetArtifact) -> BudgetCallRecord {
    store
        .reserve_budget_call(&host(), &reservation(call_id, request))
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

/// A response in the shapes the persistent broker stores: a transport body and a
/// usable response that both carry the model output, and a blocked response that
/// names none.
fn settlement_evidence(call_id: &str) -> ModelCallSettlementEvidence {
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

/// Dispatches `call` and settles it with a usable response while its sources are
/// live: 9 micros are spent and the output is stored.
async fn dispatch_and_settle(store: &Store, call: &BudgetCallRecord) {
    let decision = store
        .begin_budget_dispatch(&host(), &fence(call, 11))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&decision.call, 12),
            &UsageCharge {
                amount_micros: 9,
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                provider_request_id: format!("provider-{}", call.call_id),
                usage_record_id: format!("usage-{}", call.call_id),
                output_digest: hash(OUTPUT.as_bytes()),
            },
            &settlement_evidence(&call.call_id),
        )
        .await
        .unwrap();
    assert!(settled.response_usable, "{settled:?}");
    assert_eq!(settled.call.state, BudgetCallState::Finalized);
}

/// The id of the budget call reference of `call_id`, derived as the store derives it.
fn ref_id(call_id: &str) -> String {
    let digest = fingerprint(&("rsia.budget_call_ref.v1", NS, SCOPE, call_id)).unwrap();
    format!("budget-ref-{}", &digest[..32])
}

/// Loses the budget call reference of `call_id`: the object is deleted (its edges
/// stay), as `budget_ref_missing_fail_closed_v42.rs` loses it.
async fn delete_ref(store: &Store, call_id: &str) {
    let mut session = store.session().await.unwrap();
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

async fn read_only(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap()
}

async fn writable(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
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

/// The tables with a row that holds `needle`.
async fn tables_holding(path: &Path, needle: &str) -> Vec<String> {
    all_rows(path)
        .await
        .into_iter()
        .filter(|(_, rows)| rows.iter().any(|row| row.contains(needle)))
        .map(|(table, _)| table)
        .collect()
}

/// Every column of the budget row of `call_id`, as SQL text (`quote`), by name.
async fn call_row(path: &Path, call_id: &str) -> BTreeMap<String, String> {
    let mut connection = read_only(path).await;
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('root_budget_calls') ORDER BY cid")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    let mut out = BTreeMap::new();
    for column in columns {
        let value: String = sqlx::query_scalar(&format!(
            "SELECT quote(\"{column}\") FROM root_budget_calls
             WHERE billing_scope=? AND call_id=?"
        ))
        .bind(SCOPE)
        .bind(call_id)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        out.insert(column, value);
    }
    connection.close().await.unwrap();
    out
}

/// The accounting columns of the budget row of `call_id`: every column but the
/// content a cleanup redaction may write.
async fn ledger_of(path: &Path, call_id: &str) -> BTreeMap<String, String> {
    let mut row = call_row(path, call_id).await;
    for column in CONTENT_COLUMNS {
        assert!(row.remove(column).is_some(), "no column {column}");
    }
    row
}

/// The stored body of an object, read back from disk.
async fn object_body(path: &Path, kind: &str, id: &str) -> Option<String> {
    let mut connection = read_only(path).await;
    let body = sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind=? AND id=?")
        .bind(NS)
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    body
}

/// The cleanup events of `kind` of the job, oldest first, read back from disk.
async fn cleanup_events(path: &Path, job: &str, kind: &str) -> Vec<Value> {
    let mut connection = read_only(path).await;
    let encoded: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('kind',node_kind,'id',node_id,'details',json(details)))
         FROM (SELECT * FROM revoke_cleanup_events
               WHERE namespace=? AND job_id=? AND event_kind=? ORDER BY seq)",
    )
    .bind(NS)
    .bind(job)
    .bind(kind)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    serde_json::from_str::<Vec<Value>>(&encoded).unwrap()
}

fn isolation_of(call_id: &str, error: &str) -> Value {
    json!({"kind": "reservation", "id": call_id, "details": {"error": error}})
}

/// Whether the job's `__budget_scan` has read every budget call of the namespace.
async fn budget_scan_expanded(path: &Path, job: &str) -> bool {
    let mut connection = read_only(path).await;
    let expanded: i64 = sqlx::query_scalar(
        "SELECT expanded FROM revoke_cleanup_frontier
         WHERE namespace=? AND job_id=? AND node_kind='__budget_scan'",
    )
    .bind(NS)
    .bind(job)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    expanded == 1
}

/// The cleanup completed without isolating anything.
async fn assert_complete(path: &Path, status: &CleanupStatus, what: &str) {
    assert_eq!(status.state, CleanupState::Complete, "{what}: {status:?}");
    assert_eq!(status.last_error, None, "{what}: {status:?}");
    assert_eq!(status.pending_nodes, 0, "{what}: {status:?}");
    assert_eq!(
        cleanup_events(path, &status.job_id, "blocked_unknown_scope").await,
        Vec::<Value>::new(),
        "{what}"
    );
}

/// The stored artifact is the cleanup's `rsia.redacted.v1` redaction of an
/// artifact of `original_schema`.
fn assert_redacted(artifact: Option<&BudgetArtifact>, original_schema: &str, what: &str) {
    let artifact = artifact.unwrap_or_else(|| panic!("{what}: nothing stored"));
    assert_eq!(
        artifact.schema_version, REDACTED_SCHEMA,
        "{what}: {artifact:?}"
    );
    assert!(!artifact.body.contains(FORMAL_SECRET), "{what}");
    assert!(!artifact.body.contains(OUTPUT), "{what}");
    let body: Value = serde_json::from_str(&artifact.body).unwrap();
    assert_eq!(body["state"], "source_revoked", "{what}: {body}");
    assert_eq!(body["reason"], "source_revoked", "{what}: {body}");
    assert_eq!(body["original_schema"], original_schema, "{what}: {body}");
}

/// The redacted state of a formal call over the revoked run after its cleanup: the
/// request (and, when it was settled, the transport and the response) is a
/// redaction, a stored response is no longer usable, the reference is kept as it
/// was, and no accounting column moved.
async fn assert_formal_call_cleaned(
    store: &Store,
    path: &Path,
    call_id: &str,
    ledger: &BTreeMap<String, String>,
    reference: &Option<String>,
) {
    let call = call_of(store, call_id).await;
    assert_redacted(call.request_artifact.as_ref(), FORMAL_SCHEMA, call_id);
    if call.state == BudgetCallState::Finalized {
        assert_redacted(call.transport_artifact.as_ref(), TRANSPORT_SCHEMA, call_id);
        assert_redacted(call.response_artifact.as_ref(), RESPONSE_SCHEMA, call_id);
        assert_eq!(call.response_usable, Some(false), "{call_id}");
        assert_eq!(
            call.response_block_reason.as_deref(),
            Some("source_revoked"),
            "{call_id}"
        );
    }
    assert_eq!(&ledger_of(path, call_id).await, ledger, "{call_id}");
    assert!(
        reference.is_some(),
        "{call_id} was reserved without a reference"
    );
    assert_eq!(
        &object_body(path, "artifact", &ref_id(call_id)).await,
        reference,
        "{call_id}: its budget call reference changed"
    );
}

// ---------------------------------------------------------------------------
// A: a formal call unrelated to the revoked run
// ---------------------------------------------------------------------------

/// A. A formal call registered over `run-b` (reserved, and another one settled with
/// a usable response) is unrelated to the revocation of `run-a`: the cleanup leaves
/// every column of its row, and its reference, as they were, isolates nothing and
/// completes. Baseline: `Failed` with `last_error` = `FORMAL_UNKNOWN`, one
/// `blocked_unknown_scope` per formal call.
#[tokio::test]
async fn a_formal_call_unrelated_to_the_revoked_run_is_left_as_it_is_and_the_cleanup_completes() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    let settled = reserve(
        &store,
        "call-f-settled",
        formal_request("call-f-settled"),
        &[RUN_B],
    )
    .await;
    dispatch_and_settle(&store, &settled).await;
    reserve(
        &store,
        "call-f-reserved",
        formal_request("call-f-reserved"),
        &[RUN_B],
    )
    .await;
    let mut before = BTreeMap::new();
    for call_id in ["call-f-settled", "call-f-reserved"] {
        let reference = object_body(&path, "artifact", &ref_id(call_id)).await;
        assert!(reference.is_some(), "{call_id} has no reference");
        before.insert(call_id, (call_row(&path, call_id).await, reference));
    }
    let money = micros(&store).await;
    assert_eq!(money, (20, 9));

    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;
    let done = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_complete(&path, &done, "A").await;
    for (call_id, (row, reference)) in &before {
        assert_eq!(
            &call_row(&path, call_id).await,
            row,
            "{call_id}: its row changed"
        );
        assert_eq!(
            &object_body(&path, "artifact", &ref_id(call_id)).await,
            reference,
            "{call_id}: its reference changed"
        );
    }
    assert_eq!(micros(&store).await, money);
}

// ---------------------------------------------------------------------------
// B: a formal call over the revoked run
// ---------------------------------------------------------------------------

/// B. A formal call registered over `run-a` (one reserved, one settled with a usable
/// response while `run-a` was live) holds the task input and the mandatory context
/// of a task derived from `run-a`: the cleanup redacts its request, and the settled
/// one's transport and response, which is no longer usable; amounts, state and
/// every other accounting column stay, the reference is kept, and the cleanup
/// completes. A formal call over `run-b` in the same namespace is left as it is.
/// Baseline: `Failed` (`FORMAL_UNKNOWN`) and every request kept.
#[tokio::test]
async fn b_a_formal_call_over_the_revoked_run_is_redacted_and_its_accounting_kept() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    let settled = reserve(
        &store,
        "call-f-settled",
        formal_request("call-f-settled"),
        &[RUN_A],
    )
    .await;
    dispatch_and_settle(&store, &settled).await;
    reserve(
        &store,
        "call-f-reserved",
        formal_request("call-f-reserved"),
        &[RUN_A],
    )
    .await;
    reserve(
        &store,
        "call-f-other",
        formal_request("call-f-other"),
        &[RUN_B],
    )
    .await;
    let mut before = BTreeMap::new();
    for call_id in ["call-f-settled", "call-f-reserved"] {
        before.insert(
            call_id,
            (
                ledger_of(&path, call_id).await,
                object_body(&path, "artifact", &ref_id(call_id)).await,
            ),
        );
    }
    let other = call_row(&path, "call-f-other").await;
    let money = micros(&store).await;
    assert_eq!(money, (40, 9));

    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;
    let done = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_complete(&path, &done, "B").await;
    for (call_id, (ledger, reference)) in &before {
        assert_formal_call_cleaned(&store, &path, call_id, ledger, reference).await;
    }
    assert_eq!(
        call_of(&store, "call-f-settled").await.state,
        BudgetCallState::Finalized
    );
    assert_eq!(
        call_of(&store, "call-f-reserved").await.state,
        BudgetCallState::Reserved
    );
    assert_eq!(call_row(&path, "call-f-other").await, other);
    assert_eq!(micros(&store).await, money);
    assert_eq!(tables_holding(&path, OUTPUT).await, Vec::<String>::new());
    // Only the formal call over `run-b` still holds its task input.
    let holding = all_rows(&path).await["root_budget_calls"]
        .iter()
        .filter(|row| row.contains(FORMAL_SECRET))
        .count();
    assert_eq!(holding, 1);
}

// ---------------------------------------------------------------------------
// C: the recheck path (a call written after the budget scan passed)
// ---------------------------------------------------------------------------

/// Begins the revocation of `run-a` and steps it once with an edge page of one,
/// which expands the job's `__budget_scan` (its first frontier node) over a
/// namespace that holds no budget call yet; the job is still running.
async fn revoke_past_the_budget_scan(store: &Store, path: &Path, clock: &mut i64) -> String {
    let status = revoke(store, RUN_A).await;
    *clock += 1;
    let stepped = LifecycleStore::cleanup_step(&admin(), store, &status.job_id, 1, *clock)
        .await
        .unwrap();
    assert_eq!(stepped.state, CleanupState::Running, "{stepped:?}");
    assert!(budget_scan_expanded(path, &status.job_id).await);
    status.job_id
}

/// C, unrelated. A formal call registered over `run-b` after the budget scan passed
/// is read by the recheck, left as it is, and the cleanup completes. Baseline:
/// `Failed` (`FORMAL_UNKNOWN`).
#[tokio::test]
async fn c_a_late_formal_call_unrelated_to_the_revoked_run_is_left_by_the_recheck() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    let mut clock = 30;
    let job = revoke_past_the_budget_scan(&store, &path, &mut clock).await;
    reserve(
        &store,
        "call-f-late",
        formal_request("call-f-late"),
        &[RUN_B],
    )
    .await;
    let row = call_row(&path, "call-f-late").await;
    let reference = object_body(&path, "artifact", &ref_id("call-f-late")).await;
    assert!(reference.is_some());

    let done = drive(&store, &job, 1, &mut clock).await;

    assert_complete(&path, &done, "C unrelated").await;
    assert_eq!(call_row(&path, "call-f-late").await, row);
    assert_eq!(
        object_body(&path, "artifact", &ref_id("call-f-late")).await,
        reference
    );
    assert_eq!(
        cleanup_events(&path, &job, "late_budget_calls_redacted").await,
        Vec::<Value>::new()
    );
}

/// C, related. After the budget scan passed, a formal call is reserved without a
/// closure and then reserved again, as recovery reserves an existing call, with its
/// closure `run-a`: a reservation of an existing call is returned as before and
/// registers the closure (a new call over a revoked run is refused at once, so this
/// is how a late call over `run-a` reaches the ledger). The recheck redacts its
/// request by the registered closure, keeps its reference and accounting, and the
/// cleanup completes. Baseline: `Failed` (`FORMAL_UNKNOWN`), the request kept.
#[tokio::test]
async fn c_a_late_formal_call_over_the_revoked_run_is_redacted_by_the_recheck() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    let mut clock = 30;
    let job = revoke_past_the_budget_scan(&store, &path, &mut clock).await;
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(
                &host(),
                &reservation("call-f-new", formal_request("call-f-new")),
                &owned(&[RUN_A]),
            )
            .await,
        Err(Error::Forbidden)
    ));
    reserve_bare(&store, "call-f-late", formal_request("call-f-late")).await;
    reserve(
        &store,
        "call-f-late",
        formal_request("call-f-late"),
        &[RUN_A],
    )
    .await;
    let ledger = ledger_of(&path, "call-f-late").await;
    let reference = object_body(&path, "artifact", &ref_id("call-f-late")).await;
    assert!(reference.is_some());

    let done = drive(&store, &job, 1, &mut clock).await;

    assert_complete(&path, &done, "C related").await;
    assert_formal_call_cleaned(&store, &path, "call-f-late", &ledger, &reference).await;
    assert_eq!(
        cleanup_events(&path, &job, "late_budget_calls_redacted").await,
        vec![json!({"kind": null, "id": null, "details": {"count": 1}})]
    );
}

// ---------------------------------------------------------------------------
// D: Admin retry
// ---------------------------------------------------------------------------

/// D, no retry needed. With a formal call over `run-a` and one over `run-b`, the
/// first round completes; a further Admin step on the completed job changes nothing
/// and starts no retry. Baseline: the first round is `Failed` (`FORMAL_UNKNOWN`),
/// and so is the Admin retry (shown in the failure message).
#[tokio::test]
async fn d_the_first_round_completes_so_no_admin_retry_is_needed() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    reserve(&store, "call-f-a", formal_request("call-f-a"), &[RUN_A]).await;
    reserve(&store, "call-f-b", formal_request("call-f-b"), &[RUN_B]).await;
    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;

    let first = drive(&store, &status.job_id, 8, &mut clock).await;
    let retried = if first.state == CleanupState::Failed {
        Some(drive(&store, &status.job_id, 8, &mut clock).await)
    } else {
        None
    };
    assert_eq!(
        first.state,
        CleanupState::Complete,
        "first round: {first:?}; after one Admin retry: {retried:?}"
    );
    assert_complete(&path, &first, "D").await;
    let before = all_rows(&path).await;
    clock += 1;
    let again = LifecycleStore::cleanup_step(&admin(), &store, &status.job_id, 8, clock)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_nothing_written(&before, &all_rows(&path).await, "an Admin step on Complete");
    assert_eq!(
        cleanup_events(&path, &status.job_id, "cleanup_retry_started").await,
        Vec::<Value>::new()
    );
}

/// D, a job the previous code failed. The job of `run-a` is left `Failed` with the
/// diagnostic the previous code wrote for a formal call (a test-only fixture write
/// of the job row, standing for a store written before this change). A Worker step
/// does not retry it; the explicit Admin retry rescans, redacts the formal call
/// over `run-a`, leaves the one over `run-b`, and completes. Baseline: the Admin
/// retry is `Failed` again with the same diagnostic.
#[tokio::test]
async fn d_a_job_failed_on_a_formal_call_completes_on_its_admin_retry() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    reserve(&store, "call-f-a", formal_request("call-f-a"), &[RUN_A]).await;
    reserve(&store, "call-f-b", formal_request("call-f-b"), &[RUN_B]).await;
    let ledger = ledger_of(&path, "call-f-a").await;
    let reference = object_body(&path, "artifact", &ref_id("call-f-a")).await;
    let other = call_row(&path, "call-f-b").await;
    let status = revoke(&store, RUN_A).await;
    let mut connection = writable(&path).await;
    let updated = sqlx::query(
        "UPDATE revoke_cleanup_jobs SET state='failed',last_error=?
         WHERE namespace=? AND job_id=? AND state='pending'",
    )
    .bind(FORMAL_UNKNOWN)
    .bind(NS)
    .bind(&status.job_id)
    .execute(&mut connection)
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    connection.close().await.unwrap();

    let idle = LifecycleStore::cleanup_step(&worker(), &store, &status.job_id, 8, 31)
        .await
        .unwrap();
    assert_eq!(idle.state, CleanupState::Failed, "{idle:?}");
    assert_eq!(idle.last_error.as_deref(), Some(FORMAL_UNKNOWN));
    let mut clock = 31;
    let retried = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_complete(&path, &retried, "D retry").await;
    assert_eq!(
        cleanup_events(&path, &status.job_id, "cleanup_retry_started")
            .await
            .len(),
        1
    );
    assert_formal_call_cleaned(&store, &path, "call-f-a", &ledger, &reference).await;
    assert_eq!(call_row(&path, "call-f-b").await, other);
}

// ---------------------------------------------------------------------------
// E: guards — a call without a reference is judged by its request as before
// ---------------------------------------------------------------------------

/// E, unknown schema. A call reserved without a closure whose request is of a
/// schema the cleanup does not read cannot be shown free of `run-a`: it is
/// isolated, its bytes kept, and the job fails (as `cleanup_fixpoint.rs` asserts
/// for a late call). Unchanged from the baseline.
#[tokio::test]
async fn e_a_call_without_a_reference_and_of_an_unknown_schema_still_fails_the_job() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    authorize(&store).await;
    let unknown = BudgetArtifact::from_serializable(
        "rsia.unknown_request.v9",
        &json!({"input": "BUDGET-SECRET-call-unknown", "source_ids": [RUN_A]}),
    )
    .unwrap();
    reserve_bare(&store, "call-unknown", unknown).await;
    let row = call_row(&path, "call-unknown").await;
    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;

    let done = drive(&store, &status.job_id, 8, &mut clock).await;

    let error = "unknown_budget_request_schema:rsia.unknown_request.v9";
    assert_eq!(done.state, CleanupState::Failed, "{done:?}");
    assert_eq!(done.last_error.as_deref(), Some(error));
    assert_eq!(
        cleanup_events(&path, &status.job_id, "blocked_unknown_scope").await,
        vec![isolation_of("call-unknown", error)]
    );
    assert_eq!(call_row(&path, "call-unknown").await, row);
}

/// E, a formal call without a reference: reserved without a closure, or with its
/// reference lost. Its request names no closure and a missing reference is not an
/// empty closure, so it is isolated and fails the job as before; its bytes are
/// kept. Unchanged from the baseline.
#[tokio::test]
async fn e_a_formal_call_without_a_reference_is_still_isolated() {
    for lost in [false, true] {
        let (_dir, path, store) = database().await;
        put_run(&store, RUN_A).await;
        put_run(&store, RUN_B).await;
        authorize(&store).await;
        if lost {
            reserve(&store, "call-f", formal_request("call-f"), &[RUN_B]).await;
            delete_ref(&store, "call-f").await;
        } else {
            reserve_bare(&store, "call-f", formal_request("call-f")).await;
        }
        assert_eq!(
            object_body(&path, "artifact", &ref_id("call-f")).await,
            None
        );
        let row = call_row(&path, "call-f").await;
        let status = revoke(&store, RUN_A).await;
        let mut clock = 30;

        let done = drive(&store, &status.job_id, 8, &mut clock).await;

        assert_eq!(done.state, CleanupState::Failed, "lost={lost}: {done:?}");
        assert_eq!(
            done.last_error.as_deref(),
            Some(FORMAL_UNKNOWN),
            "lost={lost}"
        );
        assert_eq!(
            cleanup_events(&path, &status.job_id, "blocked_unknown_scope").await,
            vec![isolation_of("call-f", FORMAL_UNKNOWN)],
            "lost={lost}"
        );
        assert_eq!(call_row(&path, "call-f").await, row, "lost={lost}");
    }
}

/// E, a model request without a reference: judged by the closure its request
/// names. One that names `run-a` is redacted; one that names `run-b` only is left as
/// it is; the job completes. Unchanged from the baseline.
#[tokio::test]
async fn e_a_model_request_without_a_reference_is_still_judged_by_its_request() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    reserve_bare(
        &store,
        "call-m-a",
        model_request("call-m-a", &[RUN_B, RUN_A]),
    )
    .await;
    reserve_bare(&store, "call-m-b", model_request("call-m-b", &[RUN_B])).await;
    for call_id in ["call-m-a", "call-m-b"] {
        assert_eq!(object_body(&path, "artifact", &ref_id(call_id)).await, None);
    }
    let ledger = ledger_of(&path, "call-m-a").await;
    let other = call_row(&path, "call-m-b").await;
    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;

    let done = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_complete(&path, &done, "E model request").await;
    let cleaned = call_of(&store, "call-m-a").await;
    let request = cleaned.request_artifact.as_ref().unwrap();
    assert_eq!(request.schema_version, REDACTED_SCHEMA);
    assert!(!request.body.contains("BUDGET-SECRET"));
    assert_eq!(ledger_of(&path, "call-m-a").await, ledger);
    assert_eq!(call_row(&path, "call-m-b").await, other);
}

// ---------------------------------------------------------------------------
// F: the dispatch gate is unchanged
// ---------------------------------------------------------------------------

/// When the gate is asked, relative to the revocation of `run-a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    BeforeTheRevocation,
    AfterBeginRevoke,
    AfterTheCleanup,
}

/// F. The legacy dispatch gate reads a formal call's registered closure, before and
/// after this change: before the revocation a call over `run-a` is dispatched;
/// once `run-a` is revoked (only `begin_revoke`, or its cleanup run to its end,
/// whatever that end is) a call over `run-a` is refused as `SourceRevoked` on the
/// typed API (`Cancelled` on the historical one) and nothing is written, and a call
/// over `run-b` is dispatched. The same on the baseline.
#[tokio::test]
async fn f_the_dispatch_gate_answers_a_formal_call_as_before() {
    for phase in [
        Phase::BeforeTheRevocation,
        Phase::AfterBeginRevoke,
        Phase::AfterTheCleanup,
    ] {
        let (_dir, path, store) = database().await;
        put_run(&store, RUN_A).await;
        put_run(&store, RUN_B).await;
        authorize(&store).await;
        let over_a = reserve(&store, "call-f-a", formal_request("call-f-a"), &[RUN_A]).await;
        let over_b = reserve(&store, "call-f-b", formal_request("call-f-b"), &[RUN_B]).await;
        if phase == Phase::BeforeTheRevocation {
            let decision = store
                .begin_budget_dispatch_typed(&host(), &fence(&over_a, 11))
                .await
                .unwrap()
                .unwrap_or_else(|refusal| panic!("{phase:?}: refused {refusal:?}"));
            assert!(decision.new_dispatch, "{phase:?}");
            assert_eq!(
                decision.call.state,
                BudgetCallState::Dispatched,
                "{phase:?}"
            );
            continue;
        }
        let status = revoke(&store, RUN_A).await;
        if phase == Phase::AfterTheCleanup {
            let mut clock = 30;
            drive(&store, &status.job_id, 8, &mut clock).await;
        }
        let before = all_rows(&path).await;
        let refused = store
            .begin_budget_dispatch_typed(&host(), &fence(&over_a, 50))
            .await
            .unwrap();
        assert_eq!(
            refused.map(|decision| decision.call.state),
            Err(BudgetPreDispatchRefusal::SourceRevoked),
            "{phase:?}"
        );
        assert!(
            matches!(
                store
                    .begin_budget_dispatch(&host(), &fence(&over_a, 50))
                    .await,
                Err(Error::Cancelled)
            ),
            "{phase:?}"
        );
        assert_nothing_written(&before, &all_rows(&path).await, &format!("{phase:?}"));
        assert_eq!(
            call_of(&store, "call-f-a").await.state,
            BudgetCallState::Reserved
        );
        let decision = store
            .begin_budget_dispatch_typed(&host(), &fence(&over_b, 51))
            .await
            .unwrap()
            .unwrap_or_else(|refusal| panic!("{phase:?}: refused {refusal:?}"));
        assert!(decision.new_dispatch, "{phase:?}");
        assert_eq!(
            decision.call.state,
            BudgetCallState::Dispatched,
            "{phase:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// G: a reference that does not decode
// ---------------------------------------------------------------------------

/// The diagnostic the cleanup isolates a formal call under when its reference does
/// not decode.
const UNREADABLE_REF: &str = "unreadable_budget_call_ref:rsia.formal_task_request.v1";

/// Test-only damage: stores a body that is JSON but not a budget call reference
/// as the reference of `call_id` (as a damaged file would leave it). Nothing else
/// is written.
async fn damage_ref(path: &Path, call_id: &str) {
    let mut connection = writable(path).await;
    let updated =
        sqlx::query("UPDATE objects SET body=? WHERE namespace=? AND kind='artifact' AND id=?")
            .bind(r#"{"x":1}"#)
            .bind(NS)
            .bind(ref_id(call_id))
            .execute(&mut connection)
            .await
            .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    connection.close().await.unwrap();
}

/// Both legacy dispatch entry points refuse `call` with `Internal` and write
/// nothing: a reference that does not decode is neither trusted nor guessed at.
async fn assert_gate_internal(store: &Store, path: &Path, call: &BudgetCallRecord, what: &str) {
    let before = all_rows(path).await;
    let typed = store
        .begin_budget_dispatch_typed(&host(), &fence(call, 60))
        .await;
    assert!(
        matches!(typed, Err(Error::Internal)),
        "{what}: typed dispatch answered {typed:?}"
    );
    let legacy = store.begin_budget_dispatch(&host(), &fence(call, 60)).await;
    assert!(
        matches!(legacy, Err(Error::Internal)),
        "{what}: dispatch answered {legacy:?}"
    );
    assert_nothing_written(&before, &all_rows(path).await, what);
}

/// G. Four formal calls, in scan order: over `run-a`, one whose reference no longer
/// decodes (registered over `run-b`, so the reference is not itself a dependent of
/// `run-a` and the cleanup has no reason to touch it), over `run-a` again, and over
/// `run-b`. The cleanup isolates the damaged call alone (`blocked_unknown_scope` on
/// it, `last_error` = `UNREADABLE_REF`) and does not read its request in place of
/// the reference; the two calls over `run-a` are redacted on either side of it, the
/// call over `run-b` is left as it is, and the job is `Failed`. An Admin retry
/// isolates the damaged call again and nothing else. Its row and its reference keep
/// their bytes throughout. The dispatch gate refuses the damaged call with
/// `Internal` and writes nothing, before the revocation and after the cleanup.
/// Baseline: `Failed` with `last_error` = `FORMAL_UNKNOWN`, every formal call
/// isolated and none redacted (the gate assertions hold on the baseline).
#[tokio::test]
async fn g_a_call_whose_reference_does_not_decode_is_isolated_and_the_rest_cleaned() {
    let (_dir, path, store) = database().await;
    put_run(&store, RUN_A).await;
    put_run(&store, RUN_B).await;
    authorize(&store).await;
    reserve(&store, "call-1-a", formal_request("call-1-a"), &[RUN_A]).await;
    let bad = reserve(&store, "call-2-bad", formal_request("call-2-bad"), &[RUN_B]).await;
    reserve(&store, "call-3-a", formal_request("call-3-a"), &[RUN_A]).await;
    reserve(&store, "call-4-b", formal_request("call-4-b"), &[RUN_B]).await;
    damage_ref(&path, "call-2-bad").await;
    let bad_row = call_row(&path, "call-2-bad").await;
    let bad_ref = object_body(&path, "artifact", &ref_id("call-2-bad")).await;
    assert_eq!(bad_ref.as_deref(), Some(r#"{"x":1}"#));
    let mut cleaned = BTreeMap::new();
    for call_id in ["call-1-a", "call-3-a"] {
        cleaned.insert(
            call_id,
            (
                ledger_of(&path, call_id).await,
                object_body(&path, "artifact", &ref_id(call_id)).await,
            ),
        );
    }
    let other = call_row(&path, "call-4-b").await;
    let money = micros(&store).await;
    assert_gate_internal(&store, &path, &bad, "before the revocation").await;

    let status = revoke(&store, RUN_A).await;
    let mut clock = 30;
    let failed = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_eq!(failed.state, CleanupState::Failed, "{failed:?}");
    assert_eq!(
        failed.last_error.as_deref(),
        Some(UNREADABLE_REF),
        "{failed:?}"
    );
    assert_eq!(failed.pending_nodes, 0, "{failed:?}");
    assert_eq!(
        cleanup_events(&path, &status.job_id, "blocked_unknown_scope").await,
        vec![isolation_of("call-2-bad", UNREADABLE_REF)]
    );
    for (call_id, (ledger, reference)) in &cleaned {
        assert_formal_call_cleaned(&store, &path, call_id, ledger, reference).await;
    }
    assert_eq!(call_row(&path, "call-4-b").await, other);
    assert_eq!(call_row(&path, "call-2-bad").await, bad_row);
    assert_eq!(
        object_body(&path, "artifact", &ref_id("call-2-bad")).await,
        bad_ref
    );
    assert_eq!(micros(&store).await, money);

    let retried = drive(&store, &status.job_id, 8, &mut clock).await;

    assert_eq!(retried.state, CleanupState::Failed, "{retried:?}");
    assert_eq!(retried.last_error.as_deref(), Some(UNREADABLE_REF));
    assert_eq!(
        cleanup_events(&path, &status.job_id, "cleanup_retry_started")
            .await
            .len(),
        1
    );
    assert_eq!(
        cleanup_events(&path, &status.job_id, "blocked_unknown_scope").await,
        vec![
            isolation_of("call-2-bad", UNREADABLE_REF),
            isolation_of("call-2-bad", UNREADABLE_REF),
        ]
    );
    for (call_id, (ledger, reference)) in &cleaned {
        assert_formal_call_cleaned(&store, &path, call_id, ledger, reference).await;
    }
    assert_eq!(call_row(&path, "call-4-b").await, other);
    assert_eq!(call_row(&path, "call-2-bad").await, bad_row);
    assert_eq!(
        object_body(&path, "artifact", &ref_id("call-2-bad")).await,
        bad_ref
    );
    assert_eq!(micros(&store).await, money);
    assert_gate_internal(&store, &path, &bad, "after the cleanup").await;
}
