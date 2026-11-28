//! AG-162 two-tree probes (plan v4.5 L823, §11.3.1, E08). They use only API
//! that predates AG-162 and read every new fact as JSON, so they compile on
//! the parent tree, where both names are registered but not wired and each
//! probe fails with `PARENT-EXPECTED-FAILURE`:
//! - CPP-A (persistent facts): `source.revoke` writes the tombstone, raises
//!   the watermark and opens a pending cleanup with its three frontier rows,
//!   and the job result names that cleanup;
//! - CPP-B (semantics): `cleanup.continue` advances exactly one page, a
//!   same-key replay changes nothing, a new key continues, and every page
//!   equals the old `cleanup_step` on a twin store.
#[path = "support/revocation_management.rs"]
mod revocation_management;

use revocation_management::{Fixture, OLD_NOW, continue_payload, revoke_payload};
use serde_json::json;

#[tokio::test]
async fn cpp_a_source_revoke_commits_the_revocation_and_names_its_cleanup() {
    // Healthy control: the old storage API on a twin store.
    let twin = Fixture::new().await;
    twin.run("cpp-a-source").await;
    let control = twin.old_revoke("cpp-a-source").await;
    assert_eq!(control.pending_nodes, 3);

    let fixture = Fixture::new().await;
    fixture.run("cpp-a-source").await;
    let before = fixture.snapshot();
    assert_eq!(before["watermark"], json!([]));
    let status = fixture
        .run_job(
            "source.revoke",
            revoke_payload("cpp-a", "run", "cpp-a-source"),
        )
        .await;
    assert_eq!(status["state"], json!("succeeded"), "{status}");
    let after = fixture.snapshot();
    let seq = control.watermark_seq;
    assert_eq!(after["watermark"], json!([[seq]]));
    assert_eq!(
        after["tombstones"],
        json!([["cpp-a-source", "run", "management_source_revoke", seq]])
    );
    let jobs = after["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    let cleanup_job_id = jobs[0][0].clone();
    assert_eq!(
        jobs[0],
        json!([cleanup_job_id, "run", "cpp-a-source", seq, "pending", 0])
    );
    let frontier: Vec<_> = after["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row[1].clone(),
                row[2].clone(),
                row[3].clone(),
                row[4].clone(),
            )
        })
        .collect();
    assert_eq!(
        frontier,
        [
            (
                cleanup_job_id.clone(),
                json!("__budget_scan"),
                json!("cpp-a-source"),
                json!(0)
            ),
            (
                cleanup_job_id.clone(),
                json!("__world_scan"),
                json!("cpp-a-source"),
                json!(0)
            ),
            (
                cleanup_job_id.clone(),
                json!("run"),
                json!("cpp-a-source"),
                json!(0)
            ),
        ]
    );
    assert_eq!(
        after["events"],
        json!([[cleanup_job_id.clone(), "logical_block_committed"]])
    );
    let result = &status["result"];
    assert_eq!(result["result"], json!("source_revoked"));
    assert_eq!(result["cleanup_job_id"], cleanup_job_id);
    assert_eq!(result["source_kind"], json!("run"));
    assert_eq!(result["source_id"], json!("cpp-a-source"));
    assert_eq!(result["watermark_seq"], json!(seq));
    assert_eq!(result["cleanup_state"], json!("pending"));
}

#[tokio::test]
async fn cpp_b_cleanup_continue_pages_once_replays_by_key_and_matches_the_old_step() {
    let fixture = Fixture::new().await;
    let twin = Fixture::new().await;
    fixture.graph("cpp-b-source").await;
    twin.graph("cpp-b-source").await;
    let cleanup = fixture.old_revoke("cpp-b-source").await;
    assert_eq!(twin.old_revoke("cpp-b-source").await.job_id, cleanup.job_id);
    assert_eq!(fixture.snapshot(), twin.snapshot());

    let first = fixture
        .run_job(
            "cleanup.continue",
            continue_payload("cpp-b-1", &cleanup.job_id, 1),
        )
        .await;
    assert_eq!(first["state"], json!("succeeded"), "{first}");
    let page = twin.old_step(&cleanup.job_id, 1, OLD_NOW + 1).await;
    let after_first = fixture.snapshot();
    assert_eq!(after_first, twin.snapshot(), "one page equals the old step");
    assert_eq!(first["result"]["result"], json!("cleanup_continued"));
    assert_eq!(first["result"]["cleanup_job_id"], json!(cleanup.job_id));
    assert_eq!(
        first["result"]["processed_nodes"],
        json!(page.processed_nodes)
    );
    assert_eq!(first["result"]["pending_nodes"], json!(page.pending_nodes));
    assert_eq!(first["result"]["cleanup_state"], json!("running"));
    assert!(page.pending_nodes > 0, "the graph needs more than one page");

    // The same key and payload replay the original job: nothing changes.
    let replay = fixture
        .submit(
            "cleanup.continue",
            continue_payload("cpp-b-1", &cleanup.job_id, 1),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, first["id"].as_str().unwrap());
    assert_eq!(fixture.snapshot(), after_first);

    // A new key continues with the next page, again equal to the old step.
    let second = fixture
        .run_job(
            "cleanup.continue",
            continue_payload("cpp-b-2", &cleanup.job_id, 1),
        )
        .await;
    assert_eq!(second["state"], json!("succeeded"), "{second}");
    let page = twin.old_step(&cleanup.job_id, 1, OLD_NOW + 2).await;
    assert_ne!(fixture.snapshot(), after_first);
    assert_eq!(fixture.snapshot(), twin.snapshot());
    assert_eq!(
        second["result"]["processed_nodes"],
        json!(page.processed_nodes)
    );
    assert_eq!(second["result"]["pending_nodes"], json!(page.pending_nodes));
}
