//! AG-036 (E08, plan §11 and §11.1): a second revocation source that shares an id
//! with an already revoked source is refused by name, and nothing is written for
//! it.
//!
//! A revocation tombstone is keyed by the id of its source alone
//! (`objects(namespace, kind='tombstone', id)`, and the body must carry that id),
//! while a revocable source is a run or a strict E16 `import_source` artifact, and
//! the two kinds do not share an id space. A run and an artifact that carry the
//! same id therefore share one tombstone. `begin_revoke` of the second one used to
//! read the first one's tombstone, look for a cleanup job of the *requested* kind
//! under that tombstone's watermark, find none and answer `Internal`: the
//! revocation of the second source could never be recorded, and the operator was
//! told the store was broken.
//!
//! `begin_revoke` now compares the tombstone's `source_kind` with the requested
//! kind. A tombstone of the other kind is a name conflict, not corruption:
//! `NotFound` when the requested source does not exist, otherwise a `Conflict`
//! that names the id and both kinds. Nothing is written: the watermark does not
//! move, no job is created, and the first tombstone is not overwritten (`put` is an
//! upsert, and the restore replay reads the tombstone's `watermark_seq`, so an
//! overwrite would corrupt the first revocation). A repeated revocation of the
//! same kind still answers the one job it already has.
//!
//! The long-term answer (the tombstone key carries the kind, plan §11.1) needs a
//! trust-anchor format change and a dual-read restore and is not part of this
//! change; a run and an artifact that already share an id stay unrevocable for the
//! second one.
//!
//! The "nothing is written" assertions compare every row of every table, read
//! through a second read-only connection, before and after the refused call. Real
//! SQLite store, no model, no provider, zero monetary cost.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{
    CleanupState, CleanupStatus, LifecycleStore, RevokeTombstone, TypedObjectRef,
};
use serde_json::{Value, json};
use sqlx::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ID: &str = "shared-source-id";
const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";
const REASON: &str = "revoked by the operator";

fn ctx(actor: &str, role: Role) -> Context {
    Context::new("n", actor, role).unwrap()
}

async fn database() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    (dir, path, store)
}

async fn raw_put(store: &Store, kind: &str, id: &str, body: &Value) {
    let admin = ctx("admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, kind, id, admin.actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn raw_get(store: &Store, kind: &str, id: &str) -> Option<Value> {
    let admin = ctx("admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    let value = session.get(&admin, kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn watermark(store: &Store) -> Option<(i64, String)> {
    let admin = ctx("admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    let value = session.watermark(&admin).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// A trusted run row, as `put_source` in `tests/lifecycle.rs` writes it.
fn run_body(id: &str) -> Value {
    json!({"id": id, "schema_version": "rsia.optimization.source.v1", "body": "run-body"})
}

/// The one artifact `begin_revoke` accepts as a source: a strict E16 `import_source`
/// envelope (the same shape as `e16_import_envelope` in `tests/lifecycle.rs`).
fn import_source_body(id: &str) -> Value {
    json!({
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
    })
}

async fn put_run(store: &Store, id: &str) {
    raw_put(store, "run", id, &run_body(id)).await;
}

async fn put_import_source(store: &Store, id: &str) {
    raw_put(store, "artifact", id, &import_source_body(id)).await;
}

async fn begin_revoke(store: &Store, kind: &str, id: &str) -> Result<CleanupStatus, Error> {
    LifecycleStore::begin_revoke(
        &ctx("admin", Role::Admin),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        REASON,
        7,
    )
    .await
}

async fn status_json(store: &Store, job_id: &str) -> Value {
    let status = LifecycleStore::cleanup_status(&ctx("admin", Role::Admin), store, job_id)
        .await
        .unwrap();
    serde_json::to_value(status).unwrap()
}

/// Every row of every table (the FTS shadow tables and the migration ledger
/// included), as text, through a second read-only connection: what a call that
/// "wrote nothing" must leave exactly as it found it.
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

fn assert_nothing_written(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
    what: &str,
) {
    let changed: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
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

fn expect_conflict(result: Result<CleanupStatus, Error>, what: &str) -> String {
    match result {
        Err(Error::Conflict(message)) => message,
        other => panic!("{what}: expected a Conflict, got {other:?}"),
    }
}

fn expect_not_found(result: Result<CleanupStatus, Error>, what: &str) {
    match result {
        Err(Error::NotFound) => {}
        other => panic!("{what}: expected NotFound, got {other:?}"),
    }
}

fn expect_internal(result: Result<CleanupStatus, Error>, what: &str) {
    match result {
        Err(Error::Internal) => {}
        other => panic!("{what}: expected Internal, got {other:?}"),
    }
}

/// A run and a strict import-source artifact share one id and both exist. The
/// first kind is revoked; the revocation of the second kind is then refused by
/// name and leaves every table exactly as it was.
async fn second_source_of_the_other_kind_is_refused(first_kind: &str, second_kind: &str) {
    let (_dir, path, store) = database().await;
    put_run(&store, ID).await;
    put_import_source(&store, ID).await;
    let pristine = fingerprint(&path).await;

    let first = begin_revoke(&store, first_kind, ID).await.unwrap();
    assert_eq!(first.state, CleanupState::Pending, "{first:?}");
    assert_eq!(
        first.source,
        TypedObjectRef {
            kind: first_kind.into(),
            id: ID.into()
        }
    );
    // The control for the "nothing written" check below: a real revocation does
    // change the fingerprint.
    let after_first = fingerprint(&path).await;
    assert_ne!(pristine, after_first);

    let tombstone_before = raw_get(&store, "tombstone", ID).await.unwrap();
    let decoded: RevokeTombstone = serde_json::from_value(tombstone_before.clone()).unwrap();
    assert_eq!(decoded.source_kind, first_kind);
    assert_eq!(decoded.watermark_seq, first.watermark_seq);
    let watermark_before = watermark(&store).await;
    assert_eq!(
        watermark_before.as_ref().map(|(seq, _)| *seq),
        Some(i64::try_from(first.watermark_seq).unwrap())
    );
    let status_before = status_json(&store, &first.job_id).await;
    let run_before = raw_get(&store, "run", ID).await;
    let artifact_before = raw_get(&store, "artifact", ID).await;
    assert_eq!(run_before, Some(run_body(ID)));
    assert_eq!(artifact_before, Some(import_source_body(ID)));

    let message = expect_conflict(
        begin_revoke(&store, second_kind, ID).await,
        &format!("revoking {second_kind} {ID} after {first_kind} {ID}"),
    );
    assert_eq!(
        message,
        format!(
            "cannot revoke {second_kind} {ID}: the id is already revoked as {first_kind} \
             (a revocation tombstone is keyed by id)"
        )
    );

    // The watermark did not move.
    assert_eq!(watermark(&store).await, watermark_before);
    // The first tombstone is the one the first revocation wrote (`put` is an
    // upsert: an overwrite would change `source_kind` and the watermark sequence,
    // and the row's revision, which the fingerprint below covers).
    assert_eq!(
        raw_get(&store, "tombstone", ID).await,
        Some(tombstone_before)
    );
    // The first job is the same job in the same state, and there is no second one.
    assert_eq!(status_json(&store, &first.job_id).await, status_before);
    // Both objects are still there, untouched.
    assert_eq!(raw_get(&store, "run", ID).await, run_before);
    assert_eq!(raw_get(&store, "artifact", ID).await, artifact_before);
    // And nothing else was written, in any table.
    assert_nothing_written(
        &after_first,
        &fingerprint(&path).await,
        "refused second revocation source",
    );

    // Asking again is refused the same way, and the first kind still answers its
    // own job.
    let again = expect_conflict(
        begin_revoke(&store, second_kind, ID).await,
        "second refusal",
    );
    assert_eq!(again, message);
    let repeat = begin_revoke(&store, first_kind, ID).await.unwrap();
    assert_eq!(repeat.job_id, first.job_id);
    assert_nothing_written(
        &after_first,
        &fingerprint(&path).await,
        "refusals and a repeat of the first kind",
    );
}

#[tokio::test]
async fn a_run_revoked_first_refuses_the_revocation_of_the_artifact_with_its_id() {
    second_source_of_the_other_kind_is_refused("run", "artifact").await;
}

#[tokio::test]
async fn an_artifact_revoked_first_refuses_the_revocation_of_the_run_with_its_id() {
    // The orientation the controller's probe P2 found: the artifact is revoked
    // first, then the operator revokes the run of the same id. Before this change
    // the answer was `Internal`.
    second_source_of_the_other_kind_is_refused("artifact", "run").await;
}

#[tokio::test]
async fn a_tombstone_of_the_other_kind_with_no_source_of_the_requested_kind_is_not_found() {
    // Only the artifact's tombstone exists: there is no run row of that id.
    let (_dir, path, store) = database().await;
    put_import_source(&store, ID).await;
    let first = begin_revoke(&store, "artifact", ID).await.unwrap();
    assert!(raw_get(&store, "run", ID).await.is_none());
    let tombstone = raw_get(&store, "tombstone", ID).await;
    let watermark_before = watermark(&store).await;
    let status_before = status_json(&store, &first.job_id).await;
    let fingerprint_before = fingerprint(&path).await;

    expect_not_found(begin_revoke(&store, "run", ID).await, "run with no run row");

    assert_eq!(watermark(&store).await, watermark_before);
    assert_eq!(raw_get(&store, "tombstone", ID).await, tombstone);
    assert_eq!(status_json(&store, &first.job_id).await, status_before);
    assert_nothing_written(
        &fingerprint_before,
        &fingerprint(&path).await,
        "not-found revocation",
    );

    // The same the other way round: only the run's tombstone exists.
    let (_dir, path, store) = database().await;
    put_run(&store, ID).await;
    let first = begin_revoke(&store, "run", ID).await.unwrap();
    assert!(raw_get(&store, "artifact", ID).await.is_none());
    let status_before = status_json(&store, &first.job_id).await;
    let fingerprint_before = fingerprint(&path).await;
    expect_not_found(
        begin_revoke(&store, "artifact", ID).await,
        "artifact with no artifact row",
    );
    assert_eq!(status_json(&store, &first.job_id).await, status_before);
    assert_nothing_written(
        &fingerprint_before,
        &fingerprint(&path).await,
        "not-found revocation (artifact)",
    );
}

#[tokio::test]
async fn nothing_to_revoke_is_not_found_as_before() {
    // No tombstone and no source: `NotFound`, unchanged.
    let (_dir, path, store) = database().await;
    let fingerprint_before = fingerprint(&path).await;
    expect_not_found(
        begin_revoke(&store, "run", ID).await,
        "no run, no tombstone",
    );
    expect_not_found(
        begin_revoke(&store, "artifact", ID).await,
        "no artifact, no tombstone",
    );
    assert_nothing_written(
        &fingerprint_before,
        &fingerprint(&path).await,
        "revocation of nothing",
    );
}

#[tokio::test]
async fn repeating_a_revocation_of_the_same_kind_answers_the_same_job() {
    for kind in ["run", "artifact"] {
        let (_dir, path, store) = database().await;
        // Both objects exist, so the repeat has an object of the other kind to be
        // confused with.
        put_run(&store, ID).await;
        put_import_source(&store, ID).await;
        let first = begin_revoke(&store, kind, ID).await.unwrap();
        let after_first = fingerprint(&path).await;
        let again = begin_revoke(&store, kind, ID).await.unwrap();
        assert_eq!(again.job_id, first.job_id, "{kind}");
        assert_eq!(
            serde_json::to_value(&again).unwrap(),
            serde_json::to_value(&first).unwrap(),
            "{kind}"
        );
        assert_eq!(again.watermark_seq, first.watermark_seq, "{kind}");
        assert_nothing_written(&after_first, &fingerprint(&path).await, kind);
    }
}

#[tokio::test]
async fn a_tombstone_of_one_id_does_not_touch_the_revocation_of_another_id() {
    let (_dir, _path, store) = database().await;
    put_run(&store, ID).await;
    put_run(&store, "another-source-id").await;
    let first = begin_revoke(&store, "run", ID).await.unwrap();
    let other = begin_revoke(&store, "run", "another-source-id")
        .await
        .unwrap();
    assert_ne!(other.job_id, first.job_id);
    assert_eq!(other.watermark_seq, first.watermark_seq + 1);
    assert_eq!(other.state, CleanupState::Pending);
}

#[tokio::test]
async fn a_tombstone_that_does_not_decode_is_still_internal_and_writes_nothing() {
    // Unchanged by this task: a body `begin_revoke` never writes (here: a field
    // missing) cannot be read as a tombstone at all, and the read fails with
    // `Internal` before the source kinds are compared.
    let (_dir, path, store) = database().await;
    put_run(&store, ID).await;
    put_import_source(&store, ID).await;
    raw_put(
        &store,
        "tombstone",
        ID,
        &json!({"id": ID, "schema_version": "rsia.revoke_tombstone.v1"}),
    )
    .await;
    let fingerprint_before = fingerprint(&path).await;
    for kind in ["run", "artifact"] {
        expect_internal(begin_revoke(&store, kind, ID).await, kind);
    }
    assert_nothing_written(
        &fingerprint_before,
        &fingerprint(&path).await,
        "undecodable tombstone",
    );
}

#[tokio::test]
async fn a_tombstone_of_an_unrecognised_kind_is_refused_without_echoing_the_kind() {
    // A tombstone `begin_revoke` never writes, but that decodes: its source kind is
    // neither a run nor an artifact. It still holds the id, so a revocation of an
    // existing source under that id is a conflict (the message does not repeat
    // the stored string) and a revocation of a missing one is not found.
    let (_dir, path, store) = database().await;
    put_run(&store, ID).await;
    raw_put(
        &store,
        "tombstone",
        ID,
        &json!({
            "id": ID,
            "schema_version": "rsia.revoke_tombstone.v1",
            "source_kind": "mystery-kind-that-must-not-be-echoed",
            "reason": "fixture",
            "watermark_seq": 1,
            "watermark_digest": hash(b"fixture"),
            "created_at": 1,
        }),
    )
    .await;
    let fingerprint_before = fingerprint(&path).await;
    let message = expect_conflict(begin_revoke(&store, "run", ID).await, "run");
    assert_eq!(
        message,
        format!(
            "cannot revoke run {ID}: the id is already revoked as an unrecognised source kind \
             (a revocation tombstone is keyed by id)"
        )
    );
    assert!(!message.contains("mystery"));
    expect_not_found(begin_revoke(&store, "artifact", ID).await, "artifact");
    assert_nothing_written(
        &fingerprint_before,
        &fingerprint(&path).await,
        "tombstone of an unrecognised kind",
    );
}
