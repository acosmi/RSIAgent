//! AG-043 (E08/§11, E04): a model response that reaches the ledger after one of
//! its sources was revoked is billed, unusable, and leaves no plaintext behind.
//!
//! Plan §11 says that when a source is deleted while a model request is in
//! flight, the late output can form no usable candidate while its real cost is
//! still booked ("迟到输出不能形成可用候选，但真实费用仍应入账"), and that the late
//! result of a call that was already dispatched cannot regain validity while its
//! cost is still reconciled ("已dispatch的晚到结果不可恢复有效性，费用仍对账"). The
//! deletion closure runs through Jobs, caches and host receipts, and an incomplete
//! deletion is never silent ("不静默漏删").
//!
//! The revocation cleanup only selects calls whose request is not redacted yet
//! (`redact_late_budget_calls`, `expand_budget_scan`). A call that was dispatched
//! before the revocation has its request redacted by the cleanup, or is about to,
//! and is never selected again, so the response that arrives after that met
//! nothing: `settle_model_budget_call` stored the transport body, which carries the
//! model output, in plaintext whatever the verdict was, and stored the response as
//! usable.
//!
//! The settlement now reads the source closure registered for the call (its
//! budget call reference) in the transaction that books the charge. When any
//! source was revoked as a `run`:
//!
//! * the charge is booked exactly as before (amount, usage id, provider request
//!   id, state, spent and reserved micros);
//! * the response is blocked with the reason `source_revoked`, so the usable
//!   response, which holds the output, is never stored;
//! * the transport body is stored as its `rsia.redacted.v1` redaction (the stored
//!   digest is the digest of the redaction, and the redaction names the digest of
//!   the body it replaces), whichever reason ends up on the response.
//!
//! "No plaintext" is read two ways: every row of every table through a second
//! read-only connection (the FTS shadow tables and the migration ledger included),
//! and the raw bytes of the database file and its write-ahead log, which also hold
//! pages that were written and then overwritten. Real SQLite store, no model, no
//! provider, and the only money is the fixture charge of the settlement.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlement, ModelCallSettlementEvidence,
    RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use sqlx::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SCOPE: &str = "scope-1";
const GROUP: &str = "group-1";
/// The run the tests revoke.
const SOURCE: &str = "run-r";
/// A run that is never revoked.
const LIVE: &str = "run-live";
const REASON: &str = "source revoked";
const MODEL_REQUEST_SCHEMA: &str = "rsia.model_request.artifact.v1";
const TRANSPORT_SCHEMA: &str = "rsia.model_transport.artifact.v1";
const RESPONSE_SCHEMA: &str = "rsia.model_response.artifact.v1";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";
/// The model output of the late response: plaintext that must not outlive the
/// revocation of a source the call was derived from. It appears nowhere else.
const OUTPUT: &str = "LATE-MODEL-OUTPUT-7c1e9a52";

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

/// The one artifact `begin_revoke` accepts as an artifact source: a strict E16
/// `import_source` envelope (the shape `tests/revocation_source_collision.rs` uses).
async fn put_import_source(store: &Store, id: &str) {
    raw_put(
        store,
        "artifact",
        id,
        &json!({
            "schema_version": IMPORT_SOURCE_SCHEMA,
            "id": id,
            "namespace": "n",
            "owner_actor": "admin",
            "request_key": format!("request-{id}"),
            "input_digest": hash(format!("input-{id}").as_bytes()),
            "created_at": 1,
            "updated_at": 1,
            "source_refs": [],
            "revoke_watermark": 0,
            "payload": {"raw_blob_digest": null, "status": "prepared"},
        }),
    )
    .await;
}

async fn revoke(store: &Store, kind: &str, id: &str) -> evo_storage::lifecycle::CleanupStatus {
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

/// A model request whose source closure is `sources`, carrying content a revoked
/// source must not leave behind.
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

fn reservation(call_id: &str, sources: &[&str]) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: call_id.into(),
        dispatch_group_id: GROUP.into(),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(model_request(call_id, sources)),
        max_cost_micros: 20,
        lease_token: format!("lease-{call_id}"),
        lease_until: 100,
        now: 10,
    }
}

fn owned(sources: &[&str]) -> Vec<String> {
    sources.iter().map(|id| (*id).to_string()).collect()
}

/// Reserves `call_id` over `sources` (a registered source closure).
async fn reserve(store: &Store, call_id: &str, sources: &[&str]) -> BudgetCallRecord {
    store
        .reserve_budget_call_with_sources(&host(), &reservation(call_id, sources), &owned(sources))
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

async fn dispatch(store: &Store, call: &BudgetCallRecord) -> BudgetCallRecord {
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(call, 11))
        .await
        .unwrap()
        .call;
    assert_eq!(dispatched.state, BudgetCallState::Dispatched);
    dispatched
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

/// The `(kind, id)` pairs that depend on the run `id` (a budget-ref is one).
async fn dependents_of(store: &Store, id: &str) -> Vec<(String, String)> {
    let mut session = store.session().await.unwrap();
    let dependents = session.dependents(&admin(), "run", id).await.unwrap();
    session.commit().await.unwrap();
    dependents
}

async fn read_only(path: &Path) -> sqlx::SqliteConnection {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

/// Every row of every table (the FTS shadow tables and the migration ledger
/// included), as text, through a second read-only connection.
async fn fingerprint(path: &Path) -> BTreeMap<String, Vec<String>> {
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

/// Where `needle` can be read in the database at `path`: in a row of a table (as
/// `fingerprint` reads it), or in the raw bytes of the database file or its
/// write-ahead log. The raw bytes also hold pages that were written and then
/// overwritten, so a value that was stored and redacted afterwards is found too.
/// Empty when it is nowhere.
async fn found_in(path: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (table, rows) in fingerprint(path).await {
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

/// How many budget call references (the objects that register a call's source
/// closure) the database holds.
async fn budget_refs(path: &Path) -> i64 {
    let mut connection = read_only(path).await;
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects WHERE kind='artifact' AND id LIKE 'budget-ref-%'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    count
}

// ---------------------------------------------------------------------------
// The late response
// ---------------------------------------------------------------------------

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

/// The evidence of a late response in the shapes the persistent broker stores: a
/// transport body that carries the model output (`TransportCompletion`), the usable
/// response that carries it too (`ModelResponse::Completed`) and the blocked
/// response, which names no output (`ModelResponse::Rejected`).
fn late_evidence(call_id: &str, output: &str) -> ModelCallSettlementEvidence {
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
                "output": output,
                "actual_cost_micros": 10,
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
                    "output": output,
                    "output_digest": hash(output.as_bytes()),
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

/// How far the revocation cleanup has got when the late response arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    /// `begin_revoke` only: the tombstone is committed and no cleanup step has run.
    NotStarted,
    /// One step has run and the job is `Running`, but the budget scan has not read
    /// the call yet, so its request is still unredacted.
    RunningBeforeTheCall,
    /// One step has run and redacted the call's request; the job is still `Running`.
    RunningAfterTheCall,
    /// The job is `Complete`.
    Complete,
}

/// One call dispatched over `closure` before the revocation of `SOURCE`, with the
/// cleanup `cleanup` far along; its response has not arrived.
struct Scenario {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    job_id: String,
    /// The call as the dispatch left it, before the revocation.
    dispatched: BudgetCallRecord,
    /// Micros the root still holds for the other reservation of the scenario.
    held: i64,
}

async fn scenario(cleanup: Cleanup, closure: &[&str]) -> Scenario {
    let (dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let bystander = cleanup == Cleanup::RunningBeforeTheCall;
    if bystander {
        // Reserved over a live run and never dispatched. It sorts before `call-1`, so
        // a budget scan that reads one row per step stops before the call.
        reserve(&store, "call-0", &[LIVE]).await;
    }
    let call = reserve(&store, "call-1", closure).await;
    let dispatched = dispatch(&store, &call).await;
    let status = revoke(&store, "run", SOURCE).await;
    assert_eq!(status.state, CleanupState::Pending);
    match cleanup {
        Cleanup::NotStarted => {}
        Cleanup::RunningBeforeTheCall | Cleanup::RunningAfterTheCall => {
            let status = LifecycleStore::cleanup_step(&admin(), &store, &status.job_id, 1, 6)
                .await
                .unwrap();
            assert_eq!(
                status.state,
                CleanupState::Running,
                "{cleanup:?}: {status:?}"
            );
        }
        Cleanup::Complete => complete_cleanup(&store, &status.job_id).await,
    }
    // The state the late response meets: in flight, with the request redacted or not.
    let in_flight = call_of(&store, "call-1").await;
    assert_eq!(in_flight.state, BudgetCallState::Dispatched, "{cleanup:?}");
    assert!(in_flight.transport_artifact.is_none() && in_flight.response_artifact.is_none());
    let request_redacted = in_flight.request_artifact.unwrap().schema_version == REDACTED_SCHEMA;
    assert_eq!(
        request_redacted,
        matches!(cleanup, Cleanup::RunningAfterTheCall | Cleanup::Complete),
        "{cleanup:?}: request redacted = {request_redacted}"
    );
    Scenario {
        _dir: dir,
        path,
        store,
        job_id: status.job_id,
        dispatched,
        held: if bystander { 20 } else { 0 },
    }
}

impl Scenario {
    /// The late response of `call-1` arrives and is settled at `now`.
    async fn settle_at(
        &self,
        now: i64,
        charge: &UsageCharge,
        evidence: &ModelCallSettlementEvidence,
    ) -> Result<ModelCallSettlement, Error> {
        self.store
            .settle_model_budget_call(&host(), &fence(&self.dispatched, now), charge, evidence)
            .await
    }

    async fn settle(&self, evidence: &ModelCallSettlementEvidence) -> ModelCallSettlement {
        self.settle_at(20, &late_charge("call-1", 10), evidence)
            .await
            .unwrap()
    }
}

/// The accounting facts of a settled call, which a revocation never reverses (§11:
/// money, the outbound action and history are not reversed by a deletion).
async fn assert_booked(s: &Scenario, call: &BudgetCallRecord, what: &str) {
    assert_eq!(call.state, BudgetCallState::Finalized, "{what}");
    assert_eq!(call.actual_cost_micros, Some(10), "{what}");
    assert_eq!(call.actual_currency.as_deref(), Some("USD"), "{what}");
    assert_eq!(
        call.actual_pricing_version.as_deref(),
        Some("price-v1"),
        "{what}"
    );
    assert_eq!(
        call.provider_request_id.as_deref(),
        Some("provider-call-1"),
        "{what}"
    );
    assert_eq!(
        call.usage_record_id.as_deref(),
        Some("usage-call-1"),
        "{what}"
    );
    assert_eq!(call.output_digest, Some(hash(OUTPUT.as_bytes())), "{what}");
    assert_eq!(call.dispatch_id, s.dispatched.dispatch_id, "{what}");
    assert_eq!(call.finalized_at, Some(20), "{what}");
    assert_eq!(call.terminal_reason, None, "{what}");
    assert_eq!(
        call.execution_provenance,
        Some(BudgetExecutionProvenance::Fixture),
        "{what}"
    );
    assert_eq!(call.actual_model_digest, Some(hash(b"model")), "{what}");
    assert_eq!(micros(&s.store).await, (s.held, 10), "{what}");
}

/// What is wrong with the late response of a revoked source, as the settlement
/// returned it. Empty when the response handed back is the blocked one.
fn returned_defects(
    settled: &ModelCallSettlement,
    evidence: &ModelCallSettlementEvidence,
) -> Vec<String> {
    let mut defects = Vec::new();
    if settled.response_usable {
        defects.push("the settlement returns a usable response".to_string());
    }
    if settled.response_block_reason.as_deref() != Some("source_revoked") {
        defects.push(format!(
            "the settlement returns the block reason {:?}, not source_revoked",
            settled.response_block_reason
        ));
    }
    if settled.response_artifact != evidence.blocked_response {
        defects.push(format!(
            "the settlement returns a response other than the blocked one: {}",
            settled.response_artifact.body
        ));
    }
    defects
}

/// What is wrong with the stored late response of a revoked source: empty when it
/// is unusable, its transport body is the redaction of the one the provider sent,
/// and the model output is nowhere in the database.
async fn stored_defects(
    path: &Path,
    call: &BudgetCallRecord,
    evidence: &ModelCallSettlementEvidence,
) -> Vec<String> {
    let mut defects = Vec::new();
    if call.response_usable != Some(false) {
        defects.push(format!("response_usable is {:?}", call.response_usable));
    }
    if call.response_block_reason.as_deref() != Some("source_revoked") {
        defects.push(format!(
            "response_block_reason is {:?}, not source_revoked",
            call.response_block_reason
        ));
    }
    match &call.transport_artifact {
        Some(transport) if transport.schema_version == REDACTED_SCHEMA => {
            let body: Value = serde_json::from_str(&transport.body).unwrap();
            if body["state"] != "source_revoked"
                || body["original_schema"] != TRANSPORT_SCHEMA
                || body["original_digest"] != evidence.transport_artifact.digest
            {
                defects.push(format!(
                    "the redacted transport does not name the transport it replaces: {body}"
                ));
            }
        }
        other => defects.push(format!(
            "the transport body is stored as {:?}, not redacted",
            other.as_ref().map(|artifact| &artifact.schema_version)
        )),
    }
    if call
        .response_artifact
        .as_ref()
        .is_none_or(|response| response.body.contains(OUTPUT))
    {
        defects.push("the stored response is missing or holds the output".to_string());
    }
    let found = found_in(path, OUTPUT).await;
    if !found.is_empty() {
        defects.push(format!("the model output is readable in {found:?}"));
    }
    defects
}

/// One late response: settled while the cleanup is at `cleanup`, then the cleanup
/// is run to the end. Asserts the books, the verdict and the plaintext both times.
async fn check_late_response(cleanup: Cleanup, closure: &[&str]) {
    let what = format!("{cleanup:?} over {closure:?}");
    let s = scenario(cleanup, closure).await;
    let evidence = late_evidence("call-1", OUTPUT);

    let settled = s.settle(&evidence).await;
    let stored = call_of(&s.store, "call-1").await;
    assert_eq!(
        settled.call, stored,
        "{what}: returned and stored row differ"
    );
    assert_booked(&s, &stored, &what).await;
    let mut defects = returned_defects(&settled, &evidence);
    defects.extend(stored_defects(&s.path, &stored, &evidence).await);
    assert!(
        defects.is_empty(),
        "{what}: the late response was not blocked and redacted:\n{defects:#?}"
    );

    // Whatever the cleanup does afterwards changes none of it.
    complete_cleanup(&s.store, &s.job_id).await;
    let finished = call_of(&s.store, "call-1").await;
    assert_booked(&s, &finished, &format!("{what}, after the cleanup")).await;
    let defects = stored_defects(&s.path, &finished, &evidence).await;
    assert!(
        defects.is_empty(),
        "{what}, after the cleanup:\n{defects:#?}"
    );
    assert_eq!(
        finished.request_artifact.unwrap().schema_version,
        REDACTED_SCHEMA
    );
}

// ---------------------------------------------------------------------------
// A response over a revoked source is billed, unusable and leaves no plaintext
// ---------------------------------------------------------------------------

/// Dispatched before the revocation, the revocation, the cleanup to `Complete`,
/// then the response arrives: its cost is booked, it is unusable
/// (`source_revoked`), its transport body is redacted, and the output is nowhere in
/// the database. The source is the whole closure, or one of two.
#[tokio::test]
async fn a_late_response_is_billed_unusable_and_unreadable_once_the_cleanup_has_completed() {
    for closure in [&[SOURCE][..], &[LIVE, SOURCE][..]] {
        check_late_response(Cleanup::Complete, closure).await;
    }
}

/// The same, while the cleanup is still `Running`: the call's request is not read
/// by the budget scan yet, or already redacted by it.
#[tokio::test]
async fn a_late_response_is_billed_unusable_and_unreadable_while_the_cleanup_is_running() {
    for cleanup in [Cleanup::RunningBeforeTheCall, Cleanup::RunningAfterTheCall] {
        check_late_response(cleanup, &[SOURCE]).await;
    }
}

/// The same with the tombstone committed and no cleanup step run: the revocation is
/// read from the tombstone, not from what the cleanup has done.
#[tokio::test]
async fn a_late_response_is_billed_unusable_and_unreadable_before_any_cleanup_step() {
    for closure in [&[SOURCE][..], &[LIVE, SOURCE][..]] {
        check_late_response(Cleanup::NotStarted, closure).await;
    }
}

/// The cleanup that reaches the call after the settlement redacts the redacted
/// transport again. The redaction of a redaction would leave only the digest of the
/// first redaction, so the digest of the transport the provider sent would be gone
/// from the row: a redacted artifact is left as it is.
#[tokio::test]
async fn redacting_a_redacted_transport_keeps_the_digest_of_the_original() {
    let s = scenario(Cleanup::RunningBeforeTheCall, &[SOURCE]).await;
    let evidence = late_evidence("call-1", OUTPUT);
    s.settle(&evidence).await;
    let settled = call_of(&s.store, "call-1").await;
    complete_cleanup(&s.store, &s.job_id).await;
    let finished = call_of(&s.store, "call-1").await;

    assert_eq!(
        finished.transport_artifact, settled.transport_artifact,
        "the cleanup replaced the redacted transport"
    );
    let transport = finished.transport_artifact.unwrap();
    let body: Value = serde_json::from_str(&transport.body).unwrap();
    assert_eq!(body["original_schema"], TRANSPORT_SCHEMA);
    assert_eq!(body["original_digest"], evidence.transport_artifact.digest);
    // the response, which held no output, is the one thing the cleanup redacts here
    let response = finished.response_artifact.unwrap();
    assert_eq!(response.schema_version, REDACTED_SCHEMA);
    assert_eq!(finished.response_usable, Some(false));
    assert_eq!(
        finished.response_block_reason.as_deref(),
        Some("source_revoked")
    );
}

/// The row stays readable (`artifact_from_row` rechecks every artifact digest), the
/// stored digest is the digest of what is stored, and the redaction names the
/// digest of the body it replaces: that is how the digest and the redacted body
/// stay consistent. The event that records the settlement does the same.
#[tokio::test]
async fn the_stored_digest_is_that_of_the_redaction_and_the_event_names_the_original() {
    let s = scenario(Cleanup::Complete, &[SOURCE]).await;
    let evidence = late_evidence("call-1", OUTPUT);
    s.settle(&evidence).await;
    let call = call_of(&s.store, "call-1").await;

    let transport = call.transport_artifact.as_ref().unwrap();
    assert_eq!(transport.schema_version, REDACTED_SCHEMA);
    assert_eq!(transport.digest, hash(transport.body.as_bytes()));
    assert_ne!(transport.digest, evidence.transport_artifact.digest);

    let events = events_of(&s.path, "call-1", "model_response_persisted").await;
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(event["transport_artifact_digest"], transport.digest);
    assert_eq!(
        event["transport_original_digest"],
        evidence.transport_artifact.digest
    );
    assert_eq!(event["source_revoked"], true);
    assert_eq!(event["response_usable"], false);
    assert_eq!(event["response_block_reason"], "source_revoked");
    assert_eq!(
        event["response_artifact_digest"],
        evidence.blocked_response.digest
    );
}

// ---------------------------------------------------------------------------
// Regressions: a live source settles exactly as before
// ---------------------------------------------------------------------------

/// A call over a source that is not revoked: the response is usable and what is
/// stored is the evidence, byte for byte (schema, digest and body), the output
/// included. The same holds when some other run was revoked, and its cleanup
/// completed, around the call.
#[tokio::test]
async fn a_response_over_a_live_source_stays_usable_and_is_stored_unchanged() {
    for revoked_elsewhere in [false, true] {
        let (_dir, path, store) = database().await;
        put_run(&store, SOURCE).await;
        put_run(&store, LIVE).await;
        authorize(&store).await;
        let call = reserve(&store, "call-1", &[LIVE]).await;
        let dispatched = dispatch(&store, &call).await;
        if revoked_elsewhere {
            let status = revoke(&store, "run", SOURCE).await;
            complete_cleanup(&store, &status.job_id).await;
        }

        let evidence = late_evidence("call-1", OUTPUT);
        let settled = store
            .settle_model_budget_call(
                &host(),
                &fence(&dispatched, 20),
                &late_charge("call-1", 10),
                &evidence,
            )
            .await
            .unwrap();
        let what = format!("revoked elsewhere: {revoked_elsewhere}");
        assert!(settled.response_usable, "{what}");
        assert_eq!(settled.response_block_reason, None, "{what}");
        assert_eq!(
            Some(&settled.response_artifact),
            evidence.usable_response.as_ref(),
            "{what}"
        );
        let stored = call_of(&store, "call-1").await;
        assert_eq!(stored, settled.call, "{what}");
        assert_eq!(stored.response_usable, Some(true), "{what}");
        assert_eq!(stored.response_block_reason, None, "{what}");
        assert_eq!(
            stored.transport_artifact.as_ref(),
            Some(&evidence.transport_artifact),
            "{what}"
        );
        assert_eq!(
            stored.response_artifact.as_ref(),
            evidence.usable_response.as_ref(),
            "{what}"
        );
        assert_eq!(stored.state, BudgetCallState::Finalized, "{what}");
        assert_eq!(stored.actual_cost_micros, Some(10), "{what}");
        assert_eq!(micros(&store).await, (0, 10), "{what}");
        // The control of the plaintext scans above: they do see a stored output.
        let found = found_in(&path, OUTPUT).await;
        assert!(
            found
                .iter()
                .any(|place| place.starts_with("table root_budget_calls")),
            "{what}: the scan does not see a live output: {found:?}"
        );
        // and the event is the one it always was
        let events = events_of(&path, "call-1", "model_response_persisted").await;
        assert_eq!(events.len(), 1, "{what}");
        assert_eq!(
            events[0]["transport_artifact_digest"], evidence.transport_artifact.digest,
            "{what}"
        );
        assert!(
            events[0].get("source_revoked").is_none(),
            "{what}: {}",
            events[0]
        );
        assert_eq!(events[0]["response_usable"], true, "{what}");
    }
}

/// A revocation tombstone is keyed by the id of its source alone and records the
/// kind it was written for. The sources of a budget call are runs, so the tombstone
/// of an artifact that happens to carry the id of a live run revokes nothing the
/// call was derived from: the response stays usable (AG-032, AG-036, AG-038).
#[tokio::test]
async fn a_tombstone_of_an_artifact_with_the_same_id_does_not_block_the_response() {
    let (_dir, path, store) = database().await;
    // Both exist only because the test writes them around the production creators,
    // which refuse the pair (AG-036).
    put_run(&store, "shared-id").await;
    put_import_source(&store, "shared-id").await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &["shared-id"]).await;
    let dispatched = dispatch(&store, &call).await;
    revoke(&store, "artifact", "shared-id").await;
    let mut session = store.session().await.unwrap();
    let tombstone: evo_storage::lifecycle::RevokeTombstone = session
        .need(&admin(), "tombstone", "shared-id")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(tombstone.source_kind, "artifact");

    let evidence = late_evidence("call-1", OUTPUT);
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 10),
            &evidence,
        )
        .await
        .unwrap();
    assert!(settled.response_usable);
    assert_eq!(settled.response_block_reason, None);
    let stored = call_of(&store, "call-1").await;
    assert_eq!(stored.response_usable, Some(true));
    assert_eq!(
        stored.transport_artifact.as_ref(),
        Some(&evidence.transport_artifact)
    );
    assert_eq!(
        stored.response_artifact.as_ref(),
        evidence.usable_response.as_ref()
    );
    assert_eq!(micros(&store).await, (0, 10));
    assert!(!found_in(&path, OUTPUT).await.is_empty());
}

// ---------------------------------------------------------------------------
// What else decides the verdict, and the replay of a settlement
// ---------------------------------------------------------------------------

/// The settlement of a late response is idempotent, as before: the same charge and
/// the same evidence give the same settlement and write nothing; another charge,
/// or other evidence, is refused. The transport the provider sent is recognised
/// under its redaction, and only under its own: another transport is not.
#[tokio::test]
async fn a_settlement_replayed_after_a_revoked_settlement_is_idempotent() {
    let s = scenario(Cleanup::Complete, &[SOURCE]).await;
    let evidence = late_evidence("call-1", OUTPUT);
    let first = s.settle(&evidence).await;
    let before = fingerprint(&s.path).await;

    let again = s
        .settle_at(21, &late_charge("call-1", 10), &evidence)
        .await
        .unwrap();
    assert_eq!(again.call, first.call);
    assert_eq!(again.response_artifact, first.response_artifact);
    assert!(!again.response_usable);
    assert_eq!(
        again.response_block_reason.as_deref(),
        Some("source_revoked")
    );
    assert_nothing_written(&before, &fingerprint(&s.path).await, "replayed settlement");

    let other_charge = s.settle_at(22, &late_charge("call-1", 11), &evidence).await;
    assert!(
        matches!(other_charge, Err(Error::Conflict(_))),
        "another charge: {other_charge:?}"
    );
    let other_output = s
        .settle_at(
            23,
            &late_charge("call-1", 10),
            &late_evidence("call-1", "ANOTHER-OUTPUT-31d0"),
        )
        .await;
    assert!(
        matches!(other_output, Err(Error::Conflict(_))),
        "another transport: {other_output:?}"
    );
    assert_nothing_written(&before, &fingerprint(&s.path).await, "refused replays");
    assert!(found_in(&s.path, "ANOTHER-OUTPUT-31d0").await.is_empty());
}

/// The reason on the response follows the chain it always had (pricing identity,
/// the caller's forced reason, accounting overflow, cost overrun) and only the
/// reasons that were already there before the response was judged
/// (`preexisting_block`: source revoked, lease expired, root stopped, dispatch
/// group stopped) gain `source_revoked` at their head. The transport body is
/// redacted when any source is revoked, whichever reason wins, and the output is
/// never stored.
#[tokio::test]
async fn the_transport_is_redacted_whichever_reason_wins_and_the_books_keep_their_rules() {
    // (what, charge, evidence tweak, settle at, expected reason, expected state,
    //  expected (reserved, spent) with the call settled)
    struct Case {
        what: &'static str,
        charge: UsageCharge,
        forced: Option<&'static str>,
        at: i64,
        reason: &'static str,
        state: BudgetCallState,
        micros: (i64, i64),
    }
    let cases = [
        Case {
            what: "the caller forces a reason",
            charge: late_charge("call-1", 10),
            forced: Some("actual_model_digest_mismatch"),
            at: 20,
            reason: "actual_model_digest_mismatch",
            state: BudgetCallState::Finalized,
            micros: (0, 10),
        },
        Case {
            what: "the cost overruns the reservation",
            charge: late_charge("call-1", 30),
            forced: None,
            at: 20,
            reason: "cost_overrun",
            state: BudgetCallState::Finalized,
            micros: (0, 30),
        },
        Case {
            what: "the charge has another pricing identity",
            charge: UsageCharge {
                currency: "EUR".into(),
                ..late_charge("call-1", 10)
            },
            forced: None,
            at: 20,
            reason: "charge_pricing_identity_mismatch",
            state: BudgetCallState::Uncertain,
            micros: (20, 0),
        },
        Case {
            what: "the lease has expired as well",
            charge: late_charge("call-1", 10),
            forced: None,
            at: 150,
            reason: "source_revoked",
            state: BudgetCallState::Finalized,
            micros: (0, 10),
        },
    ];
    for case in cases {
        let s = scenario(Cleanup::Complete, &[SOURCE]).await;
        let evidence = ModelCallSettlementEvidence {
            forced_block_reason: case.forced.map(str::to_string),
            ..late_evidence("call-1", OUTPUT)
        };
        let settled = s
            .settle_at(case.at, &case.charge, &evidence)
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", case.what));
        let stored = call_of(&s.store, "call-1").await;
        assert_eq!(settled.call, stored, "{}", case.what);
        assert_eq!(stored.state, case.state, "{}", case.what);
        assert_eq!(stored.response_usable, Some(false), "{}", case.what);
        assert_eq!(
            stored.response_block_reason.as_deref(),
            Some(case.reason),
            "{}",
            case.what
        );
        assert_eq!(
            stored.actual_cost_micros,
            Some(case.charge.amount_micros),
            "{}",
            case.what
        );
        assert_eq!(micros(&s.store).await, case.micros, "{}", case.what);
        let transport = stored.transport_artifact.as_ref().unwrap();
        assert_eq!(transport.schema_version, REDACTED_SCHEMA, "{}", case.what);
        assert!(!transport.body.contains(OUTPUT), "{}", case.what);
        assert!(
            found_in(&s.path, OUTPUT).await.is_empty(),
            "{}: the output is readable",
            case.what
        );
    }

    // A root or a dispatch group that was stopped does not hide the revocation
    // either: `source_revoked` is the reason at the head of the preexisting ones.
    for stop_root in [true, false] {
        let s = scenario(Cleanup::Complete, &[SOURCE]).await;
        if stop_root {
            s.store
                .stop_root_budget(&admin(), SCOPE, "operator", 12)
                .await
                .unwrap();
        } else {
            s.store
                .stop_dispatch_group(&admin(), SCOPE, GROUP, "operator", 12)
                .await
                .unwrap();
        }
        let evidence = late_evidence("call-1", OUTPUT);
        let settled = s.settle(&evidence).await;
        let what = if stop_root {
            "root stopped"
        } else {
            "group stopped"
        };
        assert_eq!(
            settled.response_block_reason.as_deref(),
            Some("source_revoked"),
            "{what}"
        );
        let stored = call_of(&s.store, "call-1").await;
        let defects = stored_defects(&s.path, &stored, &evidence).await;
        assert!(defects.is_empty(), "{what}:\n{defects:#?}");
        assert_eq!(stored.actual_cost_micros, Some(10), "{what}");
    }
}

/// The closure registered for the call is read here, in the transaction that books
/// the charge, and one that cannot be read is neither trusted nor guessed at (the
/// same answer the dispatch gives, `Internal`): nothing is written, so the output
/// is not stored either, and the call stays dispatched, so its real cost can still
/// be reconciled and booked.
#[tokio::test]
async fn an_unreadable_source_closure_fails_the_settlement_and_leaves_the_cost_reconcilable() {
    let (_dir, path, store) = database().await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &[LIVE]).await;
    let dispatched = dispatch(&store, &call).await;
    let references = dependents_of(&store, LIVE).await;
    assert_eq!(references.len(), 1, "{references:?}");
    let (kind, reference_id) = &references[0];
    assert_eq!(kind, "artifact");
    raw_put(
        &store,
        "artifact",
        reference_id,
        &json!({"id": reference_id, "garbage": true}),
    )
    .await;
    let before = fingerprint(&path).await;

    let result = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 10),
            &late_evidence("call-1", OUTPUT),
        )
        .await;
    assert!(
        matches!(result, Err(Error::Internal)),
        "expected Internal, got {result:?}"
    );
    assert_nothing_written(&before, &fingerprint(&path).await, "the refused settlement");
    assert_eq!(
        call_of(&store, "call-1").await.state,
        BudgetCallState::Dispatched
    );
    assert!(found_in(&path, OUTPUT).await.is_empty());

    let reconciled = store
        .reconcile_budget_call_cost(&admin(), SCOPE, "call-1", &late_charge("call-1", 10), 21)
        .await
        .unwrap();
    assert_eq!(reconciled.state, BudgetCallState::Finalized);
    assert_eq!(reconciled.actual_cost_micros, Some(10));
    assert_eq!(reconciled.response_artifact, None);
    assert_eq!(micros(&store).await, (0, 10));
}

// ---------------------------------------------------------------------------
// A call that carries no registered closure
// ---------------------------------------------------------------------------

/// A call reserved without a source list (`reserve_budget_call`; the evaluator
/// reserves its calls this way) has no budget call reference for the settlement to
/// read, so its settlement is what it was: usable, stored as sent. The cleanup
/// rescan is its backstop. It finds the call by the closure its request body names
/// and redacts what the call holds, which the first half shows. The second half
/// shows that a call the rescan has already reached is covered at settlement: the
/// rescan registers the closure it read before it redacts the request.
///
/// A call with no registered closure that the rescan can not reach (its request
/// names no revoked source, or it was reserved after the cleanup completed) is
/// still not covered; that boundary is not claimed.
#[tokio::test]
async fn a_call_without_a_registered_closure_is_settled_as_before_and_the_rescan_is_its_backstop() {
    // The response arrives before the rescan reaches the call.
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = store
        .reserve_budget_call(&host(), &reservation("call-1", &[SOURCE]))
        .await
        .unwrap();
    assert_eq!(budget_refs(&path).await, 0, "a closure is registered");
    let dispatched = dispatch(&store, &call).await;
    let status = revoke(&store, "run", SOURCE).await;
    let evidence = late_evidence("call-1", OUTPUT);
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 10),
            &evidence,
        )
        .await
        .unwrap();
    assert!(
        settled.response_usable,
        "nothing to read: settled as before"
    );
    assert!(!found_in(&path, OUTPUT).await.is_empty());

    complete_cleanup(&store, &status.job_id).await;
    let finished = call_of(&store, "call-1").await;
    assert_eq!(finished.response_usable, Some(false));
    assert_eq!(
        finished.response_block_reason.as_deref(),
        Some("source_revoked")
    );
    for artifact in [
        finished.request_artifact.as_ref(),
        finished.transport_artifact.as_ref(),
        finished.response_artifact.as_ref(),
    ] {
        assert_eq!(artifact.unwrap().schema_version, REDACTED_SCHEMA);
    }
    assert!(
        found_in(&path, OUTPUT)
            .await
            .iter()
            .all(|place| place.starts_with("file")),
        "the rows still hold the output: {:?}",
        found_in(&path, OUTPUT).await
    );
    assert_eq!(finished.actual_cost_micros, Some(10));
    assert_eq!(micros(&store).await, (0, 10));

    // The rescan reaches the call first: it registers the closure, so the response
    // that arrives afterwards is covered.
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = store
        .reserve_budget_call(&host(), &reservation("call-1", &[SOURCE]))
        .await
        .unwrap();
    let dispatched = dispatch(&store, &call).await;
    let status = revoke(&store, "run", SOURCE).await;
    complete_cleanup(&store, &status.job_id).await;
    assert_eq!(
        budget_refs(&path).await,
        1,
        "the rescan registered no closure"
    );
    let settled = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &late_charge("call-1", 10),
            &evidence,
        )
        .await
        .unwrap();
    assert!(!settled.response_usable);
    assert_eq!(
        settled.response_block_reason.as_deref(),
        Some("source_revoked")
    );
    let stored = call_of(&store, "call-1").await;
    let defects = stored_defects(&path, &stored, &evidence).await;
    assert!(defects.is_empty(), "{defects:#?}");
    assert_eq!(stored.actual_cost_micros, Some(10));
    assert_eq!(micros(&store).await, (0, 10));
}
