//! Old-API controls: both the healthy and malicious lanes compile against the
//! real parent API. Full physical zero-write observation is a separate gate.
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use serde_json::{Value, json};
use sqlx::{Column, Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;

fn admin() -> Context {
    Context::new("clock-ns", "clock-admin", Role::Admin).unwrap()
}

async fn connection(path: &Path, read_only: bool) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(read_only)
            .create_if_missing(false),
    )
    .await
    .unwrap()
}

async fn seed_audit(path: &Path) {
    let store = Store::open(path).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .audit(&admin(), "clock.control.seed", "seed")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(store.verify_audit(&admin()).await.unwrap(), 1);
    store.close().await;
}

async fn future_audit_with_valid_original_hash_chain(path: &Path) {
    let mut connection = connection(path, false).await;
    let row =
        sqlx::query("SELECT seq,payload,previous_hash FROM audit WHERE namespace=? ORDER BY seq")
            .bind(admin().namespace())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let mut payload: Value = serde_json::from_str(row.get::<&str, _>("payload")).unwrap();
    payload["timestamp"] = json!(i64::MAX);
    let raw = payload.to_string();
    let previous: String = row.get("previous_hash");
    let digest = hash(format!("{previous}\n{raw}").as_bytes());
    sqlx::query("UPDATE audit SET payload=?,digest=? WHERE namespace=? AND seq=?")
        .bind(raw)
        .bind(digest)
        .bind(admin().namespace())
        .bind(row.get::<i64, _>("seq"))
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

fn quoted_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Exact SQLite value text (including BLOBs and SQL NULL), in every physical
/// table. This is a logical control, not a substitute for native FS events.
async fn logical_rows(path: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    let mut connection = connection(path, true).await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    let mut result = Vec::new();
    for table in tables {
        let statement = format!("SELECT * FROM {} LIMIT 0", quoted_identifier(&table));
        let described = sqlx::Executor::describe(&mut connection, &statement)
            .await
            .unwrap();
        let projections: Vec<_> = described
            .columns()
            .iter()
            .map(|column| format!("quote({})", quoted_identifier(column.name())))
            .collect();
        let ordering = (1..=projections.len())
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let statement = format!(
            "SELECT {} FROM {} ORDER BY {}",
            projections.join(","),
            quoted_identifier(&table),
            ordering
        );
        let rows = sqlx::query(&statement)
            .fetch_all(&mut connection)
            .await
            .unwrap();
        result.push((
            table,
            rows.iter()
                .map(|row| {
                    (0..projections.len())
                        .map(|column| row.get::<String, _>(column))
                        .collect()
                })
                .collect(),
        ));
    }
    connection.close().await.unwrap();
    result
}

#[tokio::test]
async fn healthy_parent_control_reopens_and_reads_original_audit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite3");
    seed_audit(&path).await;
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.verify_audit(&admin()).await.unwrap(), 1);
    let mut session = store.session().await.unwrap();
    session
        .audit(&admin(), "clock.control.healthy", "healthy")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(store.verify_audit(&admin()).await.unwrap(), 2);
    store.close().await;
}

#[tokio::test]
async fn rechained_future_audit_refuses_before_staging_cleanup_or_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite3");
    seed_audit(&path).await;
    future_audit_with_valid_original_hash_chain(&path).await;
    let staging = directory
        .path()
        .join(".blob-staging")
        .join(hash(b"clock-ns"));
    std::fs::create_dir_all(&staging).unwrap();
    let orphan = staging.join("orphan.tmp");
    std::fs::write(&orphan, b"preserve-original-orphan").unwrap();
    let before = logical_rows(&path).await;
    match Store::open(&path).await {
        Err(Error::ClockRegressed) => {}
        Ok(store) => {
            store.close().await;
            panic!("future_audit_first_write: expected typed clock_regressed before cleanup");
        }
        Err(error) => panic!("future_audit_first_write: wrong refusal {error:?}"),
    }
    assert_eq!(std::fs::read(&orphan).unwrap(), b"preserve-original-orphan");
    assert_eq!(logical_rows(&path).await, before);
}

#[tokio::test]
async fn malformed_installed_on_refuses_without_rewriting_original_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite3");
    seed_audit(&path).await;
    let mut connection = connection(&path, false).await;
    sqlx::query("UPDATE _sqlx_migrations SET installed_on='not-a-clock' WHERE version=1")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let before = logical_rows(&path).await;
    match Store::open(&path).await {
        Err(Error::ClockRegressed) => {}
        Ok(store) => {
            store.close().await;
            panic!("installed_on_first_write: expected typed clock_regressed before migration");
        }
        Err(error) => panic!("installed_on_first_write: wrong refusal {error:?}"),
    }
    assert_eq!(logical_rows(&path).await, before);
}

#[tokio::test]
async fn caught_clock_refusal_terminates_every_session_write_and_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .audit(&admin(), "clock.control.seed", "seed")
        .await
        .unwrap();
    session.commit().await.unwrap();
    future_audit_with_valid_original_hash_chain(&path).await;
    let before = logical_rows(&path).await;
    let mut session = store.session().await.unwrap();
    assert!(
        matches!(
            session.audit(&admin(), "refused", "refused").await,
            Err(Error::ClockRegressed)
        ),
        "caught_clock_first_event"
    );
    assert!(matches!(
        session
            .put(
                &admin(),
                "note",
                "after-refusal",
                admin().actor(),
                &json!({"text":"must not persist"})
            )
            .await,
        Err(Error::ClockRegressed)
    ));
    assert!(matches!(
        session
            .stop_root_budget(&admin(), "absent-scope", "stop", 0)
            .await,
        Err(Error::ClockRegressed)
    ));
    assert!(matches!(
        session.delete(&admin(), "note", "after-refusal").await,
        Err(Error::ClockRegressed)
    ));
    assert!(matches!(
        session
            .direct_edges_page(&admin(), "artifact", "root", None, 1)
            .await,
        Err(Error::ClockRegressed)
    ));
    assert!(matches!(session.commit().await, Err(Error::ClockRegressed)));
    // A fresh read-only diagnostic is independent of the failed timed operation.
    assert_eq!(store.verify_audit(&admin()).await.unwrap(), 1);
    assert_eq!(logical_rows(&path).await, before);
    store.close().await;
}
