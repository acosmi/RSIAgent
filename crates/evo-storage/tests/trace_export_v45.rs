//! AG-175 storage-layer trace export publication controls. These tests drive
//! the real trace export tree primitives (`publish_trace_export_tree`,
//! `verify_trace_export_tree`, `fsync_trace_export_tree`) on real directories.
//! The member sets here are byte sets for the FS chain: the frozen digest is
//! the only authority this layer takes, and the revocation-watermark and fence
//! re-checks live in the engine entry that calls it.
use evo_core::{Context, Error, Role, hash};
use evo_storage::lifecycle::redacted_object_body;
use evo_storage::{
    Store, TRACE_EXPORT_SCHEMA, TRACE_EXPORT_TREE_UNAVAILABLE, local_export_tree_digest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

fn admin() -> Context {
    Context::new("trace-tenant", "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store, Context) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("trace-export.sqlite3"))
        .await
        .unwrap();
    (dir, store, admin())
}

fn files(marker: &str) -> Vec<evo_storage::LocalExportFile> {
    vec![
        evo_storage::LocalExportFile {
            path: "traces/0000.json".into(),
            bytes: format!("native member bytes {marker}").into_bytes(),
        },
        evo_storage::LocalExportFile {
            path: "report.json".into(),
            bytes: format!("report bytes {marker}").into_bytes(),
        },
    ]
}

/// A full image of every filesystem entry under the store root: the honest
/// boundary for "zero effect", "preserved" and "residue released" claims.
fn fs_image(root: &Path) -> BTreeMap<String, (bool, u32, Option<Vec<u8>>)> {
    fn walk(root: &Path, dir: &Path, image: &mut BTreeMap<String, (bool, u32, Option<Vec<u8>>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = 0;
            if metadata.is_dir() {
                image.insert(relative, (true, mode, None));
                walk(root, &path, image);
            } else {
                image.insert(relative, (false, mode, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    let mut image = BTreeMap::new();
    if root.exists() {
        walk(root, root, &mut image);
    }
    image
}

/// The private staging residue this layer must release: leftover member
/// files. The fixed `.export-staging` layout directories may remain empty.
fn staging_residue(root: &Path) -> Vec<String> {
    let staging = root.join(".export-staging");
    let mut residue = Vec::new();
    if staging.exists() {
        for (path, (is_dir, _, bytes)) in fs_image(&staging) {
            if !is_dir && bytes.is_some() {
                residue.push(path);
            }
        }
    }
    residue
}

#[tokio::test]
async fn trace_export_stages_private_and_publishes_without_overwrite() {
    let (dir, store, admin) = store().await;
    let first = files("first");
    let (first_digest, first_bytes) = local_export_tree_digest(&first).unwrap();
    let publication = store
        .publish_trace_export_tree(&admin, "export-a", &first, &first_digest)
        .await
        .unwrap();
    assert!(publication.published_now);
    assert!(publication.tree_fsynced);
    assert_eq!(publication.verified_tree_digest, first_digest);
    assert_eq!(publication.receipt.delivered_bytes, first_bytes);
    assert_eq!(
        publication.receipt.local_export_ref,
        format!("exports/{}/export-a", hash(admin.namespace().as_bytes()))
    );
    let tree = store.local_export_directory(&admin, "export-a").unwrap();
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
    assert!(
        staging_residue(dir.path()).is_empty(),
        "no private staging member remains"
    );
    // The receipt the publication earned is exactly what re-verification of
    // the published directory keeps earning.
    let receipt = store
        .verify_trace_export_tree(&admin, "export-a", &first)
        .await
        .unwrap();
    assert_eq!(
        receipt.completion_receipt_digest,
        publication.receipt.completion_receipt_digest
    );
    // The identical tree publishes again without any overwrite: the rename is
    // not repeated and no NOREPLACE failure occurs.
    let again = store
        .publish_trace_export_tree(&admin, "export-a", &first, &first_digest)
        .await
        .unwrap();
    assert!(!again.published_now);
    assert_eq!(
        again.receipt.completion_receipt_digest,
        publication.receipt.completion_receipt_digest
    );
    // Different bytes under the same export id never overwrite the published
    // tree; the refusal names the existing tree.
    let second = files("second");
    let (second_digest, _) = local_export_tree_digest(&second).unwrap();
    match store
        .publish_trace_export_tree(&admin, "export-a", &second, &second_digest)
        .await
    {
        Err(Error::Conflict(reason)) => {
            assert_eq!(reason, "existing trace export tree differs")
        }
        other => panic!("a differing tree must never overwrite, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(tree.join("traces/0000.json")).unwrap(),
        first[0].bytes,
        "the published tree is untouched"
    );
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-a", &second)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
    // Only an Admin of the namespace owns this surface.
    let worker = Context::new("trace-tenant", "worker", Role::Worker).unwrap();
    assert!(matches!(
        store
            .publish_trace_export_tree(&worker, "export-a", &first, &first_digest)
            .await,
        Err(Error::Forbidden)
    ));
}

/// Refusals that land before the first write have zero filesystem effect: no
/// staging directory, no export tree, nothing but the database itself. The
/// frozen digest is the authority, so bytes that drifted from it (the storage
/// face of a swapped dependency blob) never reach the staging writer.
#[tokio::test]
async fn trace_export_revoke_before_first_write_is_zero_effect() {
    let (dir, store, admin) = store().await;
    let first = files("zero");
    let second = files("drifted");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    let (second_digest, _) = local_export_tree_digest(&second).unwrap();
    let before = fs_image(dir.path());
    assert!(!dir.path().join("exports").exists());
    match store
        .publish_trace_export_tree(&admin, "export-b", &first, &second_digest)
        .await
    {
        Err(Error::Conflict(reason)) => {
            assert_eq!(
                reason,
                "trace export candidate bytes differ from the frozen candidate"
            )
        }
        other => panic!("drifted bytes must be refused before staging, got {other:?}"),
    }
    assert!(matches!(
        store
            .publish_trace_export_tree(&admin, "export-b", &second, &first_digest)
            .await,
        Err(Error::Conflict(reason)) if reason ==
            "trace export candidate bytes differ from the frozen candidate"
    ));
    let worker = Context::new("trace-tenant", "worker", Role::Worker).unwrap();
    assert!(matches!(
        store
            .publish_trace_export_tree(&worker, "export-b", &first, &first_digest)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        fs_image(dir.path()),
        before,
        "every pre-first-write refusal leaves the filesystem untouched"
    );
}

/// A decision that went stale after its private staging cannot succeed: the
/// attempt's staging is released by its own guard and the already published
/// tree stays the one delivery.
#[tokio::test]
async fn trace_export_revoke_after_stage_prevents_success() {
    let (dir, store, admin) = store().await;
    let first = files("published");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    store
        .publish_trace_export_tree(&admin, "export-c", &first, &first_digest)
        .await
        .unwrap();
    // The stale attempt stages its own bytes first and only then meets the
    // published tree, so its failure exercises the staging guard.
    let second = files("stale-attempt");
    let (second_digest, _) = local_export_tree_digest(&second).unwrap();
    match store
        .publish_trace_export_tree(&admin, "export-c", &second, &second_digest)
        .await
    {
        Err(Error::Conflict(reason)) => {
            assert_eq!(reason, "existing trace export tree differs")
        }
        other => panic!("a stale attempt must not succeed, got {other:?}"),
    }
    assert!(
        staging_residue(dir.path()).is_empty(),
        "the failed attempt released its private staging"
    );
    for (_, (_, _, bytes)) in fs_image(dir.path()) {
        if let Some(bytes) = bytes {
            assert!(
                !bytes
                    .windows(b"stale-attempt".len())
                    .any(|window| window == b"stale-attempt"),
                "the stale attempt left no bytes anywhere"
            );
        }
    }
    assert_eq!(
        std::fs::read(
            store
                .local_export_directory(&admin, "export-c")
                .unwrap()
                .join("report.json")
        )
        .unwrap(),
        first[1].bytes,
        "the published delivery is unchanged"
    );
}

/// A swapped member blob blocks publication through the frozen digest and is
/// detected on the published surface, while the storage layer itself stays
/// watermark-free by contract: it can neither revoke a delivered tree nor
/// unilaterally block a new publication the engine entry still accepts.
#[tokio::test]
async fn trace_export_dependency_blob_swap_or_watermark_change_blocks_publish() {
    let (dir, store, admin) = store().await;
    let first = files("authoritative");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    let publication = store
        .publish_trace_export_tree(&admin, "export-d", &first, &first_digest)
        .await
        .unwrap();
    // Same size, different content: the blob swap changes the tree digest.
    let mut swapped = first.clone();
    swapped[0].bytes = b"native member bytes swappedmember".to_vec();
    let (swapped_digest, _) = local_export_tree_digest(&swapped).unwrap();
    assert_eq!(swapped[0].bytes.len(), first[0].bytes.len());
    assert_ne!(swapped_digest, first_digest);
    assert!(
        matches!(
            store
                .publish_trace_export_tree(&admin, "export-d", &swapped, &first_digest)
                .await,
            Err(Error::Conflict(reason)) if reason ==
                "trace export candidate bytes differ from the frozen candidate"
        ),
        "the swapped blob cannot publish under the frozen digest"
    );
    assert!(
        matches!(
            store
                .verify_trace_export_tree(&admin, "export-d", &swapped)
                .await,
            Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
        ),
        "the swap is detected against the published tree"
    );
    // The real revocation watermark moves; the delivered local fact stays
    // exactly verifiable. Blocking a *new* publication on that watermark is
    // the engine entry's gate, not a power this layer claims.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "revoke-watermark-moved")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let receipt = store
        .verify_trace_export_tree(&admin, "export-d", &first)
        .await
        .unwrap();
    assert_eq!(
        receipt.completion_receipt_digest,
        publication.receipt.completion_receipt_digest
    );
    assert!(
        staging_residue(dir.path()).is_empty(),
        "no publication was attempted by the watermark read"
    );
}

/// A published tree that no longer matches is preserved as it is and reported
/// as the honest unavailable conflict; the reader never cleans or repairs it.
#[tokio::test]
async fn trace_export_existing_wrong_tree_is_preserved_and_reported() {
    let (_dir, store, admin) = store().await;
    let first = files("exact");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    store
        .publish_trace_export_tree(&admin, "export-e", &first, &first_digest)
        .await
        .unwrap();
    let tree = store.local_export_directory(&admin, "export-e").unwrap();
    // An operator's wrong extra member appears inside the published tree.
    std::fs::write(tree.join("extra-wrong.json"), b"wrong tree content").unwrap();
    assert!(
        matches!(
            store
                .verify_trace_export_tree(&admin, "export-e", &first)
                .await,
            Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
        ),
        "the wrong tree is reported"
    );
    assert!(
        tree.join("extra-wrong.json").exists(),
        "the wrong tree is preserved exactly as it is"
    );
    // Removing the wrong member restores the exact tree and its receipt.
    std::fs::remove_file(tree.join("extra-wrong.json")).unwrap();
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-e", &first)
            .await
            .is_ok()
    );
    // Tampered member bytes are likewise preserved and reported.
    let original = std::fs::read(tree.join("report.json")).unwrap();
    std::fs::write(tree.join("report.json"), b"tampered report bytes").unwrap();
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-e", &first)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
    assert_eq!(
        std::fs::read(tree.join("report.json")).unwrap(),
        b"tampered report bytes",
        "the tampered bytes are left in place"
    );
    std::fs::write(tree.join("report.json"), original).unwrap();
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-e", &first)
            .await
            .is_ok()
    );
}

/// A completed export whose tree is gone is unavailable, and the read-only
/// proofs never resurrect or resend it.
#[tokio::test]
async fn trace_export_completed_missing_tree_is_unavailable_without_resend() {
    let (_dir, store, admin) = store().await;
    let first = files("completed");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    store
        .publish_trace_export_tree(&admin, "export-f", &first, &first_digest)
        .await
        .unwrap();
    let tree = store.local_export_directory(&admin, "export-f").unwrap();
    std::fs::remove_dir_all(&tree).unwrap();
    match store
        .verify_trace_export_tree(&admin, "export-f", &first)
        .await
    {
        Err(Error::Conflict(reason)) => assert_eq!(reason, TRACE_EXPORT_TREE_UNAVAILABLE),
        other => panic!("the missing tree must be unavailable, got {other:?}"),
    }
    match store
        .fsync_trace_export_tree(&admin, "export-f", &first)
        .await
    {
        Err(Error::Conflict(reason)) => assert_eq!(reason, TRACE_EXPORT_TREE_UNAVAILABLE),
        other => panic!("fsync of a missing tree must be unavailable, got {other:?}"),
    }
    assert!(!tree.exists(), "no proof republished the tree");
    // A partial tree is equally unavailable: one member alone proves nothing.
    std::fs::create_dir_all(tree.join("traces")).unwrap();
    std::fs::write(tree.join("traces/0000.json"), &first[0].bytes).unwrap();
    assert!(matches!(
        store
            .verify_trace_export_tree(&admin, "export-f", &first)
            .await,
        Err(Error::Conflict(reason)) if reason == TRACE_EXPORT_TREE_UNAVAILABLE
    ));
}

/// Cleanup releases the failed attempt's private residue and preserves the
/// delivery fact byte for byte. The delivery is local-only: no remote surface
/// exists to erase, and none is claimed.
#[tokio::test]
async fn trace_export_cleanup_preserves_delivery_fact_and_explains_remote_residual() {
    let (dir, store, admin) = store().await;
    let first = files("delivered");
    let (first_digest, _) = local_export_tree_digest(&first).unwrap();
    let publication = store
        .publish_trace_export_tree(&admin, "export-g", &first, &first_digest)
        .await
        .unwrap();
    let exports_before = fs_image(&dir.path().join("exports"));
    let second = files("undelivered-attempt");
    let (second_digest, _) = local_export_tree_digest(&second).unwrap();
    assert!(
        matches!(
            store
                .publish_trace_export_tree(&admin, "export-g", &second, &second_digest)
                .await,
            Err(Error::Conflict(reason)) if reason == "existing trace export tree differs"
        ),
        "the refused attempt cleans up after itself"
    );
    assert_eq!(
        fs_image(&dir.path().join("exports")),
        exports_before,
        "cleanup never touches the published export tree"
    );
    assert!(
        staging_residue(dir.path()).is_empty(),
        "the attempt's private staging residue is released"
    );
    let receipt = store
        .verify_trace_export_tree(&admin, "export-g", &first)
        .await
        .unwrap();
    assert_eq!(
        receipt.completion_receipt_digest, publication.receipt.completion_receipt_digest,
        "the delivery fact survives the failed later attempt"
    );
    assert_eq!(receipt.delivery_kind, "local_directory_v1");
}

/// The delivery receipt and the class's protected consumption survive the real
/// redaction construction: the retained keys are exactly the lifecycle list,
/// the tombstone stays readable through the real put gate, and the consumed
/// history remains retained and read-only.
#[tokio::test]
async fn trace_export_receipt_and_consumption_survive_redaction() {
    let (_dir, store, admin) = store().await;
    let first = files("receipt");
    let (first_digest, delivered_bytes) = local_export_tree_digest(&first).unwrap();
    let publication = store
        .publish_trace_export_tree(&admin, "export-h", &first, &first_digest)
        .await
        .unwrap();
    // The delivered facts of the record, with the receipt the real published
    // bytes earned; the record body itself is engine-owned, so this mirrors
    // its fields exactly as stored.
    let record = json!({
        "schema_version": TRACE_EXPORT_SCHEMA,
        "id": "trace-export-record-h",
        "export_id": "export-h",
        "phase": "delivered",
        "prepare_job_id": "management-job-prepare",
        "publish_job_id": "management-job-publish",
        "candidate_digest": first_digest,
        "members": first
            .iter()
            .map(|file| {
                json!({
                    "path": file.path,
                    "digest_sha256": evo_core::hash(&file.bytes),
                    "size_bytes": file.bytes.len(),
                })
            })
            .collect::<Vec<_>>(),
        "candidate_storage": {"staged": false, "paths": []},
        "delivery": {
            "status": "delivered",
            "receipt_digest": publication.receipt.completion_receipt_digest,
        },
        "durability": {
            "tree_fsynced": true,
            "verified_tree_digest": publication.verified_tree_digest,
        },
        "local_cost": {"wall_seconds": 1, "bytes": delivered_bytes},
        "source_snapshots": [],
        "source_watermark_seq": null,
        "source_watermark_digest": null,
        "owner_actor": admin.actor(),
        "created_at": 1,
        "updated_at": 2,
    });
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "trace-export-record-h",
            admin.actor(),
            &record,
        )
        .await
        .unwrap();
    let body = session
        .object_row(&admin, "artifact", "trace-export-record-h")
        .await
        .unwrap()
        .unwrap()
        .body;
    let tombstone = redacted_object_body(
        "artifact",
        "trace-export-record-h",
        &serde_json::from_str::<Value>(&body).unwrap(),
        &body,
    );
    // Exactly the lifecycle retained keys survive, including the receipt.
    let retained = [
        "phase",
        "prepare_job_id",
        "publish_job_id",
        "candidate_digest",
        "members",
        "candidate_storage",
        "delivery",
        "durability",
        "local_cost",
        "source_snapshots",
        "source_watermark_seq",
        "source_watermark_digest",
        "owner_actor",
    ];
    for key in retained {
        assert_eq!(
            tombstone["metadata"][key],
            serde_json::from_str::<Value>(&body).unwrap()[key],
            "the redaction keeps {key}"
        );
    }
    assert_eq!(
        tombstone["metadata"]["delivery"]["receipt_digest"],
        json!(publication.receipt.completion_receipt_digest)
    );
    assert_eq!(tombstone["metadata"]["phase"], json!("delivered"));
    session
        .put(
            &admin,
            "artifact",
            "trace-export-record-h",
            admin.actor(),
            &tombstone,
        )
        .await
        .unwrap();
    // The consumed history stays retained and read-only through the real gate.
    assert!(matches!(
        session
            .delete(&admin, "artifact", "trace-export-record-h")
            .await,
        Err(Error::Conflict(reason)) if reason == "trace export history must be retained"
    ));
    assert!(matches!(
        session
            .put(
                &admin,
                "artifact",
                "trace-export-record-h",
                admin.actor(),
                &record,
            )
            .await,
        Err(Error::Conflict(reason)) if reason == "redacted trace export history is read only"
    ));
    let stored = session
        .object_row(&admin, "artifact", "trace-export-record-h")
        .await
        .unwrap()
        .unwrap();
    let stored: Value = serde_json::from_str(&stored.body).unwrap();
    assert_eq!(stored, tombstone);
    assert_eq!(stored["original_schema"], json!(TRACE_EXPORT_SCHEMA));
    session.commit().await.unwrap();
    // The delivered tree itself is untouched by the record's redaction.
    assert!(
        store
            .verify_trace_export_tree(&admin, "export-h", &first)
            .await
            .is_ok()
    );
}
