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

/// Warm the prefix, then corrupt exactly one tail field while the Store stays
/// open. The next witness read must fall back to the full walk and reject it.
async fn assert_tail_tamper_is_rejected(field: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = ctx("ns-a", "admin", Role::Admin);
    let reader = ctx("ns-a", "worker", Role::Worker);
    let op = operation("ns-a", "artifact", "host");
    let value = entity("ns-a", "artifact", "host", field);
    let mut session = store.session().await.unwrap();
    session.audit(&admin, &op, &value).await.unwrap();
    session.commit().await.unwrap();
    assert!(witness(&store, &reader, &op, &value).await);
    let (statement, replacement): (&str, Option<String>) = match field {
        "previous_hash" => (
            "UPDATE audit SET previous_hash=? WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
            Some(hash(b"wrong previous link")),
        ),
        "digest" => (
            "UPDATE audit SET digest=? WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
            Some(hash(b"wrong digest")),
        ),
        "payload" => (
            "UPDATE audit SET payload=? WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
            Some("{\"wrong\":true}".to_owned()),
        ),
        // SQLite TEXT functions see only the prefix before an embedded NUL;
        // appending one must still be rejected by the byte-wise guard.
        "payload_nul" => (
            "UPDATE audit SET payload=payload||char(0)||'x' WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
            None,
        ),
        // Same bytes, different storage class: the chain walk only accepts a
        // TEXT payload, so a prefix hit must not accept the BLOB either.
        "payload_blob" => (
            "UPDATE audit SET payload=CAST(payload AS BLOB) WHERE namespace=? AND seq=(SELECT MAX(seq) FROM audit WHERE namespace=?)",
            None,
        ),
        other => panic!("unknown tail field {other}"),
    };
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let mut query = sqlx::query(statement);
    if let Some(replacement) = replacement {
        query = query.bind(replacement);
    }
    query
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
        "a warm prefix must re-check the tail {field}: {matched:?}"
    );
    store.close().await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_previous_hash() {
    assert_tail_tamper_is_rejected("previous_hash").await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_digest() {
    assert_tail_tamper_is_rejected("digest").await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_payload() {
    assert_tail_tamper_is_rejected("payload").await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_payload_with_nul() {
    assert_tail_tamper_is_rejected("payload_nul").await;
}

#[tokio::test]
async fn witness_prefix_rechecks_the_tail_payload_storage_class() {
    assert_tail_tamper_is_rejected("payload_blob").await;
}
