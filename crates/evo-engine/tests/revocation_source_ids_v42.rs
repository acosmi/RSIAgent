//! AG-036 (E08, plan §11 and §11.1): the two production creators of a revocation
//! source no longer produce a run and an E16 `import_source` artifact that share
//! an id.
//!
//! A revocation tombstone is keyed by the id of its source alone, so a run and an
//! artifact with one id share one tombstone, and the first one revoked makes the
//! other one unrevocable (`begin_revoke` refuses it by name since this change; see
//! `crates/evo-storage/tests/revocation_source_collision.rs`). The ids are chosen
//! by different parties: a run's id by the Host that stores its trace authority
//! (`store_trace_authority`, any valid identifier), an import source's id derived
//! from its selection (`e16src-` plus 24 hex digits of a digest, which a caller
//! that knows the request can compute). Both creators now refuse to create their
//! object over the other kind and write nothing:
//!
//! - `store_trace_authority` over an `rsia.e16.import_source.v1` artifact of the
//!   same id: `Conflict` (before: stored, and the id was shared).
//! - `PersistentImportService::register` whose derived import-source id already
//!   names a run: `Conflict`; whose id already has a tombstone: `Forbidden`, the
//!   answer every other tombstone gate gives (before: the prepared rows were
//!   committed first and the registration failed later while finalizing).
//!
//! Only the two revocation sources are covered. An object of another kind that
//! shares an id with a run is a different object (AG-032), and still is: see
//! `an_artifact_that_is_not_an_import_source_does_not_block_a_run_of_the_same_id`
//! and `replay_redacted_reads_v42.rs`, whose coexistence case is unchanged.
//!
//! A collision written around these creators (a raw `put`) is not prevented here,
//! and a pair that already exists stays unrevocable for its second member until
//! the tombstone key carries the kind (§11.1, a separate task).
//!
//! Real SQLite store, no model, no provider, zero monetary cost. The fixtures are
//! copied from `tests/replay_redacted_reads_v42.rs` (the trace authority) and
//! `tests/import_v41.rs` (the registration request); the originals are untouched.

use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportRetentionScope, ImportSourceSpec,
    PersistentImportService, SourceFormat,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::Path;

const HISTORY: &[u8] = br#"{"role":"user","content":"private imported material"}"#;
const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";
const KEY_NOTE: &str = "(a revocation tombstone is keyed by id)";

/// Every `objects.kind` the schema allows.
const KINDS: [&str; 15] = [
    "run",
    "feedback",
    "event",
    "candidate",
    "release",
    "pointer",
    "receipt",
    "dataset",
    "evaluation",
    "budget",
    "reservation",
    "job",
    "improvement",
    "artifact",
    "tombstone",
];

fn host() -> Context {
    Context::new("n", "host", Role::Host).unwrap()
}

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}

async fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    // The registration reads the namespace's revoke watermark.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin(), &hash(b"ag036-initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    (dir, store)
}

async fn raw(store: &Store, kind: &str, id: &str) -> Option<Value> {
    let mut session = store.session().await.unwrap();
    let value = session.get(&admin(), kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn raw_put(store: &Store, kind: &str, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(&admin(), kind, id, "admin", body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// What a refused call must leave as it found it: the body of every object of the
/// namespace, the number of audit rows and the revoke watermark. (The cache and
/// the edges are asserted separately, per call.)
#[derive(Debug, PartialEq)]
struct Snapshot {
    objects: Vec<(&'static str, Vec<Value>)>,
    audit_rows: usize,
    watermark: Option<(i64, String)>,
}

async fn snapshot(store: &Store) -> Snapshot {
    let mut session = store.session().await.unwrap();
    let mut objects = Vec::new();
    for kind in KINDS {
        objects.push((kind, session.list::<Value>(&admin(), kind).await.unwrap()));
    }
    let watermark = session.watermark(&admin()).await.unwrap();
    session.commit().await.unwrap();
    Snapshot {
        objects,
        audit_rows: store.verify_audit(&admin()).await.unwrap(),
        watermark,
    }
}

/// A Host-issued trace authority (copied from `exploration_fx` in
/// `tests/replay_redacted_reads_v42.rs`, a successful run).
fn authority(id: &str) -> StoredTraceAuthority {
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: id.as_bytes().to_vec(),
            parent_family: "family-a".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: "family-a".into(),
            source_digest: hash(id.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: id.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: id.len(),
    }
}

async fn store_run(store: &Store, id: &str) -> Result<(), Error> {
    store_trace_authority(store, &host(), &authority(id)).await
}

/// A registration whose sources (`logical_ids`) all read `history.jsonl` of `dir`,
/// shaped like `reconnect_request` in `tests/import_v41.rs`.
fn registration(dir: &Path, request_key: &str, logical_ids: &[&str]) -> ImportRegistrationRequest {
    let path = dir.join("history.jsonl");
    ImportRegistrationRequest {
        schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
        request_key: request_key.into(),
        roots: vec![dir.to_string_lossy().into_owned()],
        purpose: Purpose::Development,
        allow_model_excerpts: false,
        outbound_authorized: false,
        retention_scope: ImportRetentionScope::LocalPrivate,
        sources: logical_ids
            .iter()
            .map(|logical| ImportSourceSpec {
                source_id: (*logical).into(),
                path: path.to_string_lossy().into_owned(),
                reader: SourceFormat::ClaudeFixture,
                expected_digest: hash(HISTORY),
            })
            .collect(),
    }
}

/// The id the import source of `logical_source_id` gets for this request (the
/// derivation in `import_source_id`/`register`, which is private): the selection
/// id is `e16sel-` plus 24 hex digits of the request's fingerprint, the source id
/// `e16src-` plus 24 hex digits of the digest of `selection \n logical id`.
/// `a_registration_gives_its_sources_the_derived_ids` pins this against the real
/// registration, so a drift fails there and not silently here.
fn derived_source_id(request: &ImportRegistrationRequest, logical_source_id: &str) -> String {
    let selection_id = format!("e16sel-{}", &fingerprint(request).unwrap()[..24]);
    format!(
        "e16src-{}",
        &hash(format!("{selection_id}\n{logical_source_id}").as_bytes())[..24]
    )
}

fn expect_conflict<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) -> String {
    match result {
        Err(Error::Conflict(message)) => message,
        other => panic!("{what}: expected a Conflict, got {other:?}"),
    }
}

fn expect_forbidden<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) {
    match result {
        Err(Error::Forbidden) => {}
        other => panic!("{what}: expected Forbidden, got {other:?}"),
    }
}

/// No half-recorded registration: no idempotency row, no edge into the blob and
/// none into an import source (the selection's).
async fn assert_no_registration_trace(
    store: &Store,
    request: &ImportRegistrationRequest,
    source_ids: &[String],
) {
    let mut session = store.session().await.unwrap();
    let cached = session
        .cached::<Value, _>(
            &admin(),
            "e16.import.register",
            &request.request_key,
            request,
        )
        .await;
    assert!(
        matches!(cached, Ok(None)),
        "registration cache row: {cached:?}"
    );
    for source_id in source_ids {
        for destination in [("blob", hash(HISTORY)), ("artifact", source_id.clone())] {
            let dependents = session
                .dependents(&admin(), destination.0, &destination.1)
                .await
                .unwrap();
            assert!(
                dependents.is_empty(),
                "edges into {destination:?}: {dependents:?}"
            );
        }
    }
    session.commit().await.unwrap();
}

async fn write_history(dir: &Path) {
    tokio::fs::write(dir.join("history.jsonl"), HISTORY)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// The derivation the tests below rely on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_registration_gives_its_sources_the_derived_ids() {
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    let request = registration(dir.path(), "derived-ids", &["source-one", "source-two"]);
    let selection = PersistentImportService::new(store.clone())
        .register(&admin(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        selection.payload.import_source_ids,
        vec![
            derived_source_id(&request, "source-one"),
            derived_source_id(&request, "source-two")
        ]
    );
    for id in &selection.payload.import_source_ids {
        let artifact = raw(&store, "artifact", id).await.unwrap();
        assert_eq!(artifact["schema_version"], IMPORT_SOURCE_SCHEMA);
    }
}

// ---------------------------------------------------------------------------
// store_trace_authority: a run over an import source
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_trusted_run_cannot_be_stored_under_the_id_of_an_import_source() {
    // A real registration: its import source has the id the Host now tries to give
    // a run.
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    let selection = PersistentImportService::new(store.clone())
        .register(
            &admin(),
            registration(dir.path(), "import-then-run", &["source-one"]),
        )
        .await
        .unwrap();
    let source_id = selection.payload.import_source_ids[0].clone();
    let artifact = raw(&store, "artifact", &source_id).await.unwrap();
    assert_eq!(artifact["schema_version"], IMPORT_SOURCE_SCHEMA);

    let before = snapshot(&store).await;
    let message = expect_conflict(
        store_run(&store, &source_id).await,
        "store_trace_authority over an import source",
    );
    assert_eq!(
        message,
        format!(
            "run {source_id} cannot be stored: an E16 import_source artifact already has \
             this id {KEY_NOTE}"
        )
    );
    // Nothing was written: no run row, no audit row, no cache row (and every other
    // object, the watermark and the audit chain are as they were).
    assert_eq!(snapshot(&store).await, before);
    assert!(raw(&store, "run", &source_id).await.is_none());
    assert_eq!(raw(&store, "artifact", &source_id).await, Some(artifact));
    let mut session = store.session().await.unwrap();
    let cached = session
        .cached::<StoredTraceAuthority, _>(
            &host(),
            "optimization.source",
            &source_id,
            &authority(&source_id),
        )
        .await;
    assert!(matches!(cached, Ok(None)), "cache row: {cached:?}");
    session.commit().await.unwrap();

    // Control: a run with an id of its own is stored, with its cache and audit row.
    store_run(&store, "run-with-its-own-id").await.unwrap();
    assert!(raw(&store, "run", "run-with-its-own-id").await.is_some());
    let after = snapshot(&store).await;
    assert_eq!(after.audit_rows, before.audit_rows + 1);
    assert_ne!(after.objects, before.objects);
}

#[tokio::test]
async fn an_artifact_that_is_not_an_import_source_does_not_block_a_run_of_the_same_id() {
    // AG-032: an object of another kind that shares a run's id is a different
    // object, unless it is a revocation source of its own. Only the E16
    // import_source schema is.
    let (_dir, store) = open().await;
    for (id, schema) in [
        ("shared-with-a-pool-row", "rsia.replay_pool.fixture.v1"),
        ("shared-with-a-selection", "rsia.e16.source_selection.v1"),
        ("shared-with-a-result", "rsia.e16.import_result.v1"),
    ] {
        raw_put(
            &store,
            "artifact",
            id,
            &json!({"id": id, "schema_version": schema}),
        )
        .await;
        store_run(&store, id).await.unwrap();
        assert!(raw(&store, "run", id).await.is_some(), "{id}");
        assert_eq!(
            raw(&store, "artifact", id).await.unwrap()["schema_version"],
            schema
        );
    }
}

#[tokio::test]
async fn storing_the_same_trace_authority_again_is_unchanged() {
    // The idempotent path of an existing run is not touched by the collision check.
    let (_dir, store) = open().await;
    store_run(&store, "run-stored-twice").await.unwrap();
    let before = snapshot(&store).await;
    store_run(&store, "run-stored-twice").await.unwrap();
    assert_eq!(snapshot(&store).await, before);
}

// ---------------------------------------------------------------------------
// register: an import source over a run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_import_source_cannot_be_created_under_the_id_of_a_run() {
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    let request = registration(dir.path(), "run-then-import", &["source-one"]);
    let source_id = derived_source_id(&request, "source-one");
    // The Host stores a run under the id the import source will derive.
    store_run(&store, &source_id).await.unwrap();
    let run = raw(&store, "run", &source_id).await.unwrap();

    let before = snapshot(&store).await;
    let service = PersistentImportService::new(store.clone());
    let message = expect_conflict(
        service.register(&admin(), request.clone()).await,
        "register over a run",
    );
    assert_eq!(
        message,
        format!(
            "import source {source_id} cannot be created: a run already has this id {KEY_NOTE}"
        )
    );
    // Nothing was written: no selection, no import source, no edge, no cache row,
    // no audit row. The run is untouched.
    assert_eq!(snapshot(&store).await, before);
    assert_no_registration_trace(&store, &request, std::slice::from_ref(&source_id)).await;
    assert!(raw(&store, "artifact", &source_id).await.is_none());
    assert_eq!(raw(&store, "run", &source_id).await, Some(run));
    // The refusal is not remembered: asking again is refused again.
    expect_conflict(
        service.register(&admin(), request.clone()).await,
        "register over a run, again",
    );
    assert_eq!(snapshot(&store).await, before);

    // Control: the same request in a store without that run registers, and its
    // source has exactly the id the run took.
    // (The request names the first directory's file, which is all it reads.)
    let (_other_dir, other) = open().await;
    let selection = PersistentImportService::new(other.clone())
        .register(&admin(), request.clone())
        .await
        .unwrap();
    assert_eq!(selection.payload.import_source_ids, vec![source_id.clone()]);
    assert!(raw(&other, "artifact", &source_id).await.is_some());
}

#[tokio::test]
async fn a_registration_with_one_colliding_source_writes_none_of_its_sources() {
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    let request = registration(dir.path(), "second-collides", &["source-one", "source-two"]);
    let first = derived_source_id(&request, "source-one");
    let second = derived_source_id(&request, "source-two");
    store_run(&store, &second).await.unwrap();

    let before = snapshot(&store).await;
    let message = expect_conflict(
        PersistentImportService::new(store.clone())
            .register(&admin(), request.clone())
            .await,
        "register with a colliding second source",
    );
    assert!(
        message.contains(&second) && !message.contains(&first),
        "{message}"
    );
    assert_eq!(snapshot(&store).await, before);
    assert_no_registration_trace(&store, &request, &[first.clone(), second]).await;
    assert!(raw(&store, "artifact", &first).await.is_none());
}

#[tokio::test]
async fn an_import_source_cannot_be_created_under_an_id_that_has_a_tombstone() {
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    let request = registration(dir.path(), "tombstone-then-import", &["source-one"]);
    let source_id = derived_source_id(&request, "source-one");
    store_run(&store, &source_id).await.unwrap();
    let status = LifecycleStore::begin_revoke(
        &admin(),
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: source_id.clone(),
        },
        "the run is revoked",
        9,
    )
    .await
    .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
    let service = PersistentImportService::new(store.clone());

    // The run row is still there (cleanup has not run), and the tombstone decides
    // first, as it does in `store_trace_authority`.
    let before = snapshot(&store).await;
    expect_forbidden(
        service.register(&admin(), request.clone()).await,
        "register over a revoked run that is not cleaned yet",
    );
    assert_eq!(snapshot(&store).await, before);
    assert_no_registration_trace(&store, &request, std::slice::from_ref(&source_id)).await;

    // The cleanup deletes the run row and leaves the tombstone; deleting the row
    // directly keeps this case independent of the cleanup. The tombstone alone
    // refuses the id, as every tombstone gate does.
    let mut session = store.session().await.unwrap();
    session.delete(&admin(), "run", &source_id).await.unwrap();
    session.commit().await.unwrap();
    assert!(raw(&store, "tombstone", &source_id).await.is_some());
    let before = snapshot(&store).await;
    expect_forbidden(
        service.register(&admin(), request.clone()).await,
        "register over a tombstone",
    );
    assert_eq!(snapshot(&store).await, before);
    assert_no_registration_trace(&store, &request, std::slice::from_ref(&source_id)).await;
    assert!(raw(&store, "artifact", &source_id).await.is_none());
}

#[tokio::test]
async fn registrations_with_no_shared_id_are_unchanged() {
    // A run with an unrelated id does not get in the way of a registration (its
    // repeat and its execution) whose sources have ids of their own.
    let (dir, store) = open().await;
    write_history(dir.path()).await;
    store_run(&store, "an-unrelated-run").await.unwrap();
    let service = PersistentImportService::new(store.clone());
    let selection = service
        .register(
            &admin(),
            registration(dir.path(), "plain-registration", &["source-one"]),
        )
        .await
        .unwrap();
    let again = service
        .register(
            &admin(),
            registration(dir.path(), "plain-registration", &["source-one"]),
        )
        .await
        .unwrap();
    assert_eq!(again.id, selection.id);
    let result = service.execute(&admin(), &selection.id).await.unwrap();
    assert_eq!(result.payload.selection_id, selection.id);
}

// ---------------------------------------------------------------------------
// The engine's own entry to a revocation: a second source of the shared id
// ---------------------------------------------------------------------------

/// The one artifact `begin_revoke` accepts as a source: a strict E16 `import_source`
/// envelope (copied from `import_source_envelope` in
/// `tests/replay_redacted_reads_v42.rs`).
fn import_source_envelope(id: &str) -> Value {
    json!({
        "schema_version": IMPORT_SOURCE_SCHEMA,
        "id": id,
        "namespace": "n",
        "owner_actor": "admin",
        "request_key": "import-source-fixture",
        "input_digest": hash(b"import-input"),
        "created_at": 1,
        "updated_at": 1,
        "source_refs": [],
        "revoke_watermark": 0,
        "payload": {},
    })
}

#[tokio::test]
async fn a_run_revoked_after_an_import_source_of_its_id_is_a_named_conflict() {
    // The controller's probe P2 for AG-032: a run and an import source of one id (the
    // creators refuse to make the pair now, so the artifact is written raw), the
    // artifact is revoked, then the operator revokes the run through the engine's
    // coordinator. The answer was `Internal` and the run's revocation could never be
    // recorded; it is a named `Conflict` and nothing is written.
    let (_dir, store) = open().await;
    store_run(&store, "run-failure").await.unwrap();
    raw_put(
        &store,
        "artifact",
        "run-failure",
        &import_source_envelope("run-failure"),
    )
    .await;
    let first = LifecycleStore::begin_revoke(
        &admin(),
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "run-failure".into(),
        },
        "operator revokes the import source",
        600,
    )
    .await
    .unwrap();
    assert_eq!(first.state, CleanupState::Pending);
    let tombstone = raw(&store, "tombstone", "run-failure").await.unwrap();
    assert_eq!(tombstone["source_kind"], "artifact");
    let status_before = serde_json::to_value(
        LifecycleStore::cleanup_status(&admin(), &store, &first.job_id)
            .await
            .unwrap(),
    )
    .unwrap();
    let before = snapshot(&store).await;

    let message = expect_conflict(
        LifecycleCoordinator::revoke_source(
            &admin(),
            &store,
            "run-failure",
            "operator revokes the run",
            601,
        )
        .await,
        "revoke_source over the revoked import source of its id",
    );
    assert_eq!(
        message,
        "cannot revoke run run-failure: the id is already revoked as artifact \
         (a revocation tombstone is keyed by id)"
    );
    // The watermark did not move, the first tombstone and job are as they were and
    // the run is still stored (the storage test compares every table).
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        raw(&store, "tombstone", "run-failure").await,
        Some(tombstone)
    );
    assert_eq!(
        serde_json::to_value(
            LifecycleStore::cleanup_status(&admin(), &store, &first.job_id)
                .await
                .unwrap()
        )
        .unwrap(),
        status_before
    );
    assert!(raw(&store, "run", "run-failure").await.is_some());
}
