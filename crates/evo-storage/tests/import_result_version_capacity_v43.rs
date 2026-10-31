//! AG-148 (v4.5 §3.3.2, E16.5): legacy v1 and current v2 import results share
//! the one instance event capacity of 10 000, counted across namespaces; a new
//! result only ever lands as v2 through the bounded writer.
use evo_core::{Context, Error, Role};
use evo_storage::{CAPACITY_IMPORT_RESULT_SCHEMA, CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1, Store};
use serde_json::{Value, json};

const LIMIT: u64 = 10_000;

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

fn result(schema: &str, id: &str, events: Value) -> Value {
    json!({"schema_version": schema, "id": id, "payload": {"aggregate_summary": {"total_events": events}}})
}

async fn database() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    (directory, store)
}

async fn put(store: &Store, ctx: &Context, kind: &str, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session.put(ctx, kind, id, ctx.actor(), body).await.unwrap();
    session.commit().await.unwrap();
}

async fn create(store: &Store, ctx: &Context, id: &str, events: u64) -> evo_core::Result<()> {
    let mut session = store.session().await.unwrap();
    session
        .put_new_import_result_below_event_capacity(
            ctx,
            id,
            ctx.actor(),
            &result(CAPACITY_IMPORT_RESULT_SCHEMA, id, json!(events)),
            events,
            LIMIT,
        )
        .await?;
    session.commit().await
}

async fn used(store: &Store) -> u64 {
    let mut session = store.session().await.unwrap();
    let usage = session.capacity_usage_v41(0).await.unwrap();
    session.commit().await.unwrap();
    usage.events
}

#[track_caller]
fn exceeded(outcome: evo_core::Result<()>, used: u64, delta: u64) {
    let expected = format!("event capacity exceeded: {used} + {delta} > {LIMIT}");
    assert!(
        matches!(&outcome, Err(Error::Conflict(message)) if *message == expected),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn result_versions_share_one_event_capacity() {
    let (_directory, store) = database().await;
    let ctx = admin("n");
    put(
        &store,
        &ctx,
        "artifact",
        "legacy",
        &result(
            CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1,
            "legacy",
            json!(6_000),
        ),
    )
    .await;
    // An `event` object counts with the results, as in capacity_usage_v41.
    put(&store, &ctx, "event", "event-1", &json!({"id": "event-1"})).await;
    create(&store, &ctx, "current", 3_999).await.unwrap();
    assert_eq!(used(&store).await, LIMIT);
    exceeded(create(&store, &ctx, "one-more", 1).await, LIMIT, 1);
    // The writer creates only a current result whose declared count is the delta.
    for (body, delta) in [
        (
            result(CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1, "v1", json!(0)),
            0,
        ),
        (result(CAPACITY_IMPORT_RESULT_SCHEMA, "v2", json!(2)), 1),
    ] {
        let mut session = store.session().await.unwrap();
        let outcome = session
            .put_new_import_result_below_event_capacity(&ctx, "new", "admin", &body, delta, LIMIT)
            .await;
        assert!(
            matches!(&outcome, Err(Error::Invalid(message)) if message == "created object does not match its capacity schema or delta"),
            "{outcome:?}"
        );
    }
    let mut session = store.session().await.unwrap();
    let outcome = session
        .put_new_import_result_below_event_capacity(
            &ctx,
            "current",
            "admin",
            &result(CAPACITY_IMPORT_RESULT_SCHEMA, "current", json!(0)),
            0,
            LIMIT,
        )
        .await;
    assert!(
        matches!(&outcome, Err(Error::Conflict(message)) if message == "bounded creation requires a new object identity"),
        "{outcome:?}"
    );
    drop(session);
    // Freeing legacy events makes room for a new current result.
    put(
        &store,
        &ctx,
        "artifact",
        "legacy",
        &result(
            CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1,
            "legacy",
            json!(5_999),
        ),
    )
    .await;
    create(&store, &ctx, "one-more", 1).await.unwrap();
    assert_eq!(used(&store).await, LIMIT);
}

#[tokio::test]
async fn result_capacity_spans_namespaces() {
    let (directory, store) = database().await;
    let (first, second, third) = (admin("first"), admin("second"), admin("third"));
    put(
        &store,
        &first,
        "artifact",
        "legacy",
        &result(
            CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1,
            "legacy",
            json!(5_000),
        ),
    )
    .await;
    create(&store, &second, "current", 5_000).await.unwrap();
    exceeded(create(&store, &third, "elsewhere", 1).await, LIMIT, 1);
    // The count is instance wide; reads stay namespace scoped.
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .get::<Value>(&third, "artifact", "legacy")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .get::<Value>(&third, "artifact", "current")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
    store.close().await;
    let reopened = Store::open(&directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(used(&reopened).await, LIMIT);
    exceeded(create(&reopened, &third, "elsewhere", 1).await, LIMIT, 1);
    let mut session = reopened.session().await.unwrap();
    assert!(
        session
            .get::<Value>(&third, "artifact", "elsewhere")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn invalid_versioned_event_count_fails_closed() {
    let (_directory, store) = database().await;
    let (other, ctx) = (admin("other"), admin("n"));
    for schema in [
        CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1,
        CAPACITY_IMPORT_RESULT_SCHEMA,
    ] {
        for invalid in [json!(null), json!(-1), json!("7"), json!(1.5), json!(true)] {
            put(
                &store,
                &other,
                "artifact",
                "bad",
                &result(schema, "bad", invalid.clone()),
            )
            .await;
            let outcome = create(&store, &ctx, "new", 1).await;
            assert!(
                matches!(&outcome, Err(Error::Conflict(message)) if message == "existing capacity records contain an invalid numeric fact"),
                "{schema} {invalid}: {outcome:?}"
            );
            let mut session = store.session().await.unwrap();
            assert!(
                session
                    .get::<Value>(&ctx, "artifact", "new")
                    .await
                    .unwrap()
                    .is_none()
            );
            session.commit().await.unwrap();
        }
        let mut session = store.session().await.unwrap();
        let mut missing = result(schema, "bad", json!(0));
        missing["payload"]
            .as_object_mut()
            .unwrap()
            .remove("aggregate_summary");
        session
            .put(&other, "artifact", "bad", "admin", &missing)
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(
            create(&store, &ctx, "new", 1).await,
            Err(Error::Conflict(message)) if message == "existing capacity records contain an invalid numeric fact"
        ));
    }
    // A valid zero count is a legal fact.
    put(
        &store,
        &other,
        "artifact",
        "bad",
        &result(CAPACITY_IMPORT_RESULT_SCHEMA, "bad", json!(0)),
    )
    .await;
    create(&store, &ctx, "new", 1).await.unwrap();
    assert_eq!(used(&store).await, 1);
}
