//! Explicit Admin retries of real failed cleanup jobs; no SQL job-state mutation.
//! Object/edge corruption below is labelled as a test fixture, not trusted input.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::{Path, PathBuf};

const NS: &str = "retry-tenant";
const RETRY: &str = "cleanup_retry_started";

fn context(namespace: &str, role: Role) -> Context {
    Context::new(namespace, format!("{role:?}"), role).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    store: Store,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("rsia.sqlite3");
        let store = Store::open(&path).await.unwrap();
        Self {
            _dir: dir,
            root,
            path,
            store,
        }
    }

    async fn reopen(&mut self) {
        self.store.close().await;
        self.store = Store::open(&self.path).await.unwrap();
    }
}

fn fact(id: &str) -> Value {
    json!({"id":id,"schema_version":"rsia.optimization.stage_fact.v1",
        "kind":"terminal_rejected","payload":format!("SECRET-{id}")})
}

fn unknown(id: &str) -> Value {
    json!({"id":id,"schema_version":"rsia.unknown.v9","payload":format!("SECRET-{id}")})
}

async fn put(
    store: &Store,
    namespace: &str,
    kind: &str,
    id: &str,
    body: &Value,
    deps: &[(&str, &str)],
) {
    let ctx = context(namespace, Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .put(&ctx, kind, id, ctx.actor(), body)
        .await
        .unwrap();
    for (dst_kind, dst_id) in deps {
        session
            .put_edge(&ctx, kind, id, dst_kind, dst_id)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

async fn source(store: &Store, namespace: &str, id: &str) {
    put(
        store,
        namespace,
        "run",
        id,
        &json!({"id":id,"body":"source content"}),
        &[],
    )
    .await;
}

async fn begin(store: &Store, namespace: &str, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &context(namespace, Role::Admin),
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: id.into(),
        },
        "explicit retry fixture",
        10,
    )
    .await
    .unwrap()
}

async fn page(store: &Store, namespace: &str, role: Role, job: &str, now: i64) -> CleanupStatus {
    LifecycleStore::cleanup_step(&context(namespace, role), store, job, 1, now)
        .await
        .unwrap()
}

async fn drain_failed(store: &Store, namespace: &str, mut status: CleanupStatus) -> CleanupStatus {
    // Worker retains the existing ability to drain a Failed job's pending queue,
    // but must never initiate a retry round, so every old node is truly expanded.
    for now in 11..211 {
        if status.pending_nodes == 0 {
            break;
        }
        status = page(store, namespace, Role::Worker, &status.job_id, now).await;
    }
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    assert_eq!(status.pending_nodes, 0, "{status:?}");
    status
}

async fn retry(fixture: &mut Fixture, namespace: &str, job: &str) -> CleanupStatus {
    let mut status = page(&fixture.store, namespace, Role::Admin, job, 300).await;
    for now in 301..601 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            return status;
        }
        fixture.reopen().await;
        let persisted =
            LifecycleStore::cleanup_status(&context(namespace, Role::Admin), &fixture.store, job)
                .await
                .unwrap();
        assert_eq!(persisted.state, status.state);
        assert_eq!(persisted.processed_nodes, status.processed_nodes);
        status = page(&fixture.store, namespace, Role::Admin, job, now).await;
    }
    panic!("retry did not terminate: {status:?}");
}

async fn connection(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

async fn snapshot(path: &Path, namespace: &str) -> Value {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await
            .unwrap();
    let queries = [
        (
            "jobs",
            "SELECT json_group_array(json_object('job_id',job_id,'source_kind',source_kind,'source_id',source_id,'watermark_seq',watermark_seq,'watermark_digest',watermark_digest,'state',state,'processed_nodes',processed_nodes,'last_error',last_error,'created_at',created_at)) FROM (SELECT * FROM revoke_cleanup_jobs WHERE namespace=? ORDER BY job_id)",
        ),
        (
            "frontier",
            "SELECT json_group_array(json_object('seq',seq,'job_id',job_id,'kind',node_kind,'id',node_id,'expanded',expanded,'cursor_kind',cursor_src_kind,'cursor_id',cursor_src_id)) FROM (SELECT * FROM revoke_cleanup_frontier WHERE namespace=? ORDER BY seq)",
        ),
        (
            "events",
            "SELECT json_group_array(json_object('seq',seq,'job_id',job_id,'kind',node_kind,'id',node_id,'event_kind',event_kind,'details',json(details))) FROM (SELECT * FROM revoke_cleanup_events WHERE namespace=? ORDER BY seq)",
        ),
        (
            "objects",
            "SELECT json_group_array(json_object('kind',kind,'id',id,'body',json(body))) FROM (SELECT * FROM objects WHERE namespace=? ORDER BY kind,id)",
        ),
        (
            "cache",
            "SELECT json_group_array(json_object('subject',subject_id,'response',response,'redacted',redacted)) FROM (SELECT * FROM idempotency WHERE namespace=? ORDER BY actor,operation,request_key)",
        ),
        (
            "watermark",
            "SELECT json_group_array(json_object('seq',seq,'digest',digest)) FROM revoke_watermark WHERE namespace=?",
        ),
        (
            "edges",
            "SELECT json_group_array(json_object('src_kind',src_kind,'src_id',src_id,'dst_kind',dst_kind,'dst_id',dst_id)) FROM (SELECT * FROM dependencies WHERE namespace=? ORDER BY src_kind,src_id,dst_kind,dst_id)",
        ),
    ];
    let mut result = serde_json::Map::new();
    for (name, query) in queries {
        let encoded: String = sqlx::query_scalar(query)
            .bind(namespace)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        result.insert(name.into(), serde_json::from_str(&encoded).unwrap());
    }
    result.into()
}

async fn value(store: &Store, namespace: &str, kind: &str, id: &str) -> Option<Value> {
    let mut session = store.session().await.unwrap();
    let body = session
        .get(&context(namespace, Role::Admin), kind, id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    body
}

fn events<'a>(snapshot: &'a Value, job: &str, kind: &str) -> Vec<&'a Value> {
    snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["job_id"] == job && event["event_kind"] == kind)
        .collect()
}

async fn failed_graph(fixture: &Fixture) -> CleanupStatus {
    for id in ["primary", "secondary"] {
        source(&fixture.store, NS, id).await;
    }
    for id in ["unknown-a", "unknown-b", "unknown-c"] {
        put(
            &fixture.store,
            NS,
            "artifact",
            id,
            &unknown(id),
            &[("run", "primary"), ("run", "secondary")],
        )
        .await;
    }
    drain_failed(
        &fixture.store,
        NS,
        begin(&fixture.store, NS, "secondary").await,
    )
    .await
}

async fn legalize(store: &Store, ids: &[&str]) {
    // Repair fixture bodies only; never change job state, frontier or old events.
    for id in ids {
        put(store, NS, "artifact", id, &fact(id), &[]).await;
    }
}

#[tokio::test]
async fn admin_retries_all_expanded_nodes_once_and_preserves_history_identity_cache_and_isolation()
{
    let mut f = Fixture::new().await;
    source(&f.store, "other", "secondary").await;
    put(
        &f.store,
        "other",
        "artifact",
        "unknown-a",
        &unknown("unknown-a"),
        &[("run", "secondary")],
    )
    .await;
    let old = failed_graph(&f).await;
    let before = snapshot(&f.path, NS).await;
    let other = snapshot(&f.path, "other").await;
    assert_eq!(
        events(&before, &old.job_id, "blocked_unknown_scope").len(),
        3
    );
    assert!(
        before["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["expanded"] == 1)
    );
    legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
    let mut s = f.store.session().await.unwrap();
    s.cache(
        &context(NS, Role::Admin),
        "fixture",
        "old-cache",
        &1,
        "unknown-a",
        &fact("unknown-a"),
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    let worker = page(&f.store, NS, Role::Worker, &old.job_id, 250).await;
    assert_eq!(worker.state, CleanupState::Failed);
    assert_eq!(worker.processed_nodes, old.processed_nodes);
    assert_eq!(
        events(&snapshot(&f.path, NS).await, &old.job_id, RETRY).len(),
        0
    );
    assert!(matches!(
        LifecycleStore::cleanup_step(
            &context("other", Role::Admin),
            &f.store,
            &old.job_id,
            1,
            260
        )
        .await,
        Err(Error::NotFound)
    ));
    let running = page(&f.store, NS, Role::Admin, &old.job_id, 300).await;
    assert_eq!(running.state, CleanupState::Running, "{running:?}");
    assert_eq!(running.last_error, old.last_error);
    assert_eq!(
        running.processed_nodes, 1,
        "new round counts only its own nodes"
    );
    let started = snapshot(&f.path, NS).await;
    let retry_events = events(&started, &old.job_id, RETRY);
    assert_eq!(retry_events.len(), 1);
    assert_eq!(
        retry_events[0]["details"]["previous_processed_nodes"],
        old.processed_nodes
    );
    assert_eq!(
        retry_events[0]["details"]["previous_last_error"],
        json!(old.last_error)
    );
    let complete = retry(&mut f, NS, &old.job_id).await;
    assert_eq!(complete.state, CleanupState::Complete, "{complete:?}");
    assert!(complete.last_error.is_none());
    assert_eq!(complete.processed_nodes, old.processed_nodes);
    assert_eq!(complete.source, old.source);
    assert_eq!(complete.watermark_seq, old.watermark_seq);
    assert_eq!(complete.watermark_digest, old.watermark_digest);
    assert!(value(&f.store, NS, "run", "secondary").await.is_none());
    assert!(value(&f.store, NS, "run", "primary").await.is_some());
    for id in ["unknown-a", "unknown-b", "unknown-c"] {
        let redacted = value(&f.store, NS, "artifact", id).await.unwrap();
        assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
        assert!(!redacted.to_string().contains("SECRET"));
    }
    let after = snapshot(&f.path, NS).await;
    assert_eq!(after["watermark"], before["watermark"]);
    assert_eq!(after["edges"], before["edges"]);
    assert_eq!(
        after["jobs"][0]["created_at"],
        before["jobs"][0]["created_at"]
    );
    assert_eq!(
        events(&after, &old.job_id, "blocked_unknown_scope").len(),
        3
    );
    assert_eq!(events(&after, &old.job_id, RETRY).len(), 1);
    assert_eq!(after["cache"][0]["response"], "null");
    assert_eq!(after["cache"][0]["redacted"], 1);
    assert_eq!(snapshot(&f.path, "other").await, other);
    for now in 700..703 {
        f.reopen().await;
        assert_eq!(
            page(&f.store, NS, Role::Admin, &old.job_id, now)
                .await
                .state,
            CleanupState::Complete
        );
        assert_eq!(snapshot(&f.path, NS).await, after);
    }
}

#[tokio::test]
async fn repairing_only_the_last_error_node_cannot_hide_earlier_unknowns() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-c"]).await;
    let retried = retry(&mut f, NS, &old.job_id).await;
    assert_eq!(retried.state, CleanupState::Failed);
    assert!(
        retried
            .last_error
            .as_deref()
            .unwrap()
            .contains("rsia.unknown.v9")
    );
    assert_eq!(
        value(&f.store, NS, "artifact", "unknown-a").await.unwrap(),
        unknown("unknown-a")
    );
    // Drain the current round with Worker, then another explicit Admin retry
    // must genuinely rescan; a permanently unknown wire never becomes trusted.
    let retried = drain_failed(&f.store, NS, retried).await;
    let again = retry(&mut f, NS, &retried.job_id).await;
    assert_eq!(again.state, CleanupState::Failed);
    assert!(events(&snapshot(&f.path, NS).await, &old.job_id, RETRY).len() >= 2);
}

#[tokio::test]
async fn retry_fixpoint_catches_edges_before_a_cursor_and_after_a_reexpanded_parent() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
    for id in ["child-b", "child-d"] {
        put(
            &f.store,
            NS,
            "artifact",
            id,
            &fact(id),
            &[("artifact", "unknown-a")],
        )
        .await;
    }
    let mut status = page(&f.store, NS, Role::Admin, &old.job_id, 300).await;
    for now in 301..450 {
        let snap = snapshot(&f.path, NS).await;
        let row = snap["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "unknown-a")
            .unwrap();
        if row["cursor_id"] == "child-b" {
            break;
        }
        assert_eq!(status.state, CleanupState::Running);
        status = page(&f.store, NS, Role::Admin, &old.job_id, now).await;
    }
    assert!(
        snapshot(&f.path, NS).await["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "unknown-a" && row["cursor_id"] == "child-b")
    );
    put(
        &f.store,
        NS,
        "artifact",
        "child-a",
        &fact("child-a"),
        &[("artifact", "unknown-a")],
    )
    .await;
    while value(&f.store, NS, "artifact", "unknown-a").await.unwrap()["schema_version"]
        != "rsia.redacted.v1"
    {
        status = page(&f.store, NS, Role::Admin, &old.job_id, 450).await;
        assert_eq!(status.state, CleanupState::Running);
    }
    put(
        &f.store,
        NS,
        "artifact",
        "child-late",
        &fact("child-late"),
        &[("artifact", "unknown-a")],
    )
    .await;
    let complete = retry(&mut f, NS, &old.job_id).await;
    assert_eq!(complete.state, CleanupState::Complete);
    for id in ["child-a", "child-b", "child-d", "child-late"] {
        assert_eq!(
            value(&f.store, NS, "artifact", id).await.unwrap()["schema_version"],
            "rsia.redacted.v1"
        );
    }
}

#[tokio::test]
async fn a_new_unknown_added_during_the_retry_fails_the_current_round() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
    page(&f.store, NS, Role::Admin, &old.job_id, 300).await;
    put(
        &f.store,
        NS,
        "artifact",
        "new-unknown",
        &unknown("new-unknown"),
        &[("run", "secondary")],
    )
    .await;
    let result = retry(&mut f, NS, &old.job_id).await;
    assert_eq!(result.state, CleanupState::Failed);
    assert_eq!(
        value(&f.store, NS, "artifact", "new-unknown")
            .await
            .unwrap(),
        unknown("new-unknown")
    );
}

fn export(id: &str, namespace: &str) -> Value {
    json!({"schema_version":"rsia.e16.export_attempt.v2","id":id,"namespace":namespace,
        "owner_actor":"admin","request_key":format!("export-{id}"),"input_digest":hash(id.as_bytes()),
        "created_at":1,"updated_at":1,"source_refs":[],"revoke_watermark":1,
        "payload":{"state":"prepared","private_text":format!("EXPORT-SECRET-{id}")}})
}

fn namespace_root(f: &Fixture, namespace: &str) -> PathBuf {
    f.root.join("exports").join(hash(namespace.as_bytes()))
}

#[cfg(unix)]
async fn export_failure_then_retry(absent: bool) {
    let mut f = Fixture::new().await;
    source(&f.store, NS, "source-a").await;
    source(&f.store, NS, "source-b").await;
    let root = namespace_root(&f, NS);
    std::fs::create_dir_all(&root).unwrap();
    let outside = f.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("canary"), b"external-canary").unwrap();
    let other = namespace_root(&f, "other").join("X");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("canary"), b"other-namespace").unwrap();
    for id in ["X", "Y"] {
        put(
            &f.store,
            NS,
            "artifact",
            id,
            &export(id, NS),
            &[("run", "source-b")],
        )
        .await;
    }
    put(
        &f.store,
        NS,
        "artifact",
        "X",
        &export("X", NS),
        &[("run", "source-a")],
    )
    .await;
    std::os::unix::fs::symlink(&outside, root.join("X")).unwrap();
    let a = drain_failed(&f.store, NS, begin(&f.store, NS, "source-a").await).await;
    let redacted_x = value(&f.store, NS, "artifact", "X").await.unwrap();
    assert_eq!(redacted_x["schema_version"], "rsia.redacted.v1");
    assert_eq!(
        events(
            &snapshot(&f.path, NS).await,
            &a.job_id,
            "blocked_unknown_scope"
        )
        .len(),
        1
    );
    // While B traverses X it is safely absent. B's sole failed node is Y;
    // X's earlier failure belongs to A, so a query restricted to B will miss it.
    std::fs::remove_file(root.join("X")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("Y")).unwrap();
    let b = drain_failed(&f.store, NS, begin(&f.store, NS, "source-b").await).await;
    let before = snapshot(&f.path, NS).await;
    let b_errors = events(&before, &b.job_id, "blocked_unknown_scope");
    assert_eq!(b_errors.len(), 1);
    assert_eq!(b_errors[0]["id"], "Y");
    std::fs::remove_file(root.join("Y")).unwrap();
    std::fs::create_dir(root.join("Y")).unwrap();
    std::fs::write(root.join("Y").join("payload"), b"controlled payload").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("X")).unwrap();
    if absent {
        let mut session = f.store.session().await.unwrap();
        session
            .delete(&context(NS, Role::Admin), "artifact", "X")
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    let blocked = retry(&mut f, NS, &b.job_id).await;
    assert_eq!(
        blocked.state,
        CleanupState::Failed,
        "unresolved X directory must not be hidden: {blocked:?}"
    );
    assert!(
        blocked
            .last_error
            .as_deref()
            .unwrap()
            .contains("invalid_local_export_directory")
    );
    assert_eq!(
        std::fs::read(outside.join("canary")).unwrap(),
        b"external-canary"
    );
    assert_eq!(
        std::fs::read(other.join("canary")).unwrap(),
        b"other-namespace"
    );
    if !absent {
        assert_eq!(
            value(&f.store, NS, "artifact", "X").await.unwrap(),
            redacted_x
        );
    }
    // Resolve the obstacle without traversing the linked target; cleanup removes
    // only the new safe controlled directory and keeps historical failure facts.
    std::fs::remove_file(root.join("X")).unwrap();
    std::fs::create_dir(root.join("X")).unwrap();
    std::fs::write(root.join("X").join("payload"), b"controlled payload").unwrap();
    let complete = retry(&mut f, NS, &b.job_id).await;
    assert_eq!(complete.state, CleanupState::Complete);
    assert!(complete.last_error.is_none());
    assert!(!root.join("X").exists() && !root.join("Y").exists());
    assert_eq!(
        std::fs::read(outside.join("canary")).unwrap(),
        b"external-canary"
    );
    assert_eq!(
        std::fs::read(other.join("canary")).unwrap(),
        b"other-namespace"
    );
    if !absent {
        assert_eq!(
            value(&f.store, NS, "artifact", "X").await.unwrap(),
            redacted_x
        );
    }
    assert_eq!(
        retry(&mut f, NS, &a.job_id).await.state,
        CleanupState::Complete
    );
    let after = snapshot(&f.path, NS).await;
    assert!(!events(&after, &a.job_id, "blocked_unknown_scope").is_empty());
    assert!(events(&after, &b.job_id, "blocked_unknown_scope").len() >= 2);
    page(&f.store, NS, Role::Admin, &b.job_id, 800).await;
    assert_eq!(snapshot(&f.path, NS).await, after);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_job_redacted_export_failure_is_rechecked_before_complete() {
    export_failure_then_retry(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cross_job_absent_export_failure_is_rechecked_before_complete() {
    export_failure_then_retry(true).await;
}

#[cfg(unix)]
#[tokio::test]
async fn ancestor_then_directory_errors_remain_historical_and_do_not_permanently_block_safe_retry()
{
    let mut f = Fixture::new().await;
    source(&f.store, NS, "source").await;
    put(
        &f.store,
        NS,
        "artifact",
        "X",
        &export("X", NS),
        &[("run", "source")],
    )
    .await;
    let outside = f.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("canary"), b"canary").unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("exports")).unwrap();
    let failed = drain_failed(&f.store, NS, begin(&f.store, NS, "source").await).await;
    assert!(
        failed
            .last_error
            .as_deref()
            .unwrap()
            .contains("linked_local_export_ancestor")
    );
    let original = value(&f.store, NS, "artifact", "X").await.unwrap();
    std::fs::remove_file(f.root.join("exports")).unwrap();
    let root = namespace_root(&f, NS);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("X"), b"not a directory").unwrap();
    let failed = retry(&mut f, NS, &failed.job_id).await;
    assert_eq!(failed.state, CleanupState::Failed);
    assert!(
        failed
            .last_error
            .as_deref()
            .unwrap()
            .contains("invalid_local_export_directory")
    );
    std::fs::remove_file(root.join("X")).unwrap();
    assert_eq!(
        retry(&mut f, NS, &failed.job_id).await.state,
        CleanupState::Complete
    );
    assert_eq!(
        value(&f.store, NS, "artifact", "X").await.unwrap(),
        original
    );
    let snap = snapshot(&f.path, NS).await;
    let errors = events(&snap, &failed.job_id, "blocked_unknown_scope");
    assert!(
        errors.iter().any(|event| event["details"]["error"]
            == "blocked_unknown_scope:linked_local_export_ancestor")
    );
    assert!(
        errors.iter().any(|event| event["details"]["error"]
            == "blocked_unknown_scope:invalid_local_export_directory")
    );
    assert_eq!(std::fs::read(outside.join("canary")).unwrap(), b"canary");
}

#[tokio::test]
async fn invalid_export_id_and_wrong_namespace_stay_failed_without_touching_canaries() {
    for (id, namespace) in [("..", NS), ("X", "other")] {
        let mut f = Fixture::new().await;
        source(&f.store, NS, "source").await;
        put(
            &f.store,
            NS,
            "artifact",
            id,
            &export(id, namespace),
            &[("run", "source")],
        )
        .await;
        let canary = namespace_root(&f, "other").join("X").join("canary");
        std::fs::create_dir_all(canary.parent().unwrap()).unwrap();
        std::fs::write(&canary, b"canary").unwrap();
        let old = drain_failed(&f.store, NS, begin(&f.store, NS, "source").await).await;
        assert_eq!(
            retry(&mut f, NS, &old.job_id).await.state,
            CleanupState::Failed
        );
        assert_eq!(std::fs::read(&canary).unwrap(), b"canary");
    }
}

#[tokio::test]
async fn another_namespace_failure_event_does_not_delete_an_absent_same_id_node_directory() {
    let mut f = Fixture::new().await;
    source(&f.store, "other", "source").await;
    put(
        &f.store,
        "other",
        "artifact",
        "..",
        &export("..", "other"),
        &[("run", "source")],
    )
    .await;
    drain_failed(&f.store, "other", begin(&f.store, "other", "source").await).await;
    // A legal same ID is needed for a real historical directory failure.
    source(&f.store, "other", "source-two").await;
    put(
        &f.store,
        "other",
        "artifact",
        "X",
        &export("X", "other"),
        &[("run", "source-two")],
    )
    .await;
    let other_root = namespace_root(&f, "other");
    std::fs::create_dir_all(&other_root).unwrap();
    std::fs::write(other_root.join("X"), b"bad directory").unwrap();
    drain_failed(
        &f.store,
        "other",
        begin(&f.store, "other", "source-two").await,
    )
    .await;
    source(&f.store, NS, "source").await;
    let mut session = f.store.session().await.unwrap();
    session
        .put_edge(&context(NS, Role::Admin), "artifact", "X", "run", "source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let own_canary = namespace_root(&f, NS).join("X").join("canary");
    std::fs::create_dir_all(own_canary.parent().unwrap()).unwrap();
    std::fs::write(&own_canary, b"keep absent node directory").unwrap();
    let status = begin(&f.store, NS, "source").await;
    assert_eq!(
        retry(&mut f, NS, &status.job_id).await.state,
        CleanupState::Complete
    );
    assert_eq!(
        std::fs::read(own_canary).unwrap(),
        b"keep absent node directory"
    );
}

async fn budget_call(store: &Store, scope: &str, id: &str, schema: &str) {
    store
        .authorize_root_budget(
            &context(NS, Role::Admin),
            &RootBudgetAuthorization {
                root_budget_id: format!("root-{scope}"),
                billing_scope: scope.into(),
                allowed_namespaces: vec![NS.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let reservation = BudgetCallReservation {
        billing_scope: scope.into(),
        call_id: id.into(),
        dispatch_group_id: format!("group-{scope}"),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{scope}-{id}").as_bytes()),
        request_artifact: Some(
            BudgetArtifact::from_serializable(
                schema,
                &json!({"source_closure":[],"input":"BUDGET-SECRET"}),
            )
            .unwrap(),
        ),
        max_cost_micros: 20,
        lease_token: format!("lease-{scope}"),
        lease_until: 100,
        now: 1,
    };
    let call = store
        .reserve_budget_call(&context(NS, Role::Host), &reservation)
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: scope.into(),
        call_id: id.into(),
        actual_input_digest: call.actual_input_digest,
        lease_token: call.lease_token,
        lease_epoch: call.lease_epoch,
        now: 2,
    };
    store
        .begin_budget_dispatch(&context(NS, Role::Host), &fence)
        .await
        .unwrap();
    store
        .finalize_budget_call(
            &context(NS, Role::Host),
            &BudgetCallFence { now: 3, ..fence },
            &UsageCharge {
                amount_micros: 7,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: format!("provider-{scope}"),
                usage_record_id: format!("usage-{scope}"),
                output_digest: hash(b"actual fixture output"),
            },
        )
        .await
        .unwrap();
}

async fn accounting(path: &Path) -> Value {
    let mut c = connection(path).await;
    let rows = sqlx::query("SELECT billing_scope,call_id,state,reserved_micros,actual_cost_micros,dispatch_id,provider_request_id,usage_record_id,output_digest,lease_epoch FROM root_budget_calls ORDER BY billing_scope,call_id").fetch_all(&mut c).await.unwrap();
    let calls:Vec<Value> = rows.iter().map(|r| json!({"scope":r.get::<String,_>("billing_scope"),"id":r.get::<String,_>("call_id"),
        "state":r.get::<String,_>("state"),"reserved":r.get::<i64,_>("reserved_micros"),"charged":r.get::<Option<i64>,_>("actual_cost_micros"),
        "dispatch":r.get::<Option<String>,_>("dispatch_id"),"provider":r.get::<Option<String>,_>("provider_request_id"),
        "usage":r.get::<Option<String>,_>("usage_record_id"),"output":r.get::<Option<String>,_>("output_digest"),"epoch":r.get::<i64,_>("lease_epoch")})).collect();
    let roots:String = sqlx::query_scalar("SELECT json_group_array(json_object('scope',billing_scope,'spent',spent_micros,'reserved',reserved_micros)) FROM (SELECT * FROM root_budgets ORDER BY billing_scope)").fetch_one(&mut c).await.unwrap();
    let groups: String = sqlx::query_scalar("SELECT json_group_array(json_object('scope',billing_scope,'group',dispatch_group_id,'owner',owner_namespace,'stopped',stopped,'reason',stop_reason,'committed_at',stop_committed_at,'created_at',created_at)) FROM (SELECT * FROM root_budget_dispatch_groups ORDER BY billing_scope,dispatch_group_id)").fetch_one(&mut c).await.unwrap();
    // Content-redaction events are new cleanup verification, while all actual
    // dispatch, settlement and budget facts must survive byte-for-byte.
    let historical_events: String = sqlx::query_scalar("SELECT json_group_array(json_object('seq',seq,'scope',billing_scope,'call',call_id,'kind',event_kind,'at',event_at,'details',details)) FROM (SELECT * FROM root_budget_events WHERE event_kind<>'content_redacted' ORDER BY seq)").fetch_one(&mut c).await.unwrap();
    json!({"calls":calls,"roots":serde_json::from_str::<Value>(&roots).unwrap(),
        "groups":serde_json::from_str::<Value>(&groups).unwrap(),
        "historical_events":serde_json::from_str::<Value>(&historical_events).unwrap()})
}

async fn set_request(path: &Path, scope: &str, id: &str, body: &str) {
    let mut c = connection(path).await;
    // Fixture body correction/corruption; no cleanup state or accounting writes.
    // Deliberately damaged bytes need a test-only bypass of the JSON CHECK; the
    // production cleanup must still reject them and roll its transaction back.
    if serde_json::from_str::<Value>(body).is_err() {
        sqlx::query("PRAGMA ignore_check_constraints=ON")
            .execute(&mut c)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE root_budget_calls SET request_artifact_schema='rsia.model_request.artifact.v1',request_artifact_body=?,request_artifact_digest=? WHERE namespace=? AND billing_scope=? AND call_id=?")
        .bind(body).bind(hash(body.as_bytes())).bind(NS).bind(scope).bind(id).execute(&mut c).await.unwrap();
}

#[tokio::test]
async fn both_scan_cursors_reset_before_old_blockers_and_accounting_never_rewinds() {
    let mut f = Fixture::new().await;
    source(&f.store, NS, "source").await;
    for scope in ["scope-a", "scope-z"] {
        budget_call(&f.store, scope, "same-call", "rsia.unknown_request.v9").await;
    }
    let mut session = f.store.session().await.unwrap();
    for id in ["world-a", "world-z"] {
        session
            .put_world(
                &context(NS, Role::Worker),
                id,
                false,
                &json!({"schema_version":"unknown.world.v9","id":id}),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let old = drain_failed(&f.store, NS, begin(&f.store, NS, "source").await).await;
    let before = snapshot(&f.path, NS).await;
    let budget_cursor = before["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "__budget_scan")
        .unwrap();
    assert_eq!(budget_cursor["cursor_kind"], "scope-a");
    assert_eq!(budget_cursor["cursor_id"], "same-call");
    let world_cursor = before["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "__world_scan")
        .unwrap();
    assert_eq!(world_cursor["cursor_id"], "world-a");
    let money = accounting(&f.path).await;
    for scope in ["scope-a", "scope-z"] {
        set_request(
            &f.path,
            scope,
            "same-call",
            &json!({"source_closure":[{"id":"source"}],"input":"BUDGET-SECRET"}).to_string(),
        )
        .await;
    }
    let mut session = f.store.session().await.unwrap();
    for id in ["world-a", "world-z"] {
        session.put_world(&context(NS, Role::Worker), id, false, &json!({"schema_version":"rsia.replay_world.v2","manifest":{"source_closure":[{"source_id":"source"}]},"transitions":[]})).await.unwrap();
    }
    session.commit().await.unwrap();
    let first = page(&f.store, NS, Role::Admin, &old.job_id, 300).await;
    assert_eq!(first.state, CleanupState::Running, "{first:?}");
    let started = snapshot(&f.path, NS).await;
    let world = started["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "__world_scan")
        .unwrap();
    assert_eq!(world["cursor_kind"], Value::Null);
    assert_eq!(world["cursor_id"], Value::Null);
    assert_eq!(world["expanded"], 0);
    let budget = started["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "__budget_scan")
        .unwrap();
    assert_eq!(budget["cursor_kind"], "scope-a");
    // This first retry page must include the call equal to the old cursor,
    // something the final fixpoint could otherwise mask as a broken reset.
    let mut c = connection(&f.path).await;
    let schema:String = sqlx::query_scalar("SELECT request_artifact_schema FROM root_budget_calls WHERE billing_scope='scope-a' AND call_id='same-call'").fetch_one(&mut c).await.unwrap();
    assert_eq!(schema, "rsia.redacted.v1");
    c.close().await.unwrap();
    assert_eq!(
        retry(&mut f, NS, &old.job_id).await.state,
        CleanupState::Complete
    );
    assert_eq!(accounting(&f.path).await, money);
    let mut c = connection(&f.path).await;
    let requests: Vec<String> = sqlx::query_scalar(
        "SELECT request_artifact_schema FROM root_budget_calls ORDER BY billing_scope",
    )
    .fetch_all(&mut c)
    .await
    .unwrap();
    assert_eq!(requests, vec!["rsia.redacted.v1", "rsia.redacted.v1"]);
    let worlds: Vec<String> = sqlx::query_scalar(
        "SELECT json_extract(manifest,'$.schema_version') FROM replay_worlds ORDER BY id",
    )
    .fetch_all(&mut c)
    .await
    .unwrap();
    assert_eq!(worlds, vec!["rsia.redacted.v1", "rsia.redacted.v1"]);
}

#[tokio::test]
async fn a_budget_request_added_after_the_retry_scan_is_found_by_the_fixpoint() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
    let first = page(&f.store, NS, Role::Admin, &old.job_id, 300).await;
    assert_eq!(first.state, CleanupState::Running);
    assert!(
        snapshot(&f.path, NS).await["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "__budget_scan" && row["expanded"] == 1)
    );
    // Real local accounting APIs create and settle the late call. Adding the
    // revoked source to its persisted request is a hostile old-writer fixture,
    // not permission for a new writer to bypass the source gate.
    budget_call(
        &f.store,
        "late-scope",
        "late-call",
        "rsia.model_request.artifact.v1",
    )
    .await;
    set_request(
        &f.path,
        "late-scope",
        "late-call",
        &json!({"source_closure":[{"id":"secondary"}],"input":"LATE-BUDGET-SECRET"}).to_string(),
    )
    .await;
    let money = accounting(&f.path).await;
    assert_eq!(
        retry(&mut f, NS, &old.job_id).await.state,
        CleanupState::Complete
    );
    let mut connection = connection(&f.path).await;
    let schema: String = sqlx::query_scalar("SELECT request_artifact_schema FROM root_budget_calls WHERE billing_scope='late-scope' AND call_id='late-call'").fetch_one(&mut connection).await.unwrap();
    assert_eq!(schema, "rsia.redacted.v1");
    assert_eq!(accounting(&f.path).await, money);
}

#[tokio::test]
async fn a_redacted_export_with_wrong_namespace_is_rejected_before_directory_io() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-b", "unknown-c"]).await;
    let bad = json!({"id":"unknown-a","schema_version":"rsia.redacted.v1",
        "original_kind":"export_attempt","original_schema":"rsia.e16.export_attempt.v2",
        "original_digest":hash(b"original export"),"metadata":{"namespace":"other"}});
    put(&f.store, NS, "artifact", "unknown-a", &bad, &[]).await;
    for namespace in [NS, "other"] {
        let directory = namespace_root(&f, namespace).join("unknown-a");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("canary"), b"canary").unwrap();
    }
    let result = retry(&mut f, NS, &old.job_id).await;
    assert_eq!(result.state, CleanupState::Failed);
    assert!(
        result
            .last_error
            .as_deref()
            .unwrap()
            .contains("invalid_e16_envelope")
    );
    assert_eq!(
        value(&f.store, NS, "artifact", "unknown-a").await.unwrap(),
        bad
    );
    for namespace in [NS, "other"] {
        assert_eq!(
            std::fs::read(
                namespace_root(&f, namespace)
                    .join("unknown-a")
                    .join("canary")
            )
            .unwrap(),
            b"canary"
        );
    }
}

#[tokio::test]
async fn retry_requires_original_tombstone_and_watermark_before_any_rewind() {
    for attack in [
        "missing_tombstone",
        "low_watermark",
        "equal_wrong_digest",
        "higher_watermark",
    ] {
        let mut f = Fixture::new().await;
        let old = failed_graph(&f).await;
        legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
        let mut c = connection(&f.path).await;
        match attack {
            "missing_tombstone" => {
                sqlx::query(
                    "DELETE FROM objects WHERE namespace=? AND kind='tombstone' AND id='secondary'",
                )
                .bind(NS)
                .execute(&mut c)
                .await
                .unwrap();
            }
            "low_watermark" => {
                sqlx::query("UPDATE revoke_watermark SET seq=0 WHERE namespace=?")
                    .bind(NS)
                    .execute(&mut c)
                    .await
                    .unwrap();
            }
            "equal_wrong_digest" => {
                sqlx::query("UPDATE revoke_watermark SET digest=? WHERE namespace=?")
                    .bind(hash(b"wrong"))
                    .bind(NS)
                    .execute(&mut c)
                    .await
                    .unwrap();
            }
            "higher_watermark" => {
                let mut s = f.store.session().await.unwrap();
                s.bump_watermark(&context(NS, Role::Admin), &hash(b"higher"))
                    .await
                    .unwrap();
                s.commit().await.unwrap();
            }
            _ => unreachable!(),
        }
        c.close().await.unwrap();
        let before = snapshot(&f.path, NS).await;
        if attack == "higher_watermark" {
            assert_eq!(
                retry(&mut f, NS, &old.job_id).await.state,
                CleanupState::Complete
            );
        } else {
            assert!(matches!(
                LifecycleStore::cleanup_step(
                    &context(NS, Role::Admin),
                    &f.store,
                    &old.job_id,
                    1,
                    300
                )
                .await,
                Err(Error::Conflict(_))
            ));
            assert_eq!(snapshot(&f.path, NS).await, before, "{attack}");
        }
    }
}

#[tokio::test]
async fn sql_failure_during_retry_rolls_back_state_frontier_and_retry_event() {
    let mut f = Fixture::new().await;
    let old = failed_graph(&f).await;
    legalize(&f.store, &["unknown-a", "unknown-b", "unknown-c"]).await;
    let mut c = connection(&f.path).await;
    sqlx::query("CREATE TRIGGER reject_retry BEFORE UPDATE OF expanded ON revoke_cleanup_frontier BEGIN SELECT RAISE(ABORT,'fixture SQL failure'); END").execute(&mut c).await.unwrap();
    let before = snapshot(&f.path, NS).await;
    assert!(
        LifecycleStore::cleanup_step(&context(NS, Role::Admin), &f.store, &old.job_id, 1, 300)
            .await
            .is_err()
    );
    assert_eq!(snapshot(&f.path, NS).await, before);
    sqlx::query("DROP TRIGGER reject_retry")
        .execute(&mut c)
        .await
        .unwrap();
    c.close().await.unwrap();
    assert_eq!(
        retry(&mut f, NS, &old.job_id).await.state,
        CleanupState::Complete
    );
}

#[tokio::test]
async fn raw_json_parser_failure_cannot_commit_a_retry_or_fake_complete() {
    let mut f = Fixture::new().await;
    source(&f.store, NS, "source").await;
    budget_call(&f.store, "scope-a", "call", "rsia.unknown_request.v9").await;
    let old = drain_failed(&f.store, NS, begin(&f.store, NS, "source").await).await;
    set_request(&f.path, "scope-a", "call", "{").await;
    let before = snapshot(&f.path, NS).await;
    assert!(
        LifecycleStore::cleanup_step(&context(NS, Role::Admin), &f.store, &old.job_id, 1, 300)
            .await
            .is_err()
    );
    assert_eq!(snapshot(&f.path, NS).await, before);
    set_request(
        &f.path,
        "scope-a",
        "call",
        &json!({"source_closure":[{"id":"source"}]}).to_string(),
    )
    .await;
    assert_eq!(
        retry(&mut f, NS, &old.job_id).await.state,
        CleanupState::Complete
    );
}
