//! AG-038 (E08/§11, E04): a budget call over a revoked source is neither
//! reserved nor dispatched, and settlement stays on the books.
//!
//! Plan §11 says to write the revocation first so that new access is refused at
//! once; §11.3 and §11.5 say to block new generation, derivation and outbound
//! dispatch before the cleanup runs, while an already dispatched call keeps its
//! real cost on the books ("金额、外发动作和历史事实不因删除逆转", "已dispatch的晚到
//! 结果不可恢复有效性，费用仍对账").
//!
//! Before this change nothing in the budget ledger looked at a revocation
//! tombstone. A reservation over a revoked run was written (request body in
//! plaintext, a budget-ref object and a budget-ref -> run edge included), and a
//! call reserved before the revocation was still dispatched afterwards: the
//! cleanup only redacts the ledger copy, so the caller sent its in-memory request
//! to the provider anyway.
//!
//! The gates under test, all inside the one transaction that writes:
//!
//! * a new reservation that names a source revoked as a `run` is refused with
//!   `Forbidden` and nothing is written (an existing call is still returned, so
//!   recovery and reconciliation keep working);
//! * a reserved call whose registered source closure (its budget-ref) names a
//!   revoked run is not dispatched: `begin_budget_dispatch` answers `Cancelled`,
//!   the decision the broker already releases the reservation on;
//! * settlement, finalization, uncertainty, release, cancellation, execution
//!   close and re-fencing are not gated.
//!
//! A tombstone is keyed by the id of its source alone and records the kind it was
//! written for, so it decides a dependency only when that is the dependency's own
//! kind (a source of a budget call is a `run`); one that cannot be read as a
//! revocation fails closed (AG-032, AG-036).
//!
//! "Nothing is written" compares every row of every table, read through a second
//! read-only connection, before and after the refused call. Real SQLite store, no
//! model, no provider, zero monetary cost.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallRefence, BudgetCallReservation,
    BudgetCallState, BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence,
    REGISTERED_EXECUTION_SETTLEMENT_SCHEMA, RegisteredExecutionProvenance,
    RegisteredExecutionSettlement, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{
    CleanupState, CleanupStatus, LifecycleStore, RevokeTombstone, TypedObjectRef,
};
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
const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";

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

/// Every row of every table (the FTS shadow tables and the migration ledger
/// included), as text, through a second read-only connection: what a refused call
/// must leave exactly as it found it.
async fn fingerprint(path: &Path) -> BTreeMap<String, Vec<String>> {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
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

fn tables_written(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn assert_nothing_written(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
    what: &str,
) {
    let changed = tables_written(before, after);
    assert!(
        changed.is_empty(),
        "{what}: tables written: {changed:?}\nbefore: {:#?}\nafter: {:#?}",
        changed
            .iter()
            .map(|t| (t, before.get(t)))
            .collect::<Vec<_>>(),
        changed
            .iter()
            .map(|t| (t, after.get(t)))
            .collect::<Vec<_>>(),
    );
}

// ---------------------------------------------------------------------------
// The reservation gate
// ---------------------------------------------------------------------------

/// Only `begin_revoke` has run. A reservation whose source closure names the
/// revoked run, alone or among live ones, is refused with `Forbidden`, and the
/// refusal leaves every table as it was: no call row, no reserved micros, no
/// dispatch group, no budget-ref object, no edge, no event.
#[tokio::test]
async fn a_revoked_run_source_refuses_the_reservation_and_nothing_is_written() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    revoke(&store, "run", SOURCE).await;
    let before = fingerprint(&path).await;

    for sources in [&[SOURCE][..], &[LIVE, SOURCE][..]] {
        let result = store
            .reserve_budget_call_with_sources(
                &host(),
                &reservation("call-1", sources),
                &owned(sources),
            )
            .await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{sources:?}: expected Forbidden, got {result:?}"
        );
        assert_nothing_written(
            &before,
            &fingerprint(&path).await,
            &format!("refused reservation over {sources:?}"),
        );
    }
    assert!(
        store
            .budget_call(&host(), SCOPE, "call-1")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(micros(&store).await, (0, 0));
    assert!(
        store
            .dispatch_group(&host(), SCOPE, GROUP)
            .await
            .unwrap()
            .is_none()
    );
    assert!(dependents_of(&store, SOURCE).await.is_empty());
    assert!(dependents_of(&store, LIVE).await.is_empty());

    // The control: over a live source the same call is reserved, and that does
    // change the database, so the comparison above is not vacuous.
    let call = reserve(&store, "call-1", &[LIVE]).await;
    assert_eq!(call.state, BudgetCallState::Reserved);
    assert_ne!(before, fingerprint(&path).await);
    assert_eq!(micros(&store).await, (20, 0));
    assert!(
        store
            .dispatch_group(&host(), SCOPE, GROUP)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(dependents_of(&store, LIVE).await.len(), 1);
}

/// A call that already exists is returned again after the revocation, whatever
/// state it is in: recovery and reconciliation must still find it.
#[tokio::test]
async fn an_existing_call_is_returned_again_after_the_revoke() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    // `finished` is settled and closed before the revocation; `pending` is only
    // reserved.
    let finished = reserve(&store, "finished", &[SOURCE]).await;
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(&finished, 11))
        .await
        .unwrap()
        .call;
    let settled = store
        .finalize_budget_call(&host(), &fence(&dispatched, 12), &charge(7, "finished"))
        .await
        .unwrap();
    let finished = store
        .close_budget_call_execution(
            &host(),
            SCOPE,
            "finished",
            settled.dispatch_id.as_deref().unwrap(),
            "closed",
            13,
        )
        .await
        .unwrap();
    let pending = reserve(&store, "pending", &[SOURCE]).await;

    revoke(&store, "run", SOURCE).await;
    let before = fingerprint(&path).await;
    for (call_id, original) in [("finished", &finished), ("pending", &pending)] {
        let again = store
            .reserve_budget_call_with_sources(
                &host(),
                &reservation(call_id, &[SOURCE]),
                &owned(&[SOURCE]),
            )
            .await
            .unwrap();
        assert_eq!(&again, original, "{call_id}");
    }
    assert_nothing_written(
        &before,
        &fingerprint(&path).await,
        "replayed reservations after the revoke",
    );
    // a different call id over the same revoked source is new, and refused
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(
                &host(),
                &reservation("third", &[SOURCE]),
                &owned(&[SOURCE]),
            )
            .await,
        Err(Error::Forbidden)
    ));
}

/// A revocation tombstone is keyed by the id of its source alone and records which
/// kind it was written for. A run source is not refused by the tombstone of an
/// artifact that happens to carry its id (AG-032, AG-036).
#[tokio::test]
async fn a_tombstone_of_an_artifact_with_the_same_id_does_not_refuse_the_run_source() {
    let (_dir, _path, store) = database().await;
    // Both exist only because the test writes them around the production creators,
    // which refuse the pair (AG-036).
    put_run(&store, "shared-id").await;
    put_import_source(&store, "shared-id").await;
    authorize(&store).await;
    revoke(&store, "artifact", "shared-id").await;
    let mut session = store.session().await.unwrap();
    let tombstone: RevokeTombstone = session
        .need(&admin(), "tombstone", "shared-id")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(tombstone.source_kind, "artifact");

    let call = reserve(&store, "call-1", &["shared-id"]).await;
    assert_eq!(call.state, BudgetCallState::Reserved);
    let decision = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    assert_eq!(decision.call.state, BudgetCallState::Dispatched);
}

/// A tombstone that cannot be read as the revocation `begin_revoke` writes cannot
/// be matched to a kind and fails closed, at the reservation (`Forbidden`, nothing
/// written) and at the dispatch (`Cancelled`).
#[tokio::test]
async fn a_tombstone_that_cannot_be_read_fails_closed() {
    let (_dir, path, store) = database().await;
    authorize(&store).await;
    // The object table only requires valid JSON whose `$.id` is the id.
    raw_put(
        &store,
        "tombstone",
        "bad-shape",
        &json!({"id": "bad-shape", "garbage": true}),
    )
    .await;
    raw_put(
        &store,
        "tombstone",
        "bad-kind",
        &json!({
            "id": "bad-kind",
            "schema_version": "rsia.revoke_tombstone.v1",
            "source_kind": "blob",
            "reason": REASON,
            "watermark_seq": 1,
            "watermark_digest": hash(b"watermark"),
            "created_at": 1,
        }),
    )
    .await;
    let before = fingerprint(&path).await;
    for source in ["bad-shape", "bad-kind"] {
        let result = store
            .reserve_budget_call_with_sources(
                &host(),
                &reservation("call-1", &[source]),
                &owned(&[source]),
            )
            .await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{source}: expected Forbidden, got {result:?}"
        );
        assert_nothing_written(&before, &fingerprint(&path).await, source);
    }

    // Reserved while the tombstone did not exist yet, then the unreadable
    // tombstone appears: the dispatch is refused.
    for source in ["late-shape", "late-kind"] {
        let call_id = format!("call-{source}");
        let call = reserve(&store, &call_id, &[source]).await;
        let body = if source == "late-shape" {
            json!({"id": source, "garbage": true})
        } else {
            json!({
                "id": source,
                "schema_version": "rsia.revoke_tombstone.v1",
                "source_kind": "blob",
                "reason": REASON,
                "watermark_seq": 1,
                "watermark_digest": hash(b"watermark"),
                "created_at": 1,
            })
        };
        raw_put(&store, "tombstone", source, &body).await;
        let result = store
            .begin_budget_dispatch(&host(), &fence(&call, 11))
            .await;
        assert!(
            matches!(result, Err(Error::Cancelled)),
            "{source}: expected Cancelled, got {result:?}"
        );
        assert_eq!(
            call_of(&store, &call_id).await.state,
            BudgetCallState::Reserved
        );
    }
}

// ---------------------------------------------------------------------------
// The dispatch gate
// ---------------------------------------------------------------------------

/// Reserved over a live run, then the run is revoked: the dispatch is refused with
/// the decision the broker releases on, writes nothing, and the reservation can be
/// released, which returns the quota.
#[tokio::test]
async fn a_call_reserved_before_the_revoke_is_not_dispatched_after_it() {
    let (_dir, path, store) = database().await;
    put_run(&store, SOURCE).await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &[LIVE, SOURCE]).await;
    assert_eq!(micros(&store).await, (20, 0));

    revoke(&store, "run", SOURCE).await;
    let before = fingerprint(&path).await;
    let refused = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await;
    assert!(
        matches!(refused, Err(Error::Cancelled)),
        "expected Cancelled, got {refused:?}"
    );
    // The same decision inside a caller's transaction (the evaluator's path).
    let mut session = store.session().await.unwrap();
    let refused = session
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await;
    assert!(
        matches!(refused, Err(Error::Cancelled)),
        "session: expected Cancelled, got {refused:?}"
    );
    drop(session);
    assert_nothing_written(&before, &fingerprint(&path).await, "refused dispatch");
    let still = call_of(&store, "call-1").await;
    assert_eq!(still.state, BudgetCallState::Reserved);
    assert_eq!(still.dispatch_id, None);
    assert_eq!(still.dispatched_at, None);
    assert_eq!(micros(&store).await, (20, 0));

    // What the broker does with that decision: release the undispatched call.
    let released = store
        .release_undispatched_budget_call(&host(), &fence(&call, 12), "source_revoked")
        .await
        .unwrap();
    assert_eq!(released.state, BudgetCallState::Released);
    assert_eq!(released.dispatch_id, None);
    assert_eq!(micros(&store).await, (0, 0));
    // a released call is not dispatched by a repeat either
    let repeat = store
        .begin_budget_dispatch(&host(), &fence(&call, 13))
        .await
        .unwrap();
    assert!(!repeat.new_dispatch);
    assert_eq!(repeat.call.state, BudgetCallState::Released);

    // The control: a call over the live run only is dispatched.
    let live = reserve(&store, "call-2", &[LIVE]).await;
    let decision = store
        .begin_budget_dispatch(&host(), &fence(&live, 14))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    assert_eq!(decision.call.state, BudgetCallState::Dispatched);
    // The root runs one call at a time: settle and close it before the next.
    store
        .finalize_budget_call(&host(), &fence(&decision.call, 15), &charge(3, "live"))
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &host(),
            SCOPE,
            "call-2",
            decision.call.dispatch_id.as_deref().unwrap(),
            "closed",
            16,
        )
        .await
        .unwrap();
    // A call without a registered source closure (the evaluator reserves its
    // calls this way) has no source to judge and is dispatched as before.
    let bare = store
        .reserve_budget_call(
            &host(),
            &BudgetCallReservation {
                request_artifact: None,
                ..reservation("call-3", &[])
            },
        )
        .await
        .unwrap();
    let decision = store
        .begin_budget_dispatch(&host(), &fence(&bare, 17))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
}

/// The cleanup redacts the request content the ledger holds and deletes the run,
/// but it keeps the revocation tombstone and the call's source closure (a budget
/// call reference is a historical accounting fact), so both gates still refuse
/// once the cleanup is `Complete`.
#[tokio::test]
async fn the_gates_still_refuse_after_the_cleanup_has_completed() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &[SOURCE]).await;
    let status = revoke(&store, "run", SOURCE).await;
    complete_cleanup(&store, &status.job_id).await;

    let redacted = call_of(&store, "call-1").await;
    assert_eq!(
        redacted.request_artifact.unwrap().schema_version,
        "rsia.redacted.v1"
    );
    assert_eq!(redacted.state, BudgetCallState::Reserved);
    let refused = store
        .begin_budget_dispatch(&host(), &fence(&call, 20))
        .await;
    assert!(
        matches!(refused, Err(Error::Cancelled)),
        "expected Cancelled, got {refused:?}"
    );
    let refused = store
        .reserve_budget_call_with_sources(
            &host(),
            &reservation("call-2", &[SOURCE]),
            &owned(&[SOURCE]),
        )
        .await;
    assert!(
        matches!(refused, Err(Error::Forbidden)),
        "expected Forbidden, got {refused:?}"
    );
    // the reservation that was already held can still be released
    let released = store
        .release_undispatched_budget_call(&host(), &fence(&call, 21), "source_revoked")
        .await
        .unwrap();
    assert_eq!(released.state, BudgetCallState::Released);
    assert_eq!(micros(&store).await, (0, 0));
}

/// A source closure that cannot be read is neither trusted nor guessed at: the
/// dispatch is not made (`Internal`, the class `ensure_budget_call_ref` gives the
/// same body), and the reservation is left as it was.
#[tokio::test]
async fn an_unreadable_source_closure_blocks_the_dispatch() {
    let (_dir, _path, store) = database().await;
    put_run(&store, LIVE).await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &[LIVE]).await;
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

    let result = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await;
    assert!(
        matches!(result, Err(Error::Internal)),
        "expected Internal, got {result:?}"
    );
    let still = call_of(&store, "call-1").await;
    assert_eq!(still.state, BudgetCallState::Reserved);
    assert_eq!(still.dispatch_id, None);
}

// ---------------------------------------------------------------------------
// Settlement is never gated (plan §11.3 and §11.5: real cost stays on the books)
// ---------------------------------------------------------------------------

/// One call dispatched over `SOURCE` before the revocation, then the revocation.
/// Returns the store with the call's dispatched record.
async fn dispatched_then_revoked() -> (tempfile::TempDir, Store, BudgetCallRecord) {
    let (dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = reserve(&store, "call-1", &[SOURCE]).await;
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await
        .unwrap()
        .call;
    assert_eq!(dispatched.state, BudgetCallState::Dispatched);
    revoke(&store, "run", SOURCE).await;
    (dir, store, dispatched)
}

#[tokio::test]
async fn finalize_and_close_after_the_revoke_book_the_real_cost() {
    let (_dir, store, dispatched) = dispatched_then_revoked().await;
    let finalized = store
        .finalize_budget_call(&host(), &fence(&dispatched, 20), &charge(7, "call-1"))
        .await
        .unwrap();
    assert_eq!(finalized.state, BudgetCallState::Finalized);
    assert_eq!(finalized.actual_cost_micros, Some(7));
    assert_eq!(micros(&store).await, (0, 7));
    let closed = store
        .close_budget_call_execution(
            &host(),
            SCOPE,
            "call-1",
            dispatched.dispatch_id.as_deref().unwrap(),
            "closed",
            21,
        )
        .await
        .unwrap();
    assert!(closed.execution_closed);
    // a replay of the same charge is idempotent
    let again = store
        .finalize_budget_call(&host(), &fence(&dispatched, 22), &charge(7, "call-1"))
        .await
        .unwrap();
    assert_eq!(again.actual_cost_micros, Some(7));
    assert_eq!(micros(&store).await, (0, 7));
}

#[tokio::test]
async fn model_settlement_after_the_revoke_books_the_real_cost() {
    let (_dir, store, dispatched) = dispatched_then_revoked().await;
    let evidence = ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            "test.transport.v1",
            &json!({"provider_request_id": "provider-call-1"}),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable("test.response.v1", &json!({"status": "completed"}))
                .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "test.response.v1",
            &json!({"status": "cancelled_after_dispatch"}),
        )
        .unwrap(),
        forced_block_reason: None,
    };
    let settlement = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 20),
            &charge(10, "call-1"),
            &evidence,
        )
        .await
        .unwrap();
    // The call and its cost are on the books; whether the late response may be
    // used is a separate question this change does not answer.
    assert_eq!(settlement.call.state, BudgetCallState::Finalized);
    assert_eq!(settlement.call.actual_cost_micros, Some(10));
    assert_eq!(micros(&store).await, (0, 10));
}

/// The settlement of a registered in-process execution (what the development
/// runner issues) is a settlement too: a revocation that lands after the dispatch
/// does not keep its zero charge off the books.
#[tokio::test]
async fn registered_settlement_after_the_revoke_closes_the_call() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let call = store
        .reserve_budget_call_with_sources(
            &host(),
            &BudgetCallReservation {
                stage: BudgetStage::DevelopmentExecution,
                ..reservation("call-1", &[SOURCE])
            },
            &owned(&[SOURCE]),
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(&call, 11))
        .await
        .unwrap()
        .call;
    revoke(&store, "run", SOURCE).await;

    let charge = charge(0, "call-1");
    let settlement = RegisteredExecutionSettlement {
        schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
        provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
        call_id: "call-1".into(),
        dispatch_id: dispatched.dispatch_id.clone().unwrap(),
        request_digest: dispatched.actual_input_digest.clone(),
        output_digest: charge.output_digest.clone(),
        target_id: "target-1".into(),
        target_digest: hash(b"target"),
        runner_digest: hash(b"runner"),
    };
    let settled = store
        .settle_registered_execution_call(&host(), &fence(&dispatched, 20), &charge, &settlement)
        .await
        .unwrap();
    assert_eq!(settled.state, BudgetCallState::Finalized);
    assert_eq!(settled.actual_cost_micros, Some(0));
    assert!(settled.execution_closed);
    assert_eq!(micros(&store).await, (0, 0));
}

#[tokio::test]
async fn an_uncertain_call_after_the_revoke_reconciles_to_its_real_cost() {
    let (_dir, store, dispatched) = dispatched_then_revoked().await;
    let uncertain = store
        .mark_budget_call_uncertain(&host(), &fence(&dispatched, 20), "transport_failed")
        .await
        .unwrap();
    assert_eq!(uncertain.state, BudgetCallState::Uncertain);
    // the reservation is still held for an uncertain call
    assert_eq!(micros(&store).await, (20, 0));
    let reconciled = store
        .reconcile_budget_call_cost(&admin(), SCOPE, "call-1", &charge(9, "call-1"), 21)
        .await
        .unwrap();
    assert_eq!(reconciled.state, BudgetCallState::Finalized);
    assert_eq!(reconciled.actual_cost_micros, Some(9));
    assert_eq!(micros(&store).await, (0, 9));
}

#[tokio::test]
async fn a_cancelled_dispatched_call_after_the_revoke_becomes_uncertain() {
    let (_dir, store, dispatched) = dispatched_then_revoked().await;
    let cancelled = store
        .cancel_budget_call(&host(), &fence(&dispatched, 20), "operator_cancel")
        .await
        .unwrap();
    // a dispatched call cannot be un-spent: it is uncertain, not released
    assert_eq!(cancelled.state, BudgetCallState::Uncertain);
    assert_eq!(micros(&store).await, (20, 0));
}

/// Release, cancel and re-fence of reservations made before the revocation.
#[tokio::test]
async fn release_cancel_and_refence_of_a_reservation_still_work_after_the_revoke() {
    let (_dir, _path, store) = database().await;
    put_run(&store, SOURCE).await;
    authorize(&store).await;
    let to_release = reserve(&store, "to-release", &[SOURCE]).await;
    let to_cancel = reserve(&store, "to-cancel", &[SOURCE]).await;
    let to_refence = reserve(&store, "to-refence", &[SOURCE]).await;
    assert_eq!(micros(&store).await, (60, 0));
    revoke(&store, "run", SOURCE).await;

    let released = store
        .release_undispatched_budget_call(&host(), &fence(&to_release, 20), "released")
        .await
        .unwrap();
    assert_eq!(released.state, BudgetCallState::Released);
    let cancelled = store
        .cancel_budget_call(&host(), &fence(&to_cancel, 21), "cancelled")
        .await
        .unwrap();
    assert_eq!(cancelled.state, BudgetCallState::Cancelled);
    assert_eq!(micros(&store).await, (20, 0));

    // the lease of the third reservation (until 100) has expired at 101
    let refenced = store
        .refence_reserved_budget_call(
            &host(),
            &BudgetCallRefence {
                billing_scope: SCOPE.into(),
                call_id: "to-refence".into(),
                expected_epoch: to_refence.lease_epoch,
                new_lease_token: "lease-renewed".into(),
                new_lease_until: 300,
                now: 101,
            },
        )
        .await
        .unwrap();
    assert_eq!(refenced.lease_epoch, to_refence.lease_epoch + 1);
    assert_eq!(refenced.state, BudgetCallState::Reserved);
    assert_eq!(micros(&store).await, (20, 0));
}
