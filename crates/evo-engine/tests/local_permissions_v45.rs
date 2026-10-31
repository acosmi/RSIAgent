//! Local filesystem fixtures; no execution, formal authority or multi-UID claim.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use evo_core::{Context, Error, Role, hash};
use evo_engine::data_directory_lock::{DataDirectoryLock, mount_gate};
use evo_engine::startup_gate::{
    AdmissionRecord, DATA_LOCK_FILE, RECEIPT_SCOPE, RESTORE_ADMISSION_FILE,
    RESTORE_ADMISSION_SCHEMA, RESTORE_RECEIPT_FILE, StartupGate, StartupGateError,
};
use evo_storage::Store;
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    mode: u32,
    uid: u32,
    links: u64,
    device: u64,
    inode: u64,
    target: Option<PathBuf>,
    bytes: Vec<u8>,
}

fn image(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Entry>) {
        let info = fs::symlink_metadata(path).unwrap();
        let link = info.file_type().is_symlink();
        result.insert(path.strip_prefix(root).unwrap().to_path_buf(), Entry {
            mode: info.mode(), uid: info.uid(), links: info.nlink(),
            device: info.dev(), inode: info.ino(),
            target: if link { Some(fs::read_link(path).unwrap()) } else { None },
            bytes: if info.is_file() && !link { fs::read(path).unwrap() } else { Vec::new() },
        });
        if info.is_dir() && !link {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn private_directory(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn private_member(path: &Path, bytes: &[u8]) {
    OpenOptions::new().create_new(true).write(true).mode(0o600)
        .open(path).unwrap().write_all(bytes).unwrap();
}

async fn closed_database(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(name);
    private_directory(&directory);
    let database = directory.join("db.sqlite3");
    let (guard, gate) = mount_gate(&database, None).await.unwrap();
    let store = Store::open(&database).await.unwrap();
    gate.admit().unwrap();
    let admin = Context::new("permissions", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session.put(&admin, "run", "software-fixture", admin.actor(),
        &serde_json::json!({"id":"software-fixture","schema_version":"fixture.source.v1","body":"retained bytes"}))
        .await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    store.close().await;
    drop(guard);
    database
}

async fn permission_refusal(database: &Path, protected: &Path) {
    let before = image(protected);
    let error = mount_gate(database, None).await.unwrap_err();
    let StartupGateError::Rejected(reason) = &error else {
        panic!("expected exact permission rejection, got {error}");
    };
    assert_eq!(reason, "data_directory_insecure");
    assert_eq!(error.to_string(), "startup_rejected: data_directory_insecure");
    let after = image(protected);
    assert_eq!(after.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>());
    assert_eq!(after, before, "permission refusal changed names, full bytes or metadata");
}

// Only metadata is exercised here. The separate admission test uses an actual
// Store backup and the real restore script before checking the receipt binding.
fn previously_admitted_members(directory: &Path) {
    let receipt = br#"{"schema_version":"rsia.restore_receipt.v2","receipt_scope":"local_only_not_for_export"}"#;
    private_member(&directory.join(RESTORE_RECEIPT_FILE), receipt);
    let admission = AdmissionRecord {
        schema_version: RESTORE_ADMISSION_SCHEMA.into(),
        receipt_sha256: hash(receipt),
        anchor_path_local_only: "/operator/current.sqlite3".into(),
        verified_facts_digest: hash(b"metadata-only previously admitted fixture"),
        namespaces: vec!["permissions".into()],
        admitted_at: 1,
        scope: RECEIPT_SCOPE.into(),
    };
    private_member(&directory.join(RESTORE_ADMISSION_FILE), &serde_json::to_vec(&admission).unwrap());
}

#[tokio::test]
async fn insecure_data_directory_refuses_before_lock_or_store() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("data");
    private_directory(&directory);
    private_member(&directory.join("canary"), b"unchanged");
    fs::set_permissions(&directory, Permissions::from_mode(0o755)).unwrap();
    let database = directory.join("db.sqlite3");
    permission_refusal(&database, root.path()).await;
    assert!(!database.exists());
    assert!(!directory.join(DATA_LOCK_FILE).exists());
}

#[tokio::test]
async fn insecure_managed_members_leave_the_full_snapshot_unchanged() {
    let root = tempfile::tempdir().unwrap();
    for (index, (member, directory_member)) in [
        ("db.sqlite3", false), (DATA_LOCK_FILE, false),
        (RESTORE_RECEIPT_FILE, false), (RESTORE_ADMISSION_FILE, false),
        ("restore.migrations.json", false), ("blobs", true),
        (".blob-staging", true), (".export-staging", true), ("exports", true),
    ].into_iter().enumerate() {
        let database = closed_database(root.path(), &format!("member-{index}")).await;
        let directory = database.parent().unwrap();
        if matches!(member, RESTORE_RECEIPT_FILE | RESTORE_ADMISSION_FILE) {
            previously_admitted_members(directory);
        }
        let path = directory.join(member);
        if !path.exists() {
            if directory_member { private_directory(&path); }
            else { private_member(&path, b"{}"); }
        }
        StartupGate::new(&database, None).evaluate().await.unwrap();
        fs::set_permissions(&path, Permissions::from_mode(if directory_member { 0o755 } else { 0o644 })).unwrap();
        permission_refusal(&database, root.path()).await;
    }
    // These are actual SQLite sidecars from a live migrated connection, never
    // fabricated WAL/SHM stubs. No SQL runs between the before/after images.
    for suffix in ["-wal", "-shm"] {
        let directory = root.path().join(format!("live{suffix}"));
        private_directory(&directory);
        let database = directory.join("db.sqlite3");
        let (guard, gate) = mount_gate(&database, None).await.unwrap();
        let store = Store::open(&database).await.unwrap();
        gate.admit().unwrap();
        let sidecar = directory.join(format!("db.sqlite3{suffix}"));
        assert!(fs::metadata(&sidecar).unwrap().len() > 0);
        fs::set_permissions(&sidecar, Permissions::from_mode(0o644)).unwrap();
        permission_refusal(&database, root.path()).await;
        fs::set_permissions(&sidecar, Permissions::from_mode(0o600)).unwrap();
        store.close().await;
        drop(guard);
    }
    for (index, member) in ["db.sqlite3", DATA_LOCK_FILE, "blobs", ".blob-staging", ".export-staging", "exports"].into_iter().enumerate() {
        let database = closed_database(root.path(), &format!("type-{index}")).await;
        let path = database.parent().unwrap().join(member);
        if path.exists() { fs::remove_file(&path).unwrap(); }
        if matches!(member, "db.sqlite3" | DATA_LOCK_FILE) { private_directory(&path); }
        else { private_member(&path, b"directory position holds a regular file"); }
        permission_refusal(&database, root.path()).await;
    }
}

#[tokio::test]
async fn group_writable_ancestor_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    private_directory(&parent);
    let database = closed_database(&parent, "data").await;
    fs::set_permissions(&parent, Permissions::from_mode(0o770)).unwrap();
    permission_refusal(&database, root.path()).await;
}

#[tokio::test]
async fn data_directory_symlink_and_dot_alias_do_not_hide_the_link() {
    let root = tempfile::tempdir().unwrap();
    let database = closed_database(root.path(), "real").await;
    let link = root.path().join("linked");
    symlink(database.parent().unwrap(), &link).unwrap();
    for path in [link.join("db.sqlite3"), link.join(".").join("db.sqlite3")] {
        permission_refusal(&path, root.path()).await;
    }
}

#[tokio::test]
async fn linked_managed_member_is_refused_before_staging_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let database = closed_database(root.path(), "data").await;
    let staging = database.parent().unwrap().join(".blob-staging");
    private_directory(&staging);
    let namespace = staging.join("permissions");
    private_directory(&namespace);
    let actual = root.path().join("unchanged-payload");
    private_member(&actual, b"the link must not be followed or removed");
    symlink(&actual, namespace.join("payload.tmp")).unwrap();
    permission_refusal(&database, root.path()).await;
    let before = image(root.path());
    match Store::open(&database).await {
        Err(Error::Invalid(reason)) => assert_eq!(reason, "data_directory_insecure"),
        Err(error) => panic!("unexpected Store refusal: {error}"),
        Ok(store) => { store.close().await; panic!("Store opened an insecure staging member"); }
    }
    assert_eq!(image(root.path()), before);
}

#[tokio::test]
async fn hard_linked_database_is_refused_before_lock() {
    let root = tempfile::tempdir().unwrap();
    let database = closed_database(root.path(), "data").await;
    let alias = root.path().join("database-alias");
    fs::hard_link(&database, &alias).unwrap();
    let original = fs::metadata(&database).unwrap();
    let linked = fs::metadata(&alias).unwrap();
    assert_eq!((original.dev(), original.ino()), (linked.dev(), linked.ino()));
    assert_eq!(original.nlink(), 2);
    permission_refusal(&database, root.path()).await;
}

#[tokio::test]
async fn hard_linked_lock_is_refused_without_repair() {
    let root = tempfile::tempdir().unwrap();
    let database = closed_database(root.path(), "data").await;
    let lock = database.parent().unwrap().join(DATA_LOCK_FILE);
    OpenOptions::new().write(true).open(&lock).unwrap().write_all(b"original lock bytes").unwrap();
    let alias = root.path().join("lock-alias");
    fs::hard_link(&lock, &alias).unwrap();
    assert_eq!(fs::metadata(&lock).unwrap().nlink(), 2);
    permission_refusal(&database, root.path()).await;
}

#[tokio::test]
async fn permission_gate_is_rechecked_while_os_lock_is_held() {
    let root = tempfile::tempdir().unwrap();
    let database = closed_database(root.path(), "data").await;
    let directory = database.parent().unwrap();
    StartupGate::new(&database, None).evaluate().await.unwrap();
    let guard = DataDirectoryLock::acquire(&database).unwrap();
    fs::set_permissions(directory, Permissions::from_mode(0o755)).unwrap();
    let before = image(root.path());
    let error = StartupGate::new(&database, None).evaluate().await.unwrap_err();
    assert_eq!(error, StartupGateError::Rejected("data_directory_insecure".into()));
    assert!(directory.join(DATA_LOCK_FILE).is_file(), "the legal lock predates this rejection");
    let other = OpenOptions::new().read(true).write(true).open(directory.join(DATA_LOCK_FILE)).unwrap();
    assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    assert_eq!(image(root.path()), before);
    drop(other);
    drop(guard);
    fs::set_permissions(directory, Permissions::from_mode(0o700)).unwrap();
    let (guard, decision) = mount_gate(&database, None).await.unwrap();
    let store = Store::open(&database).await.unwrap();
    decision.admit().unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    store.close().await;
    drop(guard);
}

#[tokio::test]
async fn only_the_absent_final_directory_is_created_0700() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("new-final");
    let database = directory.join("db.sqlite3");
    let (guard, gate) = mount_gate(&database, None).await.unwrap();
    assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(directory.join(DATA_LOCK_FILE)).unwrap().mode() & 0o777, 0o600);
    let store = Store::open(&database).await.unwrap();
    gate.admit().unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    assert_eq!(fs::metadata(&database).unwrap().mode() & 0o777, 0o600);
    assert_eq!(DataDirectoryLock::acquire(&database).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    store.close().await;
    drop(guard);
    DataDirectoryLock::acquire(&database).unwrap();
}

#[tokio::test]
async fn missing_ancestors_are_not_recursively_created() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("absent-parent/final/db.sqlite3");
    let before = image(root.path());
    assert_eq!(DataDirectoryLock::acquire(&database).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert!(mount_gate(&database, None).await.is_err());
    assert_eq!(image(root.path()), before);
    assert!(!root.path().join("absent-parent").exists());
}
