use evo_core::{Context, Error, Role};
use evo_storage::Store;
use serde_json::{Value, json};
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

#[tokio::test]
async fn object_pages_reach_eof_beyond_the_old_collection_limit_and_keep_the_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("objects.sqlite3"))
        .await
        .unwrap();
    let ctx = Context::new("page-a", "reader", Role::Admin).unwrap();
    let other = Context::new("page-b", "reader", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    for i in 0..10_020 {
        let id = format!("object-{i:05}");
        let owner = format!("owner-{}", i % 3);
        session
            .put(
                &ctx,
                "artifact",
                &id,
                &owner,
                &json!({"id": id, "number": i}),
            )
            .await
            .unwrap();
    }
    session
        .put(
            &other,
            "artifact",
            "object-00000",
            "other-owner",
            &json!({"id": "object-00000", "number": -1}),
        )
        .await
        .unwrap();
    session
        .put(
            &ctx,
            "dataset",
            "object-00000",
            "other-kind",
            &json!({"id": "object-00000", "number": -2}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let mut session = store.session().await.unwrap();
    assert!(matches!(
        session.list::<Value>(&ctx, "artifact").await,
        Err(Error::Conflict(_))
    ));
    // The page API neither commits nor changes the caller's transaction.
    session
        .put(
            &ctx,
            "artifact",
            "uncommitted",
            "own-owner",
            &json!({"id": "uncommitted", "number": 10_020}),
        )
        .await
        .unwrap();
    let mut cursor = None;
    let mut actual = Vec::new();
    let mut last = None;
    loop {
        let rows = session
            .object_rows_page(&ctx, "artifact", cursor)
            .await
            .unwrap();
        if rows.is_empty() {
            break;
        }
        assert!(rows.len() <= 64);
        for row in &rows {
            assert!(last.is_none_or(|before| before < row.rowid));
            last = Some(row.rowid);
            let body: Value = serde_json::from_str(&row.body).unwrap();
            assert_eq!(body["id"], row.id);
            if row.id == "uncommitted" {
                assert_eq!(row.owner, "own-owner");
            } else {
                let number = body["number"].as_u64().unwrap();
                assert_eq!(row.owner, format!("owner-{}", number % 3));
            }
            actual.push(row.id.clone());
        }
        cursor = rows.last().map(|row| row.rowid);
    }
    assert_eq!(actual.len(), 10_021);
    let own = session
        .object_row(&ctx, "artifact", "object-00000")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(own.owner, "owner-0");
    let shadow = session
        .object_row(&other, "artifact", "object-00000")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shadow.owner, "other-owner");
    let kind = session
        .object_row(&ctx, "dataset", "object-00000")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind.owner, "other-kind");
    // Drop rolls back only our explicit insert. The SELECT helpers did not
    // commit it, even after >10,000 rows and a complete EOF scan.
    drop(session);
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .object_row(&ctx, "artifact", "uncommitted")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn object_pages_preserve_raw_duplicate_keys_and_negative_rowids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("raw.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let ctx = Context::new("raw-page", "reader", Role::Admin).unwrap();
    // A historical/corrupt SQL object can contain duplicate keys even though
    // normal serde_json writers cannot produce them. Consumers need raw bytes.
    let raw = r#"{"id":"raw-row","record_kind":"first","record_kind":"second","payload":{"key":1,"key":2}}"#;
    let mut sql = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("INSERT INTO objects(rowid,namespace,kind,id,owner,body) VALUES(-9,?,'artifact','raw-row','actual-sql-owner',?)")
        .bind(ctx.namespace()).bind(raw).execute(&mut sql).await.unwrap();
    sql.close().await.unwrap();
    let mut session = store.session().await.unwrap();
    let first = session
        .object_rows_page(&ctx, "artifact", None)
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].rowid, -9);
    assert_eq!(first[0].owner, "actual-sql-owner");
    assert_eq!(first[0].body, raw);
    assert!(
        session
            .object_rows_page(&ctx, "artifact", Some(-9))
            .await
            .unwrap()
            .is_empty()
    );
    let exact = session
        .object_row(&ctx, "artifact", "raw-row")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exact.body, raw);
    assert_eq!(exact.owner, "actual-sql-owner");
    assert!(
        session
            .object_row(&ctx, "dataset", "raw-row")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
    store.close().await;
}
