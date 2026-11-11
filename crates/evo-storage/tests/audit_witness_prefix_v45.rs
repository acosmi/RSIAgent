//! Role witness answers come from the verified audit prefix. These tests pin
//! the latest-row rule, the rollback invalidation and namespace isolation of
//! that prefix; they do not extend what a witness authorizes.

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use sqlx::{Connection, SqliteConnection};

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn operation(namespace: &str, kind: &str, id: &str) -> String {
    format!(
        "formal.admin.content.v1.{}",
        fingerprint(&(namespace, kind, id)).unwrap()
    )
}

fn entity(namespace: &str, kind: &str, id: &str, label: &str) -> String {
    fingerprint(&(namespace, kind, id, hash(label.as_bytes()))).unwrap()
}

async fn witness(store: &Store, reader: &Context, op: &str, value: &str) -> bool {
    let mut session = store.session().await.unwrap();
    let matched = session
        .has_role_audit_witness(reader, Role::Admin, op, value)
        .await
        .unwrap();
    session.commit().await.unwrap();
    matched
}

#[tokio::test]
async fn witness_prefix_keeps_the_latest_row_across_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let reader = ctx("ns-a", "worker", Role::Worker);
    let op = operation("ns-a", "artifact", "host");
    let first = entity("ns-a", "artifact", "host", "first");
    let second = entity("ns-a", "artifact", "host", "second");
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &first).await.unwrap();
    session.commit().await.unwrap();
    // The first read verifies the chain; later reads reuse its prefix.
    for _ in 0..3 {
        assert!(witness(&store, &reader, &op, &first).await);
        assert!(!witness(&store, &reader, &op, &second).await);
    }
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &second).await.unwrap();
    session.commit().await.unwrap();
    assert!(witness(&store, &reader, &op, &second).await);
    assert!(!witness(&store, &reader, &op, &first).await);
    store.close().await;
}

#[tokio::test]
async fn witness_prefix_is_invalidated_by_a_rolled_back_append() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let reader = ctx("ns-a", "worker", Role::Worker);
    let op = operation("ns-a", "artifact", "profile");
    let value = entity("ns-a", "artifact", "profile", "rolled-back");
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &value).await.unwrap();
    assert!(
        session
            .has_role_audit_witness(&reader, Role::Admin, &op, &value)
            .await
            .unwrap()
    );
    // Dropping the session rolls the append back; the cached prefix must not
    // answer for a row that never committed.
    drop(session);
    assert!(!witness(&store, &reader, &op, &value).await);
    store.close().await;
}

#[tokio::test]
async fn witness_prefix_is_namespace_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let reader_a = ctx("ns-a", "worker-a", Role::Worker);
    let reader_b = ctx("ns-b", "worker-b", Role::Worker);
    let op = operation("ns-a", "artifact", "host");
    let value = entity("ns-a", "artifact", "host", "same-op");
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &value).await.unwrap();
    session.commit().await.unwrap();
    assert!(witness(&store, &reader_a, &op, &value).await);
    assert!(!witness(&store, &reader_b, &op, &value).await);
    store.close().await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_like_a_broken_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let reader = ctx("ns-a", "worker", Role::Worker);
    let op = operation("ns-a", "artifact", "host");
    let value = entity("ns-a", "artifact", "host", "tail");
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &value).await.unwrap();
    session.commit().await.unwrap();
    // Warm the verified prefix, then break only the tail row's link while the
    // same Store stays open. A prefix hit must not skip this check.
    assert!(witness(&store, &reader, &op, &value).await);
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE audit SET previous_hash=? WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
    )
    .bind(hash(b"wrong previous link"))
    .bind("ns-a")
    .bind("ns-a")
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let mut session = store.session().await.unwrap();
    let matched = session
        .has_role_audit_witness(&reader, Role::Admin, &op, &value)
        .await;
    session.commit().await.unwrap();
    assert!(
        matches!(matched, Err(Error::Conflict(_))),
        "a warm prefix must re-check the tail link: {matched:?}"
    );
    store.close().await;
}
