//! AG-166a recovery controls for the two import management operations.
//!
//! AG-155 semantics: a crash is reconciled, never re-run. Each import handler
//! writes its business rows, terminal job state and result in one transaction,
//! so a crash before the commit leaves no partial import row and a crash after
//! it is already terminal. These tests use real triggers and real stores; they
//! do not claim "resume" of a claimed job.
use evo_core::{Context, Role, hash, now};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementImportResultV2, ManagementJobState, ManagementJobStatusV1,
    ManagementJobV2, ManagementResultV2,
};
use evo_engine::import::PersistentImportService;
use evo_storage::Store;
use serde_json::{Value, json};

const CLAUDE: &[u8] = include_bytes!("../../../fixtures/imports/claude.fixture.jsonl");
const PI: &[u8] = include_bytes!("../../../fixtures/imports/rsih.pi.fixture.json");

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("import-recovery.sqlite3"))
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

fn register_payload(key: &str, digest: &str) -> Value {
    json!({
        "schema_version": "rsia.management.import_register.v1",
        "request_key": key,
        "sources": [{
            "source_id": "source-one",
            "blob_digest": digest,
            "reader": "claude_fixture",
        }],
    })
}

fn execute_payload(key: &str, selection_id: &str) -> Value {
    json!({
        "schema_version": "rsia.management.import_execute.v1",
        "request_key": key,
        "selection_id": selection_id,
    })
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

async fn done(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    operation: &str,
    payload: Value,
) -> ManagementJobStatusV1 {
    let queued = dispatcher.submit(ctx, operation, payload).await.unwrap();
    terminal(dispatcher, ctx, queued.identity().id).await
}

fn registered(status: &ManagementJobStatusV1) -> &str {
    match status.result.as_ref() {
        Some(ManagementResultV2::Import(ManagementImportResultV2::ImportRegistered {
            selection_id,
            ..
        })) => selection_id,
        other => panic!("expected ImportRegistered, got {other:?}"),
    }
}

fn completed(status: &ManagementJobStatusV1) -> &str {
    match status.result.as_ref() {
        Some(ManagementResultV2::Import(ManagementImportResultV2::ImportCompleted {
            result_id,
            ..
        })) => result_id,
        other => panic!("expected ImportCompleted, got {other:?}"),
    }
}

/// Every import artifact schema this feature can write.
const IMPORT_SCHEMAS: [&str; 3] = [
    "rsia.e16.source_selection.v1",
    "rsia.e16.import_source.v1",
    "rsia.e16.import_result.v2",
];

async fn import_rows(store: &Store, ctx: &Context) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    artifacts
        .into_iter()
        .filter(|value| {
            value["schema_version"]
                .as_str()
                .is_some_and(|schema| IMPORT_SCHEMAS.contains(&schema))
        })
        .collect()
}

fn sqlite(path: &std::path::Path, script: &str) {
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A real accepted-but-never-claimed job row is the deterministic fixture for
/// cancellation before any claim; it is exactly the queued shape `submit`
/// writes, with the same stable ids.
async fn accepted_register_job(store: &Store, ctx: &Context, key: &str) -> String {
    let owner = ctx.actor();
    let namespace = ctx.namespace();
    let job_digest = hash(format!("{namespace}\0{owner}\0import.register\0{key}").as_bytes());
    let id = format!("management-job-{}", &job_digest[..24]);
    let input_digest = hash(format!("{namespace}\0{owner}\0import.register\0{key}").as_bytes());
    let private_input_ref = format!("management-input-{}", &input_digest[..24]);
    let job = ManagementJobV2 {
        id: id.clone(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "import.register".into(),
        request_key: key.into(),
        payload_digest: hash(b"accepted-fixture"),
        owner_actor: owner.into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref,
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    let mut session = store.session().await.unwrap();
    session.put(ctx, "job", &id, owner, &job).await.unwrap();
    session.commit().await.unwrap();
    id
}

/// A failure after the source rows were staged still leaves no selection,
/// source or result row: the import phase and the job terminal state share the
/// one transaction.
#[tokio::test]
async fn import_phase_and_job_progress_commit_together() {
    let (dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "atomic-watermark").await;
    let digest = put_blob(&store, &ctx, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let path = dir.path().join("import-recovery.sqlite3");
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute(\"CREATE TRIGGER stop_selection BEFORE INSERT ON objects WHEN json_extract(NEW.body,'$.schema_version')='rsia.e16.source_selection.v1' BEGIN SELECT RAISE(ABORT,'injected-selection-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("atomic-register", &digest),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert!(
        import_rows(&store, &ctx).await.is_empty(),
        "no half-committed selection/source/result"
    );
    assert_eq!(
        store.read_blob(&ctx, &digest, CLAUDE.len()).await.unwrap(),
        CLAUDE
    );
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute('DROP TRIGGER stop_selection')\nc.commit()\nprint('ok')",
    );
    let healthy = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("atomic-register-healthy", &digest),
    )
    .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    let rows = import_rows(&store, &ctx).await;
    assert_eq!(
        rows.iter()
            .filter(|value| value["schema_version"] == "rsia.e16.source_selection.v1")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|value| value["schema_version"] == "rsia.e16.import_source.v1")
            .count(),
        1
    );
    store.close().await;
}

/// A restarted dispatcher reconciles terminal import jobs without re-reading
/// anything, and replaying the same keys returns the same job/selection/result
/// with the stored wire that names no client path.
#[tokio::test]
async fn import_restart_reconciles_terminal_jobs_without_client_path_reads() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "restart-watermark").await;
    let digest = put_blob(&store, &ctx, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let registered_status = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("restart-register", &digest),
    )
    .await;
    assert_eq!(registered_status.state, ManagementJobState::Succeeded);
    let selection_id = registered(&registered_status).to_owned();
    let executed_status = done(
        &dispatcher,
        &ctx,
        "import.execute",
        execute_payload("restart-execute", &selection_id),
    )
    .await;
    assert_eq!(executed_status.state, ManagementJobState::Succeeded);
    drop(dispatcher);

    let restarted = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
    assert_eq!(
        restarted.recover_pending().await.unwrap(),
        0,
        "terminal jobs are never re-run"
    );
    let replay_register = done(
        &restarted,
        &ctx,
        "import.register",
        register_payload("restart-register", &digest),
    )
    .await;
    assert_eq!(replay_register.id, registered_status.id);
    assert_eq!(
        serde_json::to_value(&replay_register.result).unwrap(),
        serde_json::to_value(&registered_status.result).unwrap()
    );
    let replay_execute = done(
        &restarted,
        &ctx,
        "import.execute",
        execute_payload("restart-execute", &selection_id),
    )
    .await;
    assert_eq!(replay_execute.id, executed_status.id);
    assert_eq!(
        serde_json::to_value(&replay_execute.result).unwrap(),
        serde_json::to_value(&executed_status.result).unwrap()
    );

    // The stored private request is the strict wire: no roots or paths.
    let mut session = store.session().await.unwrap();
    let input: Value = session
        .need(&ctx, "artifact", &replay_register.private_input_ref)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let wire = input["payload"].to_string();
    for forbidden in ["roots", "authorized_path", "\"path\"", "namespace"] {
        assert!(!wire.contains(forbidden), "wire holds {forbidden}: {wire}");
    }
    assert!(wire.contains("blob_digest"));
    assert_eq!(
        store.read_blob(&ctx, &digest, CLAUDE.len()).await.unwrap(),
        CLAUDE
    );
    store.close().await;
}

/// Two dispatchers racing on one request key produce one job, one selection
/// and one result; a later stale submit observes the same terminal job and
/// lease generation instead of claiming it again.
#[tokio::test]
async fn import_two_workers_and_old_lease_have_one_result() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "workers-watermark").await;
    let digest = put_blob(&store, &ctx, CLAUDE).await;
    let first = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let second = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();

    let register = register_payload("workers-register", &digest);
    let (a, b) = tokio::join!(
        first.submit(&ctx, "import.register", register.clone()),
        second.submit(&ctx, "import.register", register),
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.identity().id, b.identity().id);
    let finished = terminal(&first, &ctx, a.identity().id).await;
    assert_eq!(
        finished.state,
        ManagementJobState::Succeeded,
        "{finished:?}"
    );
    assert_eq!(finished.generation, 1);
    let selection_id = registered(&finished).to_owned();
    let rows = import_rows(&store, &ctx).await;
    assert_eq!(
        rows.iter()
            .filter(|value| value["schema_version"] == "rsia.e16.source_selection.v1")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|value| value["schema_version"] == "rsia.e16.import_source.v1")
            .count(),
        1
    );

    let execute = execute_payload("workers-execute", &selection_id);
    let (a, b) = tokio::join!(
        first.submit(&ctx, "import.execute", execute.clone()),
        second.submit(&ctx, "import.execute", execute),
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.identity().id, b.identity().id);
    let finished = terminal(&first, &ctx, a.identity().id).await;
    assert_eq!(
        finished.state,
        ManagementJobState::Succeeded,
        "{finished:?}"
    );
    assert_eq!(finished.generation, 1);
    let result_id = completed(&finished);
    let rows = import_rows(&store, &ctx).await;
    assert_eq!(
        rows.iter()
            .filter(|value| value["schema_version"] == "rsia.e16.import_result.v2")
            .count(),
        1
    );
    assert_eq!(
        rows.iter().filter(|value| value["id"] == result_id).count(),
        1
    );

    // The stale dispatcher's old lease observation cannot start a new claim:
    // same job id, same generation, same result.
    let replayed = second
        .submit(
            &ctx,
            "import.execute",
            execute_payload("workers-execute", &selection_id),
        )
        .await
        .unwrap();
    assert_eq!(replayed.identity().id, finished.id);
    assert_eq!(replayed.generation(), 1);
    assert_eq!(
        serde_json::to_value(replayed.result()).unwrap(),
        serde_json::to_value(finished.result.as_ref()).unwrap()
    );
    store.close().await;
}

/// Cancelling a never-claimed accepted job is closure before any effect: it
/// writes no import row and records `before_side_effect`.
#[tokio::test]
async fn import_cancel_before_effect_creates_no_import_rows() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "cancel-before-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let job_id = accepted_register_job(&store, &ctx, "cancel-before-effect").await;
    let cancelled = dispatcher.cancel(&ctx, &job_id).await.unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Cancelled,
        "{cancelled:?}"
    );
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert!(import_rows(&store, &ctx).await.is_empty());
    store.close().await;
}

/// A terminal register/execute result keeps its facts; cancelling afterwards
/// only marks `after_side_effect` and starts no new import effect.
#[tokio::test]
async fn import_cancel_after_terminal_register_marks_after_side_effect() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "cancel-after-watermark").await;
    let digest = put_blob(&store, &ctx, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let registered_status = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("cancel-after-register", &digest),
    )
    .await;
    let selection_id = registered(&registered_status).to_owned();
    let before = import_rows(&store, &ctx).await;
    let cancelled = dispatcher
        .cancel(&ctx, &registered_status.id)
        .await
        .unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Succeeded,
        "a completed import is an audit fact"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(registered_status.result.as_ref()).unwrap()
    );
    assert_eq!(import_rows(&store, &ctx).await, before);

    let executed_status = done(
        &dispatcher,
        &ctx,
        "import.execute",
        execute_payload("cancel-after-execute", &selection_id),
    )
    .await;
    let before = import_rows(&store, &ctx).await;
    let cancelled = dispatcher.cancel(&ctx, &executed_status.id).await.unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Succeeded);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(executed_status.result.as_ref()).unwrap()
    );
    assert_eq!(import_rows(&store, &ctx).await, before);
    store.close().await;
}

/// A crash before the commit leaves no partial row; retrying the same blob
/// digest converges to the same event normalization.
#[tokio::test]
async fn import_crash_before_commit_leaves_no_partial_rows_and_same_digest_converges() {
    let (dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "crash-watermark").await;
    let digest = put_blob(&store, &ctx, CLAUDE).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let path = dir.path().join("import-recovery.sqlite3");
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute(\"CREATE TRIGGER stop_source BEFORE INSERT ON objects WHEN json_extract(NEW.body,'$.schema_version')='rsia.e16.import_source.v1' BEGIN SELECT RAISE(ABORT,'injected-source-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("crash-register", &digest),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert!(import_rows(&store, &ctx).await.is_empty());
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute('DROP TRIGGER stop_source')\nc.commit()\nprint('ok')",
    );

    async fn events_of(
        dispatcher: &ManagementDispatcher,
        ctx: &Context,
        store: &Store,
        key: &str,
        digest: &str,
    ) -> Vec<(usize, usize, usize, String, String, String)> {
        let registered_status = done(
            dispatcher,
            ctx,
            "import.register",
            register_payload(key, digest),
        )
        .await;
        assert_eq!(registered_status.state, ManagementJobState::Succeeded);
        let selection_id = registered(&registered_status).to_owned();
        let executed_status = done(
            dispatcher,
            ctx,
            "import.execute",
            execute_payload(&format!("{key}-execute"), &selection_id),
        )
        .await;
        assert_eq!(executed_status.state, ManagementJobState::Succeeded);
        let result_id = completed(&executed_status).to_owned();
        let service = PersistentImportService::new(store.clone());
        let record = service.load_live_result(ctx, &result_id).await.unwrap();
        record
            .payload
            .event_records
            .iter()
            .map(|event| {
                (
                    event.event_index,
                    event.byte_start,
                    event.byte_end,
                    event.raw_fragment_digest.clone(),
                    event.normalized_content_digest.clone(),
                    event.role.clone(),
                )
            })
            .collect()
    }

    let first = events_of(&dispatcher, &ctx, &store, "crash-retry-one", &digest).await;
    let second = events_of(&dispatcher, &ctx, &store, "crash-retry-two", &digest).await;
    assert_eq!(first, second, "the same blob digest converges");
    assert!(!first.is_empty());
    assert_eq!(
        store.read_blob(&ctx, &digest, CLAUDE.len()).await.unwrap(),
        CLAUDE
    );
    store.close().await;
}

/// A deleted blob or an advanced revocation watermark blocks a stale
/// completion before the result row is written.
#[tokio::test]
async fn import_changed_source_or_watermark_blocks_stale_completion() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "stale-watermark").await;
    let claude_digest = put_blob(&store, &ctx, CLAUDE).await;
    let pi_digest = put_blob(&store, &ctx, PI).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();

    let registered_status = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload("stale-deleted", &claude_digest),
    )
    .await;
    let selection_id = registered(&registered_status).to_owned();
    store.delete_blob(&ctx, &claude_digest).await.unwrap();
    let failed = done(
        &dispatcher,
        &ctx,
        "import.execute",
        execute_payload("stale-deleted-execute", &selection_id),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("not_found"));
    assert!(
        import_rows(&store, &ctx)
            .await
            .iter()
            .all(|value| value["schema_version"] != "rsia.e16.import_result.v2"),
        "a missing pinned blob completes no stale result"
    );

    let mut session = store.session().await.unwrap();
    session
        .put(
            &ctx,
            "artifact",
            "unused-marker",
            ctx.actor(),
            &json!({"schema_version": "rsia.marker.v1", "id": "unused-marker"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let registered_status = done(
        &dispatcher,
        &ctx,
        "import.register",
        register_payload_with_source(
            "stale-watermark",
            &pi_digest,
            "source-pi",
            "rsih_pi_fixture",
        ),
    )
    .await;
    assert_eq!(registered_status.state, ManagementJobState::Succeeded);
    let selection_id = registered(&registered_status).to_owned();
    bump(&store, &ctx, "stale-revoked").await;
    let failed = done(
        &dispatcher,
        &ctx,
        "import.execute",
        execute_payload("stale-watermark-execute", &selection_id),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert!(
        import_rows(&store, &ctx)
            .await
            .iter()
            .all(|value| value["schema_version"] != "rsia.e16.import_result.v2"),
        "a stale watermark completes no result"
    );
    store.close().await;
}

fn register_payload_with_source(key: &str, digest: &str, source_id: &str, reader: &str) -> Value {
    json!({
        "schema_version": "rsia.management.import_register.v1",
        "request_key": key,
        "sources": [{
            "source_id": source_id,
            "blob_digest": digest,
            "reader": reader,
        }],
    })
}
