//! AG-175 storage-layer trace export publication controls. They drive the
//! real trace export primitives (`stage_trace_export`,
//! `publish_staged_trace_export`, `verify_trace_export_tree`) and the real
//! revocation lifecycle (`LifecycleStore::begin_revoke`, `cleanup_step`) on
//! real directories and a real record. The record body is engine-owned; these
//! tests store it with exactly the engine's fields through the real put gate.
use evo_core::{Context, Error, Role, hash};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef, redacted_object_body};
use evo_storage::{
    Store, TRACE_EXPORT_SCHEMA, TRACE_EXPORT_TREE_UNAVAILABLE, TraceExportMember,
    local_export_tree_digest,
};
use serde_json::{Value, json};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use std::collections::BTreeMap;
use std::path::Path;

const DATABASE: &str = "trace-export.sqlite3";
const DISCLAIMER: &str = "Local revocation stops future distribution and invalidates local usage; remote erasure of third-party downloaded copies cannot be guaranteed.";

fn admin() -> Context {
    Context::new("trace-tenant", "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store, Context) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join(DATABASE)).await.unwrap();
    (dir, store, admin())
}

fn files(marker: &str) -> Vec<evo_storage::LocalExportFile> {
    vec![
        evo_storage::LocalExportFile {
            path: "traces/0000.json".into(),
            bytes: format!("native member bytes {marker}").into_bytes(),
        },
        evo_storage::LocalExportFile {
            path: "manifest.json".into(),
            bytes: format!("manifest bytes {marker}").into_bytes(),
        },
    ]
}

fn members(files: &[evo_storage::LocalExportFile]) -> Vec<TraceExportMember> {
    files
        .iter()
        .map(|file| TraceExportMember {
            path: file.path.clone(),
            digest_sha256: hash(&file.bytes),
            size_bytes: file.bytes.len() as u64,
        })
        .collect()
}

fn source_run(export_id: &str) -> String {
    format!("source-{export_id}")
}

fn record_id(export_id: &str) -> String {
    format!("trace-export-record-{export_id}")
}

/// A live captured source run and the `reviewed` record of one export over the
/// frozen `files`, with its typed edge to that run, as the engine writes them.
async fn reviewed_record(
    store: &Store,
    admin: &Context,
    export_id: &str,
    files: &[evo_storage::LocalExportFile],
) -> Value {
    let (candidate_digest, total_bytes) = local_export_tree_digest(files).unwrap();
    let run = json!({"id": source_run(export_id), "facts": "captured service facts"});
    let record = json!({
        "schema_version": TRACE_EXPORT_SCHEMA,
        "id": record_id(export_id),
        "export_id": export_id,
        "phase": "reviewed",
        "prepare_job_id": "management-job-prepare",
        "publish_job_id": "management-job-publish",
        "candidate_digest": candidate_digest,
        "members": members(files),
        "delivery": null,
        "local_cost": {"wall_seconds": 1, "bytes": total_bytes},
        "source_snapshots": [{
            "source_id": source_run(export_id),
            "owner_actor": admin.actor(),
            "storage_body_sha256": hash(run.to_string().as_bytes()),
        }],
        "owner_actor": admin.actor(),
        "created_at": 1,
        "updated_at": 2,
    });
    let mut session = store.session().await.unwrap();
    session
        .put(admin, "run", &source_run(export_id), admin.actor(), &run)
        .await
        .unwrap();
    session
        .put(
            admin,
            "artifact",
            &record_id(export_id),
            admin.actor(),
            &record,
        )
        .await
        .unwrap();
    session
        .put_edge(
            admin,
            "artifact",
            &record_id(export_id),
            "run",
            &source_run(export_id),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    record
}

async fn record_body(store: &Store, admin: &Context, export_id: &str) -> String {
    let mut session = store.session().await.unwrap();
    let body = session
        .object_row(admin, "artifact", &record_id(export_id))
        .await
        .unwrap()
        .unwrap()
        .body;
    session.commit().await.unwrap();
    body
}

async fn revoke(store: &Store, admin: &Context, run: &str) -> String {
    LifecycleStore::begin_revoke(
        admin,
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: run.into(),
        },
        "trace export source revoked",
        10,
    )
    .await
    .unwrap()
    .job_id
}

/// Every filesystem entry under the store root except the database files:
/// the honest boundary for "zero effect", "preserved" and "released" claims.
fn fs_image(root: &Path) -> BTreeMap<String, (bool, Option<Vec<u8>>)> {
    fn walk(root: &Path, dir: &Path, image: &mut BTreeMap<String, (bool, Option<Vec<u8>>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if relative.starts_with(DATABASE) {
                continue;
            }
            if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                image.insert(relative, (true, None));
                walk(root, &path, image);
            } else {
                image.insert(relative, (false, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    let mut image = BTreeMap::new();
    walk(root, root, &mut image);
    image
}

#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn make_readonly(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// Private staging members left anywhere under the staging root.
fn staging_residue(root: &Path) -> Vec<String> {
    let staging = root.join(".export-staging");
    if !staging.exists() {
        return Vec::new();
    }
    fs_image(&staging)
        .into_iter()
        .filter(|(_, (is_dir, _))| !is_dir)
        .map(|(path, _)| path)
        .collect()
}

#[tokio::test]
async fn trace_export_stages_private_and_publishes_without_overwrite() {
    let (dir, store, admin) = store().await;
    let first = files("first");
    let (first_digest, first_bytes) = local_export_tree_digest(&first).unwrap();
    reviewed_record(&store, &admin, "export-a", &first).await;
    let tree = store.trace_export_directory(&admin, "export-a").unwrap();
    let staged = store
        .stage_trace_export(&admin, &record_id("export-a"), &first)
        .await
        .unwrap();
    assert_eq!(
        staging_residue(dir.path()).len(),
        2,
        "the staged candidate is private material only"
    );
    assert!(!tree.exists(), "staging never reaches the formal location");
    let receipt = store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    assert_eq!(receipt.package_tree_digest, first_digest);
    assert_eq!(receipt.delivered_bytes, first_bytes);
    assert_eq!(
        receipt.local_export_ref,
        format!(
            "trace-exports/{}/export-a",
            hash(admin.namespace().as_bytes())
        )
    );
    assert_eq!(
        std::fs::read(tree.join("traces/0000.json")).unwrap(),
        first[0].bytes
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tree.join("traces/0000.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o222, 0, "published members are read-only");
    }
    assert!(staging_residue(dir.path()).is_empty());
    // Trace exports and E16 package exports never share a directory space.
    assert_ne!(
        store.local_export_directory(&admin, "export-a").unwrap(),
        tree
    );
    assert!(!dir.path().join("exports").exists());
    // The receipt the publication earned is what the published tree keeps
    // earning against the frozen members.
    assert_eq!(
        store
            .verify_trace_export_tree(&admin, "export-a", &members(&first), &first_digest)
            .await
            .unwrap(),
        receipt
    );
    // A second publication of the same record never overwrites the tree.
    let again = store
        .stage_trace_export(&admin, &record_id("export-a"), &first)
        .await
        .unwrap();
    match store.publish_staged_trace_export(&admin, again).await {
        Err(Error::Conflict(reason)) => assert_eq!(reason, "trace export target already exists"),
        other => panic!("an existing tree must never be overwritten, got {other:?}"),
    }
    assert!(staging_residue(dir.path()).is_empty());
    // Bytes that are not the frozen candidate never reach staging.
    let before = fs_image(dir.path());
    match store
        .stage_trace_export(&admin, &record_id("export-a"), &files("second"))
        .await
    {
        Err(Error::Conflict(reason)) => assert_eq!(
            reason,
            "trace export candidate bytes differ from the frozen candidate"
        ),
        other => panic!(
            "drifted bytes must be refused before staging, got {:?}",
            other.err()
        ),
    }
    assert_eq!(fs_image(dir.path()), before);
    // Only an Admin of the namespace owns this surface.
    let worker = Context::new("trace-tenant", "worker", Role::Worker).unwrap();
    assert!(matches!(
        store
            .stage_trace_export(&worker, &record_id("export-a"), &first)
            .await,
        Err(Error::Forbidden)
    ));
}

/// A source revoked before the first write: the fence refuses before any
/// filesystem effect — no staging, no tree — and the record is untouched.
#[tokio::test]
async fn trace_export_revoke_before_first_write_is_zero_effect() {
    let (dir, store, admin) = store().await;
    let first = files("zero");
    reviewed_record(&store, &admin, "export-b", &first).await;
    let record = record_body(&store, &admin, "export-b").await;
    revoke(&store, &admin, &source_run("export-b")).await;
    let before = fs_image(dir.path());
    match store
        .stage_trace_export(&admin, &record_id("export-b"), &first)
        .await
    {
        Err(Error::Conflict(reason)) => {
            assert_eq!(reason, "trace export source closure was revoked")
        }
        other => panic!(
            "a revoked source must be refused before staging, got {:?}",
            other.err()
        ),
    }
    assert_eq!(
        fs_image(dir.path()),
        before,
        "the refusal has no filesystem effect at all"
    );
    assert!(!dir.path().join(".export-staging").exists());
    assert!(!dir.path().join("trace-exports").exists());
    assert_eq!(record_body(&store, &admin, "export-b").await, record);
}

/// A source revoked after staging: the publication fence refuses under the
/// write lock, the staging guard releases the private tree, and nothing
/// reaches the formal location.
#[tokio::test]
async fn trace_export_revoke_after_stage_prevents_success() {
    let (dir, store, admin) = store().await;
    let first = files("stale-attempt");
    reviewed_record(&store, &admin, "export-c", &first).await;
    let record = record_body(&store, &admin, "export-c").await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-c"), &first)
        .await
        .unwrap();
    assert!(!staging_residue(dir.path()).is_empty());
    revoke(&store, &admin, &source_run("export-c")).await;
    match store.publish_staged_trace_export(&admin, staged).await {
        Err(Error::Conflict(reason)) => {
            assert_eq!(reason, "trace export source closure was revoked")
        }
        other => panic!("a revoked source must not publish, got {other:?}"),
    }
    assert!(
        staging_residue(dir.path()).is_empty(),
        "the refused attempt released its private staging"
    );
    assert!(
        !store
            .trace_export_directory(&admin, "export-c")
            .unwrap()
            .exists(),
        "no tree reached the formal location"
    );
    for (_, (_, bytes)) in fs_image(dir.path()) {
        if let Some(bytes) = bytes {
            assert!(
                !bytes
                    .windows(b"stale-attempt".len())
                    .any(|window| window == b"stale-attempt"),
                "the refused attempt left no bytes anywhere"
            );
        }
    }
    assert_eq!(record_body(&store, &admin, "export-c").await, record);
}

/// A swapped dependency blob — candidate bytes that drifted from the frozen
/// digest, or a staged member changed after staging — blocks publication, and
/// so does a revocation watermark move inside the record's own source closure
/// (an upstream run of its source). A revocation outside that closure is not
/// this export's watermark: an unrelated sibling never blocks it.
#[tokio::test]
async fn trace_export_dependency_blob_swap_or_watermark_change_blocks_publish() {
    let (dir, store, admin) = store().await;
    let first = files("authoritative");
    reviewed_record(&store, &admin, "export-d", &first).await;
    // Same size, different content.
    let mut swapped = first.clone();
    swapped[0].bytes = b"native member bytes swappedmember".to_vec();
    assert_eq!(swapped[0].bytes.len(), first[0].bytes.len());
    let before = fs_image(dir.path());
    assert!(matches!(
        store
            .stage_trace_export(&admin, &record_id("export-d"), &swapped)
            .await,
        Err(Error::Conflict(reason)) if reason ==
            "trace export candidate bytes differ from the frozen candidate"
    ));
    assert_eq!(
        fs_image(dir.path()),
        before,
        "the swap never reached staging"
    );
    // A staged member swapped after staging.
    let staged = store
        .stage_trace_export(&admin, &record_id("export-d"), &first)
        .await
        .unwrap();
    let staged_member = fs_image(&dir.path().join(".export-staging"))
        .into_iter()
        .find(|(path, _)| path.ends_with("traces/0000.json"))
        .map(|(path, _)| dir.path().join(".export-staging").join(path))
        .unwrap();
    make_writable(&staged_member);
    std::fs::write(&staged_member, b"native member bytes swappedmember").unwrap();
    assert!(matches!(
        store.publish_staged_trace_export(&admin, staged).await,
        Err(Error::Conflict(reason)) if reason == "staged trace export identity changed"
    ));
    assert!(staging_residue(dir.path()).is_empty());
    assert!(
        !store
            .trace_export_directory(&admin, "export-d")
            .unwrap()
            .exists()
    );

    // An unrelated sibling revocation moves the namespace watermark but not
    // this export's closure: a second export still publishes.
    let second = files("sibling-live");
    reviewed_record(&store, &admin, "export-e", &second).await;
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "run",
            "unrelated-sibling",
            admin.actor(),
            &json!({"id": "unrelated-sibling"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    revoke(&store, &admin, "unrelated-sibling").await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-e"), &second)
        .await
        .unwrap();
    store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();

    // An upstream run inside export-d's closure is revoked: blocked.
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "run",
            "upstream-of-d",
            admin.actor(),
            &json!({"id": "upstream-of-d"}),
        )
        .await
        .unwrap();
    session
        .put_edge(
            &admin,
            "run",
            &source_run("export-d"),
            "run",
            "upstream-of-d",
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let staged = store
        .stage_trace_export(&admin, &record_id("export-d"), &first)
        .await
        .unwrap();
    revoke(&store, &admin, "upstream-of-d").await;
    assert!(matches!(
        store.publish_staged_trace_export(&admin, staged).await,
        Err(Error::Conflict(reason)) if reason == "trace export source closure was revoked"
    ));
    assert!(matches!(
        store
            .stage_trace_export(&admin, &record_id("export-d"), &first)
            .await,
        Err(Error::Conflict(reason)) if reason == "trace export source closure was revoked"
    ));
    assert!(staging_residue(dir.path()).is_empty());
    assert!(
        !store
            .trace_export_directory(&admin, "export-d")
            .unwrap()
            .exists()
    );
}

/// A wrong tree is preserved exactly as it is and reported: a published tree
/// that no longer matches is the honest unavailable conflict, and a foreign
/// tree already occupying an export's formal directory is never overwritten.
#[tokio::test]
async fn trace_export_existing_wrong_tree_is_preserved_and_reported() {
    let (_dir, store, admin) = store().await;
    let first = files("exact");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    reviewed_record(&store, &admin, "export-f", &first).await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-f"), &first)
        .await
        .unwrap();
    store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    let tree = store.trace_export_directory(&admin, "export-f").unwrap();
    let frozen = members(&first);
    std::fs::write(tree.join("extra-wrong.json"), b"wrong tree content").unwrap();
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-f", &frozen, &first_digest)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
    assert!(tree.join("extra-wrong.json").exists(), "preserved as it is");
    std::fs::remove_file(tree.join("extra-wrong.json")).unwrap();
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-f", &frozen, &first_digest)
            .await
            .is_ok()
    );
    let original = std::fs::read(tree.join("manifest.json")).unwrap();
    make_writable(&tree.join("manifest.json"));
    std::fs::write(tree.join("manifest.json"), b"tampered manifest bytes").unwrap();
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-f", &frozen, &first_digest)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
    assert_eq!(
        std::fs::read(tree.join("manifest.json")).unwrap(),
        b"tampered manifest bytes",
        "the tampered bytes are left in place"
    );
    std::fs::write(tree.join("manifest.json"), original).unwrap();
    make_readonly(&tree.join("manifest.json"));
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-f", &frozen, &first_digest)
            .await
            .is_ok()
    );

    // A foreign tree already at another export's formal directory.
    let second = files("second");
    reviewed_record(&store, &admin, "export-g", &second).await;
    let occupied = store.trace_export_directory(&admin, "export-g").unwrap();
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join("foreign.json"), b"foreign tree").unwrap();
    let staged = store
        .stage_trace_export(&admin, &record_id("export-g"), &second)
        .await
        .unwrap();
    assert!(matches!(
        store.publish_staged_trace_export(&admin, staged).await,
        Err(Error::Conflict(reason)) if reason == "trace export target already exists"
    ));
    assert_eq!(
        std::fs::read(occupied.join("foreign.json")).unwrap(),
        b"foreign tree"
    );
    assert_eq!(std::fs::read_dir(&occupied).unwrap().count(), 1);
}

/// A completed export whose tree is gone is unavailable, and the read-only
/// proof never resurrects or resends it; a partial tree proves nothing.
#[tokio::test]
async fn trace_export_completed_missing_tree_is_unavailable_without_resend() {
    let (_dir, store, admin) = store().await;
    let first = files("completed");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    reviewed_record(&store, &admin, "export-h", &first).await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-h"), &first)
        .await
        .unwrap();
    store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    let tree = store.trace_export_directory(&admin, "export-h").unwrap();
    std::fs::remove_dir_all(&tree).unwrap();
    match store
        .verify_trace_export_tree(&admin, "export-h", &members(&first), &first_digest)
        .await
    {
        Err(Error::Conflict(reason)) => assert_eq!(reason, TRACE_EXPORT_TREE_UNAVAILABLE),
        other => panic!("the missing tree must be unavailable, got {other:?}"),
    }
    assert!(!tree.exists(), "no proof republished the tree");
    std::fs::create_dir_all(tree.join("traces")).unwrap();
    std::fs::write(tree.join("traces/0000.json"), &first[0].bytes).unwrap();
    make_readonly(&tree.join("traces/0000.json"));
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-h", &members(&first), &first_digest)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
}

/// The real revocation cleanup of a delivered export removes its published
/// tree under control, keeps the delivery fact (receipt, local scope, cost)
/// in the redacted record, states that remote erasure is not claimed, and
/// never touches another export's tree.
#[tokio::test]
async fn trace_export_cleanup_preserves_delivery_fact_and_explains_remote_residual() {
    let (dir, store, admin) = store().await;
    let first = files("delivered");
    let mut delivered = reviewed_record(&store, &admin, "export-i", &first).await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-i"), &first)
        .await
        .unwrap();
    let receipt = store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    delivered["phase"] = json!("delivered");
    delivered["delivery"] = json!({
        "receipt_digest": receipt.completion_receipt_digest,
        "local_export_ref": receipt.local_export_ref,
        "remote_erasure_disclaimer": DISCLAIMER,
    });
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &record_id("export-i"),
            admin.actor(),
            &delivered,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let other = files("other-live");
    reviewed_record(&store, &admin, "export-j", &other).await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-j"), &other)
        .await
        .unwrap();
    store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    let other_tree = fs_image(&store.trace_export_directory(&admin, "export-j").unwrap());

    let job = revoke(&store, &admin, &source_run("export-i")).await;
    let mut clock = 10;
    let status = loop {
        clock += 1;
        let status = LifecycleStore::cleanup_step(&admin, &store, &job, 8, clock)
            .await
            .unwrap();
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break status;
        }
        assert!(clock < 100, "the cleanup finishes");
    };
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert!(
        !store
            .trace_export_directory(&admin, "export-i")
            .unwrap()
            .exists(),
        "the revoked export's local tree was removed"
    );
    assert_eq!(
        fs_image(&store.trace_export_directory(&admin, "export-j").unwrap()),
        other_tree,
        "another export's tree is untouched"
    );
    let redacted: Value =
        serde_json::from_str(&record_body(&store, &admin, "export-i").await).unwrap();
    assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
    assert_eq!(redacted["state"], "source_revoked");
    assert_eq!(redacted["original_schema"], TRACE_EXPORT_SCHEMA);
    for key in ["export_id", "phase", "delivery", "local_cost", "members"] {
        assert_eq!(redacted["metadata"][key], delivered[key], "keeps {key}");
    }
    assert_eq!(
        redacted["metadata"]["delivery"]["remote_erasure_disclaimer"],
        DISCLAIMER
    );
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(dir.path().join(DATABASE))
            .read_only(true),
    )
    .await
    .unwrap();
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT details FROM revoke_cleanup_events
         WHERE namespace=? AND job_id=? AND node_kind='artifact' AND node_id=?
           AND event_kind='local_export_directory_deleted'",
    )
    .bind(admin.namespace())
    .bind(&job)
    .bind(record_id("export-i"))
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    assert_eq!(details.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&details[0]).unwrap(),
        json!({"remote_erasure_not_claimed": true})
    );
}

/// The delivery receipt and the class's protected consumption survive the real
/// redaction construction: the retained keys are exactly the lifecycle list,
/// the tombstone stays readable through the real put gate, and the consumed
/// history remains retained and read-only.
#[tokio::test]
async fn trace_export_receipt_and_consumption_survive_redaction() {
    let (_dir, store, admin) = store().await;
    let first = files("receipt");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    let mut record = reviewed_record(&store, &admin, "export-k", &first).await;
    let staged = store
        .stage_trace_export(&admin, &record_id("export-k"), &first)
        .await
        .unwrap();
    let receipt = store
        .publish_staged_trace_export(&admin, staged)
        .await
        .unwrap();
    record["phase"] = json!("delivered");
    record["delivery"] = json!({
        "receipt_digest": receipt.completion_receipt_digest,
        "local_export_ref": receipt.local_export_ref,
        "remote_erasure_disclaimer": DISCLAIMER,
    });
    let id = record_id("export-k");
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", &id, admin.actor(), &record)
        .await
        .unwrap();
    let body = session
        .object_row(&admin, "artifact", &id)
        .await
        .unwrap()
        .unwrap()
        .body;
    let tombstone = redacted_object_body(
        "artifact",
        &id,
        &serde_json::from_str::<Value>(&body).unwrap(),
        &body,
    );
    let retained = [
        "export_id",
        "phase",
        "prepare_job_id",
        "publish_job_id",
        "candidate_digest",
        "members",
        "delivery",
        "local_cost",
        "source_snapshots",
        "owner_actor",
    ];
    for key in retained {
        assert_eq!(
            tombstone["metadata"][key], record[key],
            "the redaction keeps {key}"
        );
    }
    assert_eq!(
        tombstone["metadata"]["delivery"]["receipt_digest"],
        json!(receipt.completion_receipt_digest)
    );
    session
        .put(&admin, "artifact", &id, admin.actor(), &tombstone)
        .await
        .unwrap();
    // The consumed history stays retained and read-only through the real gate.
    assert!(matches!(
        session.delete(&admin, "artifact", &id).await,
        Err(Error::Conflict(reason)) if reason == "trace export history must be retained"
    ));
    assert!(matches!(
        session.put(&admin, "artifact", &id, admin.actor(), &record).await,
        Err(Error::Conflict(reason)) if reason == "redacted trace export history is read only"
    ));
    let stored: Value = serde_json::from_str(
        &session
            .object_row(&admin, "artifact", &id)
            .await
            .unwrap()
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(stored, tombstone);
    session.commit().await.unwrap();
    // The delivered tree itself is untouched by the record's redaction.
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-k", &members(&first), &first_digest)
            .await
            .is_ok()
    );
}
