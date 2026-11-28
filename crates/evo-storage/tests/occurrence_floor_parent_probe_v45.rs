//! AG-132 two-tree counterexample. Only API that existed before the occurrence
//! floor, so it compiles on the parent tree and fails there on its assertions:
//! the parent opens a database whose `installed_on` is after the clock (P1) and
//! has no `occurrence_time_column_invalid` refusal (P2).

use evo_storage::Store;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use std::path::Path;

async fn reopen_after(dir: &Path, statement: &str) -> String {
    let path = dir.join("rsia.sqlite3");
    Store::open(&path).await.unwrap().close().await;
    let mut conn = SqliteConnectOptions::new()
        .filename(&path)
        .connect()
        .await
        .unwrap();
    sqlx::query(statement).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
    match Store::open(&path).await {
        Ok(store) => {
            store.close().await;
            panic!("PARENT-EXPECTED-FAILURE: opened after `{statement}`");
        }
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn p1_future_installed_on_refuses_open() {
    let dir = tempfile::tempdir().unwrap();
    let code = reopen_after(
        dir.path(),
        "UPDATE _sqlx_migrations SET installed_on='9999-12-31 23:59:59'",
    )
    .await;
    assert_eq!(code, "clock_regressed", "PARENT-EXPECTED-FAILURE");
}

#[tokio::test]
async fn p2_unparsable_installed_on_refuses_with_the_fixed_code() {
    let dir = tempfile::tempdir().unwrap();
    let code = reopen_after(
        dir.path(),
        "UPDATE _sqlx_migrations SET installed_on='not-a-clock'",
    )
    .await;
    assert_eq!(
        code, "occurrence_time_column_invalid",
        "PARENT-EXPECTED-FAILURE"
    );
}
