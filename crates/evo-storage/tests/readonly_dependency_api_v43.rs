#![forbid(unsafe_code)]

use rusqlite::config::DbConfig;
use sqlx::{ConnectOptions, Connection};

// Compatibility only: two independent in-memory connections, no file reader.
#[tokio::test]
async fn safe_no_checkpoint_config_roundtrip_matches_locked_sqlx() {
    let connection = rusqlite::Connection::open_in_memory().expect("rusqlite memory open");
    let option = DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE;
    assert!(!connection.db_config(option).expect("initial config"));
    assert!(connection.set_db_config(option, true).expect("set true"));
    assert!(connection.db_config(option).expect("get true"));
    assert!(
        !connection
            .set_db_config(option, false)
            .expect("restore false")
    );
    assert!(!connection.db_config(option).expect("get false"));
    let rusqlite_version: String = connection
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .expect("rusqlite linked version");
    let rusqlite_source: String = connection
        .query_row("SELECT sqlite_source_id()", [], |row| row.get(0))
        .expect("rusqlite linked source id");
    let mut sqlx_connection = sqlx::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .shared_cache(false)
        .connect()
        .await
        .expect("existing SQLx memory open");
    let sqlx_version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&mut sqlx_connection)
        .await
        .expect("SQLx linked version");
    let sqlx_source: String = sqlx::query_scalar("SELECT sqlite_source_id()")
        .fetch_one(&mut sqlx_connection)
        .await
        .expect("SQLx linked source id");
    println!(
        "AG150_LINKED_SQLITE rusqlite={rusqlite_version} sqlx={sqlx_version} source={rusqlite_source}"
    );
    assert_eq!(rusqlite_version, "3.46.0");
    assert_eq!(sqlx_version, "3.46.0");
    assert_eq!(rusqlite_source, sqlx_source);
    sqlx_connection.close().await.expect("SQLx memory close");
    connection
        .close()
        .unwrap_or_else(|(_, error)| panic!("rusqlite memory close: {error}"));
}
