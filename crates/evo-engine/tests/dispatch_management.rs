use evo_core::{Context, Error, Job, JobState, Role};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_storage::Store;
use serde_json::json;

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("management.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn wait_terminal(
    dispatcher: &ManagementDispatcher,
    context: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..100 {
        let job = dispatcher.status(context, job_id).await.unwrap();
        if matches!(
            job.state,
            ManagementJobState::Succeeded
                | ManagementJobState::Failed
                | ManagementJobState::Cancelled
                | ManagementJobState::Blocked
        ) {
            return job;
        }
        tokio::task::yield_now().await;
    }
    panic!("management job did not finish");
}

#[tokio::test]
async fn queued_cancel_persists_without_running_a_consumer() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let queued = ManagementJob {
        id: "management-job-queued".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "meta.start".into(),
        request_key: "queued".into(),
        payload_digest: "a".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: "management-input-queued".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &queued.id, admin.actor(), &queued)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let cancelled = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert!(cancelled.cancel_requested);
}

#[tokio::test]
async fn cancelled_evaluation_status_stays_cancelled_without_reading_missing_ticket() {
    let (_dir, store) = store().await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let queued = ManagementJob {
        id: "management-job-cancelled-status".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "evaluation.status".into(),
        request_key: "cancelled-status".into(),
        payload_digest: "b".repeat(64),
        owner_actor: "evaluator".into(),
        owner_role: Role::Evaluator,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: "missing-private-status-input".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&evaluator, "job", &queued.id, evaluator.actor(), &queued)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let dispatcher = ManagementDispatcher::new(store, vec![evaluator.clone()]).unwrap();
    let cancelled = dispatcher.cancel(&evaluator, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert_eq!(cancelled.step, "cancelled_before_claim");
    assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));

    // The missing private input and ticket make any live E05 read fail. A
    // successful GET therefore also proves that terminal cancellation did
    // not re-enter the evaluation consumer.
    let observed = dispatcher.status(&evaluator, &queued.id).await.unwrap();
    assert_eq!(observed.state, ManagementJobState::Cancelled);
    assert_eq!(observed.step, "cancelled_before_claim");
    assert_eq!(observed.error_code.as_deref(), Some("cancelled"));
    assert!(observed.result.is_none());
}

#[tokio::test]
async fn future_operations_persist_accurate_blocked_jobs_and_reconnect() {
    let (dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let request = json!({
        "schema_version":"rsia.management.replay_run.v1",
        "request_key":"blocked-1"
    });
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "replay.run", request.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let first = wait_terminal(&dispatcher, &admin, &queued.id).await;
    let mut reset = first.clone();
    reset.state = ManagementJobState::Running;
    reset.step = "claimed_before_crash".into();
    reset.result = None;
    reset.error_code = None;
    reset.lease_token = Some("dead-process-lease".into());
    reset.lease_until = i64::MAX;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &reset.id, admin.actor(), &reset)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    let reopened = Store::open(&dir.path().join("management.sqlite3"))
        .await
        .unwrap();
    let reopened_dispatcher =
        ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    reopened_dispatcher.recover_pending().await.unwrap();
    let replay = wait_terminal(&reopened_dispatcher, &admin, &first.id).await;
    assert_eq!(first.id, replay.id);
    assert_eq!(first.state, ManagementJobState::Blocked);
    assert_eq!(
        first.error_code.as_deref(),
        Some("replay.run_consumer_unavailable")
    );
    let reconnect = reopened_dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    assert_eq!(reconnect.id, first.id);
    assert_eq!(
        reopened_dispatcher
            .status(&admin, &first.id)
            .await
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        reopened_dispatcher
            .cancel(&admin, &first.id)
            .await
            .unwrap()
            .state,
        ManagementJobState::Blocked
    );
}

#[tokio::test]
async fn same_key_double_submit_claims_once_and_cancel_cannot_be_overwritten() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store, vec![admin.clone()]).unwrap();
    let request = json!({
        "schema_version":"rsia.management.meta_start.v1",
        "request_key":"double-submit"
    });
    let (left, right) = tokio::join!(
        dispatcher.submit(&admin, "meta.start", request.clone()),
        dispatcher.submit(&admin, "meta.start", request)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.id, right.id);
    let cancelled_or_done = dispatcher.cancel(&admin, &left.id).await.unwrap();
    let final_job = wait_terminal(&dispatcher, &admin, &left.id).await;
    assert!(matches!(
        final_job.state,
        ManagementJobState::Cancelled | ManagementJobState::Blocked
    ));
    assert_ne!(final_job.state, ManagementJobState::Failed);
    if cancelled_or_done.state == ManagementJobState::Cancelled {
        tokio::task::yield_now().await;
        assert_eq!(
            dispatcher.status(&admin, &left.id).await.unwrap().state,
            ManagementJobState::Cancelled
        );
    }
}

#[tokio::test]
async fn startup_recovery_without_bound_credential_fails_closed() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let queued = ManagementJob {
        id: "management-job-no-credential".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "meta.start".into(),
        request_key: "missing-credential".into(),
        payload_digest: "a".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: "management-input-no-credential".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &queued.id, admin.actor(), &queued)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![]).unwrap();
    dispatcher.recover_pending().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut session = store.session().await.unwrap();
    let unchanged: ManagementJob = session.need(&admin, "job", &queued.id).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(unchanged.state, ManagementJobState::Queued);
    assert!(dispatcher.status(&admin, &queued.id).await.is_err());
}

#[tokio::test]
async fn legacy_job_coexists_duplicate_context_recovers_once_and_damage_is_visible() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let legacy = Job {
        id: "legacy-job".into(),
        owner: "admin".into(),
        run_id: "run".into(),
        state: JobState::Queued,
        lease_token: None,
        lease_until: 0,
        deadline: 10,
        attempts: 0,
        improver_version: "v1".into(),
        task_snapshot: "snapshot".into(),
        result_ids: vec![],
        error_code: None,
    };
    let queued = ManagementJob {
        id: "management-job-recovery".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "meta.start".into(),
        request_key: "recover".into(),
        payload_digest: "a".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: "missing-private-input".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &legacy.id, admin.actor(), &legacy)
        .await
        .unwrap();
    session
        .put(&admin, "job", &queued.id, admin.actor(), &queued)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), admin.clone()]).unwrap();
    assert_eq!(dispatcher.recover_pending().await.unwrap(), 1);
    assert_eq!(dispatcher.recover_pending().await.unwrap(), 0);
    let failed = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed);
    assert_eq!(failed.generation, 1);

    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "job",
            "management-job-damaged",
            admin.actor(),
            &json!({"id":"management-job-damaged","schema_version":"rsia.management_job.v999"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let fresh = ManagementDispatcher::new(store, vec![admin]).unwrap();
    assert!(matches!(
        fresh.recover_pending().await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn role_owner_and_unknown_fields_are_enforced() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let other = Context::new("n", "other", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(
        store.clone(),
        vec![admin.clone(), evaluator.clone(), other.clone()],
    )
    .unwrap();
    assert!(matches!(
        dispatcher.submit(
            &admin,
            "evaluation.status",
            json!({"schema_version":"rsia.management.evaluation_status.v1","request_key":"x","ticket_id":"t"})
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert!(
        dispatcher
            .submit(
                &evaluator,
                "replay.run",
                json!({"schema_version":"rsia.management.replay_run.v1","request_key":"x"})
            )
            .await
            .is_err()
    );
    assert!(dispatcher.submit(
        &admin,
        "meta.start",
        json!({"schema_version":"rsia.management.meta_start.v1","request_key":"x","settings":{}})
    )
    .await
    .is_err());
    let job = dispatcher
        .submit(
            &admin,
            "meta.start",
            json!({"schema_version":"rsia.management.meta_start.v1","request_key":"ok"}),
        )
        .await
        .unwrap();
    assert!(matches!(
        dispatcher.status(&other, &job.id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn evaluator_job_same_key_different_ticket_conflicts_after_restart_safe_failure() {
    let (_dir, store) = store().await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![evaluator.clone()]).unwrap();
    let first_queued = dispatcher
        .submit(
            &evaluator,
            "evaluation.status",
            json!({
                "schema_version":"rsia.management.evaluation_status.v1",
                "request_key":"status-key",
                "ticket_id":"missing-a"
            }),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut session = store.session().await.unwrap();
    let first: ManagementJob = session
        .need(&evaluator, "job", &first_queued.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(first.state, ManagementJobState::Failed);
    assert_eq!(first.error_code.as_deref(), Some("not_found"));
    assert!(matches!(
        dispatcher
            .submit(
                &evaluator,
                "evaluation.status",
                json!({
                    "schema_version":"rsia.management.evaluation_status.v1",
                    "request_key":"status-key",
                    "ticket_id":"missing-b"
                })
            )
            .await,
        Err(Error::Conflict(_))
    ));
}
