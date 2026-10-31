//! Fresh and historical SQLx migrations for typed formal parent storage.
//! These storage fixtures do not attest formal principals or an ordinary P.

use evo_core::{Context, Error, Role};
use evo_storage::Store;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, migrate::Migrator, sqlite::SqliteConnectOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const LEGACY_KINDS: [&str; 15] = [
    "run",
    "feedback",
    "event",
    "candidate",
    "release",
    "pointer",
    "receipt",
    "dataset",
    "evaluation",
    "budget",
    "reservation",
    "job",
    "improvement",
    "artifact",
    "tombstone",
];
const PRIOR_VERSIONS: [i64; 5] = [1, 2, 4, 5, 6];
const CURRENT_VERSIONS: [i64; 6] = [1, 2, 4, 5, 6, 7];

type ObjectRow = (i64, String, String, String, String, String, i64);
type MigrationRow = (i64, String, String, bool, Vec<u8>, i64);
type ColumnRow = (i64, String, String, i64, Option<String>, i64);
type SchemaRow = (String, String, String, Option<String>);
type TableRows = (String, Vec<Vec<String>>);

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    objects: Vec<ObjectRow>,
    ledger: Vec<MigrationRow>,
    object_indexes: Vec<(String, String)>,
    unrelated_schema: Vec<SchemaRow>,
    unrelated_rows: Vec<TableRows>,
}

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

fn empty_root_selection(id: &str) -> Value {
    json!({
        "id": id,
        "schema_version": "rsia.formal_parent_selection.v1",
        "authority": {"EmptyRoot": {"id": "authorized-empty-root"}}
    })
}

fn approved_release_selection(id: &str) -> Value {
    json!({
        "id": id,
        "schema_version": "rsia.formal_parent_selection.v1",
        "authority": {"ApprovedRelease": {"release_id": "approved-release"}}
    })
}

async fn connect(path: &Path, create: bool) -> SqliteConnection {
    if create && !path.try_exists().unwrap() {
        let mut file = std::fs::OpenOptions::new();
        file.create_new(true).write(true);
        #[cfg(unix)]
        file.mode(0o600);
        drop(file.open(path).unwrap());
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .foreign_keys(true);
    SqliteConnection::connect_with(&options).await.unwrap()
}

fn prior_migrator() -> Migrator {
    // Keep the actual compiled SQL, descriptions and checksums. Running this
    // Migrator creates the genuine historical _sqlx_migrations ledger.
    let mut migrations = sqlx::migrate!("./migrations");
    migrations.migrations.to_mut().retain(|m| m.version <= 6);
    assert_eq!(
        migrations.iter().map(|m| m.version).collect::<Vec<_>>(),
        PRIOR_VERSIONS
    );
    migrations
}

async fn historical_database(path: &Path) -> SqliteConnection {
    let mut connection = connect(path, true).await;
    prior_migrator().run(&mut connection).await.unwrap();
    connection
}

async fn ledger(connection: &mut SqliteConnection) -> Vec<MigrationRow> {
    sqlx::query_as(
        "SELECT version,description,CAST(installed_on AS TEXT),success,checksum,execution_time
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(connection)
    .await
    .unwrap()
}

fn assert_ledger_checksums(rows: &[MigrationRow], versions: &[i64]) {
    let migrations = sqlx::migrate!("./migrations");
    assert_eq!(rows.iter().map(|row| row.0).collect::<Vec<_>>(), versions);
    for row in rows {
        let migration = migrations.iter().find(|m| m.version == row.0).unwrap();
        assert_eq!(row.1, migration.description.as_ref());
        assert!(row.3);
        assert_eq!(row.4.as_slice(), migration.checksum.as_ref());
    }
}

fn quoted_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

async fn snapshot(connection: &mut SqliteConnection) -> Snapshot {
    let objects = sqlx::query_as(
        "SELECT rowid,namespace,kind,id,owner,body,revision FROM objects ORDER BY rowid",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let ledger = ledger(connection).await;
    let object_indexes = sqlx::query_as(
        "SELECT name,sql FROM sqlite_master
         WHERE type='index' AND tbl_name='objects' AND sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let unrelated_schema = sqlx::query_as(
        "SELECT type,name,tbl_name,sql FROM sqlite_master
         WHERE tbl_name NOT IN ('objects','_sqlx_migrations') ORDER BY type,name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table'
         AND name NOT IN ('objects','_sqlx_migrations') ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let mut unrelated_rows = Vec::new();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&table)
                .fetch_all(&mut *connection)
                .await
                .unwrap();
        // quote() keeps raw TEXT, INTEGER, BLOB and NULL facts comparable without
        // deserializing or normalizing stored JSON. Sorting also supports the
        // FTS config table, which has no rowid.
        let expressions = columns
            .iter()
            .map(|column| format!("quote({})", quoted_identifier(column)))
            .collect::<Vec<_>>()
            .join(",");
        let statement = format!(
            "SELECT {expressions} FROM {} ORDER BY {expressions}",
            quoted_identifier(&table)
        );
        let rows = sqlx::query(&statement)
            .fetch_all(&mut *connection)
            .await
            .unwrap();
        let values = rows
            .into_iter()
            .map(|row| {
                (0..columns.len())
                    .map(|index| row.try_get::<String, _>(index).unwrap())
                    .collect()
            })
            .collect();
        unrelated_rows.push((table, values));
    }
    Snapshot {
        objects,
        ledger,
        object_indexes,
        unrelated_schema,
        unrelated_rows,
    }
}

async fn assert_object_schema_and_constraints(connection: &mut SqliteConnection) {
    let columns: Vec<ColumnRow> = sqlx::query_as("PRAGMA table_info(objects)")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    let expected = [
        ("namespace", "TEXT", 1, None, 1),
        ("kind", "TEXT", 1, None, 2),
        ("id", "TEXT", 1, None, 3),
        ("owner", "TEXT", 1, None, 0),
        ("body", "TEXT", 1, None, 0),
        ("revision", "INTEGER", 1, Some("1"), 0),
    ];
    assert_eq!(columns.len(), expected.len());
    for (index, (column, expected)) in columns.iter().zip(expected).enumerate() {
        assert_eq!(column.0, index as i64);
        assert_eq!(
            (
                column.1.as_str(),
                column.2.as_str(),
                column.3,
                column.4.as_deref(),
                column.5
            ),
            expected
        );
    }
    let indexes: Vec<(String, String)> = sqlx::query_as(
        "SELECT name,sql FROM sqlite_master
         WHERE type='index' AND tbl_name='objects' AND sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let expected_indexes: Vec<(String, String)> = vec![
        (
            "jobs_state".into(),
            "CREATE INDEX jobs_state ON objects(namespace,json_extract(body,'$.state')) WHERE kind='job'".into(),
        ),
        (
            "objects_owner".into(),
            "CREATE INDEX objects_owner ON objects(namespace,kind,owner)".into(),
        ),
    ];
    assert_eq!(indexes, expected_indexes);
    let before = snapshot(connection).await;
    for statement in [
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','unknown','x','admin','{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent_extra','x','admin','{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent','x','admin','not json')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent','x','admin','{\"id\":\"other\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('bad','formal_parent','x','admin','{\"id\":\"x\"}',0)",
        "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('bad','formal_parent','x','admin','{\"id\":\"x\"}',-1)",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES(NULL,'formal_parent','x','admin','{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad',NULL,'x','admin','{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent',NULL,'admin','{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent','x',NULL,'{\"id\":\"x\"}')",
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('bad','formal_parent','x','admin',NULL)",
        "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('bad','formal_parent','x','admin','{\"id\":\"x\"}',NULL)",
        "INSERT INTO objects SELECT namespace,kind,id,owner,body,revision FROM objects LIMIT 1",
    ] {
        let error = sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .unwrap_err();
        assert!(matches!(error, sqlx::Error::Database(_)), "{statement}");
    }
    assert_eq!(snapshot(connection).await, before);
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(connection)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

async fn seed_historical_facts(connection: &mut SqliteConnection) {
    // Explicitly gapped, interleaved rowids detect a rebuild that changes list
    // order. Preserve each original body byte and a nondefault revision.
    for (index, &kind) in LEGACY_KINDS.iter().enumerate() {
        let rowid = 100 + index as i64 * 17;
        let body =
            format!(" {{ \"id\" : \"shared\", \"kind\" : \"{kind}\", \"state\" : \"queued\" }} ");
        sqlx::query(
            "INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision)
             VALUES(?,?,?,?,?,?,?)",
        )
        .bind(rowid)
        .bind("legacy-a")
        .bind(kind)
        .bind("shared")
        .bind(format!("owner-{index}"))
        .bind(body)
        .bind(index as i64 + 2)
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    sqlx::raw_sql(
        r#"INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES
        (8,'legacy-a','artifact','first','owner-first',' { "id" : "first", "v" : 1 } ',19),
        (900,'legacy-a','artifact','last','owner-last','{"id":"last","v":2}',23),
        (3,'legacy-b','artifact','shared','owner-b','{"id":"shared","v":"other namespace"}',31),
        (950,'legacy-a','artifact','missing-body-id','owner-legacy','{"legacy":true}',41);
        INSERT INTO idempotency VALUES('legacy-a','actor','operation','key','payload','shared',' { "ok" : true } ',0);
        INSERT INTO audit(seq,namespace,payload,previous_hash,digest) VALUES(27,'legacy-a',' { "event" : "old" } ','previous','digest');
        INSERT INTO dependencies VALUES('legacy-a','artifact','last','run','shared');
        INSERT INTO revoke_watermark VALUES('legacy-a',9,'watermark');
        INSERT INTO replay_worlds VALUES('legacy-a','world',1,' { "sealed" : true } ');
        INSERT INTO skill_fts(namespace,candidate_id,tokens) VALUES('legacy-a','shared','old tokens');
        INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,spent_micros,reserved_micros,created_at)
         VALUES('scope','root','legacy-a','USD','price','payer','authorization',100,1000,11,7,123);
        INSERT INTO root_budget_namespaces VALUES('scope','legacy-a');
        INSERT INTO root_budget_dispatch_groups(billing_scope,dispatch_group_id,owner_namespace,created_at) VALUES('scope','group','legacy-a',123);
        INSERT INTO root_budget_calls(billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,reserved_micros,state,lease_token,lease_epoch,lease_until,created_at)
         VALUES('scope','call','group','legacy-a','optimization','input',7,'reserved','lease',3,300,123);
        INSERT INTO root_budget_events(seq,billing_scope,call_id,event_kind,event_at,details) VALUES(41,'scope','call','reserved',123,' { "old" : true } ');
        INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,state,created_at,updated_at)
         VALUES('legacy-a','cleanup','run','shared',9,'watermark','pending',123,124);
        INSERT INTO revoke_cleanup_frontier(seq,namespace,job_id,node_kind,node_id) VALUES(61,'legacy-a','cleanup','artifact','last');
        INSERT INTO revoke_cleanup_events(seq,namespace,job_id,node_kind,node_id,event_kind,event_at,details)
         VALUES(71,'legacy-a','cleanup','artifact','last','queued',124,' { "old" : true } ');"#,
    )
    .execute(connection)
    .await
    .unwrap();
}

#[tokio::test]
async fn fresh_store_accepts_formal_parent_and_preserves_object_constraints() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let context = admin("fresh-a");
    let other_namespace = admin("fresh-b");
    let parent = empty_root_selection("parent");
    let approved = approved_release_selection("approved");
    let mut session = store.session().await.unwrap();
    for kind in LEGACY_KINDS {
        session
            .put(
                &context,
                kind,
                "shared",
                context.actor(),
                &json!({"id":"shared","kind":kind}),
            )
            .await
            .unwrap();
    }
    session
        .put(
            &context,
            "formal_parent",
            "parent",
            context.actor(),
            &parent,
        )
        .await
        .unwrap();
    session
        .put(
            &context,
            "formal_parent",
            "approved",
            context.actor(),
            &approved,
        )
        .await
        .unwrap();
    session
        .put(
            &other_namespace,
            "formal_parent",
            "parent",
            other_namespace.actor(),
            &approved_release_selection("parent"),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let mut connection = connect(&path, false).await;
    assert_ledger_checksums(&ledger(&mut connection).await, &CURRENT_VERSIONS);
    let original: (i64, i64) = sqlx::query_as(
        "SELECT rowid,revision FROM objects WHERE namespace='fresh-a' AND kind='formal_parent' AND id='parent'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(original.1, 1);
    assert_object_schema_and_constraints(&mut connection).await;
    connection.close().await.unwrap();

    let reopened = Store::open(&path).await.unwrap();
    let mut session = reopened.session().await.unwrap();
    assert_eq!(
        session
            .get::<Value>(&context, "formal_parent", "parent")
            .await
            .unwrap(),
        Some(parent)
    );
    assert_eq!(
        session
            .get::<Value>(&context, "formal_parent", "approved")
            .await
            .unwrap(),
        Some(approved)
    );
    let updated = approved_release_selection("parent");
    session
        .put(&context, "formal_parent", "parent", "new-owner", &updated)
        .await
        .unwrap();
    assert_eq!(
        session
            .get::<Value>(&context, "formal_parent", "parent")
            .await
            .unwrap(),
        Some(updated)
    );
    assert_eq!(
        session
            .get::<Value>(&other_namespace, "formal_parent", "parent")
            .await
            .unwrap(),
        Some(approved_release_selection("parent"))
    );
    session.commit().await.unwrap();
    reopened.close().await;
    let mut connection = connect(&path, false).await;
    let current: (i64, i64, String) = sqlx::query_as(
        "SELECT rowid,revision,owner FROM objects WHERE namespace='fresh-a' AND kind='formal_parent' AND id='parent'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(current, (original.0, 2, "new-owner".to_owned()));
    connection.close().await.unwrap();
}

#[tokio::test]
async fn historical_sqlx_ledger_upgrades_without_changing_rows_order_or_other_facts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rsia.sqlite3");
    let mut connection = historical_database(&path).await;
    seed_historical_facts(&mut connection).await;
    let before = snapshot(&mut connection).await;
    assert_ledger_checksums(&before.ledger, &PRIOR_VERSIONS);
    let error = sqlx::query(
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('legacy-a','formal_parent','parent','admin','{\"id\":\"parent\"}')",
    )
    .execute(&mut connection)
    .await
    .unwrap_err();
    assert!(matches!(error, sqlx::Error::Database(_)));
    assert_eq!(snapshot(&mut connection).await, before);
    connection.close().await.unwrap();

    // Exercise the product's normal upgrade entry with the real old ledger.
    let store = Store::open(&path).await.unwrap();
    let context = admin("legacy-a");
    let mut session = store.session().await.unwrap();
    let ordered = session.list::<Value>(&context, "artifact").await.unwrap();
    assert_eq!(
        ordered,
        before
            .objects
            .iter()
            .filter(|row| row.1 == "legacy-a" && row.2 == "artifact")
            .map(|row| serde_json::from_str::<Value>(&row.5).unwrap())
            .collect::<Vec<_>>()
    );
    session.commit().await.unwrap();

    let mut connection = connect(&path, false).await;
    let after = snapshot(&mut connection).await;
    assert_eq!(after.objects, before.objects);
    assert_eq!(after.object_indexes, before.object_indexes);
    assert_eq!(after.unrelated_schema, before.unrelated_schema);
    assert_eq!(after.unrelated_rows, before.unrelated_rows);
    assert_eq!(
        &after.ledger[..before.ledger.len()],
        before.ledger.as_slice()
    );
    assert_ledger_checksums(&after.ledger, &CURRENT_VERSIONS);
    assert_object_schema_and_constraints(&mut connection).await;
    connection.close().await.unwrap();

    let parent = empty_root_selection("parent");
    let mut session = store.session().await.unwrap();
    session
        .put(
            &context,
            "formal_parent",
            "parent",
            context.actor(),
            &parent,
        )
        .await
        .unwrap();
    let old_job = before
        .objects
        .iter()
        .find(|row| row.1 == "legacy-a" && row.2 == "job")
        .unwrap();
    session
        .put(
            &context,
            "job",
            "shared",
            "updated-owner",
            &json!({"id":"shared","state":"running"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    let mut connection = connect(&path, false).await;
    let job: (i64, i64, String) = sqlx::query_as(
        "SELECT rowid,revision,owner FROM objects WHERE namespace='legacy-a' AND kind='job' AND id='shared'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(job, (old_job.0, old_job.6 + 1, "updated-owner".to_owned()));
    let settled = snapshot(&mut connection).await;
    connection.close().await.unwrap();

    let reopened = Store::open(&path).await.unwrap();
    let mut session = reopened.session().await.unwrap();
    assert_eq!(
        session
            .get::<Value>(&context, "formal_parent", "parent")
            .await
            .unwrap(),
        Some(parent)
    );
    session.commit().await.unwrap();
    reopened.close().await;
    let mut connection = connect(&path, false).await;
    assert_eq!(snapshot(&mut connection).await, settled);
    assert_ledger_checksums(&settled.ledger, &CURRENT_VERSIONS);
    connection.close().await.unwrap();
}

#[tokio::test]
async fn historical_checksum_mismatch_refuses_upgrade_before_rebuilding_objects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rsia.sqlite3");
    let mut connection = historical_database(&path).await;
    seed_historical_facts(&mut connection).await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=x'00' WHERE version=1")
        .execute(&mut connection)
        .await
        .unwrap();
    let before = snapshot(&mut connection).await;
    assert_eq!(
        before.ledger.iter().map(|row| row.0).collect::<Vec<_>>(),
        PRIOR_VERSIONS
    );
    connection.close().await.unwrap();
    match Store::open(&path).await {
        Err(Error::Internal) => {}
        Err(error) => panic!("unexpected migration refusal: {error:?}"),
        Ok(store) => {
            store.close().await;
            panic!("historical checksum tampering must not be accepted");
        }
    }
    let mut connection = connect(&path, false).await;
    assert_eq!(snapshot(&mut connection).await, before);
    connection.close().await.unwrap();
}
