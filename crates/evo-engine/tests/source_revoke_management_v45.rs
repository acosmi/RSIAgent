//! AG-162 (plan v4.5 L799–L806, L823, §11.3, §11.3.1, E08, V100.a, V101.b,
//! V102.a/b/c, V017): `source.revoke` is wired as an Admin async job. One
//! transaction raises the watermark, writes the tombstone, the pending cleanup
//! row with its frontier and event, and the terminal job state. The reason is
//! the fixed `management_source_revoke`, never wire text. Revoking an already
//! revoked source writes nothing and returns its existing cleanup job.
//!
//! R10 Draining rule: the job makes no budget call and commits in one
//! transaction, so the handler never sets Draining itself; every status poll
//! of a job that runs asserts it, and a restarted Running job fails closed. A
//! cancel of a claimed job passes through Draining on the generic cancel path
//! and recovery lands it on Cancelled with no business write.
#[path = "support/revocation_management.rs"]
mod revocation_management;

use evo_core::{Error, Role};
use evo_engine::dispatch::{
    ManagementCancelArrival, ManagementJobState, ManagementResultV2, ManagementRevocationResultV2,
    ManagementRevocationSourceKind,
};
use evo_storage::lifecycle::CleanupState;
use revocation_management::{
    Fixture, REVOKE_SCHEMA, context, continue_payload, fact, import_source, revoke_payload,
    seeded_job,
};
use serde_json::{Value, json};

const OPERATION: &str = "source.revoke";

/// The typed result of a succeeded revocation job.
fn revoked(
    status: &Value,
) -> (
    ManagementRevocationSourceKind,
    String,
    String,
    u64,
    CleanupState,
) {
    assert_eq!(status["state"], json!("succeeded"), "{status}");
    assert_eq!(status["step"], json!("source_revoked"));
    assert_eq!(status["error_code"], Value::Null);
    let Some(ManagementResultV2::Revocation(ManagementRevocationResultV2::SourceRevoked {
        source_kind,
        source_id,
        cleanup_job_id,
        watermark_seq,
        cleanup_state,
    })) = serde_json::from_value(status["result"].clone()).unwrap()
    else {
        panic!("expected a source revocation result: {status}")
    };
    (
        source_kind,
        source_id,
        cleanup_job_id,
        watermark_seq,
        cleanup_state,
    )
}

#[tokio::test]
async fn the_revocation_commits_its_facts_and_names_the_cleanup() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let status = fixture
        .run_job(OPERATION, revoke_payload("revoke-1", "run", "source"))
        .await;
    let (kind, id, cleanup_job_id, seq, state) = revoked(&status);
    assert_eq!(
        (kind, id.as_str()),
        (ManagementRevocationSourceKind::Run, "source")
    );
    assert_eq!(state, CleanupState::Pending);
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot["watermark"], json!([[seq]]));
    assert_eq!(
        snapshot["tombstones"],
        json!([["source", "run", "management_source_revoke", seq]])
    );
    assert_eq!(
        snapshot["jobs"],
        json!([[cleanup_job_id, "run", "source", seq, "pending", 0]])
    );
    assert_eq!(snapshot["frontier"].as_array().unwrap().len(), 3);
    assert_eq!(
        serde_json::to_value(&status["committed_side_effects"]).unwrap(),
        json!([{
            "kind": "source_revoked",
            "source_kind": "run",
            "source_id": "source",
            "cleanup_job_id": cleanup_job_id,
            "watermark_seq": seq,
        }])
    );
    // The result carries no reason, digest or diagnostic text.
    let text = status["result"].to_string();
    assert!(!text.contains("management_source_revoke") && !text.contains("digest"));
}

#[tokio::test]
async fn a_replay_or_a_new_key_never_raises_the_watermark_twice() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let original = fixture
        .run_job(OPERATION, revoke_payload("revoke-1", "run", "source"))
        .await;
    let (_, _, cleanup_job_id, seq, _) = revoked(&original);
    let after = fixture.snapshot();

    let replay = fixture
        .submit(OPERATION, revoke_payload("revoke-1", "run", "source"))
        .await
        .unwrap();
    assert_eq!(replay.identity().id, original["id"].as_str().unwrap());
    assert_eq!(fixture.snapshot(), after);

    // Another key revokes the same source again: the existing cleanup comes
    // back and nothing of the revocation is written.
    let again = fixture
        .run_job(OPERATION, revoke_payload("revoke-2", "run", "source"))
        .await;
    assert_ne!(again["id"], original["id"]);
    let (_, _, again_job, again_seq, _) = revoked(&again);
    assert_eq!((again_job, again_seq), (cleanup_job_id, seq));
    assert_eq!(fixture.snapshot(), after);
}

#[tokio::test]
async fn a_missing_source_or_a_kind_conflict_writes_no_revocation() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    let missing = fixture
        .run_job(OPERATION, revoke_payload("missing", "run", "absent"))
        .await;
    assert_eq!(missing["state"], json!("failed"), "{missing}");
    assert_eq!(missing["error_code"], json!("not_found"));
    assert_eq!(missing["result"], Value::Null);
    assert_eq!(fixture.snapshot(), before);

    // A run and an artifact share one id; the run is revoked first.
    fixture.run("shared").await;
    fixture
        .put("artifact", "shared", &fact("shared"), &[])
        .await;
    revoked(
        &fixture
            .run_job(OPERATION, revoke_payload("shared-run", "run", "shared"))
            .await,
    );
    let after = fixture.snapshot();
    let conflict = fixture
        .run_job(
            OPERATION,
            revoke_payload("shared-artifact", "artifact", "shared"),
        )
        .await;
    assert_eq!(conflict["state"], json!("failed"), "{conflict}");
    assert_eq!(conflict["error_code"], json!("conflict"));
    assert_eq!(fixture.snapshot(), after);

    // An artifact that is not a strict import_source is not revocable.
    fixture.put("artifact", "plain", &fact("plain"), &[]).await;
    let with_plain = fixture.snapshot();
    let plain = fixture
        .run_job(OPERATION, revoke_payload("plain", "artifact", "plain"))
        .await;
    assert_eq!(plain["state"], json!("failed"), "{plain}");
    assert_eq!(plain["error_code"], json!("invalid_input"));
    assert_eq!(fixture.snapshot(), with_plain);
}

#[tokio::test]
async fn an_import_source_artifact_is_revocable() {
    let fixture = Fixture::new().await;
    fixture
        .put(
            "artifact",
            "imported",
            &import_source("imported", fixture.admin.actor()),
            &[],
        )
        .await;
    let status = fixture
        .run_job(OPERATION, revoke_payload("import", "artifact", "imported"))
        .await;
    let (kind, id, cleanup_job_id, seq, _) = revoked(&status);
    assert_eq!(
        (kind, id.as_str()),
        (ManagementRevocationSourceKind::Artifact, "imported")
    );
    let snapshot = fixture.snapshot();
    assert_eq!(
        snapshot["tombstones"],
        json!([["imported", "artifact", "management_source_revoke", seq]])
    );
    assert_eq!(
        snapshot["jobs"],
        json!([[cleanup_job_id, "artifact", "imported", seq, "pending", 0]])
    );
}

#[tokio::test]
async fn the_revocation_and_the_terminal_job_state_share_one_commit() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let before = fixture.snapshot();
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_revoke_finish BEFORE INSERT ON objects WHEN NEW.id LIKE 'management-job-%' AND json_extract(NEW.body,'$.state')='succeeded' BEGIN SELECT RAISE(ABORT,'injected-job-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = fixture
        .run_job(OPERATION, revoke_payload("atomic-1", "run", "source"))
        .await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["error_code"], json!("internal_error"));
    assert_eq!(failed["result"], Value::Null);
    assert_eq!(fixture.snapshot(), before, "no tombstone, watermark or row");
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_revoke_finish')\nc.commit()\nprint('ok')",
    );
    revoked(
        &fixture
            .run_job(OPERATION, revoke_payload("atomic-2", "run", "source"))
            .await,
    );
}

#[tokio::test]
async fn the_cleanup_never_redacts_its_own_inputs_but_redacts_a_dependent_one() {
    let fixture = Fixture::new().await;
    let facts = fixture.graph("source").await;
    // Positive control: a drift request whose private input depends on two
    // facts of the source (it fails on its own; its input edges stay).
    let drift = fixture
        .run_job(
            "monitoring.drift.record",
            json!({
                "schema_version": "rsia.management.monitoring_drift_record.v1",
                "request_key": "dependent",
                "previous_environment_id": facts[0],
                "current_environment_id": facts[1],
                "scope_id": facts[2],
                "reason": "fixture dependent",
            }),
        )
        .await;
    let drift_input = drift["private_input_ref"].as_str().unwrap().to_owned();
    let revoke = fixture
        .run_job(OPERATION, revoke_payload("revoke", "run", "source"))
        .await;
    let (_, _, cleanup_job_id, _, _) = revoked(&revoke);
    let mut last = Value::Null;
    for key in 0..20 {
        last = fixture
            .run_job(
                "cleanup.continue",
                continue_payload(&format!("page-{key}"), &cleanup_job_id, 1000),
            )
            .await;
        if last["result"]["cleanup_state"] == json!("complete") {
            break;
        }
    }
    assert_eq!(last["result"]["cleanup_state"], json!("complete"), "{last}");
    let schema = |value: Option<Value>| value.unwrap()["schema_version"].clone();
    assert_eq!(
        schema(fixture.get("artifact", &drift_input).await),
        json!("rsia.redacted.v1")
    );
    for fact_id in &facts {
        assert_eq!(
            schema(fixture.get("artifact", fact_id).await),
            json!("rsia.redacted.v1")
        );
    }
    for job in [&revoke, &last] {
        let input = job["private_input_ref"].as_str().unwrap();
        assert_ne!(
            schema(fixture.get("artifact", input).await),
            json!("rsia.redacted.v1"),
            "{input}"
        );
        let id = job["id"].as_str().unwrap();
        let read = fixture.terminal(id).await;
        assert_eq!(read["state"], json!("succeeded"));
        assert_eq!(read["result"], job["result"]);
    }
}

#[tokio::test]
async fn the_revocation_role_and_wire_are_strict() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let before = fixture.snapshot();
    for role in [Role::Worker, Role::Host, Role::Evaluator] {
        let refused = fixture
            .dispatcher
            .submit(
                &context("someone", role),
                OPERATION,
                revoke_payload("role", "run", "source"),
            )
            .await;
        assert!(matches!(refused, Err(Error::Forbidden)), "{role:?}");
    }
    let mut reason = revoke_payload("wire", "run", "source");
    reason["reason"] = json!("free text");
    let mut actor = revoke_payload("wire", "run", "source");
    actor["actor"] = json!("admin");
    let mut watermark = revoke_payload("wire", "run", "source");
    watermark["revoke_watermark"] = json!(1);
    let mut missing = revoke_payload("wire", "run", "source");
    missing.as_object_mut().unwrap().remove("source_id");
    for payload in [
        reason,
        actor,
        watermark,
        missing,
        revoke_payload("wire", "trace", "source"),
        revoke_payload("wire", "Run", "source"),
    ] {
        let refused = fixture.submit(OPERATION, payload.clone()).await;
        assert!(
            matches!(&refused, Err(Error::Invalid(message)) if message == "invalid management payload"),
            "{payload}: {refused:?}"
        );
    }
    let mut schema = revoke_payload("wire", "run", "source");
    schema["schema_version"] = json!(REVOKE_SCHEMA.replace(".v1", ".v2"));
    let refused = fixture.submit(OPERATION, schema).await;
    assert!(
        matches!(&refused, Err(Error::Invalid(message)) if message == "unsupported management payload schema"),
        "{refused:?}"
    );
    let refused = fixture
        .submit(OPERATION, revoke_payload("wire", "run", "bad id!"))
        .await;
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert_eq!(fixture.snapshot(), before);
    let mut session = fixture.store.session().await.unwrap();
    let jobs: Vec<Value> = session.list(&fixture.admin, "job").await.unwrap();
    assert!(jobs.is_empty(), "{jobs:?}");
    session.commit().await.unwrap();
}

#[tokio::test]
async fn cancelling_before_the_effect_writes_no_revocation() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let before = fixture.snapshot();
    let job = seeded_job(
        &fixture.admin,
        OPERATION,
        "cancel-before",
        ManagementJobState::Queued,
    );
    fixture.put_job(&job).await;
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, &job.id)
        .await
        .unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn cancelling_after_the_effect_keeps_the_revocation_and_its_cleanup_continues() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let completed = fixture
        .run_job(OPERATION, revoke_payload("revoke", "run", "source"))
        .await;
    let (_, _, cleanup_job_id, _, _) = revoked(&completed);
    let after = fixture.snapshot();
    let id = completed["id"].as_str().unwrap();
    let cancelled = fixture.dispatcher.cancel(&fixture.admin, id).await.unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Succeeded);
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        completed["result"]
    );
    assert_eq!(fixture.snapshot(), after, "the revocation is never undone");
    // Cancelling the request never cancels the cleanup it opened.
    let page = fixture
        .run_job(
            "cleanup.continue",
            continue_payload("after-cancel", &cleanup_job_id, 1),
        )
        .await;
    assert_eq!(page["state"], json!("succeeded"), "{page}");
    assert_eq!(page["result"]["cleanup_state"], json!("running"));
}

#[tokio::test]
async fn a_restarted_running_revocation_fails_closed_without_draining() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let before = fixture.snapshot();
    let job = seeded_job(
        &fixture.admin,
        OPERATION,
        "restarted",
        ManagementJobState::Running,
    );
    fixture.put_job(&job).await;
    // A new process owns the data directory: the Running job left no effect,
    // so recovery reads NotCommitted and fails it closed, never Draining.
    let restarted = evo_engine::dispatch::ManagementDispatcher::new(
        fixture.store.clone(),
        vec![fixture.admin.clone()],
    )
    .unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let failed = fixture.terminal(&job.id).await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["step"], json!("worker_exited_before_commit"));
    assert_eq!(failed["error_code"], json!("management_worker_exited"));
    assert_eq!(failed["result"], Value::Null);
    assert_eq!(fixture.snapshot(), before);
}

/// R10 §1.5.1, V101.b: the handler never sets Draining itself, but a cancel of
/// a claimed job goes through the generic cancel path and is Draining until
/// recovery observes the exited worker: it proved no effect, so the job lands
/// on Cancelled with no business row written.
#[tokio::test]
async fn cancelling_a_running_revocation_drains_until_recovery_cancels_it() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    let before = fixture.snapshot();
    let job = seeded_job(
        &fixture.admin,
        OPERATION,
        "cancel-running",
        ManagementJobState::Running,
    );
    fixture.put_job(&job).await;
    let draining = fixture
        .dispatcher
        .cancel(&fixture.admin, &job.id)
        .await
        .unwrap();
    assert_eq!(draining.identity().state, ManagementJobState::Draining);
    assert!(draining.cancel_requested());
    let restarted = evo_engine::dispatch::ManagementDispatcher::new(
        fixture.store.clone(),
        vec![fixture.admin.clone()],
    )
    .unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let landed = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let job = fixture
                .dispatcher
                .status(&fixture.admin, &job.id)
                .await
                .unwrap();
            if job.identity().state != ManagementJobState::Draining {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cancelled job leaves Draining");
    assert_eq!(
        landed.identity().state,
        ManagementJobState::Cancelled,
        "{:?}",
        serde_json::to_value(&landed).unwrap()
    );
    assert_eq!(
        landed.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(landed.result().is_none());
    assert_eq!(
        fixture.snapshot(),
        before,
        "no tombstone, watermark or cleanup row"
    );
}
