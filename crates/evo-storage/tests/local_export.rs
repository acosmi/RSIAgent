use evo_core::{Context, Role, fingerprint, hash};
use evo_storage::{LocalExportFile, Store, local_export_receipt, local_export_tree_digest};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Barrier;

async fn prepared_fixture() -> (
    tempfile::TempDir,
    Store,
    Context,
    String,
    Vec<LocalExportFile>,
    Value,
    Value,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("local-export.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("tenant", "admin", Role::Admin).unwrap();
    let source = json!({"schema_version":"fixture.source.v1","id":"source-1"});
    let source_digest = fingerprint(&source).unwrap();
    let projection = b"registered projection";
    let projection_digest = store.put_blob(&admin, projection).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", "source-1", admin.actor(), &source)
        .await
        .unwrap();
    let watermark = session
        .bump_watermark(&admin, "export-watermark")
        .await
        .unwrap() as u64;
    let export_id = "local-export-1".to_string();
    let files = vec![
        LocalExportFile {
            path: "manifest.json".into(),
            bytes: b"{}".to_vec(),
        },
        LocalExportFile {
            path: "nested/member.txt".into(),
            bytes: b"member bytes".to_vec(),
        },
    ];
    let (tree_digest, delivered_bytes) = local_export_tree_digest(&files).unwrap();
    let receipt =
        local_export_receipt(admin.namespace(), &export_id, &tree_digest, delivered_bytes).unwrap();
    let source_refs = json!([{
        "kind":"artifact",
        "id":"source-1",
        "digest":source_digest,
    }]);
    let attempt = json!({
        "schema_version":"rsia.e16.export_attempt.v2",
        "id":export_id,
        "namespace":admin.namespace(),
        "owner_actor":admin.actor(),
        "request_key":"export-key",
        "input_digest":hash(b"input"),
        "created_at":1,
        "updated_at":1,
        "source_refs":source_refs,
        "revoke_watermark":watermark,
        "payload":{
            "publisher":"org.rsia",
            "asset_id":"asset",
            "staged_asset_id":"staged-1",
            "manifest_digest":hash(b"manifest"),
            "projection_blob_bytes":projection.len(),
            "state":"prepared",
            "destination_scope":"local_namespace",
            "delivery_kind":"local_directory_v1",
            "local_export_ref":receipt.local_export_ref,
            "package_tree_digest":tree_digest,
            "delivered_bytes":delivered_bytes,
            "projection_blob_digest":projection_digest,
            "delivery_audit_id":null,
            "completion_receipt_digest":null,
            "budget_ref":null,
            "error":null,
        }
    });
    session
        .put(&admin, "artifact", &export_id, admin.actor(), &attempt)
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", &export_id, "artifact", "source-1")
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", &export_id, "blob", &projection_digest)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let audit_id = "delivery-audit-1";
    let mut completed = attempt.clone();
    completed["payload"]["state"] = json!("completed");
    completed["payload"]["delivery_audit_id"] = json!(audit_id);
    completed["payload"]["completion_receipt_digest"] = json!(receipt.completion_receipt_digest);
    let audit = json!({
        "schema_version":"rsia.e16.delivery_audit.v2",
        "id":audit_id,
        "namespace":admin.namespace(),
        "owner_actor":admin.actor(),
        "request_key":"export-key",
        "input_digest":hash(b"audit"),
        "created_at":2,
        "updated_at":2,
        "source_refs":attempt["source_refs"],
        "revoke_watermark":watermark,
        "payload":{
            "export_attempt_id":export_id,
            "package_id":"org.rsia:asset",
            "projection_blob_digest":attempt["payload"]["projection_blob_digest"],
            "delivery_kind":"local_directory_v1",
            "local_export_ref":receipt.local_export_ref,
            "package_tree_digest":receipt.package_tree_digest,
            "delivered_bytes":receipt.delivered_bytes,
            "completion_receipt_digest":receipt.completion_receipt_digest,
            "delivered_to":"local_namespace",
            "delivered_at":2,
            "watermark_at_delivery":watermark,
            "revoked":false,
            "revocation_notice_sent":false,
            "remote_erasure_disclaimer":"local only",
        }
    });
    (directory, store, admin, export_id, files, completed, audit)
}

#[tokio::test]
async fn registered_local_export_publishes_exact_tree_and_detects_change() {
    let (temp_root, store, admin, export_id, files, completed, audit) = prepared_fixture().await;
    let plan = store
        .snapshot_local_export_dependencies(&admin, &export_id)
        .await
        .unwrap();
    let dependencies = store
        .prevalidate_local_export_dependency_blobs(&admin, plan)
        .await
        .unwrap();
    let receipt = store
        .publish_registered_local_export(
            &admin,
            &export_id,
            &files,
            &completed,
            &audit,
            &dependencies,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .verify_registered_local_export(&admin, &export_id, &files)
            .await
            .unwrap(),
        receipt
    );
    let directory = store.local_export_directory(&admin, &export_id).unwrap();
    assert_eq!(
        tokio::fs::read(directory.join("nested/member.txt"))
            .await
            .unwrap(),
        b"member bytes"
    );
    let other = Context::new("other", "admin", Role::Admin).unwrap();
    assert!(
        store
            .verify_registered_local_export(&other, &export_id, &files)
            .await
            .is_err()
    );
    std::fs::hard_link(
        directory.join("nested/member.txt"),
        temp_root.path().join("outside-hardlink"),
    )
    .unwrap();
    assert!(
        store
            .verify_registered_local_export(&admin, &export_id, &files)
            .await
            .is_err()
    );
    std::fs::remove_file(temp_root.path().join("outside-hardlink")).unwrap();
    tokio::fs::remove_file(directory.join("nested/member.txt"))
        .await
        .unwrap();
    assert!(
        store
            .verify_registered_local_export(&admin, &export_id, &files)
            .await
            .is_err()
    );
}

#[test]
fn local_export_tree_rejects_unsafe_or_duplicate_paths() {
    for path in ["../escape", "/absolute", "a/../b", "a\\b"] {
        assert!(
            local_export_tree_digest(&[
                LocalExportFile {
                    path: "manifest.json".into(),
                    bytes: b"{}".to_vec(),
                },
                LocalExportFile {
                    path: path.into(),
                    bytes: b"x".to_vec(),
                },
            ])
            .is_err()
        );
    }
    assert!(
        local_export_tree_digest(&[
            LocalExportFile {
                path: "manifest.json".into(),
                bytes: b"{}".to_vec(),
            },
            LocalExportFile {
                path: "manifest.json".into(),
                bytes: b"other".to_vec(),
            },
        ])
        .is_err()
    );
}

#[tokio::test]
async fn restart_removes_private_export_staging_directories() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("restart.sqlite3");
    let store = Store::open(&database).await.unwrap();
    store.close().await;
    let stale = directory
        .path()
        .join(".export-staging")
        .join("namespace")
        .join("stale");
    tokio::fs::create_dir_all(&stale).await.unwrap();
    tokio::fs::write(stale.join("secret.tmp"), b"secret")
        .await
        .unwrap();
    let reopened = Store::open(&database).await.unwrap();
    assert!(
        !tokio::fs::try_exists(directory.path().join(".export-staging"))
            .await
            .unwrap()
    );
    reopened.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn registered_local_export_rejects_symlink_target_without_completion() {
    use std::os::unix::fs::symlink;

    let (directory, store, admin, export_id, files, completed, audit) = prepared_fixture().await;
    let plan = store
        .snapshot_local_export_dependencies(&admin, &export_id)
        .await
        .unwrap();
    let dependencies = store
        .prevalidate_local_export_dependency_blobs(&admin, plan)
        .await
        .unwrap();
    let target = store.local_export_directory(&admin, &export_id).unwrap();
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .unwrap();
    let elsewhere = directory.path().join("elsewhere");
    tokio::fs::create_dir_all(&elsewhere).await.unwrap();
    symlink(&elsewhere, &target).unwrap();
    assert!(
        store
            .publish_registered_local_export(
                &admin,
                &export_id,
                &files,
                &completed,
                &audit,
                &dependencies,
            )
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    let attempt: Value = session.need(&admin, "artifact", &export_id).await.unwrap();
    assert_eq!(attempt["payload"]["state"], "prepared");
    assert!(
        session
            .get::<Value>(&admin, "artifact", "delivery-audit-1")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn dependency_blob_preflight_holds_no_database_transaction_and_final_revoke_wins() {
    let (_directory, store, admin, export_id, files, completed, audit) = prepared_fixture().await;
    let second_blob = vec![b'z'; 2 * 1024 * 1024];
    let second_digest = store
        .put_blob_bounded(&admin, &second_blob, second_blob.len())
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&admin, "artifact", &export_id, "blob", &second_digest)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let plan = store
        .snapshot_local_export_dependencies(&admin, &export_id)
        .await
        .unwrap();

    let mut held_write = store.session().await.unwrap();
    held_write
        .put(
            &admin,
            "artifact",
            "concurrent-db-write",
            admin.actor(),
            &json!({"id":"concurrent-db-write"}),
        )
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let task_store = store.clone();
    let task_admin = admin.clone();
    let task_barrier = barrier.clone();
    let preflight = tokio::spawn(async move {
        task_barrier.wait().await;
        task_store
            .prevalidate_local_export_dependency_blobs(&task_admin, plan)
            .await
    });
    barrier.wait().await;
    let dependencies = tokio::time::timeout(std::time::Duration::from_secs(5), preflight)
        .await
        .expect("filesystem-only preflight blocked on the held DB writer")
        .unwrap()
        .unwrap();
    held_write
        .put(
            &admin,
            "tombstone",
            "source-1",
            admin.actor(),
            &json!({"id":"source-1"}),
        )
        .await
        .unwrap();
    held_write
        .bump_watermark(&admin, "revoked-after-preflight")
        .await
        .unwrap();
    held_write.commit().await.unwrap();

    assert!(
        store
            .publish_registered_local_export(
                &admin,
                &export_id,
                &files,
                &completed,
                &audit,
                &dependencies,
            )
            .await
            .is_err()
    );
    assert!(
        !tokio::fs::try_exists(store.local_export_directory(&admin, &export_id).unwrap())
            .await
            .unwrap()
    );
    let mut session = store.session().await.unwrap();
    let current: Value = session.need(&admin, "artifact", &export_id).await.unwrap();
    assert_eq!(current["payload"]["state"], "prepared");
    assert!(
        session
            .get::<Value>(&admin, "artifact", "delivery-audit-1")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn dependency_set_change_after_preflight_conflicts_without_delivery_fact() {
    let (_directory, store, admin, export_id, files, completed, audit) = prepared_fixture().await;
    let plan = store
        .snapshot_local_export_dependencies(&admin, &export_id)
        .await
        .unwrap();
    let dependencies = store
        .prevalidate_local_export_dependency_blobs(&admin, plan)
        .await
        .unwrap();
    let extra_digest = store.put_blob(&admin, b"new dependency").await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&admin, "artifact", &export_id, "blob", &extra_digest)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let error = store
        .publish_registered_local_export(
            &admin,
            &export_id,
            &files,
            &completed,
            &audit,
            &dependencies,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("closure changed"));
    let mut session = store.session().await.unwrap();
    let current: Value = session.need(&admin, "artifact", &export_id).await.unwrap();
    assert_eq!(current["payload"]["state"], "prepared");
    assert!(
        session
            .get::<Value>(&admin, "artifact", "delivery-audit-1")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn staged_asset_secondary_blob_preflight_is_db_free_and_revoke_blocks_publish() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("staged-secondary.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("tenant", "admin", Role::Admin).unwrap();
    let secondary = vec![b's'; 2 * 1024 * 1024];
    let secondary_digest = store
        .put_blob_bounded(&admin, &secondary, secondary.len())
        .await
        .unwrap();
    let output = b"staged package projection";
    let output_digest = hash(output);
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&admin, "staged-secondary-watermark")
        .await
        .unwrap() as u64;
    let import_source = json!({
        "schema_version":"rsia.e16.import_source.v1",
        "id":"secondary-import-source",
        "namespace":admin.namespace(),
        "owner_actor":admin.actor(),
        "request_key":"secondary-import",
        "input_digest":hash(b"secondary-import-input"),
        "created_at":1,
        "updated_at":1,
        "source_refs":[{"kind":"blob","id":secondary_digest,"content_digest":secondary_digest}],
        "revoke_watermark":watermark,
        "payload":{
            "status":"ready",
            "raw_blob_digest":secondary_digest,
            "blob_published":true,
        }
    });
    session
        .put(
            &admin,
            "artifact",
            "secondary-import-source",
            admin.actor(),
            &import_source,
        )
        .await
        .unwrap();
    session
        .put_edge(
            &admin,
            "artifact",
            "secondary-import-source",
            "blob",
            &secondary_digest,
        )
        .await
        .unwrap();
    let staged = json!({
        "schema_version":"rsia.e16.staged_asset.v1",
        "id":"staged-secondary",
        "namespace":admin.namespace(),
        "owner_actor":admin.actor(),
        "request_key":"staged-secondary",
        "input_digest":hash(b"staged-secondary-input"),
        "created_at":1,
        "updated_at":1,
        "source_refs":[{
            "kind":"artifact",
            "id":"secondary-import-source",
            "digest":fingerprint(&import_source).unwrap(),
        }],
        "revoke_watermark":watermark,
        "payload":{
            "state":"prepared",
            "content_blob_digest":output_digest,
        }
    });
    session
        .put(
            &admin,
            "artifact",
            "staged-secondary",
            admin.actor(),
            &staged,
        )
        .await
        .unwrap();
    session
        .put_edge(
            &admin,
            "artifact",
            "staged-secondary",
            "artifact",
            "secondary-import-source",
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let plan = store
        .snapshot_local_export_dependencies(&admin, "staged-secondary")
        .await
        .unwrap();
    let mut held_write = store.session().await.unwrap();
    held_write
        .put(
            &admin,
            "artifact",
            "staged-concurrent-writer",
            admin.actor(),
            &json!({"id":"staged-concurrent-writer"}),
        )
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let task_store = store.clone();
    let task_admin = admin.clone();
    let task_barrier = barrier.clone();
    let preflight = tokio::spawn(async move {
        task_barrier.wait().await;
        task_store
            .prevalidate_local_export_dependency_blobs(&task_admin, plan)
            .await
    });
    barrier.wait().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), preflight)
        .await
        .expect("staged dependency preflight blocked on DB writer")
        .unwrap()
        .unwrap();
    held_write
        .put(
            &admin,
            "tombstone",
            "secondary-import-source",
            admin.actor(),
            &json!({"id":"secondary-import-source"}),
        )
        .await
        .unwrap();
    held_write
        .bump_watermark(&admin, "staged-secondary-revoked")
        .await
        .unwrap();
    held_write.commit().await.unwrap();

    assert!(
        store
            .publish_registered_blob(
                &admin,
                "staged-secondary",
                "rsia.e16.staged_asset.v1",
                "content_blob_digest",
                output,
                output.len(),
            )
            .await
            .is_err()
    );
    assert!(
        store
            .read_blob(&admin, &output_digest, output.len())
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    let current: Value = session
        .need(&admin, "artifact", "staged-secondary")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(current["payload"]["state"], "prepared");
}

#[tokio::test]
async fn concurrent_registered_producers_same_digest_conflict_then_reconnect_idempotently() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("same-digest.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("tenant", "admin", Role::Admin).unwrap();
    let bytes = vec![b'p'; 2 * 1024 * 1024];
    let digest = hash(&bytes);
    let source = json!({"schema_version":"fixture.source.v1","id":"shared-source"});
    let source_digest = fingerprint(&source).unwrap();
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&admin, "same-digest-watermark")
        .await
        .unwrap() as u64;
    session
        .put(&admin, "artifact", "shared-source", admin.actor(), &source)
        .await
        .unwrap();
    for id in ["same-producer-a", "same-producer-b"] {
        let staged = json!({
            "schema_version":"rsia.e16.staged_asset.v1",
            "id":id,
            "namespace":admin.namespace(),
            "owner_actor":admin.actor(),
            "request_key":id,
            "input_digest":hash(id.as_bytes()),
            "created_at":1,
            "updated_at":1,
            "source_refs":[{"kind":"artifact","id":"shared-source","digest":source_digest}],
            "revoke_watermark":watermark,
            "payload":{"state":"prepared","content_blob_digest":digest},
        });
        session
            .put(&admin, "artifact", id, admin.actor(), &staged)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "artifact", "shared-source")
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "blob", &digest)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();

    let first = store
        .preflight_registered_blob_publish(
            &admin,
            "same-producer-a",
            "rsia.e16.staged_asset.v1",
            "content_blob_digest",
            &bytes,
            bytes.len(),
        )
        .await
        .unwrap();
    let second = store
        .preflight_registered_blob_publish(
            &admin,
            "same-producer-b",
            "rsia.e16.staged_asset.v1",
            "content_blob_digest",
            &bytes,
            bytes.len(),
        )
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        store.publish_preflighted_registered_blob(&admin, first, &bytes),
        store.publish_preflighted_registered_blob(&admin, second, &bytes)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let conflict = if left.is_err() { left } else { right };
    assert!(
        conflict
            .unwrap_err()
            .to_string()
            .contains("appeared after preflight")
    );

    for id in ["same-producer-a", "same-producer-b"] {
        assert_eq!(
            store
                .publish_registered_blob(
                    &admin,
                    id,
                    "rsia.e16.staged_asset.v1",
                    "content_blob_digest",
                    &bytes,
                    bytes.len(),
                )
                .await
                .unwrap(),
            digest
        );
    }
    assert_eq!(
        store.read_blob(&admin, &digest, bytes.len()).await.unwrap(),
        bytes
    );
    let mut session = store.session().await.unwrap();
    for id in ["same-producer-a", "same-producer-b"] {
        let staged: Value = session.need(&admin, "artifact", id).await.unwrap();
        assert_eq!(staged["payload"]["state"], "prepared");
    }
    session.commit().await.unwrap();
}
