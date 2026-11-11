//! Role witness answers come from the verified audit prefix. These tests pin
//! the latest-row rule, the rollback invalidation and namespace isolation of
//! that prefix; they do not extend what a witness authorizes.

use evo_core::{Context, Role, fingerprint, hash};
use evo_storage::Store;

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
    let store = Store::open(&dir.path().join("rsia.sqlite3"))
        .await
        .unwrap();
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
    let store = Store::open(&dir.path().join("rsia.sqlite3"))
        .await
        .unwrap();
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
    let store = Store::open(&dir.path().join("rsia.sqlite3"))
        .await
        .unwrap();
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
