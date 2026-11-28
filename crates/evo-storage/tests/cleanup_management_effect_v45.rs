//! AG-162 storage kernels (plan v4.5 §11.3, §11.3.1, E08): the same-session
//! `begin_revoke_in_session` and `cleanup_step_in_session` write only into the
//! caller's transaction, so a management handler commits them together with
//! its terminal job state. Committed, each equals the old public function on a
//! twin store; dropped, each leaves no row behind. A cleanup page needs the
//! blob write session.

use evo_core::{Context, Error, Role};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::{Path, PathBuf};

const NS: &str = "effect-tenant";
const REASON: &str = "effect fixture";

fn admin() -> Context {
    Context::new(NS, "admin", Role::Admin).unwrap()
}

fn source(id: &str) -> TypedObjectRef {
    TypedObjectRef {
        kind: "run".into(),
        id: id.into(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("rsia.sqlite3");
        let store = Store::open(&path).await.unwrap();
        Self {
            _dir: dir,
            path,
            store,
        }
    }

    async fn put(&self, kind: &str, id: &str, body: &Value, deps: &[(&str, &str)]) {
        let ctx = admin();
        let mut session = self.store.session().await.unwrap();
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

    /// A run with three dependents and one second-level dependent.
    async fn graph(&self) {
        self.put(
            "run",
            "source",
            &json!({"id":"source","schema_version":"rsia.optimization.source.v1","body":"run-body"}),
            &[],
        )
        .await;
        for id in ["fact-1", "fact-2", "fact-3"] {
            self.put("artifact", id, &fact(id), &[("run", "source")])
                .await;
        }
        self.put(
            "artifact",
            "fact-nested",
            &fact("fact-nested"),
            &[("artifact", "fact-1")],
        )
        .await;
    }

    /// Settles any rollback a dropped session left on the single connection.
    async fn settle(&self) {
        self.store.session().await.unwrap().commit().await.unwrap();
    }

    async fn snapshot(&self) -> Value {
        snapshot(&self.path).await
    }
}

fn fact(id: &str) -> Value {
    json!({"id":id,"schema_version":"rsia.optimization.stage_fact.v1",
        "kind":"terminal_rejected","payload":format!("SECRET-{id}")})
}

async fn snapshot(path: &Path) -> Value {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await
            .unwrap();
    let queries = [
        (
            "jobs",
            "SELECT json_group_array(json_object('job_id',job_id,'source_kind',source_kind,'source_id',source_id,'watermark_seq',watermark_seq,'watermark_digest',watermark_digest,'state',state,'processed_nodes',processed_nodes,'last_error',last_error,'created_at',created_at,'updated_at',updated_at)) FROM (SELECT * FROM revoke_cleanup_jobs WHERE namespace=? ORDER BY job_id)",
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
            "watermark",
            "SELECT json_group_array(json_object('seq',seq,'digest',digest)) FROM revoke_watermark WHERE namespace=?",
        ),
    ];
    let mut result = serde_json::Map::new();
    for (name, query) in queries {
        let encoded: String = sqlx::query_scalar(query)
            .bind(NS)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        result.insert(name.into(), serde_json::from_str(&encoded).unwrap());
    }
    result.into()
}

fn same_status(left: &CleanupStatus, right: &CleanupStatus) {
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

#[tokio::test]
async fn the_in_session_revocation_commits_or_rolls_back_with_its_caller() {
    let fixture = Fixture::new().await;
    let twin = Fixture::new().await;
    fixture.graph().await;
    twin.graph().await;
    let before = fixture.snapshot().await;

    let mut session = fixture.store.session().await.unwrap();
    let dropped = LifecycleStore::begin_revoke_in_session(
        &admin(),
        &mut session,
        source("source"),
        REASON,
        10,
    )
    .await
    .unwrap();
    assert_eq!(dropped.state, CleanupState::Pending);
    drop(session);
    fixture.settle().await;
    assert_eq!(
        fixture.snapshot().await,
        before,
        "nothing outlives the caller"
    );

    let mut session = fixture.store.session().await.unwrap();
    let committed = LifecycleStore::begin_revoke_in_session(
        &admin(),
        &mut session,
        source("source"),
        REASON,
        10,
    )
    .await
    .unwrap();
    session.commit().await.unwrap();
    let old = LifecycleStore::begin_revoke(&admin(), &twin.store, source("source"), REASON, 10)
        .await
        .unwrap();
    same_status(&committed, &old);
    same_status(&committed, &dropped);
    let after = fixture.snapshot().await;
    assert_ne!(after, before);
    assert_eq!(after, twin.snapshot().await);

    // A replay inside a session writes nothing and returns the same cleanup.
    let mut session = fixture.store.session().await.unwrap();
    let replay = LifecycleStore::begin_revoke_in_session(
        &admin(),
        &mut session,
        source("source"),
        REASON,
        11,
    )
    .await
    .unwrap();
    session.commit().await.unwrap();
    same_status(&replay, &committed);
    assert_eq!(fixture.snapshot().await, after);
}

#[tokio::test]
async fn every_in_session_page_equals_the_old_step_and_a_dropped_page_leaves_nothing() {
    let fixture = Fixture::new().await;
    let twin = Fixture::new().await;
    fixture.graph().await;
    twin.graph().await;
    let job = LifecycleStore::begin_revoke(&admin(), &fixture.store, source("source"), REASON, 10)
        .await
        .unwrap()
        .job_id;
    LifecycleStore::begin_revoke(&admin(), &twin.store, source("source"), REASON, 10)
        .await
        .unwrap();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 50, "the cleanup never completed");
        let now = 10 + pages;
        let before = fixture.snapshot().await;
        let mut session = fixture.store.registered_blob_write_session().await.unwrap();
        LifecycleStore::cleanup_step_in_session(
            &admin(),
            &fixture.store,
            &mut session,
            &job,
            1,
            now,
        )
        .await
        .unwrap();
        drop(session);
        fixture.settle().await;
        assert_eq!(
            fixture.snapshot().await,
            before,
            "the dropped page rolled back"
        );

        let mut session = fixture.store.registered_blob_write_session().await.unwrap();
        let page = LifecycleStore::cleanup_step_in_session(
            &admin(),
            &fixture.store,
            &mut session,
            &job,
            1,
            now,
        )
        .await
        .unwrap();
        session.commit().await.unwrap();
        let old = LifecycleStore::cleanup_step(&admin(), &twin.store, &job, 1, now)
            .await
            .unwrap();
        same_status(&page, &old);
        assert_eq!(
            fixture.snapshot().await,
            twin.snapshot().await,
            "page {pages}"
        );
        if page.state == CleanupState::Complete {
            break;
        }
    }
    assert!(pages > 1);
}

#[tokio::test]
async fn a_cleanup_page_requires_the_blob_write_session() {
    let fixture = Fixture::new().await;
    fixture.graph().await;
    let job = LifecycleStore::begin_revoke(&admin(), &fixture.store, source("source"), REASON, 10)
        .await
        .unwrap()
        .job_id;
    let before = fixture.snapshot().await;
    let mut session = fixture.store.session().await.unwrap();
    let refused = LifecycleStore::cleanup_step_in_session(
        &admin(),
        &fixture.store,
        &mut session,
        &job,
        1,
        11,
    )
    .await;
    assert!(matches!(refused, Err(Error::Internal)), "{refused:?}");
    session.commit().await.unwrap();
    assert_eq!(fixture.snapshot().await, before);
}
