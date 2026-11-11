//! AG-166a management import controls (v4.5 §6.3, E16.1, §11.2.1).
//!
//! Real Store, real Admin Context, real registered blobs and the real
//! `ManagementDispatcher` job lifecycle. The strict wire accepts only
//! `source_id`/`blob_digest`/`reader` and `selection_id`; every root, path,
//! role, owner, namespace, authority boolean and outbound request is refused
//! before any session or filesystem read. No provider or external grant
//! producer exists, so generation stays blocked and the management surface
//! fails closed instead of faking one.
use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dependency_integrity::{InspectionBudget, IssueCode, inspect_e16_content_edges};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementImportResultState, ManagementImportResultV2,
    ManagementJobState, ManagementJobStatusV1, ManagementResultV2,
};
use evo_engine::import::{ImportRetentionScope, PersistentImportService};
use evo_storage::Store;
use serde_json::{Value, json};

const CLAUDE: &[u8] = include_bytes!("../../../fixtures/imports/claude.fixture.jsonl");
const PI: &[u8] = include_bytes!("../../../fixtures/imports/rsih.pi.fixture.json");

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("import-management.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

fn admin() -> Context {
    Context::new("tenant", "admin", Role::Admin).unwrap()
}

async fn bump(store: &Store, ctx: &Context, label: &str) {
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(ctx, &hash(label.as_bytes()))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn put_blob(store: &Store, ctx: &Context, bytes: &[u8]) -> String {
    store
        .put_blob_bounded(ctx, bytes, 64 * 1024 * 1024)
        .await
        .unwrap()
}

fn register_payload(key: &str, sources: &[(&str, &str, &str)]) -> Value {
    json!({
        "schema_version": "rsia.management.import_register.v1",
        "request_key": key,
        "sources": sources
            .iter()
            .map(|(source_id, blob_digest, reader)| json!({
                "source_id": source_id,
                "blob_digest": blob_digest,
                "reader": reader,
            }))
            .collect::<Vec<_>>(),
    })
}

fn execute_payload(key: &str, selection_id: &str) -> Value {
    json!({
        "schema_version": "rsia.management.import_execute.v1",
        "request_key": key,
        "selection_id": selection_id,
    })
}

async fn done(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    operation: &str,
    payload: Value,
) -> ManagementJobStatusV1 {
    let queued = dispatcher.submit(ctx, operation, payload).await.unwrap();
    terminal(dispatcher, ctx, queued.identity().id).await
}

async fn terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
) -> ManagementJobStatusV1 {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let status = dispatcher.status_view(ctx, job_id).await.unwrap();
            if matches!(
                status.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("import management job reaches a terminal state")
}

fn registered(status: &ManagementJobStatusV1) -> (&str, &str, u64) {
    match status.result.as_ref() {
        Some(ManagementResultV2::Import(ManagementImportResultV2::ImportRegistered {
            selection_id,
            selection_digest,
            source_count,
            ..
        })) => (selection_id, selection_digest, *source_count),
        other => panic!("expected ImportRegistered, got {other:?}"),
    }
}

fn completed(status: &ManagementJobStatusV1) -> &ManagementImportResultV2 {
    match status.result.as_ref() {
        Some(ManagementResultV2::Import(
            result @ ManagementImportResultV2::ImportCompleted { .. },
        )) => result,
        other => panic!("expected ImportCompleted, got {other:?}"),
    }
}

async fn artifact(store: &Store, ctx: &Context, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// The real Admin contract: the server derives the blob location, writes the
/// selection and finalized source under the authenticated owner/namespace, and
/// the bounded result names only identifiers and counters.
#[tokio::test]
async fn import_register_blob_uses_real_admin_context() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "register-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let status = done(
        &dispatcher,
        &admin,
        "import.register",
        register_payload(
            "register-real",
            &[("source-one", &digest, "claude_fixture")],
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "import_registered");
    let (selection_id, selection_digest, source_count) = registered(&status);
    assert_eq!(source_count, 1);
    assert_eq!(selection_digest.len(), 64);

    let selection = artifact(&store, &admin, selection_id).await;
    assert_eq!(selection["namespace"], "tenant");
    assert_eq!(selection["owner_actor"], "admin");
    assert_eq!(selection["schema_version"], "rsia.e16.source_selection.v1");
    assert_eq!(selection["payload"]["purpose"], "inspection");
    assert_eq!(selection["payload"]["retention_scope"], "local_private");
    assert_eq!(selection["payload"]["outbound_authorized"], false);
    assert_eq!(selection["payload"]["allow_model_excerpts"], false);
    let roots = selection["payload"]["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    let root = roots[0].as_str().unwrap();
    assert!(
        root.contains("/blobs/") && root.ends_with(&hash(b"tenant")),
        "the persisted root is the server blob directory: {root}"
    );

    let source_ids = selection["payload"]["import_source_ids"]
        .as_array()
        .unwrap();
    assert_eq!(source_ids.len(), 1);
    let source = artifact(&store, &admin, source_ids[0].as_str().unwrap()).await;
    assert_eq!(source["namespace"], "tenant");
    assert_eq!(source["owner_actor"], "admin");
    assert_eq!(source["schema_version"], "rsia.e16.import_source.v1");
    assert_eq!(source["payload"]["status"], "ready");
    assert_eq!(source["payload"]["raw_digest"], digest);
    assert_eq!(source["payload"]["observed_digest"], digest);
    assert_eq!(source["payload"]["raw_blob_digest"], digest);
    assert_eq!(source["payload"]["blob_published"], true);
    assert_eq!(source["payload"]["byte_len"], CLAUDE.len() as u64);
    assert_eq!(source["payload"]["task_origin"], "imported_history");
    assert_eq!(
        source["payload"]["execution_attestation"],
        "unverified_import"
    );
    assert_eq!(source["payload"]["purpose"], "inspection");
    assert!(
        source["payload"]["authorized_path"]
            .as_str()
            .unwrap()
            .ends_with(&digest)
    );

    // The result carries no physical root and the exact bytes stay readable.
    let result_wire = serde_json::to_string(&status.result).unwrap();
    assert!(!result_wire.contains(root), "{result_wire}");
    assert_eq!(
        store
            .read_blob(&admin, &digest, CLAUDE.len())
            .await
            .unwrap(),
        CLAUDE
    );
    assert_eq!(
        serde_json::to_value(&status.committed_side_effects).unwrap(),
        json!([{
            "kind": "import_selection",
            "selection_id": selection_id,
            "selection_digest": selection_digest,
        }])
    );
    store.close().await;
}

/// The Admin role is judged before the payload and before any source
/// observation: a trusted Evaluator gets `Forbidden` for both a malformed and
/// a well-shaped request, with every SQL table unchanged.
#[tokio::test]
async fn import_register_role_precedes_payload_and_source_observation() {
    let (dir, store) = store().await;
    let admin = admin();
    let evaluator = Context::new("tenant", "evaluator", Role::Evaluator).unwrap();
    bump(&store, &admin, "role-watermark").await;
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let path = dir.path().join("import-management.sqlite3");
    let before = management_sql_snapshot(&path);
    let digest = hash(b"never-read");

    for payload in [
        Value::Null,
        register_payload(
            "role-register",
            &[("source-one", &digest, "claude_fixture")],
        ),
    ] {
        let error = dispatcher
            .submit(&evaluator, "import.register", payload)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Forbidden), "{error:?}");
    }
    assert_eq!(management_sql_snapshot(&path), before);
    store.close().await;
}

/// Every caller-selected path, role, owner, namespace, attestation and
/// authority boolean is an unknown wire field and is refused before any
/// session; an unknown reader value is likewise refused.
#[tokio::test]
async fn import_management_rejects_paths_roles_and_authority_booleans() {
    let (dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "reject-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("import-management.sqlite3");
    let digest = hash(b"unpublished");
    let before = management_sql_snapshot(&path);

    let base = register_payload(
        "reject-register",
        &[("source-one", &digest, "claude_fixture")],
    );
    for (field, value) in [
        ("roots", json!(["/tmp"])),
        ("path", json!("/tmp/x")),
        ("authorized_path", json!("/tmp/x")),
        ("outbound_authorized", json!(true)),
        ("allow_model_excerpts", json!(true)),
        ("retention_scope", json!("local_with_authorized_excerpts")),
        ("purpose", json!("generation")),
        ("role", json!("admin")),
        ("namespace", json!("other")),
        ("owner", json!("other")),
        ("task_origin", json!("trusted_run")),
        ("attestation", json!("trusted_host")),
        ("verified", json!(true)),
        ("generation_status", json!("ready")),
    ] {
        let mut payload = base.clone();
        payload[field] = value;
        let error = dispatcher
            .submit(&admin, "import.register", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "invalid management payload"),
            "{field}: {error:?}"
        );
    }
    let mut bad_reader = base.clone();
    bad_reader["sources"][0]["reader"] = json!("claude_live");
    assert!(matches!(
        dispatcher
            .submit(&admin, "import.register", bad_reader)
            .await
            .unwrap_err(),
        Error::Invalid(_)
    ));
    let mut bad_digest = base.clone();
    bad_digest["sources"][0]["blob_digest"] = json!("ABC");
    assert!(matches!(
        dispatcher
            .submit(&admin, "import.register", bad_digest)
            .await
            .unwrap_err(),
        Error::Invalid(_)
    ));

    let execute = execute_payload("reject-execute", "e16sel-not-a-real-selection");
    for (field, value) in [
        ("blob_digest", json!(digest)),
        ("reader", json!("claude_fixture")),
        ("result_id", json!("forged-result")),
        ("outbound_authorized", json!(true)),
        ("namespace", json!("other")),
    ] {
        let mut payload = execute.clone();
        payload[field] = value;
        let error = dispatcher
            .submit(&admin, "import.execute", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "invalid management payload"),
            "{field}: {error:?}"
        );
    }
    assert_eq!(management_sql_snapshot(&path), before);
    store.close().await;
}

/// Register then execute through the management surface: the execute loads the
/// real selection and pinned source bytes, re-derives the v2 result and stores
/// it with the real reader/normalization versions and blocked generation.
#[tokio::test]
async fn import_execute_loads_actual_selection_and_pinned_sources() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "execute-watermark").await;
    let claude_digest = put_blob(&store, &admin, CLAUDE).await;
    let pi_digest = put_blob(&store, &admin, PI).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let registered_status = done(
        &dispatcher,
        &admin,
        "import.register",
        register_payload(
            "execute-register",
            &[
                ("source-claude", &claude_digest, "claude_fixture"),
                ("source-pi", &pi_digest, "rsih_pi_fixture"),
            ],
        ),
    )
    .await;
    assert_eq!(registered_status.state, ManagementJobState::Succeeded);
    let (selection_id, _, source_count) = registered(&registered_status);
    assert_eq!(source_count, 2);

    let status = done(
        &dispatcher,
        &admin,
        "import.execute",
        execute_payload("execute-run", selection_id),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "import_completed");
    let ManagementImportResultV2::ImportCompleted {
        selection_id: executed_selection,
        result_id,
        result_digest,
        state,
        total_events,
        total_sources,
        locator_count,
        event_record_count,
        native_trace_count,
        coverage,
        reader_version,
        normalization_version,
        generation_status,
    } = completed(&status)
    else {
        unreachable!()
    };
    assert_eq!(executed_selection.as_str(), selection_id);
    assert_eq!(*state, ManagementImportResultState::Complete);
    assert_eq!(*total_sources, 2);
    assert_eq!(*total_events, 2);
    assert_eq!(*event_record_count, 2);
    assert_eq!(*native_trace_count, 0);
    assert_eq!(*locator_count, 0);
    assert_eq!(reader_version.as_str(), "rsia.e16.reader_set.v2");
    assert_eq!(
        normalization_version.as_str(),
        "rsia.e16.import_normalization.v2"
    );
    assert_eq!(
        generation_status.as_str(),
        "blocked_external_generation_conditions_unavailable"
    );
    assert!(coverage.discovery_exhausted && coverage.files_known);
    assert_eq!(coverage.parsed_ok, 2);
    assert_eq!(coverage.parsed_fail, 0);
    assert_eq!(coverage.missing, 0);
    assert_eq!(coverage.events_seen, 2);
    assert_eq!(coverage.events_exported, 2);
    assert_eq!(coverage.bytes_read, (CLAUDE.len() + PI.len()) as u64);

    let service = PersistentImportService::new(store.clone());
    let record = service.load_live_result(&admin, result_id).await.unwrap();
    assert_eq!(fingerprint(&record).unwrap(), *result_digest);
    assert_eq!(record.payload.selection_id, selection_id);
    assert_eq!(record.payload.event_records.len(), 2);
    assert_eq!(record.payload.aggregate_summary.total_events, 2);
    let selection = artifact(&store, &admin, selection_id).await;
    let bound: Vec<&str> = record
        .payload
        .event_records
        .iter()
        .map(|event| event.source_id.as_str())
        .collect();
    for source_id in selection["payload"]["import_source_ids"]
        .as_array()
        .unwrap()
    {
        assert!(
            bound.contains(&source_id.as_str().unwrap()),
            "every event binds a real source of the selection"
        );
    }
    assert_eq!(
        serde_json::to_value(&status.committed_side_effects).unwrap(),
        json!([{
            "kind": "import_result",
            "selection_id": selection_id,
            "result_id": result_id,
            "result_digest": result_digest,
        }])
    );
    store.close().await;
}

/// Replaying the exact same request key returns the same job, selection and
/// result and writes nothing new.
#[tokio::test]
async fn import_management_same_key_replays_exact_effect() {
    let (dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "replay-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("import-management.sqlite3");

    let payload = register_payload(
        "replay-register",
        &[("source-one", &digest, "claude_fixture")],
    );
    let first = done(&dispatcher, &admin, "import.register", payload.clone()).await;
    let before = management_sql_snapshot(&path);
    let second = done(&dispatcher, &admin, "import.register", payload).await;
    assert_eq!(second.id, first.id);
    assert_eq!(
        serde_json::to_value(&second.result).unwrap(),
        serde_json::to_value(&first.result).unwrap()
    );
    assert_eq!(management_sql_snapshot(&path), before);

    let (selection_id, _, _) = registered(&first);
    let execute = execute_payload("replay-execute", selection_id);
    let first_execute = done(&dispatcher, &admin, "import.execute", execute.clone()).await;
    let before = management_sql_snapshot(&path);
    let second_execute = done(&dispatcher, &admin, "import.execute", execute).await;
    assert_eq!(second_execute.id, first_execute.id);
    assert_eq!(
        serde_json::to_value(&second_execute.result).unwrap(),
        serde_json::to_value(&first_execute.result).unwrap()
    );
    assert_eq!(management_sql_snapshot(&path), before);
    store.close().await;
}

/// A reused request key with different content is refused at acceptance,
/// before any job, selection or blob read.
#[tokio::test]
async fn import_management_changed_key_payload_conflicts_without_io() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "conflict-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    done(
        &dispatcher,
        &admin,
        "import.register",
        register_payload(
            "conflict-register",
            &[("source-one", &digest, "claude_fixture")],
        ),
    )
    .await;
    let other = hash(b"never-published-other-blob");
    let error = dispatcher
        .submit(
            &admin,
            "import.register",
            register_payload(
                "conflict-register",
                &[("source-one", &other, "claude_fixture")],
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        artifacts
            .iter()
            .filter(|value| value["schema_version"] == "rsia.e16.source_selection.v1")
            .count(),
        1,
        "the conflicting key wrote no second selection"
    );
    store.close().await;
}

/// Namespace and owner are server facts, not wire fields: another namespace
/// cannot read the blob, another actor cannot execute the selection, and no
/// row appears under the spoofed identity.
#[tokio::test]
async fn import_management_namespace_and_owner_cannot_be_spoofed() {
    let (_dir, store) = store().await;
    let owner = admin();
    let other_namespace = Context::new("other", "admin", Role::Admin).unwrap();
    let other_actor = Context::new("tenant", "admin-two", Role::Admin).unwrap();
    bump(&store, &owner, "spoof-watermark").await;
    let digest = put_blob(&store, &owner, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(
        store.clone(),
        vec![owner.clone(), other_namespace.clone(), other_actor.clone()],
    )
    .unwrap();

    let registered_status = done(
        &dispatcher,
        &owner,
        "import.register",
        register_payload(
            "spoof-register",
            &[("source-one", &digest, "claude_fixture")],
        ),
    )
    .await;
    let (selection_id, _, _) = registered(&registered_status);

    // Another namespace names the digest: the blob is not reachable there.
    let cross = done(
        &dispatcher,
        &other_namespace,
        "import.register",
        register_payload(
            "spoof-cross-namespace",
            &[("source-one", &digest, "claude_fixture")],
        ),
    )
    .await;
    assert_eq!(cross.state, ManagementJobState::Failed, "{cross:?}");
    assert_eq!(cross.error_code.as_deref(), Some("not_found"));

    // Another actor of the same namespace names the selection.
    let cross_owner = done(
        &dispatcher,
        &other_actor,
        "import.execute",
        execute_payload("spoof-cross-owner", selection_id),
    )
    .await;
    assert_eq!(
        cross_owner.state,
        ManagementJobState::Failed,
        "{cross_owner:?}"
    );
    assert_eq!(cross_owner.error_code.as_deref(), Some("not_found"));

    let mut session = store.session().await.unwrap();
    let other_artifacts: Vec<Value> = session.list(&other_namespace, "artifact").await.unwrap();
    let other_actor_artifacts: Vec<Value> = session.list(&other_actor, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert!(
        other_artifacts.iter().all(|value| value["schema_version"]
            != "rsia.e16.source_selection.v1"
            && value["schema_version"] != "rsia.e16.import_result.v2"),
        "no spoofed selection/result in the other namespace"
    );
    assert!(
        other_actor_artifacts
            .iter()
            .all(|value| value["schema_version"] != "rsia.e16.import_result.v2"),
        "no spoofed result for the other actor"
    );
    store.close().await;
}

/// A revoked/advanced watermark refuses execute before its first import write;
/// no result row appears.
#[tokio::test]
async fn import_management_revoked_dependency_is_refused_before_write() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "revoke-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let registered_status = done(
        &dispatcher,
        &admin,
        "import.register",
        register_payload(
            "revoke-register",
            &[("source-one", &digest, "claude_fixture")],
        ),
    )
    .await;
    let (selection_id, _, _) = registered(&registered_status);

    // A revocation bumps the namespace watermark; the stored selection's
    // watermark no longer matches, so the execute writer refuses it.
    bump(&store, &admin, "revoked-after-register").await;
    let status = done(
        &dispatcher,
        &admin,
        "import.execute",
        execute_payload("revoke-execute", selection_id),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Failed, "{status:?}");
    assert_eq!(status.error_code.as_deref(), Some("conflict"));

    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert!(
        artifacts
            .iter()
            .all(|value| value["schema_version"] != "rsia.e16.import_result.v2"),
        "a refused execute leaves no result row"
    );
    store.close().await;
}

/// The wire cannot authorize outbound use or a generation scope; the real
/// selection stays LocalPrivate/Inspection without excerpts and the result's
/// generation status is the fixed blocked value.
#[tokio::test]
async fn import_outbound_material_requires_real_scope_grant() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "outbound-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let base = register_payload(
        "outbound-register",
        &[("source-one", &digest, "claude_fixture")],
    );
    for (field, value) in [
        ("outbound_authorized", json!(true)),
        ("allow_model_excerpts", json!(true)),
        ("retention_scope", json!("local_with_authorized_excerpts")),
        ("grant", json!({"scope": "outbound"})),
    ] {
        let mut payload = base.clone();
        payload[field] = value;
        assert!(matches!(
            dispatcher
                .submit(&admin, "import.register", payload)
                .await
                .unwrap_err(),
            Error::Invalid(_)
        ));
    }

    let status = done(&dispatcher, &admin, "import.register", base).await;
    let (selection_id, _, _) = registered(&status);
    let selection = artifact(&store, &admin, selection_id).await;
    assert_eq!(selection["payload"]["purpose"], "inspection");
    assert_eq!(selection["payload"]["retention_scope"], "local_private");
    assert_eq!(selection["payload"]["outbound_authorized"], false);
    assert_eq!(selection["payload"]["allow_model_excerpts"], false);

    let status = done(
        &dispatcher,
        &admin,
        "import.execute",
        execute_payload("outbound-execute", selection_id),
    )
    .await;
    let ManagementImportResultV2::ImportCompleted {
        result_id,
        generation_status,
        ..
    } = completed(&status)
    else {
        unreachable!()
    };
    assert_eq!(
        generation_status.as_str(),
        "blocked_external_generation_conditions_unavailable"
    );
    let service = PersistentImportService::new(store.clone());
    let record = service.load_live_result(&admin, result_id).await.unwrap();
    assert_eq!(
        record.payload.generation_status,
        "blocked_external_generation_conditions_unavailable"
    );
    for member in record.payload.evidence_set.as_ref().unwrap().members.iter() {
        assert_eq!(member.purpose, Purpose::Inspection);
        assert_eq!(
            member.execution_attestation,
            evo_core::evidence::ExecutionAttestation::UnverifiedImport
        );
    }
    assert_eq!(
        selection["payload"]["retention_scope"],
        serde_json::to_value(ImportRetentionScope::LocalPrivate).unwrap()
    );
    store.close().await;
}

// A read-only SQLite snapshot retains every table, row identity and cell type.
// Python's sqlite3 is already part of the repository's required test runtime.
fn management_sql_snapshot(path: &std::path::Path) -> Vec<u8> {
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", r#"
import json, pathlib, sqlite3, struct, sys
con = sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri() + '?mode=ro', uri=True)
con.execute('BEGIN')
def cell(v):
    if v is None: return ['null']
    if isinstance(v, int): return ['integer', str(v)]
    if isinstance(v, float): return ['real', struct.pack('>d', v).hex()]
    if isinstance(v, bytes): return ['blob', v.hex()]
    return ['text', v.encode('utf-8').hex()]
facts = []
for name, ddl in con.execute("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    table = '"' + name.replace('"', '""') + '"'
    columns = '*' if 'WITHOUT ROWID' in (ddl or '').upper() else 'rowid,*'
    rows = [[cell(v) for v in row] for row in con.execute('SELECT ' + columns + ' FROM ' + table)]
    rows.sort(key=lambda row: json.dumps(row, separators=(',', ':')))
    facts.append([name, ddl, rows])
print(json.dumps(facts, separators=(',', ':')))
con.close()
"#])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.is_empty());
    result.stdout
}

/// Managed sources persist the same identity formula as the legacy register.
/// The existing E16 dependency inspection re-derives it from
/// `source_spec(source) + raw_digest`; a different tuple would mark every
/// managed source `InputDigestMismatch` and leave the selection uncheckable.
#[tokio::test]
async fn import_managed_sources_match_the_legacy_input_digest_contract() {
    let (_dir, store) = store().await;
    let admin = admin();
    bump(&store, &admin, "digest-contract-watermark").await;
    let digest = put_blob(&store, &admin, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let status = done(
        &dispatcher,
        &admin,
        "import.register",
        register_payload(
            "digest-contract",
            &[("source-digest", &digest, "claude_fixture")],
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    let (selection_id, _, _) = registered(&status);

    let report =
        inspect_e16_content_edges(&admin, &store, selection_id, InspectionBudget::default())
            .await
            .unwrap();
    for object in &report.objects {
        assert!(
            object.issues.iter().all(|issue| !matches!(
                issue.code,
                IssueCode::InputDigestMismatch | IssueCode::AuthorityNotCheckable
            )),
            "{} has blocking issues: {:?}",
            object.id,
            object.issues
        );
    }
    let selection = report
        .objects
        .iter()
        .find(|object| object.id == selection_id)
        .expect("selection inspected");
    assert!(
        selection.declarations_checked,
        "managed selection was not checked: {selection:?}"
    );
    store.close().await;
}
