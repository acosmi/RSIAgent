use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}
async fn db() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}
fn envelope(schema: &str, id: &str, refs: Value, payload: Value, watermark: u64) -> Value {
    json!({"schema_version":schema,"id":id,"namespace":"n","owner_actor":"admin","request_key":format!("request-{id}"),"input_digest":hash(format!("input-{id}").as_bytes()),"created_at":1,"updated_at":1,"source_refs":refs,"revoke_watermark":watermark,"payload":payload})
}

#[tokio::test]
async fn registered_import_blob_over_one_mib_is_bounded_restart_readable_and_role_scoped() {
    let (dir, store) = db().await;
    let ctx = admin();
    let bytes = vec![b'x'; 1024 * 1024 + 1];
    let digest = hash(&bytes);
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&ctx, &hash(b"wm")).await.unwrap();
    let source = envelope(
        "rsia.e16.import_source.v1",
        "source",
        json!([{"kind":"blob","id":digest,"content_digest":digest}]),
        json!({"status":"prepared","raw_blob_digest":digest,"blob_published":false}),
        1,
    );
    session
        .put(&ctx, "artifact", "source", ctx.actor(), &source)
        .await
        .unwrap();
    session
        .put_edge(&ctx, "artifact", "source", "blob", &digest)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        store
            .publish_registered_blob(
                &ctx,
                "source",
                "rsia.e16.import_source.v1",
                "raw_blob_digest",
                &bytes,
                64 * 1024 * 1024
            )
            .await
            .unwrap(),
        digest
    );
    store.close().await;
    let reopened = Store::open(&dir.path().join("store.sqlite3"))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .read_blob(&ctx, &digest, 64 * 1024 * 1024)
            .await
            .unwrap(),
        bytes
    );
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    assert!(matches!(
        reopened.read_blob(&agent, &digest, 64 * 1024 * 1024).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn persisted_event_capacity_rejects_unknown_usage_and_keeps_zero_legal_after_restart() {
    let (dir, store) = db().await;
    let ctx = admin();
    let schema = "rsia.e16.import_result.v1";
    let mut session = store.session().await.unwrap();
    session
        .put(
            &ctx,
            "artifact",
            "unknown",
            "admin",
            &json!({"id":"unknown","schema_version":schema,"payload":{"aggregate_summary":{}}}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    let reopened = Store::open(&dir.path().join("store.sqlite3"))
        .await
        .unwrap();
    let mut session = reopened.session().await.unwrap();
    let outcome=session.put_new_below_json_sum_capacity(&ctx,"artifact","new","admin",&json!({"id":"new","schema_version":schema,"payload":{"aggregate_summary":{"total_events":10000}}}),schema,"$.payload.aggregate_summary.total_events",10000,10000).await;
    assert!(matches!(outcome, Err(Error::Conflict(_))));
    session.delete(&ctx, "artifact", "unknown").await.unwrap();
    session.put(&ctx,"artifact","zero","admin",&json!({"id":"zero","schema_version":schema,"payload":{"aggregate_summary":{"total_events":0}}})).await.unwrap();
    session.commit().await.unwrap();
    let mut session = reopened.session().await.unwrap();
    session.put_new_below_json_sum_capacity(&ctx,"artifact","full","admin",&json!({"id":"full","schema_version":schema,"payload":{"aggregate_summary":{"total_events":10000}}}),schema,"$.payload.aggregate_summary.total_events",10000,10000).await.unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn shared_import_blob_is_kept_until_last_live_source_is_revoked() {
    let (_dir, store) = db().await;
    let ctx = admin();
    let digest = store.put_blob(&ctx, b"SHARED-SECRET").await.unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&ctx, &hash(b"wm")).await.unwrap();
    for id in ["shared-one", "shared-two"] {
        let source = envelope(
            "rsia.e16.import_source.v1",
            id,
            json!([{"kind":"blob","id":digest,"content_digest":digest}]),
            json!({"status":"ready","raw_blob_digest":digest,"blob_published":true}),
            1,
        );
        session
            .put(&ctx, "artifact", id, "admin", &source)
            .await
            .unwrap();
        session
            .put_edge(&ctx, "artifact", id, "blob", &digest)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    for (id, now, exists) in [("shared-two", 2, true), ("shared-one", 20, false)] {
        let mut status = LifecycleStore::begin_revoke(
            &ctx,
            &store,
            TypedObjectRef {
                kind: "artifact".into(),
                id: id.into(),
            },
            "delete",
            now,
        )
        .await
        .unwrap();
        for step in now + 1..now + 15 {
            status = LifecycleStore::cleanup_step(&ctx, &store, &status.job_id, 16, step)
                .await
                .unwrap();
            if status.pending_nodes == 0 {
                break;
            }
        }
        assert_eq!(status.state, CleanupState::Complete);
        assert_eq!(store.read_blob(&ctx, &digest, 1024).await.is_ok(), exists);
    }
}

#[tokio::test]
async fn second_import_source_revoke_redacts_full_closure_and_deletes_only_its_blob() {
    let (_dir, store) = db().await;
    let ctx = admin();
    let first = store.put_blob(&ctx, b"FIRST-SECRET").await.unwrap();
    let second = store.put_blob(&ctx, b"SECOND-SECRET").await.unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&ctx, &hash(b"wm")).await.unwrap();
    let s1 = envelope(
        "rsia.e16.import_source.v1",
        "source-one",
        json!([{"kind":"blob","id":first,"content_digest":first}]),
        json!({"status":"ready","raw_blob_digest":first,"blob_published":true}),
        1,
    );
    let s2 = envelope(
        "rsia.e16.import_source.v1",
        "source-two",
        json!([{"kind":"blob","id":second,"content_digest":second}]),
        json!({"status":"ready","raw_blob_digest":second,"blob_published":true}),
        1,
    );
    let selection = envelope(
        "rsia.e16.source_selection.v1",
        "selection",
        json!([{"kind":"artifact","id":"source-one","content_digest":fingerprint(&s1).unwrap()},{"kind":"artifact","id":"source-two","content_digest":fingerprint(&s2).unwrap()}]),
        json!({"secret":"SELECTION-SECRET"}),
        1,
    );
    let result = envelope(
        "rsia.e16.import_result.v1",
        "result",
        json!([{"kind":"artifact","id":"selection","content_digest":fingerprint(&selection).unwrap()},{"kind":"artifact","id":"source-one","content_digest":fingerprint(&s1).unwrap()},{"kind":"artifact","id":"source-two","content_digest":fingerprint(&s2).unwrap()}]),
        json!({"aggregate_summary":{"total_events":2},"secret":"RESULT-SECRET"}),
        1,
    );
    for (id, value) in [
        ("source-one", s1),
        ("source-two", s2),
        ("selection", selection),
        ("result", result),
    ] {
        session
            .put(&ctx, "artifact", id, "admin", &value)
            .await
            .unwrap();
    }
    for (id, digest) in [("source-one", &first), ("source-two", &second)] {
        session
            .put_edge(&ctx, "artifact", id, "blob", digest)
            .await
            .unwrap();
        session
            .put_edge(&ctx, "artifact", "selection", "artifact", id)
            .await
            .unwrap();
        session
            .put_edge(&ctx, "artifact", "result", "artifact", id)
            .await
            .unwrap();
    }
    session
        .put_edge(&ctx, "artifact", "result", "artifact", "selection")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &ctx,
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "source-two".into(),
        },
        "delete source two",
        2,
    )
    .await
    .unwrap();
    for now in 3..30 {
        status = LifecycleStore::cleanup_step(&ctx, &store, &status.job_id, 16, now)
            .await
            .unwrap();
        if status.pending_nodes == 0 {
            break;
        }
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert_eq!(
        store.read_blob(&ctx, &first, 1024).await.unwrap(),
        b"FIRST-SECRET"
    );
    assert!(matches!(
        store.read_blob(&ctx, &second, 1024).await,
        Err(Error::NotFound)
    ));
    let mut session = store.session().await.unwrap();
    for id in ["source-two", "selection", "result"] {
        let value: Value = session.need(&ctx, "artifact", id).await.unwrap();
        assert_eq!(value["schema_version"], "rsia.redacted.v1");
        assert!(!value.to_string().contains("SECRET"));
    }
    let unchanged: Value = session.need(&ctx, "artifact", "source-one").await.unwrap();
    assert_eq!(unchanged["schema_version"], "rsia.e16.import_source.v1");
    session.commit().await.unwrap();
}
