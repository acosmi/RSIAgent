//! The read-only control-plane facts reader the startup recovery gate is built on
//! (E16.5): it reads the facts `scripts/restore_backup.py` requires to be equal
//! between a restored directory and its trusted anchor, refuses unsafe databases,
//! and never writes.

use evo_core::{Context, Role, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{
    ControlPlaneFacts, LifecycleStore, PROTECTED_OBJECT_KINDS, PROTECTED_SCHEMA_VERSIONS,
    ROOT_BUDGET_TABLE_QUERY, TypedObjectRef, parse_strict_json, read_control_plane_facts,
};
use serde_json::json;
use sqlx::{ConnectOptions, Connection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

async fn database() -> (tempfile::TempDir, Store, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    (dir, store, path)
}

/// Raw SQL against a database file the `Store` has released.
async fn exec(path: &Path, sql: &str) {
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    for statement in sql.split(";\n").filter(|s| !s.trim().is_empty()) {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    connection.close().await.unwrap();
}

/// Relative path -> content hash (files), target (symlinks) or "dir".
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                out.insert(
                    relative,
                    format!("symlink:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if meta.is_dir() {
                out.insert(relative, "dir".into());
                walk(root, &path, out);
            } else {
                out.insert(relative, hash(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

async fn put_run(store: &Store, namespace: &str, id: &str) {
    let host = ctx(namespace, "host", Role::Host);
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            id,
            host.actor(),
            &json!({"id":id,"schema_version":"rsia.optimization.source.v1","body":"x"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn bump_watermark(store: &Store, namespace: &str) {
    let admin = ctx(namespace, "admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(format!("initial-{namespace}").as_bytes()))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

const ROOT_BUDGET_ROW: &str = "INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,spent_micros,reserved_micros,created_at) VALUES('scope-a','root-a','n','USD','pv1','subject','0000000000000000000000000000000000000000000000000000000000000000',100,1000,10,0,1)";

#[tokio::test]
async fn facts_cover_watermarks_tombstones_protected_objects_and_root_budget_rows() {
    let (_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    put_run(&store, "n", "run-1").await;
    put_run(&store, "n", "run-2").await;
    LifecycleStore::begin_revoke(
        &ctx("n", "admin", Role::Admin),
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-1".into(),
        },
        "revoked for the facts test",
        7,
    )
    .await
    .unwrap();
    store.close().await;
    exec(
        &path,
        &[
            // protected by kind, protected by schema, and protected by neither
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','reservation','res-1','admin','{\"id\":\"res-1\",\"state\":\"reserved\"}')",
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','ledger-1','admin','{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"ledger-1\",\"payload\":{\"spent\":1}}')",
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','plain-1','admin','{\"schema_version\":\"some.other.v1\",\"id\":\"plain-1\"}')",
            // a namespace without a revoke watermark is out of scope for accounting facts ...
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('unmarked','reservation','res-x','admin','{\"id\":\"res-x\"}')",
            // ... but a tombstone is always a fact
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('unmarked','tombstone','t-x','admin','{\"id\":\"t-x\"}')",
            ROOT_BUDGET_ROW,
            "INSERT INTO root_budget_namespaces(billing_scope,namespace) VALUES('scope-a','n')",
        ]
        .join(";\n"),
    )
    .await;

    let facts = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(facts.watermarks.len(), 1);
    let mark = &facts.watermarks["n"];
    assert_eq!(mark.seq, 2);
    assert_eq!(mark.digest.len(), 64);

    let tombstones: Vec<_> = facts.tombstones.keys().cloned().collect();
    assert_eq!(
        tombstones,
        vec![
            ("n".to_owned(), "run-1".to_owned()),
            ("unmarked".to_owned(), "t-x".to_owned())
        ]
    );
    assert_eq!(
        facts.tombstones[&("n".to_owned(), "run-1".to_owned())]["watermark_seq"],
        2
    );

    let protected: Vec<_> = facts.protected_objects.keys().cloned().collect();
    assert_eq!(
        protected,
        vec![
            ("n".to_owned(), "artifact".to_owned(), "ledger-1".to_owned()),
            ("n".to_owned(), "reservation".to_owned(), "res-1".to_owned()),
        ],
        "only protected kinds/schemas inside watermarked namespaces are facts"
    );
    assert_eq!(
        facts.protected_objects[&("n".to_owned(), "reservation".to_owned(), "res-1".to_owned())]["state"],
        "reserved"
    );

    let tables: Vec<_> = facts
        .root_budget_tables
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        tables,
        vec![
            "root_budget_calls",
            "root_budget_dispatch_groups",
            "root_budget_events",
            "root_budget_namespaces",
            "root_budgets"
        ]
    );
    let budgets = &facts.root_budget_tables["root_budgets"];
    assert_eq!(budgets.len(), 1);
    assert!(
        budgets[0].starts_with("'scope-a','root-a','n','USD','pv1','subject','0000"),
        "rows are comma-joined quote() text: {}",
        budgets[0]
    );
    assert!(
        budgets[0].ends_with(",100,1000,10,0,1,0,NULL,NULL,1"),
        "integers stay unquoted and NULL stays NULL: {}",
        budgets[0]
    );
    assert_eq!(facts.root_budget_tables["root_budget_calls"].len(), 0);
}

#[tokio::test]
async fn equal_states_give_equal_facts_and_any_consumption_changes_them() {
    let (_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    store.close().await;
    exec(&path, &format!("{ROOT_BUDGET_ROW};\nINSERT INTO root_budget_namespaces(billing_scope,namespace) VALUES('scope-a','n')")).await;
    let before = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(before, read_control_plane_facts(&path).await.unwrap());

    exec(
        &path,
        "UPDATE root_budgets SET spent_micros=spent_micros+1 WHERE billing_scope='scope-a'",
    )
    .await;
    let spent = read_control_plane_facts(&path).await.unwrap();
    assert_ne!(before, spent);
    assert_eq!(before.watermarks, spent.watermarks);
    assert_ne!(
        before.root_budget_tables["root_budgets"],
        spent.root_budget_tables["root_budgets"]
    );

    exec(
        &path,
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','consumed','admin','{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"consumed\"}')",
    )
    .await;
    let consumed = read_control_plane_facts(&path).await.unwrap();
    assert_ne!(spent.protected_objects, consumed.protected_objects);
}

#[tokio::test]
async fn reading_never_writes_and_never_creates_a_file() {
    // A cleanly closed store: WAL-mode header, no -wal or -shm left behind.
    let (dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    put_run(&store, "n", "run-1").await;
    store.close().await;
    let before = snapshot(dir.path());
    assert_eq!(
        before.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["rsia.sqlite3"]
    );
    let facts = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(facts.watermarks.len(), 1);
    assert_eq!(snapshot(dir.path()), before, "WAL-header database");

    // The restore shape: a rollback-journal copy made by `VACUUM INTO`.
    let copy_dir = tempfile::tempdir().unwrap();
    let copy = copy_dir.path().join("rsia.sqlite3");
    exec(
        &path,
        &format!(
            "VACUUM INTO '{}'",
            copy.display().to_string().replace('\'', "''")
        ),
    )
    .await;
    let before = snapshot(copy_dir.path());
    assert_eq!(read_control_plane_facts(&copy).await.unwrap(), facts);
    assert_eq!(snapshot(copy_dir.path()), before, "rollback-journal copy");

    // A missing file is an error and is not created.
    let missing = copy_dir.path().join("missing.sqlite3");
    assert!(read_control_plane_facts(&missing).await.is_err());
    assert_eq!(snapshot(copy_dir.path()), before);
    assert!(!missing.exists());
}

#[tokio::test]
async fn a_wal_left_behind_by_a_writer_is_honoured() {
    // While the store is open its committed state lives in the WAL: the main file
    // has not been checkpointed. A crashed writer leaves exactly this behind.
    let (dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    put_run(&store, "n", "run-1").await;

    let with_wal = tempfile::tempdir().unwrap();
    std::fs::copy(&path, with_wal.path().join("rsia.sqlite3")).unwrap();
    std::fs::copy(
        dir.path().join("rsia.sqlite3-wal"),
        with_wal.path().join("rsia.sqlite3-wal"),
    )
    .unwrap();
    let facts = read_control_plane_facts(&with_wal.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(facts.watermarks["n"].seq, 1, "the WAL content was read");

    // The same main file without its WAL has nothing to read: the WAL is what
    // made the read above succeed, not the main file.
    let without_wal = tempfile::tempdir().unwrap();
    std::fs::copy(&path, without_wal.path().join("rsia.sqlite3")).unwrap();
    assert!(
        read_control_plane_facts(&without_wal.path().join("rsia.sqlite3"))
            .await
            .is_err()
    );
    store.close().await;
}

#[tokio::test]
async fn a_live_store_can_be_read_while_it_is_open() {
    let (_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    put_run(&store, "n", "run-1").await;
    let facts = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(facts.watermarks["n"].seq, 1);
    store.close().await;
}

#[tokio::test]
async fn a_database_that_is_not_the_compiled_migration_set_is_refused() {
    let (_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    store.close().await;
    assert!(read_control_plane_facts(&path).await.is_ok());

    exec(
        &path,
        "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1",
    )
    .await;
    let error = read_control_plane_facts(&path)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("migration set is not the compiled-in set"),
        "{error}"
    );

    exec(&path, "DELETE FROM _sqlx_migrations WHERE version=6").await;
    assert!(read_control_plane_facts(&path).await.is_err());
}

#[tokio::test]
async fn a_corrupt_or_foreign_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();

    // not a database at all
    let garbage = dir.path().join("garbage.sqlite3");
    std::fs::write(&garbage, vec![0xAB; 8192]).unwrap();
    assert!(read_control_plane_facts(&garbage).await.is_err());

    // a valid SQLite file that is not an RSIA database
    let foreign = dir.path().join("foreign.sqlite3");
    let mut options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&foreign)
        .create_if_missing(true);
    options = options.disable_statement_logging();
    let mut connection = options.connect().await.unwrap();
    sqlx::query("CREATE TABLE t(x)")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(read_control_plane_facts(&foreign).await.is_err());

    // a real database whose own integrity check fails: the root page of a b-tree
    // is damaged, which `PRAGMA integrity_check` (the first step, before the
    // migration set or any fact is read) must refuse. A CHECK violation cannot
    // serve here: SQLite skips CHECK verification on read-only connections.
    let (_store_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    store.close().await;
    assert!(read_control_plane_facts(&path).await.is_ok());
    let (root_page, page_size) = {
        let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .disable_statement_logging()
            .connect()
            .await
            .unwrap();
        let root: i64 =
            sqlx::query_scalar("SELECT rootpage FROM sqlite_master WHERE name='objects_owner'")
                .fetch_one(&mut connection)
                .await
                .unwrap();
        let size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        (root, size)
    };
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = usize::try_from((root_page - 1) * page_size).unwrap();
    bytes[offset] = 0x00; // not a valid b-tree page type
    std::fs::write(&path, bytes).unwrap();
    let error = read_control_plane_facts(&path)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("integrity check"), "{error}");

    // a real database whose schema page is destroyed
    let (_store_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    store.close().await;
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[100..140].fill(0xFF);
    std::fs::write(&path, bytes).unwrap();
    assert!(read_control_plane_facts(&path).await.is_err());
}

#[tokio::test]
async fn unreadable_object_bodies_are_refused_like_the_restore_script() {
    let (_dir, store, path) = database().await;
    bump_watermark(&store, "n").await;
    store.close().await;
    // a duplicate key, invisible to a plain serde_json::Value parse
    exec(
        &path,
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','run','dup','admin','{\"id\":\"dup\",\"id\":\"dup\"}')",
    )
    .await;
    let error = read_control_plane_facts(&path)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate JSON key: id"), "{error}");
    exec(&path, "DELETE FROM objects WHERE id='dup'").await;

    // valid JSON that is not an object passes the table CHECK (json_extract is NULL)
    exec(
        &path,
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','run','list','admin','[1]')",
    )
    .await;
    let error = read_control_plane_facts(&path)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a JSON object"), "{error}");
}

#[test]
fn strict_json_rejects_duplicate_keys_at_any_depth_and_trailing_content() {
    assert!(parse_strict_json(r#"{"a":[{"b":1},{"b":2}],"c":{"d":null}}"#).is_ok());
    for bad in [
        r#"{"a":1,"a":2}"#,
        r#"{"a":{"b":1,"b":2}}"#,
        r#"[{"a":1,"a":1}]"#,
        r#"{"a":1} {"b":2}"#,
        r#"{"a":1}x"#,
        "",
        "{",
        "NaN",
    ] {
        assert!(parse_strict_json(bad).is_err(), "{bad}");
    }
    let value = parse_strict_json(" {\"a\": [1, 2.5, \"x\", true, null]} \n").unwrap();
    assert_eq!(value, json!({"a":[1, 2.5, "x", true, null]}));
}

#[test]
fn the_exported_selectors_are_the_restore_scripts() {
    // Pinned here; the engine's startup_gate_v42 test asserts these against the
    // script source itself.
    assert_eq!(PROTECTED_OBJECT_KINDS.len(), 4);
    assert_eq!(PROTECTED_SCHEMA_VERSIONS.len(), 9);
    // E16 budget refs preserve the original source snapshots through recovery.
    assert!(PROTECTED_SCHEMA_VERSIONS.contains(&"rsia.budget_call_e16_ref.v1"));
    assert!(ROOT_BUDGET_TABLE_QUERY.contains("LIKE 'root_budget%'"));
    let facts: Option<ControlPlaneFacts> = None;
    assert!(facts.is_none());
}
