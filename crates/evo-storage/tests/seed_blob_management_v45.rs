//! AG-166c storage controls for the seed.install blob boundary.
//!
//! The seed management input and material blobs are namespace-scoped,
//! server-rooted and content-addressed: a same-digest file of another
//! namespace is unreachable, a swapped or linked file loses its authority, and
//! a revocation watermark advance refuses the final registered publication
//! before its first filesystem effect.
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use serde_json::{Value, json};

fn admin(namespace: &str, actor: &str) -> Context {
    Context::new(namespace, actor, Role::Admin).unwrap()
}

async fn store(name: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join(name)).await.unwrap();
    (dir, store)
}

async fn put_blob(store: &Store, ctx: &Context, bytes: &[u8]) -> String {
    store
        .put_blob_bounded(ctx, bytes, 64 * 1024 * 1024)
        .await
        .unwrap()
}

/// The blob location and bytes are derived from the authenticated namespace and
/// digest alone; the root is the store's own namespace blob directory and no
/// caller-selected root or path participates.
#[tokio::test]
async fn seed_blob_input_is_namespace_owner_and_fixed_root_scoped() {
    let (dir, store) = store("seed-blob-scope.sqlite3").await;
    let first = admin("tenant-a", "admin");
    let second = admin("tenant-b", "admin");
    let bytes = b"registered seed material";
    let digest = put_blob(&store, &first, bytes).await;

    let location = store.verified_blob_location(&first, &digest).unwrap();
    let expected_root = dir
        .path()
        .join("blobs")
        .join(hash(b"tenant-a"))
        .to_string_lossy()
        .into_owned();
    assert_eq!(location.root, expected_root);
    assert_eq!(
        location.path,
        std::path::Path::new(&location.root)
            .join(&digest)
            .to_string_lossy()
    );
    let input = store
        .verified_blob_input(&first, &digest, 4096)
        .await
        .unwrap();
    assert_eq!(input.bytes, bytes);
    assert_eq!(input.root, location.root);
    assert_eq!(input.path, location.path);
    // The same digest under another authenticated namespace is not reachable.
    assert!(matches!(
        store.verified_blob_input(&second, &digest, 4096).await,
        Err(Error::NotFound)
    ));
    // Another actor of the same namespace shares the namespace blob store; the
    // path is still owned by the namespace, never by a request field.
    let peer = admin("tenant-a", "admin-two");
    let peer_input = store
        .verified_blob_input(&peer, &digest, 4096)
        .await
        .unwrap();
    assert_eq!(peer_input.path, input.path);
    assert_eq!(peer_input.root, expected_root);
    store.close().await;
}

/// Replaced content, hard links and symlinks are refused by the same
/// complete-file check the publication transaction uses.
#[tokio::test]
async fn seed_blob_swap_or_link_cannot_replace_registered_material() {
    let (_dir, store) = store("seed-blob-links.sqlite3").await;
    let ctx = admin("tenant-links", "admin");
    let bytes = b"first exact seed bytes";
    let digest = put_blob(&store, &ctx, bytes).await;
    let location = store.verified_blob_location(&ctx, &digest).unwrap();
    let blob_path = std::path::PathBuf::from(&location.path);

    let session = store.registered_blob_write_session().await.unwrap();
    assert_eq!(
        session
            .reverify_blob_input(&ctx, &digest, 4096)
            .await
            .unwrap(),
        bytes
    );
    // A hard link makes the blob untrusted content.
    let link = blob_path.with_extension("hardlink");
    std::fs::hard_link(&blob_path, &link).unwrap();
    assert!(matches!(
        session.reverify_blob_input(&ctx, &digest, 4096).await,
        Err(Error::Conflict(message)) if message == "linked blob is not trusted content"
    ));
    std::fs::remove_file(&link).unwrap();
    // A symlink at an expected blob path is not a regular file.
    let absent = hash(b"never published seed bytes");
    let absent_path = std::path::Path::new(&location.root).join(&absent);
    std::os::unix::fs::symlink(&blob_path, &absent_path).unwrap();
    assert!(matches!(
        session.reverify_blob_input(&ctx, &absent, 4096).await,
        Err(Error::Conflict(message)) if message == "blob is not a regular file"
    ));
    std::fs::remove_file(&absent_path).unwrap();
    // Replacing the content under the same name loses the digest authority.
    std::fs::write(&blob_path, b"replaced seed bytes").unwrap();
    assert!(matches!(
        session.reverify_blob_input(&ctx, &digest, 4096).await,
        Err(Error::Conflict(message)) if message == "blob content digest mismatch"
    ));
    std::fs::write(&blob_path, bytes).unwrap();
    assert_eq!(
        session
            .reverify_blob_input(&ctx, &digest, 4096)
            .await
            .unwrap(),
        bytes
    );
    session.commit().await.unwrap();
    store.close().await;
}

/// A revocation watermark advance refuses the final registered blob
/// publication before the first filesystem effect: only the already published
/// content-addressed blob remains.
#[tokio::test]
async fn seed_revoke_before_final_publish_preserves_only_actual_facts() {
    let (dir, store) = store("seed-blob-revoke.sqlite3").await;
    let ctx = admin("tenant-revoke", "admin");
    let baseline = b"seed baseline bytes";
    let local = b"seed local bytes";
    let baseline_digest = hash(baseline);
    let local_digest = hash(local);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx, "seed-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let artifact = json!({
        "schema_version": "rsia.e16.seed_install.v2",
        "id": "seed-revoke-producer",
        "namespace": ctx.namespace(),
        "owner_actor": ctx.actor(),
        "request_key": "seed-revoke-producer",
        "input_digest": hash(b"input"),
        "created_at": 1,
        "updated_at": 1,
        "source_refs": [],
        "revoke_watermark": 1,
        "payload": {
            "status": "prepared",
            "baseline_blob_digest": baseline_digest,
            "local_blob_digest": local_digest,
        },
    });
    let mut session = store.session().await.unwrap();
    session
        .put(
            &ctx,
            "artifact",
            "seed-revoke-producer",
            ctx.actor(),
            &artifact,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    // The prepared producer registers its first material blob at watermark 1.
    let mut session = store.registered_blob_write_session().await.unwrap();
    session
        .publish_registered_blob(
            &ctx,
            "seed-revoke-producer",
            "rsia.e16.seed_install.v2",
            "baseline_blob_digest",
            baseline,
            4096,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let baseline_path = dir
        .path()
        .join("blobs")
        .join(hash(b"tenant-revoke"))
        .join(&baseline_digest);
    assert!(baseline_path.is_file());
    // Revocation advances the watermark; the final publication is refused
    // before it writes the second blob.
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&ctx, "seed-revoked").await.unwrap();
    session.commit().await.unwrap();
    let local_path = dir
        .path()
        .join("blobs")
        .join(hash(b"tenant-revoke"))
        .join(&local_digest);
    let mut session = store.registered_blob_write_session().await.unwrap();
    assert!(matches!(
        session
            .publish_registered_blob(
                &ctx,
                "seed-revoke-producer",
                "rsia.e16.seed_install.v2",
                "local_blob_digest",
                local,
                4096,
            )
            .await,
        Err(Error::Conflict(message)) if message == "registered blob producer watermark is stale"
    ));
    session.commit().await.unwrap();
    assert!(!local_path.exists(), "no publication after revocation");
    assert_eq!(std::fs::read(&baseline_path).unwrap(), baseline);
    assert_eq!(store.integrity().await.unwrap(), "ok");
    let _: Value = serde_json::to_value(&artifact).unwrap();
    store.close().await;
}
