//! AG-162 (plan v4.5 L781–L785, L799–L806, L823, §11.3, E08, V100.a,
//! V101.b/c, V102.a/b/c): `cleanup.continue` is wired as an Admin async job.
//! One job is exactly one `cleanup_step` page, committed with the terminal job
//! state in one transaction; Succeeded means that page committed, and the
//! result reports whether the cleanup is still pending. A page that meets an
//! unknown scope commits the kernel's `failed` state and the job is Blocked
//! with the fixed code `blocked_unknown_scope`, never the kernel's text. An
//! Admin page over a failed cleanup starts exactly one new round.
//!
//! Not implemented here: resuming the cleanup at startup (AG-162b).
//!
//! R10 Draining rules (§1.5): both revocation operations are claimed while a
//! Draining job holds the only dispatch slot, while a non-closure job stays
//! Queued behind it; neither operation ever drains itself.
#[path = "support/revocation_management.rs"]
mod revocation_management;

use evo_core::{Error, Role, hash};
use evo_engine::dispatch::{
    ManagementCancelArrival, ManagementJobState, ManagementResultV2, ManagementRevocationResultV2,
};
use evo_storage::lifecycle::CleanupState;
use revocation_management::{
    CONTINUE_SCHEMA, Fixture, NS, context, continue_payload, revoke_payload, seeded_job, unknown,
};
use serde_json::{Value, json};

const OPERATION: &str = "cleanup.continue";

/// The typed result of a cleanup page job.
fn page_result(status: &Value) -> (CleanupState, u64, u64) {
    assert_eq!(status["step"], json!("cleanup_continued"), "{status}");
    let Some(ManagementResultV2::Revocation(ManagementRevocationResultV2::CleanupContinued {
        cleanup_state,
        processed_nodes,
        pending_nodes,
        ..
    })) = serde_json::from_value(status["result"].clone()).unwrap()
    else {
        panic!("expected a cleanup page result: {status}")
    };
    (cleanup_state, processed_nodes, pending_nodes)
}

async fn revoked_cleanup(fixture: &Fixture, source: &str) -> String {
    let status = fixture
        .run_job("source.revoke", revoke_payload(source, "run", source))
        .await;
    assert_eq!(status["state"], json!("succeeded"), "{status}");
    status["result"]["cleanup_job_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn pages_advance_by_new_keys_to_complete_and_complete_is_a_no_op() {
    let fixture = Fixture::new().await;
    let facts = fixture.graph("source").await;
    let cleanup = revoked_cleanup(&fixture, "source").await;
    let mut pages = 0;
    let mut previous = fixture.snapshot();
    let last = loop {
        pages += 1;
        assert!(pages < 50, "the cleanup never completed");
        let status = fixture
            .run_job(
                OPERATION,
                continue_payload(&format!("page-{pages}"), &cleanup, 1),
            )
            .await;
        assert_eq!(status["state"], json!("succeeded"), "{status}");
        assert_eq!(status["error_code"], Value::Null);
        let (state, processed, pending) = page_result(&status);
        let snapshot = fixture.snapshot();
        assert_ne!(snapshot, previous, "every page commits progress");
        let row = &snapshot["jobs"][0];
        assert_eq!(row[4], json!(serde_json::to_value(state).unwrap()));
        assert_eq!(row[5], json!(processed));
        previous = snapshot;
        if state == CleanupState::Complete {
            assert_eq!(pending, 0);
            break status;
        }
        assert_eq!(state, CleanupState::Running);
    };
    assert!(pages > 1, "a one-edge page needs several jobs");
    for fact_id in &facts {
        assert_eq!(
            fixture.get("artifact", fact_id).await.unwrap()["schema_version"],
            json!("rsia.redacted.v1")
        );
    }
    assert_eq!(
        last["committed_side_effects"][0]["kind"],
        json!("cleanup_continued")
    );
    let again = fixture
        .run_job(OPERATION, continue_payload("after-complete", &cleanup, 1))
        .await;
    assert_eq!(again["state"], json!("succeeded"), "{again}");
    assert_eq!(page_result(&again).0, CleanupState::Complete);
    assert_eq!(
        fixture.snapshot(),
        previous,
        "a complete cleanup writes nothing"
    );
}

#[tokio::test]
async fn a_failed_page_blocks_with_the_fixed_code_and_only_a_new_key_retries() {
    let fixture = Fixture::new().await;
    fixture.run("source").await;
    fixture
        .put(
            "artifact",
            "unknown-node",
            &unknown("unknown-node"),
            &[("run", "source")],
        )
        .await;
    let cleanup = revoked_cleanup(&fixture, "source").await;
    let mut key = 0;
    let blocked = loop {
        key += 1;
        assert!(key < 20, "the cleanup never failed");
        let status = fixture
            .run_job(
                OPERATION,
                continue_payload(&format!("page-{key}"), &cleanup, 1000),
            )
            .await;
        if status["state"] == json!("blocked") {
            break status;
        }
        assert_eq!(status["state"], json!("succeeded"), "{status}");
    };
    assert_eq!(blocked["error_code"], json!("blocked_unknown_scope"));
    assert_eq!(page_result(&blocked).0, CleanupState::Failed);
    let text = blocked.to_string();
    assert!(
        !text.contains("rsia.unknown.v9") && !text.contains("last_error"),
        "{text}"
    );
    assert_eq!(fixture.snapshot()["jobs"][0][4], json!("failed"));
    let rounds = fixture.events("cleanup_retry_started");

    // The same key and payload replay the Blocked job: no new round.
    let replay = fixture
        .submit(
            OPERATION,
            continue_payload(&format!("page-{key}"), &cleanup, 1000),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, blocked["id"].as_str().unwrap());
    assert_eq!(fixture.events("cleanup_retry_started"), rounds);

    // A new Admin key starts exactly one verification round.
    let retried = fixture
        .run_job(OPERATION, continue_payload("retry", &cleanup, 1000))
        .await;
    assert_eq!(fixture.events("cleanup_retry_started"), rounds + 1);
    assert_eq!(retried["state"], json!("blocked"), "{retried}");
    assert_eq!(retried["error_code"], json!("blocked_unknown_scope"));
}

#[tokio::test]
async fn an_unknown_cleanup_job_fails_without_writes() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    revoked_cleanup(&fixture, "source").await;
    let before = fixture.snapshot();
    let failed = fixture
        .run_job(OPERATION, continue_payload("missing", "revoke-absent", 1))
        .await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["error_code"], json!("not_found"));
    assert_eq!(failed["result"], Value::Null);
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn a_page_and_the_terminal_job_state_share_one_commit() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let cleanup = revoked_cleanup(&fixture, "source").await;
    let before = fixture.snapshot();
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_page_finish BEFORE INSERT ON objects WHEN NEW.id LIKE 'management-job-%' AND json_extract(NEW.body,'$.operation')='cleanup.continue' AND json_extract(NEW.body,'$.state')='succeeded' BEGIN SELECT RAISE(ABORT,'injected-job-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = fixture
        .run_job(OPERATION, continue_payload("atomic-1", &cleanup, 1))
        .await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["error_code"], json!("internal_error"));
    assert_eq!(fixture.snapshot(), before, "the page rolled back");
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_page_finish')\nc.commit()\nprint('ok')",
    );
    let page = fixture
        .run_job(OPERATION, continue_payload("atomic-2", &cleanup, 1))
        .await;
    assert_eq!(page["state"], json!("succeeded"), "{page}");
    assert_ne!(fixture.snapshot(), before);
}

#[tokio::test]
async fn the_page_role_and_wire_are_strict() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let cleanup = revoked_cleanup(&fixture, "source").await;
    let before = fixture.snapshot();
    for role in [Role::Worker, Role::Host, Role::Evaluator] {
        let refused = fixture
            .dispatcher
            .submit(
                &context("someone", role),
                OPERATION,
                continue_payload("role", &cleanup, 1),
            )
            .await;
        assert!(matches!(refused, Err(Error::Forbidden)), "{role:?}");
    }
    let mut actor = continue_payload("wire", &cleanup, 1);
    actor["actor"] = json!("admin");
    let mut missing = continue_payload("wire", &cleanup, 1);
    missing.as_object_mut().unwrap().remove("edge_page_limit");
    let mut typed = continue_payload("wire", &cleanup, 1);
    typed["edge_page_limit"] = json!("1");
    let mut negative = continue_payload("wire", &cleanup, 1);
    negative["edge_page_limit"] = json!(-1);
    for payload in [actor, missing, typed, negative] {
        let refused = fixture.submit(OPERATION, payload.clone()).await;
        assert!(
            matches!(&refused, Err(Error::Invalid(message)) if message == "invalid management payload"),
            "{payload}: {refused:?}"
        );
    }
    for limit in [0, 1001] {
        let refused = fixture
            .submit(OPERATION, continue_payload("wire", &cleanup, limit))
            .await;
        assert!(
            matches!(&refused, Err(Error::Invalid(message)) if message == "cleanup edge page limit must be 1..=1000"),
            "{limit}: {refused:?}"
        );
    }
    let mut schema = continue_payload("wire", &cleanup, 1);
    schema["schema_version"] = json!(CONTINUE_SCHEMA.replace(".v1", ".v2"));
    let refused = fixture.submit(OPERATION, schema).await;
    assert!(
        matches!(&refused, Err(Error::Invalid(message)) if message == "unsupported management payload schema"),
        "{refused:?}"
    );
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn cancelling_a_page_request_never_cancels_the_cleanup() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let cleanup = revoked_cleanup(&fixture, "source").await;
    let before = fixture.snapshot();
    let queued = seeded_job(
        &fixture.admin,
        OPERATION,
        "cancel-before",
        ManagementJobState::Queued,
    );
    fixture.put_job(&queued).await;
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, &queued.id)
        .await
        .unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert_eq!(
        fixture.snapshot(),
        before,
        "no page and the cleanup is pending"
    );

    let completed = fixture
        .run_job(OPERATION, continue_payload("cancel-after", &cleanup, 1))
        .await;
    assert_eq!(completed["state"], json!("succeeded"), "{completed}");
    let after = fixture.snapshot();
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, completed["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Succeeded);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        completed["result"]
    );
    assert_eq!(fixture.snapshot(), after, "the committed page is kept");
    let next = fixture
        .run_job(OPERATION, continue_payload("after-cancel", &cleanup, 1))
        .await;
    assert_eq!(next["state"], json!("succeeded"), "{next}");
    assert_ne!(fixture.snapshot(), after, "the cleanup continues");
}

#[tokio::test]
async fn a_restarted_running_page_fails_closed_without_draining() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    revoked_cleanup(&fixture, "source").await;
    let before = fixture.snapshot();
    let job = seeded_job(
        &fixture.admin,
        OPERATION,
        "restarted",
        ManagementJobState::Running,
    );
    fixture.put_job(&job).await;
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

/// R10 §1.5.2/§1.5.3: a Draining job holds the only concurrent dispatch slot.
/// Both revocation operations are still claimed and finish, while a new
/// non-closure job stays Queued behind it.
#[tokio::test]
async fn revocation_operations_are_claimed_while_a_draining_job_holds_the_slot() {
    let fixture = Fixture::new().await;
    fixture.graph("source").await;
    let draining = seeded_job(
        &fixture.admin,
        "export.complete",
        "draining-holder",
        ManagementJobState::Draining,
    );
    fixture.put_job(&draining).await;
    assert_eq!(fixture.concurrent_dispatches().await, 1);

    let other = fixture
        .submit(
            "budget.root.authorize",
            json!({
                "schema_version": "rsia.management.budget_root_authorize.v1",
                "request_key": "slot-other-root",
                "root_budget_id": "slot-other-root",
                "billing_scope": "slot-other-scope",
                "allowed_namespaces": [NS],
                "currency": "USD",
                "pricing_version": "price-v1",
                "payment_subject": "fixture-only",
                "authorization_receipt_digest": hash(b"slot-other-receipt"),
                "per_call_cap_micros": 20,
                "total_limit_micros": 1000,
            }),
        )
        .await
        .unwrap();
    let other_id = other.identity().id.to_owned();

    let cleanup = revoked_cleanup(&fixture, "source").await;
    let page = fixture
        .run_job(OPERATION, continue_payload("slot-page", &cleanup, 1))
        .await;
    assert_eq!(page["state"], json!("succeeded"), "{page}");
    assert_eq!(fixture.snapshot()["jobs"][0][4], json!("running"));

    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let other = fixture
        .dispatcher
        .status_view(&fixture.admin, &other_id)
        .await
        .unwrap();
    assert_eq!(other.state, ManagementJobState::Queued, "{other:?}");
    let holder = fixture.get("job", &draining.id).await.unwrap();
    assert_eq!(holder["state"], json!("draining"));
    assert_eq!(fixture.concurrent_dispatches().await, 1);
}
