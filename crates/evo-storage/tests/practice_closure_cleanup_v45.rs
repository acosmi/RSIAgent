//! AG-204: a practice request closure is the fact that an attempt's request was
//! authorized after the whole request was checked and before the first runner
//! call. A source revocation must classify it as a preserved historical fact
//! (instead of isolating the cleanup as an unknown scope), and the producer
//! writes the typed edge that lets a revocation reach it.

use evo_core::{Context, Role};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::Path;

const DATABASE: &str = "rsia.sqlite3";
const SOURCE: &str = "run-closure-source";
const CLOSURE: &str = "practice-closure-cleanup";
const CLOSURE_SCHEMA: &str = "rsia.practice.request_closure.v1";
const MAX_STEPS: usize = 200;

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}

fn host() -> Context {
    Context::new("n", "broker", Role::Host).unwrap()
}

async fn open(dir: &Path) -> Store {
    Store::open(&dir.join(DATABASE)).await.unwrap()
}

async fn put_source(store: &Store) {
    let host = host();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            SOURCE,
            host.actor(),
            &json!({
                "id": SOURCE,
                "schema_version": "rsia.optimization.source.v1",
                "body": "source-body",
            }),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn put_closure(store: &Store) {
    let admin = admin();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            CLOSURE,
            admin.actor(),
            &json!({
                "schema_version": CLOSURE_SCHEMA,
                "id": CLOSURE,
                "namespace": "n",
                "control_id": "control-closure",
                "set_id": "set-closure",
                "attempt": 1,
                "request_id": "request-closure",
                "episode_id": "episode-closure",
                "step": 1,
                "bundle_digest": "bundle",
                "environment_digest": "environment",
                "revoke_watermark": 0,
                "request_digest": "request-digest",
                "plan_digest": "plan-digest",
                "attempts_per_task": 1,
                "registration_id": null,
                "created_at": 1,
            }),
        )
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", CLOSURE, "run", SOURCE)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn begin(store: &Store) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: SOURCE.into(),
        },
        "source revoked",
        1,
    )
    .await
    .unwrap()
}

async fn drive(store: &Store, mut status: CleanupStatus) -> CleanupStatus {
    let mut clock = 1;
    for _ in 0..MAX_STEPS {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            return status;
        }
        clock += 1;
        status = LifecycleStore::cleanup_step(&admin(), store, &status.job_id, 8, clock)
            .await
            .unwrap();
    }
    panic!("the cleanup did not finish within {MAX_STEPS} steps: {status:?}");
}

#[tokio::test]
async fn practice_request_closure_is_preserved_by_revocation_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_source(&store).await;
    put_closure(&store).await;
    let status = drive(&store, begin(&store).await).await;
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "PARENT-EXPECTED-FAILURE: the practice request closure must be classified, not isolated: {status:?}"
    );
    assert_eq!(status.last_error, None, "cleanup error: {status:?}");
    let mut session = store.session().await.unwrap();
    let body: Value = session
        .get(&admin(), "artifact", CLOSURE)
        .await
        .unwrap()
        .expect("the closure stays");
    assert_eq!(
        body["schema_version"], CLOSURE_SCHEMA,
        "PARENT-EXPECTED-FAILURE: the closure must be preserved, not redacted: {body}"
    );
    assert_eq!(body["set_id"], "set-closure");
}
