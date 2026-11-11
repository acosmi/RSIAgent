//! AG-166a storage controls for the management import blob input.
//!
//! These tests pin the server-derived namespace location, the exact-byte
//! verification and the fact that a preflight read opens no SQL transaction or
//! object row. They are software fixtures: no provider, model call or external
//! dispatch exists here.
use evo_core::{Context, Error, Role, hash};
use evo_storage::{CAPACITY_IMPORT_RESULT_SCHEMA, Store};
use serde_json::{Value, json};

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("import-blob.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn put_blob(store: &Store, ctx: &Context, bytes: &[u8]) -> String {
    store
        .put_blob_bounded(ctx, bytes, 64 * 1024 * 1024)
        .await
        .unwrap()
}

/// The blob location and bytes are derived from the authenticated namespace and
/// digest alone; a same-digest blob of another namespace is not reachable.
#[tokio::test]
async fn import_blob_input_is_namespace_scoped() {
    let (_dir, store) = store().await;
    let first = admin("tenant-a");
    let second = admin("tenant-b");
    let bytes = br#"{"role":"user","content":"namespace scoped"}"#;
    let digest = put_blob(&store, &first, bytes).await;

    let input = store
        .verified_blob_input(&first, &digest, 4096)
        .await
        .unwrap();
    assert_eq!(input.digest, digest);
    assert_eq!(input.bytes, bytes);
    assert!(
        input.root.ends_with(&hash(b"tenant-a")),
        "root is the namespace blob directory: {}",
        input.root
    );
    assert_eq!(
        input.path,
        std::path::Path::new(&input.root)
            .join(&digest)
            .to_string_lossy()
    );
    assert!(matches!(
        store.verified_blob_input(&second, &digest, 4096).await,
        Err(Error::NotFound)
    ));
    store.close().await;
}

/// The location is the store's own `blobs/<namespace-hash>/<digest>` path; no
/// caller value can select a root or a file.
#[tokio::test]
async fn import_blob_input_root_is_server_derived() {
    let (dir, store) = store().await;
    let ctx = admin("tenant-root");
    let bytes = b"managed import bytes";
    let digest = put_blob(&store, &ctx, bytes).await;
    let location = store.verified_blob_location(&ctx, &digest).unwrap();
    let expected_root = dir
        .path()
        .join("blobs")
        .join(hash(b"tenant-root"))
        .to_string_lossy()
        .into_owned();
    assert_eq!(location.root, expected_root);
    assert_eq!(
        location.path,
        std::path::Path::new(&location.root)
            .join(&digest)
            .to_string_lossy()
    );
    assert_eq!(location.digest, digest);
    // The read path is exactly that same location; a body cannot contribute a
    // second root or path because the API takes only context and digest.
    let input = store
        .verified_blob_input(&ctx, &digest, 4096)
        .await
        .unwrap();
    assert_eq!(input.root, location.root);
    assert_eq!(input.path, location.path);
    store.close().await;
}

/// Symlinks, hard links, replaced content and a wrong digest are refused by the
/// same admission the trusted reader uses.
#[tokio::test]
async fn import_blob_input_rejects_links_swaps_and_digest_mismatch() {
    let (_dir, store) = store().await;
    let ctx = admin("tenant-links");
    let bytes = b"first exact bytes";
    let digest = put_blob(&store, &ctx, bytes).await;
    let location = store.verified_blob_location(&ctx, &digest).unwrap();
    let blob_path = std::path::PathBuf::from(&location.path);

    // A hard link makes the blob untrusted content.
    let link = blob_path.with_extension("hardlink");
    std::fs::hard_link(&blob_path, &link).unwrap();
    assert!(matches!(
        store.verified_blob_input(&ctx, &digest, 4096).await,
        Err(Error::Conflict(message)) if message == "linked blob is not trusted content"
    ));
    std::fs::remove_file(&link).unwrap();

    // A symlink at an expected blob path is not a regular file.
    let absent = hash(b"never published bytes");
    let absent_path = std::path::Path::new(&location.root).join(&absent);
    std::os::unix::fs::symlink(&blob_path, &absent_path).unwrap();
    assert!(matches!(
        store.verified_blob_input(&ctx, &absent, 4096).await,
        Err(Error::Conflict(message)) if message == "blob is not a regular file"
    ));
    std::fs::remove_file(&absent_path).unwrap();

    // Replacing the file content under the same name loses its authority.
    std::fs::write(&blob_path, b"replaced bytes with another hash").unwrap();
    assert!(matches!(
        store.verified_blob_input(&ctx, &digest, 4096).await,
        Err(Error::Conflict(message)) if message == "blob content digest mismatch"
    ));
    store.close().await;
}

/// The preflight read takes no Session and writes no object row; a caller's
/// open transaction does not block it because the blob lock is independent.
#[tokio::test]
async fn import_blob_preflight_holds_no_sql_transaction() {
    let (_dir, store) = store().await;
    let ctx = admin("tenant-preflight");
    let bytes = b"preflight bytes";
    let digest = put_blob(&store, &ctx, bytes).await;

    let mut session = store.session().await.unwrap();
    let input = store
        .verified_blob_input(&ctx, &digest, 4096)
        .await
        .unwrap();
    assert_eq!(input.bytes, bytes);
    let artifacts: Vec<Value> = session.list(&ctx, "artifact").await.unwrap();
    assert!(artifacts.is_empty(), "a read must not create an artifact");
    session.commit().await.unwrap();
    store.close().await;
}

/// The event capacity is deployment wide. At the limit a new result is
/// refused, while the same already-stored result identity is still readable
/// and never re-counted: the handler returns it before the bounded writer.
#[tokio::test]
async fn import_capacity_is_global_and_same_request_at_limit_is_idempotent() {
    let (_dir, store) = store().await;
    let first = admin("tenant-capacity-a");
    let second = admin("tenant-capacity-b");
    let body = json!({
        "schema_version": CAPACITY_IMPORT_RESULT_SCHEMA,
        "id": "existing-result",
        "payload": {"aggregate_summary": {"total_events": 10_000}}
    });
    let mut session = store.session().await.unwrap();
    session
        .put_new_import_result_below_event_capacity(
            &first,
            "existing-result",
            first.actor(),
            &body,
            10_000,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    // Another namespace sees the same instance-wide limit.
    let new_body = json!({
        "schema_version": CAPACITY_IMPORT_RESULT_SCHEMA,
        "id": "new-result",
        "payload": {"aggregate_summary": {"total_events": 1}}
    });
    let mut session = store.session().await.unwrap();
    let outcome = session
        .put_new_import_result_below_event_capacity(
            &second,
            "new-result",
            second.actor(),
            &new_body,
            1,
        )
        .await;
    assert!(
        matches!(&outcome, Err(Error::Capacity(detail))
            if detail.item == evo_core::capacity::CapacityItem::Event
                && (detail.used, detail.limit, detail.delta) == (10_000, 10_000, 1)),
        "{outcome:?}"
    );
    session.commit().await.unwrap();

    // The bounded writer refuses an existing identity before touching capacity,
    // which is what keeps a replayed management execution idempotent at the
    // limit: the stored result is returned unchanged, not inserted again.
    let mut session = store.session().await.unwrap();
    let outcome = session
        .put_new_import_result_below_event_capacity(
            &first,
            "existing-result",
            first.actor(),
            &body,
            10_000,
        )
        .await;
    assert!(
        matches!(&outcome, Err(Error::Conflict(message))
            if message == "bounded creation requires a new object identity"),
        "{outcome:?}"
    );
    let stored: Value = session
        .need(&first, "artifact", "existing-result")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(stored, body);
    let mut session = store.session().await.unwrap();
    let used = session.capacity_usage_v41(0).await.unwrap().events;
    session.commit().await.unwrap();
    assert_eq!(used, 10_000);
    store.close().await;
}
