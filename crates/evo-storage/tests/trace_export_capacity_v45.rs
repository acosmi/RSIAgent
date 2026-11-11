//! AG-203: the instance-wide trace export bound must be visible through the
//! shared capacity API, not only through the producer's private write gate.
//!
//! A trace export record keeps its slot after a lawful redaction, so the
//! counter is cross-namespace and includes redacted history. The tests write
//! through the real `Session::put` gate and never fabricate stored rows.

use evo_core::capacity::CapacityItem;
use evo_core::{Context, Role};
use evo_storage::{Store, TRACE_EXPORT_SCHEMA};
use serde_json::{Value, json};

const DATABASE: &str = "trace-export-capacity.sqlite3";
const LIMIT: usize = 1_000;

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

    let admitted = session
        .admit_capacity(&[(CapacityItem::TraceExport, 1)])
        .await;
    assert!(
        admitted.is_err(),
        "PARENT-EXPECTED-FAILURE: admit_capacity must refuse past the trace export instance limit"
    );
    let conflict = admitted.unwrap_err();
    assert!(
        format!("{conflict}").contains("capacity_conflict"),
        "{conflict}"
    );

    let export_id = "export-over-limit";
    let over = session
        .put(
            &ctx,
            "artifact",
            &format!("trace-export-record-{export_id}"),
            ctx.actor(),
            &record(export_id),
        )
        .await;
    assert!(
        over.is_err(),
        "PARENT-EXPECTED-FAILURE: put must refuse a new trace export past the instance limit"
    );
    let conflict = over.unwrap_err();
    assert!(
        format!("{conflict}").contains("MVP capacity exceeded"),
        "{conflict}"
    );
}
