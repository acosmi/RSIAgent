//! The read-only control-plane facts reader the startup recovery gate is built on
//! (E16.5): it reads the facts `scripts/restore_backup.py` requires to be equal
//! between a restored directory and its trusted anchor, refuses unsafe databases,
//! and never writes.

use evo_core::{Context, Role, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{
    ControlPlaneFacts, LifecycleStore, PROTECTED_ACTION_SCHEMA_VERSIONS, PROTECTED_OBJECT_KINDS,
    PROTECTED_SCHEMA_VERSIONS, ROOT_BUDGET_TABLE_QUERY, TypedObjectRef, parse_strict_json,
    read_control_plane_facts,
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
            "root_budget_stage_plan_bindings",
            "root_budget_stage_plan_calls",
            "root_budget_stage_plan_controls",
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
    assert_eq!(PROTECTED_SCHEMA_VERSIONS.len(), 11);
    assert_eq!(PROTECTED_SCHEMA_VERSIONS[9], "rsia.trace_export.v1");
    assert_eq!(
        PROTECTED_SCHEMA_VERSIONS[10],
        "rsia.optimization.stage_fact.v2"
    );
    assert_eq!(PROTECTED_ACTION_SCHEMA_VERSIONS.len(), 14);
    assert_eq!(
        PROTECTED_ACTION_SCHEMA_VERSIONS[11],
        "rsia.monitoring.consolidation_proposal.v2"
    );
    assert_eq!(
        PROTECTED_ACTION_SCHEMA_VERSIONS[12],
        "rsia.management_job.v2"
    );
    // E16 budget refs preserve the original source snapshots through recovery.
    assert!(PROTECTED_SCHEMA_VERSIONS.contains(&"rsia.budget_call_e16_ref.v1"));
    assert!(ROOT_BUDGET_TABLE_QUERY.contains("LIKE 'root_budget%'"));
    let facts: Option<ControlPlaneFacts> = None;
    assert!(facts.is_none());
}

#[tokio::test]
async fn current_and_legacy_consolidation_bodies_remain_protected_and_cleanable() {
    // These deliberately minimal claimed action records exercise conservative
    // metadata protection and cleanup. They are not valid compiler/Current
    // producer fixtures and confer no proposal or model authority.
    let (_dir, store, path) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    put_run(&store, "n", "proposal-source").await;
    let legacy = json!({
        "schema_version": "rsia.monitoring.consolidation_proposal.v1",
        "id": "proposal-legacy", "payload": "source-derived content"
    });
    let current = json!({
        "schema_version": "rsia.monitoring.consolidation_proposal.v2",
        "id": "proposal-current", "payload": "source-derived content"
    });
    let mut session = store.session().await.unwrap();
    for value in [&legacy, &current] {
        let id = value["id"].as_str().unwrap();
        session
            .put(&admin, "artifact", id, admin.actor(), value)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", "proposal-source")
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "proposal-source".into(),
        },
        "remove source-derived proposal content",
        evo_core::now().unwrap(),
    )
    .await
    .unwrap();
    store.close().await;

    let facts = read_control_plane_facts(&path).await.unwrap();
    let legacy_key = (
        "n".to_owned(),
        "artifact".to_owned(),
        "proposal-legacy".to_owned(),
    );
    let current_key = (
        "n".to_owned(),
        "artifact".to_owned(),
        "proposal-current".to_owned(),
    );
    assert_eq!(facts.protected_objects[&legacy_key], legacy);
    assert_eq!(facts.protected_objects[&current_key], current);

    let changed = json!({
        "schema_version": "rsia.monitoring.consolidation_proposal.v2",
        "id": "proposal-current", "payload": "changed source-derived content"
    });
    // Only this test's released temporary database is modified; the reader
    // must distinguish every field of the whole action record.
    exec(&path, &format!(
        "UPDATE objects SET body='{}' WHERE namespace='n' AND kind='artifact' AND id='proposal-current'",
        serde_json::to_string(&changed).unwrap()
    )).await;
    let after = read_control_plane_facts(&path).await.unwrap();
    assert_ne!(facts, after);
    assert_eq!(after.protected_objects[&legacy_key], legacy);
    assert_eq!(after.protected_objects[&current_key], changed);

    let reopened = Store::open(&path).await.unwrap();
    let mut result = status;
    for _ in 0..20 {
        if result.state == evo_storage::lifecycle::CleanupState::Complete {
            break;
        }
        result = LifecycleStore::cleanup_step(
            &admin,
            &reopened,
            &result.job_id,
            16,
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
    }
    assert_eq!(result.state, evo_storage::lifecycle::CleanupState::Complete);
    let mut session = reopened.session().await.unwrap();
    for id in ["proposal-legacy", "proposal-current"] {
        let value: serde_json::Value = session.need(&admin, "artifact", id).await.unwrap();
        assert_eq!(value["schema_version"], "rsia.redacted.v1");
        assert_eq!(
            value["original_schema"],
            if id == "proposal-legacy" {
                "rsia.monitoring.consolidation_proposal.v1"
            } else {
                "rsia.monitoring.consolidation_proposal.v2"
            }
        );
        assert!(
            !serde_json::to_string(&value)
                .unwrap()
                .contains("source-derived content")
        );
    }
    session.commit().await.unwrap();
    reopened.close().await;
}

// Read actual stored bytes only after this test's Store has released the file.
async fn compiler_cleanup_bodies(path: &Path) -> BTreeMap<String, String> {
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,body FROM objects WHERE namespace='n' AND kind='artifact' ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    rows.into_iter().collect()
}

#[tokio::test]
async fn known_compiler_content_is_redacted_while_binding_metadata_and_facts_survive() {
    // These are claimed-schema cleanup data, not valid Current producer,
    // independent OS principal, dispatch, grading or paid-execution proofs.
    let (_dir, store, path) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    put_run(&store, "n", "compiler-source").await;
    let sentinel = "revoked original compiler source content";
    let bundle = json!({"skill":{"content":sentinel},"improver":{"instruction":sentinel}});
    let input = json!({
        "search_parent":{"content":sentinel}, "baseline":{"content":sentinel},
        "search_parent_strategy":{"instruction":sentinel},
        "baseline_strategy":{"instruction":sentinel}
    });
    let content = [
        json!({"id":"candidate-v1","schema_version":"rsia.release_candidate.v1","bundle":bundle.clone()}),
        json!({"id":"candidate-v2","schema_version":"rsia.release_candidate.v2","bundle":bundle.clone()}),
        json!({"id":"compiler-use-v1","schema_version":"rsia.optimization.compiler_use.v1","request":input.clone()}),
        json!({"id":"compiler-use-v2","schema_version":"rsia.optimization.compiler_use.v2","request":input.clone()}),
        json!({"id":"authenticated-v1","schema_version":"rsia.authenticated_bundle.v1","bundle":bundle.clone(),"input":input.clone()}),
        json!({"id":"authenticated-v2","schema_version":"rsia.authenticated_bundle.v2","bundle":bundle.clone(),"input":input,
            "original_baseline":{"baseline_skill":{"content":sentinel}},
            "original_parent":{"skill":{"content":sentinel},"strategy":{"instruction":sentinel}}}),
        // The original standalone v2 classifier remains supported unchanged.
        json!({"id":"standalone-v2","schema_version":"rsia.resolved_bundle.v2","skill":{"content":sentinel}}),
    ];
    let digest = hash(b"x"); // The existing put_run fixture's actual body bytes.
    let mut metadata = Vec::new();
    let mut session = store.session().await.unwrap();
    for value in &content {
        let id = value["id"].as_str().unwrap();
        session
            .put(&admin, "artifact", id, admin.actor(), value)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", "compiler-source")
            .await
            .unwrap();
    }
    for (id, schema, binding_schema, candidate_id) in [
        (
            "binding-v1",
            "rsia.persisted_compiler_binding.v1",
            "rsia.formal_compiler_input_binding.v2",
            "candidate-v1",
        ),
        (
            "binding-v2",
            "rsia.persisted_compiler_binding.v2",
            "rsia.formal_compiler_input_binding.v3",
            "candidate-v2",
        ),
    ] {
        let value = json!({
            "id":id,"schema_version":schema,"candidate_id":candidate_id,
            "binding":{
                "schema_version":binding_schema,"namespace":"n","profile_registration_id":"compiler-profile",
                "profile_digest":digest,"parent_digest":digest,"baseline_digest":digest,
                "parent_skill_digest":digest,"parent_strategy_digest":digest,
                "search_parent_skill_digest":digest,"search_parent_strategy_digest":digest,
                "baseline_skill_digest":digest,"baseline_strategy_digest":digest,
                "environment_digest":digest,"capabilities_digest":digest,"purpose":"development",
                "application_order":["mandatory_context","skill","task"],"watermark":[0,digest],
                "live_profile_pointer_digest":null,
                "closure":{"nodes":[{"kind":"run","id":"compiler-source",
                    "schema_version":"rsia.optimization.source.v1","content_digest":digest}],
                    "edges":[],"digest":digest},"binding_digest":digest
            }
        });
        session
            .put(&admin, "artifact", id, admin.actor(), &value)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", "compiler-source")
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", candidate_id, "artifact", id)
            .await
            .unwrap();
        metadata.push(value);
    }
    session.commit().await.unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "compiler-source".into(),
        },
        "remove compiler-derived source content",
        evo_core::now().unwrap(),
    )
    .await
    .unwrap();
    store.close().await;
    let before = compiler_cleanup_bodies(&path).await;
    let facts = read_control_plane_facts(&path).await.unwrap();
    assert_eq!(facts.watermarks.len(), 1);
    assert_eq!(facts.tombstones.len(), 1);
    for value in &content {
        assert!(before[value["id"].as_str().unwrap()].contains(sentinel));
    }

    let reopened = Store::open(&path).await.unwrap();
    for _ in 0..20 {
        if matches!(
            status.state,
            evo_storage::lifecycle::CleanupState::Complete
                | evo_storage::lifecycle::CleanupState::Failed
        ) {
            break;
        }
        status = LifecycleStore::cleanup_step(
            &admin,
            &reopened,
            &status.job_id,
            16,
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        status.state,
        evo_storage::lifecycle::CleanupState::Complete,
        "{:?}",
        status.last_error
    );
    reopened.close().await;
    let after = compiler_cleanup_bodies(&path).await;
    for value in &content {
        let id = value["id"].as_str().unwrap();
        let redacted: serde_json::Value = serde_json::from_str(&after[id]).unwrap();
        assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
        assert_eq!(redacted["original_schema"], value["schema_version"]);
        assert_eq!(redacted["original_digest"], hash(before[id].as_bytes()));
        assert!(!after[id].contains(sentinel));
    }
    for value in &metadata {
        let id = value["id"].as_str().unwrap();
        assert_eq!(after[id], before[id]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&after[id]).unwrap(),
            *value
        );
    }
    assert_eq!(read_control_plane_facts(&path).await.unwrap(), facts);
}

#[tokio::test]
async fn future_compiler_record_versions_fail_closed_without_rewriting_their_bodies() {
    // Every unknown version gets an independent real Store and source closure;
    // claiming a familiar schema family grants neither cleanup nor authority.
    for schema in [
        "rsia.release_candidate.v3",
        "rsia.optimization.compiler_use.v3",
        "rsia.authenticated_bundle.v4",
        "rsia.persisted_compiler_binding.v3",
        // No standalone v3 writer was registered by this implementation.
        "rsia.resolved_bundle.v3",
    ] {
        let (_dir, store, path) = database().await;
        let admin = ctx("n", "admin", Role::Admin);
        put_run(&store, "n", "future-source").await;
        let claimed = json!({"id":"future-record","schema_version":schema,
            "payload":"unknown-version source content must not be silently handled"});
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "artifact", "future-record", admin.actor(), &claimed)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", "future-record", "run", "future-source")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut status = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: "future-source".into(),
            },
            "unknown compiler record cleanup boundary",
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
        store.close().await;
        let before = compiler_cleanup_bodies(&path).await;
        let facts = read_control_plane_facts(&path).await.unwrap();
        let reopened = Store::open(&path).await.unwrap();
        for _ in 0..20 {
            if matches!(
                status.state,
                evo_storage::lifecycle::CleanupState::Complete
                    | evo_storage::lifecycle::CleanupState::Failed
            ) {
                break;
            }
            status = LifecycleStore::cleanup_step(
                &admin,
                &reopened,
                &status.job_id,
                16,
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            status.state,
            evo_storage::lifecycle::CleanupState::Failed,
            "{schema}"
        );
        assert_eq!(
            status.last_error.as_deref(),
            Some(format!("blocked_unknown_scope:artifact:{schema}:").as_str())
        );
        reopened.close().await;
        let after = compiler_cleanup_bodies(&path).await;
        assert_eq!(after["future-record"], before["future-record"], "{schema}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&after["future-record"]).unwrap(),
            claimed
        );
        assert_eq!(read_control_plane_facts(&path).await.unwrap(), facts);
    }
}

#[tokio::test]
async fn current_host_restore_precision_fault_is_zero_write() {
    use evo_core::contract::{
        HostSurfaceClassificationV2, HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1,
        SurfaceCoverage, host_surface_upgrade_diff_v2, host_surface_v2_from_source,
    };
    let (dir, store, path) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let source=ReviewedHostSourceEnvelopeV1 {
        schema_version:"rsia.host_source_envelope.v1".into(),source_kind:"reviewed_fixture".into(),
        source_schema:"rsia.host_reviewed_declarations.v1".into(),
        extractor_version:"rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8:r#"{"schema_version":"rsia.host_reviewed_declarations.v1","host":"reference","host_version":"1","declarations":[{"name":"option","value_type":"object","enum_values":[],"default":{"kind":"value","value":{"decimal":0.1}}}]}"#.into(),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "1".into(),
        retired: vec![],
        classifications: vec![HostSurfaceClassificationV2 {
            name: "option".into(),
            coverage: SurfaceCoverage::Unsupported,
            mapped_field: None,
            consumer: None,
            reason: "restore raw data control".into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        }],
    };
    let (manifest, typed) = host_surface_v2_from_source(&source, &registration).unwrap();
    let body = json!({"id":"host-current","schema_version":"rsia.host_surface_record.v2",
        "manifest_digest":evo_core::fingerprint(&manifest).unwrap(),
        "upgrade_diff":host_surface_upgrade_diff_v2(&manifest,None).unwrap(),
        "manifest":manifest,"source":source,"extracted":["option"],"extracted_typed":typed,"predecessor_surface_id":null});
    let raw = serde_json::to_string(&body).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", "host-current", admin.actor(), &body)
        .await
        .unwrap();
    session
        .bump_watermark(&admin, &hash(b"raw restore watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    read_control_plane_facts(&path).await.unwrap();
    let mut faults = vec![(
        "outer",
        raw.replacen("\"decimal\":0.1", "\"decimal\":0.100000000000000001", 1),
    )];
    for (name, original) in [
        (
            "source-precision",
            source.original_utf8.replace("0.1", "0.100000000000000001"),
        ),
        (
            "source-duplicate",
            source
                .original_utf8
                .replace("\"decimal\":0.1", "\"decimal\":0.1,\"\\u0064ecimal\":0.1"),
        ),
        (
            "source-mismatch",
            source.original_utf8.replace("0.1", "0.2"),
        ),
        (
            "source-resource",
            source
                .original_utf8
                .replace("0.1", &("[".repeat(257) + "0" + &"]".repeat(257))),
        ),
    ] {
        let mut changed = body.clone();
        changed["source"]["original_utf8"] = json!(original);
        faults.push((name, serde_json::to_string(&changed).unwrap()));
    }
    for (name, damaged) in faults {
        assert_ne!(damaged, raw, "{name}");
        let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false)
            .disable_statement_logging()
            .connect()
            .await
            .unwrap();
        sqlx::query("UPDATE objects SET body=? WHERE namespace='n' AND kind='artifact' AND id='host-current'")
            .bind(&damaged).execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
        let before = snapshot(dir.path());
        let expected = if name == "source-resource" {
            "object n/artifact/host-current has an unreadable body: reviewed host JSON resource limit: depth"
        } else {
            "object n/artifact/host-current has an unreadable body: invalid reviewed host source/default"
        };
        assert!(
            matches!(read_control_plane_facts(&path).await,
            Err(evo_core::Error::Invalid(message)) if message == expected),
            "{name}"
        );
        assert_eq!(snapshot(dir.path()), before, "{name}");
        let after = compiler_cleanup_bodies(&path).await;
        assert_eq!(after["host-current"], damaged, "{name}");
    }
}

#[test]
fn current_version_table_classifies_legacy_invalid_unknown_and_lineage() {
    use evo_storage::{
        PERSISTENT_CONTRACT_VERSIONS, PersistentContractState, persistent_contract_state,
    };
    let mut schemas = std::collections::BTreeSet::new();
    for version in PERSISTENT_CONTRACT_VERSIONS {
        assert!(
            schemas.insert(version.schema),
            "duplicate version descriptor"
        );
        assert!(!version.introduced_change.is_empty());
        assert!(version.verifier_retained);
        assert_eq!(
            persistent_contract_state(version.schema, true).unwrap(),
            if version.current {
                PersistentContractState::CurrentVerified
            } else {
                PersistentContractState::Legacy
            }
        );
        assert_eq!(
            persistent_contract_state(version.schema, false).unwrap(),
            PersistentContractState::Invalid
        );
        // The version is part of the content identity. These software IDs are
        // data controls; a consumer still reloads each actual producer source.
        let input = hash(b"same original input bytes");
        let identity = evo_core::fingerprint(&(version.schema, &input)).unwrap();
        assert_eq!(
            identity,
            evo_core::fingerprint(&(version.schema, &input)).unwrap()
        );
        assert_ne!(
            identity,
            evo_core::fingerprint(&("unknown-version", &input)).unwrap()
        );
    }
    for schema in [
        "rsia.experiment_plan.v1",
        "rsia.experiment_plan.v2",
        "rsia.evaluation_ticket.v2",
        "rsia.evaluation_ticket.v3",
    ] {
        assert_eq!(
            persistent_contract_state(schema, true).unwrap(),
            PersistentContractState::Legacy
        );
        assert_eq!(
            persistent_contract_state(schema, false).unwrap(),
            PersistentContractState::Invalid
        );
    }
    for schema in ["rsia.experiment_plan.v3", "rsia.evaluation_ticket.v4"] {
        assert_eq!(
            persistent_contract_state(schema, true).unwrap(),
            PersistentContractState::CurrentVerified
        );
        assert_eq!(
            persistent_contract_state(schema, false).unwrap(),
            PersistentContractState::Invalid
        );
    }
    assert_eq!(
        persistent_contract_state("rsia.registered_external_provider.v2", true).unwrap(),
        PersistentContractState::Legacy
    );
    assert!(
        matches!(persistent_contract_state("rsia.release_candidate.v99",true),
        Err(evo_core::Error::Invalid(message)) if message=="unknown persistent contract version; isolate")
    );
}

// AG163 append-only prepared tests. The software fixture proves no provider,
// paid-effect, independent-process or promotion authority.
#[cfg(unix)]
struct Ag163History {
    store: Store,
    path: std::path::PathBuf,
    original: serde_json::Value,
    _dir: tempfile::TempDir,
}

#[cfg(unix)]
async fn ag163_history(name: &str) -> Ag163History {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let capture = include_bytes!("../../evo-engine/tests/fixtures/ag163_formal_v2_history.json");
    assert_eq!(
        evo_core::hash(capture),
        "2bc4ecd85ad308cd888067c797391473f43814a0a39988b6035de93dc84a2e79"
    );
    let all: serde_json::Value = serde_json::from_slice(capture).unwrap();
    assert_eq!(
        all["producer_commit"],
        "800eafc4ccaf39d5e4f3fb1b7891f9c142e6b482"
    );
    assert_eq!(all["software_fixture_only"], true);
    assert_eq!(all["real_provider_or_paid_or_OS_authority_proof"], false);
    let original = all["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
        .clone();
    let raw = original["database_hex"].as_str().unwrap();
    assert_eq!(raw.len() % 2, 0);
    let bytes: Vec<u8> = raw
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    assert_eq!(
        bytes.len() as u64,
        original["database_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        evo_core::hash(&bytes),
        original["database_sha256"].as_str().unwrap()
    );
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&private)
        .unwrap();
    let path = private.join("ag163.sqlite3");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    Ag163History {
        _dir: dir,
        path,
        store,
        original,
    }
}

#[cfg(unix)]
fn ag163_sid(kind: &str, id: &str) -> String {
    format!("e05-{}", evo_core::fingerprint(&(kind, id)).unwrap())
}

#[cfg(unix)]
async fn ag163_body(store: &Store, context: &Context, kind: &str, id: &str) -> String {
    let mut session = store.session().await.unwrap();
    let row = session
        .object_row(context, kind, id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    row.body
}

#[cfg(unix)]
fn ag163_image(path: &std::path::Path) -> serde_json::Value {
    let program = r#"
import json,sqlite3,struct,sys,urllib.parse
def qi(s):return '"'+s.replace('"','""')+'"'
def cell(k,v):
 if k==b'null':return ['null']
 if k==b'integer':return ['integer',str(v)]
 if k==b'real':return ['real',struct.pack('>d',v).hex()]
 if k in (b'text',b'blob'):return [k.decode(),v.hex()]
 raise AssertionError(k)
c=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1],safe='/')+'?mode=ro',uri=True,isolation_level=None);c.text_factory=bytes
c.execute('PRAGMA query_only=ON');c.execute('BEGIN');assert c.execute('PRAGMA integrity_check').fetchall()==[(b'ok',)]
schema=c.execute('SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY').fetchall();tables=[]
for typ,n,_,_,ddl in schema:
 if typ!=b'table':continue
 name=n.decode();info=c.execute('PRAGMA table_xinfo('+qi(name)+')').fetchall();cols=[x[1].decode() for x in info if x[6]!=1]
 rowid=b'WITHOUT ROWID' not in (ddl or b'').upper();fields=['_rowid_'] if rowid else []
 for col in cols:fields.extend(['typeof('+qi(col)+')',qi(col)])
 rows=[]
 for v in c.execute('SELECT '+','.join(fields)+' FROM '+qi(name)):
  off=1 if rowid else 0;rows.append([str(v[0]) if rowid else None,[cell(v[off+2*i],v[off+2*i+1]) for i in range(len(cols))]])
 rows.sort(key=lambda v:json.dumps(v,separators=(',',':')));tables.append({'name':name,'columns':cols,'rowid':rowid,'rows':rows})
calls=c.execute('SELECT state,actual_cost_micros,execution_closed FROM root_budget_calls ORDER BY billing_scope,call_id').fetchall()
c.execute('ROLLBACK');c.close();print(json.dumps({'schema':[[a.hex(),b.hex(),d.hex(),p,None if sql is None else sql.hex()] for a,b,d,p,sql in schema],'tables':tables,'calls':[[s.decode(),a,x] for s,a,x in calls]},sort_keys=True,separators=(',',':')))
"#;
    let output = std::process::Command::new("python3")
        .args(["-I", "-B", "-c", program])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned readonly full database image failed"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The full image includes SQL storage classes, original TEXT/BLOB bytes,
/// schema, physical rowids and sqlite_sequence. New action records may append;
/// Only the existing S2 deletion, witnessed private redactions and specific
/// cleanup state columns may change; every original budget/report fact is kept.
#[cfg(unix)]
fn ag163_preserves_original_rows(
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> BTreeMap<(String, String, String), Option<serde_json::Value>> {
    fn text(cell: &serde_json::Value) -> String {
        assert_eq!(cell[0], "text");
        let raw = cell[1].as_str().unwrap();
        let bytes: Vec<u8> = raw
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        String::from_utf8(bytes).unwrap()
    }
    fn column<'a>(
        table: &serde_json::Value,
        row: &'a serde_json::Value,
        name: &str,
    ) -> &'a serde_json::Value {
        let index = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .position(|value| value == name)
            .unwrap();
        &row[1][index]
    }
    fn get<'a>(image: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        image["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["name"] == name)
            .unwrap()
    }
    fn retained_call(value: &serde_json::Value) -> serde_json::Value {
        let call: evo_storage::budget::BudgetCallRecord =
            serde_json::from_value(value.clone()).unwrap();
        let normalized = serde_json::to_value(call).unwrap();
        let mut expected = normalized.as_object().unwrap().clone();
        expected.remove("lease_token");
        for key in [
            "response_block_reason",
            "execution_close_reason",
            "terminal_reason",
        ] {
            let value = expected.remove(key).unwrap();
            expected.insert(
                format!("{key}_digest"),
                value
                    .as_str()
                    .map(|text| json!(hash(text.as_bytes())))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        for key in [
            "request_artifact",
            "transport_artifact",
            "response_artifact",
        ] {
            let value = expected.remove(key).unwrap();
            let redacted = if value.is_null() {
                serde_json::Value::Null
            } else {
                json!({"schema_version":value["schema_version"],"digest":value["digest"],"body_redacted":true})
            };
            expected.insert(key.into(), redacted);
        }
        serde_json::Value::Object(expected)
    }
    assert!(
        before["schema"] == after["schema"],
        "cleanup changed original full schema"
    );
    let objects = get(before, "objects");
    let source = objects["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            text(column(objects, row, "kind")) == "run" && text(column(objects, row, "id")) == "S2"
        })
        .unwrap();
    let namespace = text(column(objects, source, "namespace"));
    let source_wire: serde_json::Value =
        serde_json::from_str(&text(column(objects, source, "body"))).unwrap();
    assert_eq!(source_wire["schema_version"], "rsia.optimization.source.v1");
    assert_eq!(source_wire["record"]["id"], "S2");
    let dependencies = get(before, "dependencies");
    let mut scope = std::collections::BTreeSet::from([("run".to_owned(), "S2".to_owned())]);
    loop {
        let mut added = false;
        for row in dependencies["rows"].as_array().unwrap() {
            if text(column(dependencies, row, "namespace")) != namespace {
                continue;
            }
            let destination = (
                text(column(dependencies, row, "dst_kind")),
                text(column(dependencies, row, "dst_id")),
            );
            if scope.contains(&destination) {
                added |= scope.insert((
                    text(column(dependencies, row, "src_kind")),
                    text(column(dependencies, row, "src_id")),
                ));
            }
        }
        if !added {
            break;
        }
    }
    let mut changed_protected = BTreeMap::new();
    let mut actual_formal_in_scope = false;
    for old in before["tables"].as_array().unwrap() {
        let name = old["name"].as_str().unwrap();
        let new = get(after, name);
        assert!(
            old["columns"] == new["columns"],
            "cleanup changed original columns"
        );
        let old_rows = old["rows"].as_array().unwrap();
        let new_rows = new["rows"].as_array().unwrap();
        if name == "sqlite_sequence" {
            for row in old_rows {
                let successor = new_rows
                    .iter()
                    .find(|value| value[1][0] == row[1][0])
                    .unwrap();
                assert!(
                    successor[0] == row[0] && successor[1][0] == row[1][0],
                    "cleanup changed original sequence identity"
                );
                assert_eq!(row[1][1][0], "integer");
                assert_eq!(successor[1][1][0], "integer");
                assert!(
                    successor[1][1][1].as_str().unwrap().parse::<i64>().unwrap()
                        >= row[1][1][1].as_str().unwrap().parse::<i64>().unwrap(),
                    "cleanup reset an original sequence counter"
                );
            }
            continue;
        }
        if name == "objects" {
            for row in old_rows {
                let row_namespace = text(column(old, row, "namespace"));
                let kind = text(column(old, row, "kind"));
                let id = text(column(old, row, "id"));
                let original_text = text(column(old, row, "body"));
                let original: serde_json::Value = serde_json::from_str(&original_text).unwrap();
                if original["record_kind"] == "formal_evaluation_v2" {
                    // The ordinary native wrapper keeps its ticket in result.ticket_id.
                    let payload = &original["payload"];
                    let ticket = payload
                        .get("ticket_id")
                        .unwrap_or(&payload["result"]["ticket_id"])
                        .as_str()
                        .unwrap();
                    let ticket_physical = ag163_sid("evaluation_ticket_v2", ticket);
                    assert_eq!(id, ag163_sid("formal_evaluation_v2", ticket));
                    assert!(
                        row_namespace == namespace
                            && scope.contains(&("artifact".into(), ticket_physical)),
                        "old report's actual ticket is outside the real revoked source closure"
                    );
                    actual_formal_in_scope = true;
                }
                let successor = new_rows.iter().find(|value| value[0] == row[0]);
                if kind == "run" && id == "S2" && row_namespace == namespace {
                    assert!(
                        successor.is_none(),
                        "legal run source content was not deleted"
                    );
                    changed_protected.insert((row_namespace, kind, id), None);
                    continue;
                }
                let successor = successor.expect("cleanup deleted an unrelated original object");
                if successor == row {
                    continue;
                }
                assert!(
                    row_namespace == namespace && scope.contains(&(kind.clone(), id.clone())),
                    "cleanup rewrote an object outside the actual original source closure"
                );
                for (index, key) in old["columns"].as_array().unwrap().iter().enumerate() {
                    if key != "body" && key != "revision" {
                        assert!(
                            row[1][index] == successor[1][index],
                            "cleanup changed original object identity/owner/typed bytes"
                        );
                    }
                }
                assert_eq!(column(old, row, "revision")[0], "integer");
                assert_eq!(column(new, successor, "revision")[0], "integer");
                assert_eq!(
                    column(new, successor, "revision")[1]
                        .as_str()
                        .unwrap()
                        .parse::<i64>()
                        .unwrap(),
                    column(old, row, "revision")[1]
                        .as_str()
                        .unwrap()
                        .parse::<i64>()
                        .unwrap()
                        + 1
                );
                let redacted: serde_json::Value =
                    serde_json::from_str(&text(column(new, successor, "body"))).unwrap();
                assert_eq!(redacted.as_object().unwrap().len(), 7);
                assert_eq!(redacted["id"], id);
                assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
                assert_eq!(redacted["state"], "source_revoked");
                assert_eq!(redacted["original_kind"], kind);
                assert_eq!(redacted["original_schema"], original["schema_version"]);
                assert_eq!(redacted["original_digest"], hash(original_text.as_bytes()));
                let schema = original["schema_version"].as_str().unwrap();
                let record = original["record_kind"].as_str().unwrap_or("");
                let private = matches!(
                    schema,
                    "rsia.release_candidate.v1"
                        | "rsia.release_candidate.v2"
                        | "rsia.optimization.compiler_use.v1"
                        | "rsia.optimization.compiler_use.v2"
                        | "rsia.authenticated_bundle.v1"
                        | "rsia.authenticated_bundle.v2"
                        | "rsia.resolved_bundle.v2"
                ) || (schema == "rsia.typed_artifact_envelope.v1"
                    && matches!(
                        record,
                        "registered_evaluation_control_v41"
                            | "protected_holdout_v41"
                            | "evaluation_execution_output_v1"
                    ));
                if schema == "rsia.typed_artifact_envelope.v1"
                    && record == "evaluation_resource_evidence_v1"
                {
                    let payload = &original["payload"];
                    let calls: Vec<_> = payload["calls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(retained_call)
                        .collect();
                    let expected = json!({"record_kind":record,"ticket_id":payload["ticket_id"],"billing_scope":payload["billing_scope"],
                        "cost_scope":payload["cost_scope"],"calls":calls,"formal_closure_invalidated":true});
                    assert!(
                        redacted["metadata"] == expected,
                        "resource cleanup changed original complete retained call facts"
                    );
                } else {
                    assert!(
                        private,
                        "cleanup rewrote a protected historical report/receipt/accounting object"
                    );
                    let expected = evo_storage::lifecycle::redacted_object_body(
                        &kind,
                        &id,
                        &original,
                        &original_text,
                    );
                    assert!(
                        redacted == expected,
                        "private cleanup differs from the complete original redactor output"
                    );
                }
                changed_protected.insert((row_namespace, kind, id), Some(redacted));
            }
            assert_eq!(
                new_rows.len() + 1,
                old_rows.len(),
                "cleanup added objects or deleted more than its actual run source"
            );
            continue;
        }
        if name == "idempotency"
            || name == "revoke_cleanup_jobs"
            || name == "revoke_cleanup_frontier"
        {
            for row in old_rows {
                let successor = new_rows.iter().find(|value| value[0] == row[0]).unwrap();
                if successor == row {
                    continue;
                }
                let allowed: &[&str] = match name {
                    "idempotency" => {
                        assert_eq!(text(column(old, row, "namespace")), namespace);
                        let subject = text(column(old, row, "subject_id"));
                        assert!(
                            scope.iter().any(|(_, id)| id == &subject),
                            "unrelated idempotency response was redacted"
                        );
                        assert_eq!(text(column(new, successor, "response")), "null");
                        assert_eq!(column(new, successor, "redacted"), &json!(["integer", "1"]));
                        &["response", "redacted"]
                    }
                    "revoke_cleanup_jobs" => {
                        &["state", "processed_nodes", "last_error", "updated_at"]
                    }
                    _ => &["cursor_src_kind", "cursor_src_id", "expanded"],
                };
                for (index, key) in old["columns"].as_array().unwrap().iter().enumerate() {
                    if !allowed.contains(&key.as_str().unwrap()) {
                        assert!(
                            row[1][index] == successor[1][index],
                            "cleanup changed a stable original action/idempotency column"
                        );
                    }
                }
            }
            if name != "revoke_cleanup_frontier" {
                assert_eq!(old_rows.len(), new_rows.len());
            }
            continue;
        }
        if name == "audit" || name == "revoke_cleanup_events" {
            assert!(
                old_rows.iter().all(|row| new_rows.contains(row)),
                "cleanup rewrote an original audit or cleanup event"
            );
        } else {
            assert!(
                old_rows == new_rows,
                "cleanup changed an unrelated table or complete original budget facts"
            );
        }
    }
    assert!(
        actual_formal_in_scope,
        "cleanup test never bound its actual old report to a reached source ticket"
    );
    assert!(
        before["calls"] == after["calls"],
        "cleanup changed original amount/None/late/closed facts"
    );
    changed_protected
}

#[cfg(unix)]
fn ag163_original_report_was_expanded(
    image: &serde_json::Value,
    namespace: &str,
    job: &str,
    report: &str,
) {
    let table = image["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|table| table["name"] == "revoke_cleanup_frontier")
        .unwrap();
    let index = |name: &str| {
        table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .position(|column| column == name)
            .unwrap()
    };
    let text = |value: &str| {
        json!([
            "text",
            value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ])
    };
    let rows: Vec<_> = table["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row[1][index("namespace")] == text(namespace)
                && row[1][index("job_id")] == text(job)
                && row[1][index("node_kind")] == text("artifact")
                && row[1][index("node_id")] == text(report)
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "the actual old report was not uniquely discovered in this cleanup job"
    );
    assert!(
        rows[0][1][index("expanded")] == json!(["integer", "1"]),
        "old report identity never reached the real cleanup classifier"
    );
}

#[cfg(unix)]
async fn ag163_old_running_checkpoint(
    store: &mut Store,
    path: &Path,
    admin: &Context,
    job: &str,
    ticket: &str,
    report: &str,
) -> evo_storage::lifecycle::CleanupStatus {
    let mut view = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    let mut status = LifecycleStore::cleanup_status(admin, store, job)
        .await
        .unwrap();
    let mut reached = false;
    for _ in 0..512 {
        let expanded: Option<i64> = sqlx::query_scalar(
            "SELECT expanded FROM revoke_cleanup_frontier WHERE namespace=? AND job_id=? AND node_kind='artifact' AND node_id=?",
        ).bind(admin.namespace()).bind(job).bind(ticket).fetch_optional(&mut view).await.unwrap();
        if expanded == Some(1) {
            reached = true;
            break;
        }
        assert!(matches!(
            status.state,
            evo_storage::lifecycle::CleanupState::Pending
                | evo_storage::lifecycle::CleanupState::Running
        ));
        status = LifecycleStore::cleanup_step(admin, store, job, 1, evo_core::now().unwrap())
            .await
            .unwrap();
    }
    view.close().await.unwrap();
    assert!(
        reached,
        "public cleanup never reached its real ticket before checkpoint"
    );
    assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Running);
    store.close().await;
    let before_projection = ag163_image(path);
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    // An owned upgrade-compatibility projection: remove only metadata the old
    // implementation did not write. Real Running job/expanded ticket, objects,
    // source/edges/amounts and every old business fact stay as publicly produced.
    sqlx::query("DELETE FROM revoke_cleanup_frontier WHERE namespace=? AND job_id=? AND node_kind='__formal_ticket_scan'")
        .bind(admin.namespace()).bind(job).execute(&mut connection).await.unwrap();
    sqlx::query(
        "DELETE FROM revoke_cleanup_frontier WHERE namespace=? AND job_id=? AND node_kind='artifact' AND node_id=? AND expanded=0
         AND NOT EXISTS (SELECT 1 FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=? AND dst_kind='artifact' AND dst_id=?)",
    ).bind(admin.namespace()).bind(job).bind(report).bind(admin.namespace()).bind(report).bind(ticket)
        .execute(&mut connection).await.unwrap();
    let still_expanded: i64 = sqlx::query_scalar(
        "SELECT expanded FROM revoke_cleanup_frontier WHERE namespace=? AND job_id=? AND node_kind='artifact' AND node_id=?",
    ).bind(admin.namespace()).bind(job).bind(ticket).fetch_one(&mut connection).await.unwrap();
    assert_eq!(still_expanded, 1);
    connection.close().await.unwrap();
    let after_projection = ag163_image(path);
    assert!(before_projection["schema"] == after_projection["schema"]);
    for old in before_projection["tables"].as_array().unwrap() {
        if old["name"] == "revoke_cleanup_frontier" {
            continue;
        }
        let unchanged = after_projection["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == old["name"])
            .unwrap();
        assert!(
            old == unchanged,
            "checkpoint projection changed real job/object/edge/sequence/accounting facts"
        );
    }
    *store = Store::open(path).await.unwrap();
    let resumed = LifecycleStore::cleanup_status(admin, store, job)
        .await
        .unwrap();
    assert_eq!(resumed.state, evo_storage::lifecycle::CleanupState::Running);
    assert_eq!(resumed.processed_nodes, status.processed_nodes);
    resumed
}

#[cfg(unix)]
async fn ag163_non_ticket_raw_controls() {
    use evo_core::contract::{
        HostSurfaceClassificationV2, HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1,
        SurfaceCoverage, host_surface_upgrade_diff_v2, host_surface_v2_from_source,
    };
    struct DuplicatePractice;
    impl serde::Serialize for DuplicatePractice {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut out = serializer.serialize_struct("OrdinaryRawControl", 4)?;
            out.serialize_field("schema_version", "rsia.practice_registration.v1")?;
            out.serialize_field("id", "raw-control")?;
            out.serialize_field("echo", &1)?;
            out.serialize_field("echo", &2)?;
            out.end()
        }
    }
    let nested = "[".repeat(200) + "0" + &"]".repeat(200);
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version:"rsia.host_source_envelope.v1".into(), source_kind:"reviewed_fixture".into(),
        source_schema:"rsia.host_reviewed_declarations.v1".into(),
        extractor_version:"rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8:r#"{"schema_version":"rsia.host_reviewed_declarations.v1","host":"reference","host_version":"1","declarations":[{"name":"option","value_type":"object","enum_values":[],"default":{"kind":"value","value":{"nested":DEPTH}}}]}"#.replace("DEPTH", &nested),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "1".into(),
        retired: vec![],
        classifications: vec![HostSurfaceClassificationV2 {
            name: "option".into(),
            coverage: SurfaceCoverage::Unsupported,
            mapped_field: None,
            consumer: None,
            reason: "raw cleanup control".into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        }],
    };
    let (manifest, typed) = host_surface_v2_from_source(&source, &registration).unwrap();
    let host = json!({"id":"raw-control","schema_version":"rsia.host_surface_record.v2",
        "manifest_digest":evo_core::fingerprint(&manifest).unwrap(),
        "upgrade_diff":host_surface_upgrade_diff_v2(&manifest,None).unwrap(),
        "manifest":manifest,"source":source,"extracted":["option"],"extracted_typed":typed,"predecessor_surface_id":null});
    for current_host in [false, true] {
        let (_dir, store, path) = database().await;
        let admin = ctx("ag163-raw-cleanup", "admin", Role::Admin);
        put_run(&store, admin.namespace(), "raw-source").await;
        let mut session = store.session().await.unwrap();
        if current_host {
            session
                .put(&admin, "artifact", "raw-control", admin.actor(), &host)
                .await
                .unwrap();
        } else {
            session
                .put(
                    &admin,
                    "artifact",
                    "raw-control",
                    admin.actor(),
                    &DuplicatePractice,
                )
                .await
                .unwrap();
        }
        session
            .put_edge(&admin, "artifact", "raw-control", "run", "raw-source")
            .await
            .unwrap();
        // Public original read path accepts either ordinary last-key serde or
        // the actual CurrentHost raw decoder's legal depth 200.
        let parsed: serde_json::Value = session
            .need(&admin, "artifact", "raw-control")
            .await
            .unwrap();
        if !current_host {
            assert_eq!(parsed["echo"], 2);
        }
        session.commit().await.unwrap();
        let original = ag163_body(&store, &admin, "artifact", "raw-control").await;
        let mut status = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: "raw-source".into(),
            },
            "AG163 non-ticket raw compatibility",
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
        let before = ag163_image(&path);
        for _ in 0..16 {
            if matches!(
                status.state,
                evo_storage::lifecycle::CleanupState::Complete
                    | evo_storage::lifecycle::CleanupState::Failed
            ) {
                break;
            }
            status = LifecycleStore::cleanup_step(
                &admin,
                &store,
                &status.job_id,
                8,
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
        }
        if current_host {
            assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Failed);
            assert_eq!(
                status.last_error.as_deref(),
                Some("blocked_unknown_scope:artifact:rsia.host_surface_record.v2:")
            );
        } else {
            assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Complete);
        }
        assert!(
            ag163_body(&store, &admin, "artifact", "raw-control").await == original,
            "non-ticket old raw bytes were rewritten or routed through a new decoder"
        );
        let after = ag163_image(&path);
        assert!(before["schema"] == after["schema"]);
        for old in before["tables"].as_array().unwrap().iter().filter(|t| {
            t["name"].as_str().unwrap().starts_with("root_budget")
                || matches!(
                    t["name"].as_str().unwrap(),
                    "dependencies" | "revoke_watermark"
                )
        }) {
            assert!(
                after["tables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|new| new == old),
                "non-ticket compatibility changed original typed dependency/watermark/accounting facts"
            );
        }
        store.close().await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn known_formal_history_is_cleanable_without_resetting_spend() {
    for old_running in [false, true] {
        for name in ["ag163-old-complete-v2", "ag163-old-early-v2"] {
            let mut fx = ag163_history(name).await;
            let namespace = fx.original["namespace"].as_str().unwrap();
            let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
            let ticket = fx.original["ticket_id"].as_str().unwrap();
            let physical = ag163_sid("formal_evaluation_v2", ticket);
            let original_body = ag163_body(&fx.store, &admin, "artifact", &physical).await;
            assert_eq!(
                hash(original_body.as_bytes()),
                fx.original["original_formal_body_sha256"].as_str().unwrap()
            );
            let raw: serde_json::Value = serde_json::from_str(&original_body).unwrap();
            assert_eq!(raw["payload"]["variant"], fx.original["expected_variant"]);
            assert!(raw["payload"].get("schema_version").is_none());
            if name == "ag163-old-complete-v2" {
                assert_eq!(
                    raw["payload"]["report"]["stats_version"],
                    "rsia.empirical_bernstein.v2"
                );
            } else {
                assert_eq!(
                    raw["payload"]["certificate"]["schema_version"],
                    "rsia.early_stop_certificate.v1"
                );
            }
            let before = ag163_image(&fx.path);
            assert_eq!(
                before["calls"].as_array().unwrap().len() as u64,
                fx.original["original_call_rows"].as_u64().unwrap()
            );
            assert!(
                before["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|call| call[0] == "finalized" && call[2] == 1)
            );
            assert!(
                before["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|call| call[1].as_i64().unwrap() > 0)
            );
            let source_body: serde_json::Value =
                serde_json::from_str(&ag163_body(&fx.store, &admin, "run", "S2").await).unwrap();
            assert_eq!(source_body["schema_version"], "rsia.optimization.source.v1");
            assert_eq!(source_body["record"]["id"], "S2");
            let ticket_physical = ag163_sid("evaluation_ticket_v2", ticket);
            let ticket_wire: serde_json::Value = serde_json::from_str(
                &ag163_body(&fx.store, &admin, "artifact", &ticket_physical).await,
            )
            .unwrap();
            let binding = &ticket_wire["payload"]["candidate_dependency_binding"];
            assert!(
                binding["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|source| source["kind"] == "run" && source["id"] == "S2")
            );
            let mut session = fx.store.session().await.unwrap();
            assert!(
                session
                    .dependents(&admin, "run", "S2")
                    .await
                    .unwrap()
                    .contains(&(
                        "artifact".into(),
                        binding["candidate_id"].as_str().unwrap().into()
                    ))
            );
            assert!(
                session
                    .dependents(
                        &admin,
                        "artifact",
                        binding["candidate_id"].as_str().unwrap()
                    )
                    .await
                    .unwrap()
                    .contains(&("artifact".into(), ticket_physical.clone()))
            );
            let original_report_edge = session
                .dependents(&admin, "artifact", &ticket_physical)
                .await
                .unwrap()
                .contains(&("artifact".into(), physical.clone()));
            assert_eq!(
                original_report_edge,
                name == "ag163-old-complete-v2",
                "qualified Early has no original formal edge; discovery must not backfill one"
            );
            session.commit().await.unwrap();
            assert!(
                ag163_image(&fx.path) == before,
                "real original source/edge health checks changed history"
            );
            let mut status = LifecycleStore::begin_revoke(
                &admin,
                &fx.store,
                TypedObjectRef {
                    kind: "run".into(),
                    id: "S2".into(),
                },
                "AG163 preserve history after revoking its actual S2 upstream",
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
            let revoked_before = ag163_image(&fx.path);
            for old in before["tables"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|table| table["name"].as_str().unwrap().starts_with("root_budget"))
            {
                let unchanged = revoked_before["tables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|table| table["name"] == old["name"])
                    .unwrap();
                assert!(
                    old == unchanged,
                    "logical revocation changed original complete budget facts"
                );
            }
            let original_facts = read_control_plane_facts(&fx.path).await.unwrap();
            if old_running {
                status = ag163_old_running_checkpoint(
                    &mut fx.store,
                    &fx.path,
                    &admin,
                    &status.job_id,
                    &ticket_physical,
                    &physical,
                )
                .await;
            }
            for _ in 0..100 {
                if matches!(
                    status.state,
                    evo_storage::lifecycle::CleanupState::Complete
                        | evo_storage::lifecycle::CleanupState::Failed
                ) {
                    break;
                }
                status = LifecycleStore::cleanup_step(
                    &admin,
                    &fx.store,
                    &status.job_id,
                    16,
                    evo_core::now().unwrap(),
                )
                .await
                .unwrap();
            }
            assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Complete);
            assert!(status.last_error.is_none());
            assert!(
                ag163_body(&fx.store, &admin, "artifact", &physical).await == original_body,
                "known old formal cleanup changed original wire bytes"
            );
            let after = ag163_image(&fx.path);
            ag163_original_report_was_expanded(&after, namespace, &status.job_id, &physical);
            let legal_changes = ag163_preserves_original_rows(&revoked_before, &after);
            let after_facts = read_control_plane_facts(&fx.path).await.unwrap();
            assert!(
                after_facts.watermarks == original_facts.watermarks
                    && after_facts.tombstones == original_facts.tombstones,
                "cleanup changed committed original revoke facts"
            );
            assert!(
                after_facts.root_budget_tables == original_facts.root_budget_tables,
                "cleanup changed complete original paid/None/closed facts"
            );
            let mut expected_protected = original_facts.protected_objects.clone();
            for (key, redacted) in legal_changes {
                if PROTECTED_OBJECT_KINDS.contains(&key.1.as_str()) {
                    if let Some(value) = redacted {
                        expected_protected.insert(key, value);
                    } else {
                        expected_protected.remove(&key);
                    }
                } else {
                    expected_protected.remove(&key);
                }
            }
            assert!(
                after_facts.protected_objects == expected_protected,
                "cleanup changed protected history outside its exact validated private-content transitions"
            );
            let before_second = ag163_image(&fx.path);
            let second = LifecycleStore::cleanup_step(
                &admin,
                &fx.store,
                &status.job_id,
                16,
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(second.state, evo_storage::lifecycle::CleanupState::Complete);
            assert!(
                ag163_image(&fx.path) == before_second,
                "completed cleanup retry changed any full database fact"
            );
        }
    }
    ag163_non_ticket_raw_controls().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_inner_judgment_version_is_left_in_quarantine() {
    for old_running in [false, true] {
        for fault in [
            "old_statistics",
            "old_certificate",
            "payload_schema",
            "native_wire",
        ] {
            // Early fixtures have no original formal->ticket edge: only the
            // one-slot catch-up can reach their report.
            let early = matches!(fault, "old_certificate" | "native_wire");
            let fx = ag163_history(if early {
                "ag163-old-early-v2"
            } else {
                "ag163-old-complete-v2"
            })
            .await;
            let namespace = fx.original["namespace"].as_str().unwrap();
            let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
            let ticket = fx.original["ticket_id"].as_str().unwrap();
            let physical = ag163_sid("formal_evaluation_v2", ticket);
            let healthy = ag163_body(&fx.store, &admin, "artifact", &physical).await;
            assert_eq!(
                hash(healthy.as_bytes()),
                fx.original["original_formal_body_sha256"].as_str().unwrap()
            );
            let mut damaged: serde_json::Value = serde_json::from_str(&healthy).unwrap();
            assert!(damaged["payload"].get("schema_version").is_none());
            let expected_error = if fault == "old_statistics" {
                // One actual old judgment algorithm slot changes. All original
                // outer ids, actor pins, bills, witness and payload shape stay old.
                assert_eq!(
                    damaged["payload"]["report"]["stats_version"],
                    "rsia.empirical_bernstein.v2"
                );
                damaged["payload"]["report"]["stats_version"] =
                    json!("rsia.empirical_bernstein.v999");
                "blocked_unknown_scope:formal_judgment_old_wire_or_domain"
            } else if fault == "old_certificate" {
                assert_eq!(
                    damaged["payload"]["certificate"]["schema_version"],
                    "rsia.early_stop_certificate.v1"
                );
                damaged["payload"]["certificate"]["schema_version"] =
                    json!("rsia.early_stop_certificate.v999");
                "blocked_unknown_scope:formal_judgment_old_wire_or_domain"
            } else if fault == "payload_schema" {
                // A separate unknown inner-wire fault: outside the table, never sent
                // to the derived redactor and never a claimed positive.
                damaged["payload"]["schema_version"] = json!("rsia.formal_evaluation.v999");
                "blocked_unknown_scope:formal_judgment_version_outside_table"
            } else {
                // Only the native wrapper changes: the actual original remains
                // Control2/Ticket2/Holdout1/certificate1, not the native source domain.
                // Catch-up must find its real relation, then quarantine that domain.
                let result = damaged["payload"].take();
                damaged["payload"] = json!({"schema_version":"rsia.formal_evaluation.v3",
                "decision_version":"rsia.fixed_sample_regression.v1","result":result});
                "blocked_unknown_scope:formal_judgment_native_wire_or_domain"
            };
            let source_body: serde_json::Value =
                serde_json::from_str(&ag163_body(&fx.store, &admin, "run", "S2").await).unwrap();
            assert_eq!(source_body["schema_version"], "rsia.optimization.source.v1");
            assert_eq!(source_body["record"]["id"], "S2");
            let ticket_physical = ag163_sid("evaluation_ticket_v2", ticket);
            let ticket_wire: serde_json::Value = serde_json::from_str(
                &ag163_body(&fx.store, &admin, "artifact", &ticket_physical).await,
            )
            .unwrap();
            if fault == "native_wire" {
                assert_eq!(
                    ticket_wire["payload"]["schema_version"],
                    "rsia.evaluation_ticket.v2"
                );
                let control: serde_json::Value = serde_json::from_str(
                    &ag163_body(
                        &fx.store,
                        &admin,
                        "artifact",
                        &ag163_sid(
                            "registered_evaluation_control_v41",
                            ticket_wire["payload"]["registration_id"].as_str().unwrap(),
                        ),
                    )
                    .await,
                )
                .unwrap();
                assert_eq!(
                    control["payload"]["schema_version"],
                    "rsia.registered_evaluation_control.v2"
                );
                let holdout: serde_json::Value = serde_json::from_str(
                    &ag163_body(
                        &fx.store,
                        &admin,
                        "artifact",
                        &ag163_sid(
                            "protected_holdout_v41",
                            ticket_wire["payload"]["holdout_id"].as_str().unwrap(),
                        ),
                    )
                    .await,
                )
                .unwrap();
                assert_eq!(
                    holdout["payload"]["schema_version"],
                    "rsia.protected_holdout_record.v1"
                );
                assert_eq!(
                    damaged["payload"]["result"]["certificate"]["schema_version"],
                    "rsia.early_stop_certificate.v1"
                );
            }
            let binding = &ticket_wire["payload"]["candidate_dependency_binding"];
            assert!(
                binding["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|source| source["kind"] == "run" && source["id"] == "S2")
            );
            let mut session = fx.store.session().await.unwrap();
            assert!(
                session
                    .dependents(&admin, "run", "S2")
                    .await
                    .unwrap()
                    .contains(&(
                        "artifact".into(),
                        binding["candidate_id"].as_str().unwrap().into()
                    ))
            );
            assert!(
                session
                    .dependents(
                        &admin,
                        "artifact",
                        binding["candidate_id"].as_str().unwrap()
                    )
                    .await
                    .unwrap()
                    .contains(&("artifact".into(), ticket_physical.clone()))
            );
            let original_report_edge = session
                .dependents(&admin, "artifact", &ticket_physical)
                .await
                .unwrap()
                .contains(&("artifact".into(), physical.clone()));
            assert_eq!(
                original_report_edge, !early,
                "Early fault tests must retain the genuine missing Early edge"
            );
            session.commit().await.unwrap();
            fx.store.close().await;
            let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&fx.path)
                .create_if_missing(false)
                .disable_statement_logging()
                .connect()
                .await
                .unwrap();
            let changed = sqlx::query(
                "UPDATE objects SET body=? WHERE namespace=? AND kind='artifact' AND id=?",
            )
            .bind(serde_json::to_string(&damaged).unwrap())
            .bind(namespace)
            .bind(&physical)
            .execute(&mut connection)
            .await
            .unwrap();
            assert_eq!(changed.rows_affected(), 1);
            connection.close().await.unwrap();
            let mut owned = Store::open(&fx.path).await.unwrap();
            let damaged_body = ag163_body(&owned, &admin, "artifact", &physical).await;
            let before = ag163_image(&fx.path);
            let mut status = LifecycleStore::begin_revoke(
                &admin,
                &owned,
                TypedObjectRef {
                    kind: "run".into(),
                    id: "S2".into(),
                },
                "AG163 isolate unknown original inner judgment after real S2 revoke",
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
            let revoked_before = ag163_image(&fx.path);
            for old in before["tables"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|table| table["name"].as_str().unwrap().starts_with("root_budget"))
            {
                let unchanged = revoked_before["tables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|table| table["name"] == old["name"])
                    .unwrap();
                assert!(
                    old == unchanged,
                    "logical revocation changed original complete budget facts"
                );
            }
            let original_facts = read_control_plane_facts(&fx.path).await.unwrap();
            if old_running {
                status = ag163_old_running_checkpoint(
                    &mut owned,
                    &fx.path,
                    &admin,
                    &status.job_id,
                    &ticket_physical,
                    &physical,
                )
                .await;
            }
            for _ in 0..100 {
                if matches!(
                    status.state,
                    evo_storage::lifecycle::CleanupState::Complete
                        | evo_storage::lifecycle::CleanupState::Failed
                ) {
                    break;
                }
                status = LifecycleStore::cleanup_step(
                    &admin,
                    &owned,
                    &status.job_id,
                    16,
                    evo_core::now().unwrap(),
                )
                .await
                .unwrap();
            }
            assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Failed);
            assert_eq!(status.last_error.as_deref(), Some(expected_error));
            assert!(
                ag163_body(&owned, &admin, "artifact", &physical).await == damaged_body,
                "unknown inner version was rewritten or redacted"
            );
            let after = ag163_image(&fx.path);
            ag163_original_report_was_expanded(&after, namespace, &status.job_id, &physical);
            let legal_changes = ag163_preserves_original_rows(&revoked_before, &after);
            let after_facts = read_control_plane_facts(&fx.path).await.unwrap();
            assert!(
                after_facts.watermarks == original_facts.watermarks
                    && after_facts.tombstones == original_facts.tombstones,
                "unknown-version cleanup changed committed revoke facts"
            );
            assert!(
                after_facts.root_budget_tables == original_facts.root_budget_tables,
                "unknown-version cleanup changed complete original paid/None/closed facts"
            );
            let mut expected_protected = original_facts.protected_objects.clone();
            for (key, redacted) in legal_changes {
                if PROTECTED_OBJECT_KINDS.contains(&key.1.as_str()) {
                    if let Some(value) = redacted {
                        expected_protected.insert(key, value);
                    } else {
                        expected_protected.remove(&key);
                    }
                } else {
                    expected_protected.remove(&key);
                }
            }
            assert!(
                after_facts.protected_objects == expected_protected,
                "unknown-version cleanup changed protected history beyond exact validated private-content transitions"
            );
        }
    }
}

#[tokio::test]
async fn practice_v2_and_stage_v2_are_protected_and_cleanable() {
    let (_dir, store, path) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    put_run(&store, "n", "wire-source").await;
    let mut session = store.session().await.unwrap();
    let mut original = BTreeMap::new();
    for (id, schema) in [
        ("practice-old", "rsia.practice_attempt_set.v1"),
        ("practice-new", "rsia.practice_attempt_set.v2"),
        ("stage-old", "rsia.optimization.stage_fact.v1"),
        ("stage-new", "rsia.optimization.stage_fact.v2"),
    ] {
        // Conservative restore/cleanup selector fixtures, not usable authority.
        let body = json!({"id":id,"schema_version":schema,"content":"private execution content","kind":"step_prepared","payload":{"job_scope":{"job_id":"server-job","groups":[{"request_id":"request","episode_id":"episode","step":0,"attempt":0,"input_digest":evo_core::hash(b"input")}]} }});
        session
            .put(&admin, "artifact", id, admin.actor(), &body)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", "wire-source")
            .await
            .unwrap();
        original.insert(id.to_owned(), body);
    }
    session.commit().await.unwrap();
    let status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "wire-source".into(),
        },
        "remove wire content",
        evo_core::now().unwrap(),
    )
    .await
    .unwrap();
    store.close().await;
    let facts = read_control_plane_facts(&path).await.unwrap();
    for (id, body) in &original {
        assert_eq!(
            &facts.protected_objects[&("n".into(), "artifact".into(), id.clone())],
            body
        );
    }
    let store = Store::open(&path).await.unwrap();
    let mut status = status;
    for _ in 0..20 {
        if status.state == evo_storage::lifecycle::CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(
            &admin,
            &store,
            &status.job_id,
            16,
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
    }
    assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Complete);
    let mut session = store.session().await.unwrap();
    for (id, body) in &original {
        let redacted: serde_json::Value = session.need(&admin, "artifact", id).await.unwrap();
        assert_eq!(redacted["original_schema"], body["schema_version"]);
        assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
        assert!(!redacted.to_string().contains("private execution content"));
        if id == "stage-new" {
            assert_eq!(
                redacted["metadata"]["job_scope"],
                body["payload"]["job_scope"]
            );
        }
    }
    session.commit().await.unwrap();
}
