//! Actual Store backup authorization and fixed-root filesystem behavior.
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{BackupDurability, BackupManifest, BackupName};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

async fn fixture() -> (tempfile::TempDir, Store, Context) {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let directory = tempfile::Builder::new()
        .permissions(<std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("rsia.sqlite3");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&database)
        .unwrap();
    let store = Store::open(&database).await.unwrap();
    let admin = Context::new("n", "backup-admin", Role::Admin).unwrap();
    let other = Context::new("other", "other-admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    for ctx in [&admin, &other] {
        session
            .bump_watermark(ctx, &hash(ctx.namespace().as_bytes()))
            .await
            .unwrap();
        session
            .put(
                ctx,
                "artifact",
                "kept",
                ctx.actor(),
                &json!({"id":"kept","namespace":ctx.namespace()}),
            )
            .await
            .unwrap();
        session.audit(ctx, "fixture.created", "kept").await.unwrap();
    }
    session.commit().await.unwrap();
    // Raw blob bytes are controlled inputs created privately; this fixture does
    // not claim AG157 first-publication behavior.
    let bytes = b"actual backup blob";
    let blob = hash(bytes);
    let namespace = root.join("blobs").join(hash(admin.namespace().as_bytes()));
    for path in [root.join("blobs"), namespace.clone()] {
        std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(namespace.join(&blob))
        .unwrap()
        .write_all(bytes)
        .unwrap();
    assert_eq!(store.put_blob(&admin, bytes).await.unwrap(), blob);
    (directory, store, admin)
}

#[tokio::test]
async fn backup_requires_admin_before_destination_clock_or_platform_observation() {
    let (directory, store, _) = fixture().await;
    store.close().await;
    std::fs::remove_dir_all(directory.path()).unwrap();
    for role in [Role::Evaluator, Role::Host, Role::Agent] {
        let caller = Context::new("n", "unprivileged", role).unwrap();
        assert!(matches!(
            store.backup(&caller, BackupName::parse("refused").unwrap()),
            Err(Error::Forbidden)
        ));
        assert!(!directory.path().exists());
    }
}

#[tokio::test]
async fn authorized_backup_keeps_v2_manifest_and_full_snapshot() {
    const BARE_CHILD: &str = "RSIA_AG164_BARE_DATABASE_CHILD";
    if std::env::var_os(BARE_CHILD).is_some() {
        use std::os::unix::fs::OpenOptionsExt;
        let database = std::path::Path::new("bare.sqlite3");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(database)
            .unwrap();
        let store = Store::open(database).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"bare-watermark"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        let publication = store
            .backup(&admin, BackupName::parse("bare").unwrap())
            .unwrap()
            .complete()
            .await
            .unwrap();
        assert_eq!(
            publication.directory(),
            std::env::current_dir().unwrap().join("backups/bare")
        );
        assert_eq!(publication.durability, BackupDurability::Verified);
        store.close().await;
        return;
    }
    let (directory, store, admin) = fixture().await;
    let publication = store
        .backup(&admin, BackupName::parse("complete").unwrap())
        .unwrap()
        .complete()
        .await
        .unwrap();
    assert_eq!(
        publication.directory(),
        directory
            .path()
            .canonicalize()
            .unwrap()
            .join("backups/complete")
    );
    assert_eq!(publication.durability, BackupDurability::Verified);
    let raw = std::fs::read(publication.directory().join("backup-manifest.json")).unwrap();
    let value: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    let manifest: BackupManifest = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest.schema_version, "rsia.backup.v2");
    assert!(manifest.complete);
    assert_eq!(manifest.watermarks.len(), 2);
    assert!(!manifest.migrations.is_empty());
    assert_eq!(publication.manifest_sha256, hash(&raw));
    for entry in std::iter::once(&manifest.database).chain(&manifest.blobs) {
        let bytes = std::fs::read(publication.directory().join(&entry.path)).unwrap();
        assert_eq!(hash(&bytes), entry.sha256);
        assert_eq!(bytes.len() as u64, entry.bytes);
    }
    let connection = rusqlite::Connection::open_with_flags(
        publication.directory().join("rsia.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM objects WHERE kind='artifact' AND id='kept'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM objects WHERE kind='job'", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    store.close().await;
    let child_root = tempfile::Builder::new()
        .permissions(<std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700))
        .tempdir()
        .unwrap();
    assert!(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "authorized_backup_keeps_v2_manifest_and_full_snapshot"
            ])
            .env(BARE_CHILD, "1")
            .current_dir(child_root.path().canonicalize().unwrap())
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn backup_destination_policy_rejects_escape_link_and_foreign_parent() {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
    for name in [
        "",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        ".hidden",
        "a.b",
        "é",
        &"a".repeat(65),
    ] {
        assert!(BackupName::parse(name).is_err(), "name={name}");
    }
    let (directory, store, admin) = fixture().await;
    let outside = tempfile::tempdir().unwrap();
    let parent = directory.path().join("backups");
    symlink(outside.path(), &parent).unwrap();
    assert!(
        matches!(store.backup(&admin, BackupName::parse("linked").unwrap()).unwrap().complete().await, Err(Error::Conflict(reason)) if reason == "data_directory_insecure")
    );
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    std::fs::remove_file(&parent).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        matches!(store.backup(&admin, BackupName::parse("foreign").unwrap()).unwrap().complete().await, Err(Error::Conflict(reason)) if reason == "data_directory_insecure")
    );
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o777
    );
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 0);
    store.close().await;
    let root = directory.path().canonicalize().unwrap();
    let real = root.join("real");
    let child = real.join("child");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&real)
        .unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&child)
        .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(child.join("linked.sqlite3"))
        .unwrap();
    let alias = root.join("alias");
    symlink(&real, &alias).unwrap();
    for path in [
        alias.join("child/linked.sqlite3"),
        alias.join("../real/child/linked.sqlite3"),
    ] {
        let linked = Store::open(&path).await.unwrap();
        assert!(
            matches!(linked.backup(&admin, BackupName::parse("ancestor-link").unwrap()).unwrap().complete().await,
            Err(Error::Conflict(reason)) if reason == "data_directory_insecure")
        );
        assert!(!child.join("backups").exists());
        linked.close().await;
    }
}

#[tokio::test]
async fn backup_members_have_private_modes_under_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "RSIA_AG164_UMASK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backup_members_have_private_modes_under_permissive_umask",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let prior = rustix::process::umask(rustix::fs::Mode::empty());
    let (_directory, store, admin) = fixture().await;
    let publication = store
        .backup(&admin, BackupName::parse("private").unwrap())
        .unwrap()
        .complete()
        .await
        .unwrap();
    let mut pending = vec![publication.directory().to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            if metadata.is_dir() { 0o700 } else { 0o600 },
            "{path:?}"
        );
        if metadata.is_dir() {
            pending.extend(
                std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
    store.close().await;
    rustix::process::umask(prior);
}

#[tokio::test]
async fn backup_atomic_publish_does_not_replace_existing_destination() {
    let (directory, store, admin) = fixture().await;
    let first = store
        .backup(&admin, BackupName::parse("same").unwrap())
        .unwrap()
        .complete()
        .await
        .unwrap();
    let before = std::fs::read(first.directory().join("backup-manifest.json")).unwrap();
    let refused = store
        .backup(&admin, BackupName::parse("same").unwrap())
        .unwrap()
        .complete()
        .await;
    assert!(
        matches!(refused, Err(Error::Conflict(reason)) if reason == "backup destination already exists")
    );
    assert_eq!(
        std::fs::read(first.directory().join("backup-manifest.json")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_dir(directory.path().join("backups"))
            .unwrap()
            .count(),
        1
    );
    store.close().await;
}

#[tokio::test]
async fn backup_prepublication_failure_removes_only_owned_temporary_directory() {
    let (directory, store, admin) = fixture().await;
    let preparing = store
        .backup(&admin, BackupName::parse("interrupted").unwrap())
        .unwrap()
        .begin(0, None)
        .await
        .unwrap();
    let unrelated = directory.path().join("backups/.unrelated.tmp");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep"), b"unrelated").unwrap();
    let refused = preparing
        .prepare(
            Instant::now() - Duration::from_secs(1),
            Arc::new(AtomicBool::new(true)),
        )
        .await;
    assert!(refused.is_err());
    assert!(!directory.path().join("backups/interrupted").exists());
    assert_eq!(std::fs::read(unrelated.join("keep")).unwrap(), b"unrelated");
    assert_eq!(
        std::fs::read_dir(directory.path().join("backups"))
            .unwrap()
            .count(),
        1
    );
    let mut session = store.session().await.unwrap();
    session
        .audit(&admin, "after_interruption", "handler_cleared")
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
}
