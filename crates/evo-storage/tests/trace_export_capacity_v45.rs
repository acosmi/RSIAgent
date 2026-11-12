//! AG-203/AG-205: the instance-wide trace export bound (§3.3.2) is the shared
//! `CapacityItem::TraceExport` measurement, the one gate behind `capacity_used`,
//! `admit_capacity` and the producer's own `Session::put`, which refuses with
//! the typed capacity conflict.
//!
//! The counter is cross-namespace and covers every record except one redacted
//! by a completed revocation cleanup. The tests write through the real
//! `Session::put` gate and the real revocation lifecycle and never fabricate
//! stored rows.

use evo_core::capacity::{CapacityConflict, CapacityItem};
use evo_core::{Context, Error, Role};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use evo_storage::{Store, TRACE_EXPORT_SCHEMA};
use serde_json::{Value, json};

const DATABASE: &str = "trace-export-capacity.sqlite3";
const LIMIT: usize = CapacityItem::TraceExport.limit() as usize;

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join(DATABASE)).await.unwrap();
    (dir, store)
}

fn record(export_id: &str) -> Value {
    json!({
        "schema_version": TRACE_EXPORT_SCHEMA,
        "id": format!("trace-export-record-{export_id}"),
        "export_id": export_id,
        "phase": "reviewed",
        "prepare_job_id": "management-job-prepare",
        "publish_job_id": "management-job-publish",
        "candidate_digest": "0".repeat(64),
        "members": [],
        "delivery": null,
        "local_cost": {"wall_seconds": 0, "bytes": 0},
        "source_snapshots": [],
        "owner_actor": "admin",
        "created_at": 1,
        "updated_at": 2,
    })
}

/// A new trace export at the full bound is exactly the typed capacity conflict.
fn assert_at_limit(refused: Result<(), Error>) {
    match refused {
        Err(Error::Capacity(conflict)) => assert_eq!(
            conflict,
            CapacityConflict {
                item: CapacityItem::TraceExport,
                used: LIMIT as u64,
                limit: LIMIT as u64,
                delta: 1,
            }
        ),
        other => {
            panic!("a new trace export at the limit must be a typed capacity conflict: {other:?}")
        }
    }
}

#[tokio::test]
async fn capacity_used_counts_trace_exports_across_namespaces() {
    let (_dir, store) = store().await;
    let mut session = store.session().await.unwrap();
    for (namespace, count) in [("tenant-a", 3usize), ("tenant-b", 2usize)] {
        let ctx = admin(namespace);
        for index in 0..count {
            let export_id = format!("export-{namespace}-{index}");
            session
                .put(
                    &ctx,
                    "artifact",
                    &format!("trace-export-record-{export_id}"),
                    ctx.actor(),
                    &record(&export_id),
                )
                .await
                .unwrap();
        }
    }
    assert_eq!(
        session
            .capacity_used(CapacityItem::TraceExport)
            .await
            .unwrap(),
        5,
        "PARENT-EXPECTED-FAILURE: capacity_used(TraceExport) must count real records across namespaces"
    );
    session
        .admit_capacity(&[(CapacityItem::TraceExport, 1)])
        .await
        .unwrap();
}

#[tokio::test]
async fn trace_export_capacity_is_enforced_at_the_instance_limit() {
    let (_dir, store) = store().await;
    let mut session = store.session().await.unwrap();
    let ctx = admin("tenant-a");
    for index in 0..LIMIT {
        let export_id = format!("export-{index}");
        session
            .put(
                &ctx,
                "artifact",
                &format!("trace-export-record-{export_id}"),
                ctx.actor(),
                &record(&export_id),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        session
            .capacity_used(CapacityItem::TraceExport)
            .await
            .unwrap(),
        LIMIT as u64,
        "PARENT-EXPECTED-FAILURE: capacity_used(TraceExport) must reach the real instance count"
    );

    assert_at_limit(
        session
            .admit_capacity(&[(CapacityItem::TraceExport, 1)])
            .await,
    );

    let export_id = "export-over-limit";
    assert_at_limit(
        session
            .put(
                &ctx,
                "artifact",
                &format!("trace-export-record-{export_id}"),
                ctx.actor(),
                &record(export_id),
            )
            .await,
    );
}

/// §3.3.2: a record redacted by a completed revocation cleanup stops counting
/// and nothing is deleted to make room. At the full bound the one freed slot
/// admits exactly one new record, here from another namespace.
#[tokio::test]
async fn lawful_revocation_cleanup_frees_one_trace_export_slot() {
    let (_dir, store) = store().await;
    let ctx = admin("tenant-a");
    let other = admin("tenant-b");
    let revoked = "trace-export-record-export-0";
    let mut session = store.session().await.unwrap();
    // The first record names a real run: the dependency a revocation follows.
    session
        .put(
            &ctx,
            "run",
            "source-run",
            ctx.actor(),
            &json!({"id": "source-run", "facts": "captured service facts"}),
        )
        .await
        .unwrap();
    for index in 0..LIMIT {
        let export_id = format!("export-{index}");
        session
            .put(
                &ctx,
                "artifact",
                &format!("trace-export-record-{export_id}"),
                ctx.actor(),
                &record(&export_id),
            )
            .await
            .unwrap();
    }
    session
        .put_edge(&ctx, "artifact", revoked, "run", "source-run")
        .await
        .unwrap();
    assert_eq!(
        session
            .capacity_used(CapacityItem::TraceExport)
            .await
            .unwrap(),
        LIMIT as u64
    );
    assert_at_limit(
        session
            .admit_capacity(&[(CapacityItem::TraceExport, 1)])
            .await,
    );
    session.commit().await.unwrap();

    let mut status = LifecycleStore::begin_revoke(
        &ctx,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-run".into(),
        },
        "trace export source revoked",
        10,
    )
    .await
    .unwrap();
    for now in 11..200 {
        if status.state == CleanupState::Complete {
            break;
        }
        assert_ne!(status.state, CleanupState::Failed, "{status:?}");
        status = LifecycleStore::cleanup_step(&ctx, &store, &status.job_id, 8, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");

    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .capacity_used(CapacityItem::TraceExport)
            .await
            .unwrap(),
        LIMIT as u64 - 1,
        "PARENT-EXPECTED-FAILURE: a lawfully redacted trace export record must stop counting"
    );
    let history = session
        .object_row(&ctx, "artifact", revoked)
        .await
        .unwrap()
        .expect("the redacted history row is kept, not deleted to free the slot");
    let history: Value = serde_json::from_str(&history.body).unwrap();
    assert_eq!(history["schema_version"], "rsia.redacted.v1");
    session
        .put(
            &other,
            "artifact",
            "trace-export-record-export-new",
            other.actor(),
            &record("export-new"),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .capacity_used(CapacityItem::TraceExport)
            .await
            .unwrap(),
        LIMIT as u64
    );
    assert_at_limit(
        session
            .put(
                &ctx,
                "artifact",
                "trace-export-record-export-late",
                ctx.actor(),
                &record("export-late"),
            )
            .await,
    );
    session.commit().await.unwrap();
}
