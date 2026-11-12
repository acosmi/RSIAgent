//! AG-139a (plan v4.5 L799, L801, L802, L820, L851, V100.b, V100.d):
//! `consolidation.trigger` is wired as an Admin async job. One transaction
//! closes the actual development cycle, claims the newest generation (settling
//! older ones) and writes the terminal job state; it only claims and records,
//! so it never queues, reserves or dispatches paid work. A claim failure rolls
//! the cycle back and fails the job with a fixed code.
//!
//! R10 Draining rule: a trigger job never enters Draining. Its single
//! transaction holds no budget call or external effect, so recovery reads a
//! non-terminal job as NotCommitted (never Unknown) and a terminal one as its
//! own committed outcome. Every status poll below asserts the state is never
//! `draining`, and a restarted Running trigger job fails closed instead.
#[path = "support/consolidation_cycles.rs"]
mod consolidation_cycles;
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use consolidation_cycles::{
    CLAIM_SCHEMA, CYCLE_SCHEMA, Fixture, RUN_SCHEMA, SCOPE_SCHEMA, TRIGGER_SCHEMA, context, d,
};
use evo_core::{Context, Error, Role, hash, now};
use evo_engine::dispatch::{
    ManagementCancelArrival, ManagementConsolidationTriggerState, ManagementDispatcher,
    ManagementJobState, ManagementJobStatusV1, ManagementJobV2, ManagementMonitoringResultV2,
    ManagementResultV2,
};
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

const OPERATION: &str = "consolidation.trigger";

fn new_dispatcher(fixture: &Fixture) -> ManagementDispatcher {
    ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap()
}

/// Polls to a terminal state, asserting no observed state is Draining.
async fn terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
) -> ManagementJobStatusV1 {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let status = dispatcher.status_view(ctx, job_id).await.unwrap();
            assert_ne!(
                status.state,
                ManagementJobState::Draining,
                "a consolidation trigger never drains: {status:?}"
            );
            if matches!(
                status.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("consolidation trigger job reaches a terminal state")
}

async fn trigger(
    fixture: &Fixture,
    dispatcher: &ManagementDispatcher,
    key: &str,
    fact_id: &str,
) -> ManagementJobStatusV1 {
    let queued = dispatcher
        .submit(&fixture.admin, OPERATION, fixture.payload(key, fact_id))
        .await
        .unwrap();
    terminal(dispatcher, &fixture.admin, queued.identity().id).await
}

struct Triggered {
    cycle_id: String,
    scope_id: String,
    completed_cycles: u64,
    trigger_state: ManagementConsolidationTriggerState,
    claim_id: Option<String>,
    claim_state: Option<String>,
    terminated_generations: Vec<u64>,
}

fn triggered(status: &ManagementJobStatusV1) -> Triggered {
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "consolidation_triggered");
    assert_eq!(status.error_code, None);
    let Some(ManagementResultV2::Monitoring(
        ManagementMonitoringResultV2::ConsolidationTriggered {
            cycle_id,
            scope_id,
            completed_cycles,
            trigger_state,
            claim_id,
            claim_state,
            terminated_generations,
        },
    )) = status.result.clone()
    else {
        panic!("expected a consolidation trigger result, got {status:?}")
    };
    Triggered {
        cycle_id,
        scope_id,
        completed_cycles,
        trigger_state,
        claim_id,
        claim_state,
        terminated_generations,
    }
}

async fn business_rows(fixture: &Fixture) -> (usize, usize, usize) {
    (
        fixture.artifacts(CYCLE_SCHEMA).await.len(),
        fixture.artifacts(SCOPE_SCHEMA).await.len(),
        fixture.artifacts(CLAIM_SCHEMA).await.len(),
    )
}

async fn jobs(fixture: &Fixture) -> Vec<Value> {
    let mut session = fixture.store.session().await.unwrap();
    let jobs = session.list(&fixture.admin, "job").await.unwrap();
    session.commit().await.unwrap();
    jobs
}

/// A trigger job row as the dispatcher derives its ids.
fn seeded_job(ctx: &Context, key: &str, state: ManagementJobState) -> ManagementJobV2 {
    let digest =
        hash(format!("{}\0{}\0{OPERATION}\0{key}", ctx.namespace(), ctx.actor()).as_bytes());
    let running = state == ManagementJobState::Running;
    ManagementJobV2 {
        id: format!("management-job-{}", &digest[..24]),
        schema_version: "rsia.management_job.v2".into(),
        operation: OPERATION.into(),
        request_key: key.into(),
        payload_digest: hash(b"seeded-trigger-fixture"),
        owner_actor: ctx.actor().into(),
        owner_role: Role::Admin,
        state,
        step: if running { "claimed" } else { "accepted" }.into(),
        private_input_ref: format!("management-input-{}", &digest[..24]),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: running.then(|| hash(b"seeded-trigger-lease")),
        lease_until: if running { now().unwrap() + 60 } else { 0 },
        generation: u64::from(running),
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    }
}

async fn put_job(fixture: &Fixture, job: &ManagementJobV2) {
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, "job", &job.id, fixture.admin.actor(), job)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn the_trigger_closes_the_actual_cycle_and_only_claims() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let first_fact = fixture.stage(1, true).await;
    let second_fact = fixture.stage(2, true).await;

    let first = triggered(&trigger(&fixture, &dispatcher, "trigger-1", &first_fact).await);
    assert_eq!(
        first.trigger_state,
        ManagementConsolidationTriggerState::NotEligible
    );
    assert_eq!(first.completed_cycles, 1);
    assert_eq!((first.claim_id, first.claim_state), (None, None));

    let second_status = trigger(&fixture, &dispatcher, "trigger-2", &second_fact).await;
    let second = triggered(&second_status);
    assert_eq!(
        second.trigger_state,
        ManagementConsolidationTriggerState::Claimed
    );
    assert_eq!(second.completed_cycles, 2);
    assert_eq!(second.claim_state.as_deref(), Some("claimed"));
    assert!(second.terminated_generations.is_empty());
    assert_eq!(second.scope_id, first.scope_id);

    // The cycles are the actual closed records of the two reports.
    let mut cycles = fixture.artifacts(CYCLE_SCHEMA).await;
    cycles.sort_by_key(|cycle| cycle["ordinal"].as_u64());
    assert_eq!(cycles.len(), 2);
    for (cycle, (id, fact, ordinal)) in cycles.iter().zip([
        (&first.cycle_id, &first_fact, 1),
        (&second.cycle_id, &second_fact, 2),
    ]) {
        assert_eq!(cycle["id"], json!(id));
        assert_eq!(cycle["report_fact_id"], json!(fact));
        assert_eq!(cycle["ordinal"], json!(ordinal));
    }
    let index = fixture.value(&second.scope_id).await;
    assert_eq!(
        index["cycle_ids"],
        json!([first.cycle_id.clone(), second.cycle_id.clone()])
    );
    let claim_id = second.claim_id.clone().unwrap();
    assert_eq!(index["claim_ids"], json!({"1": claim_id}));
    let claim = fixture.value(&claim_id).await;
    assert_eq!(claim["generation"], json!(1));
    assert_eq!(claim["state"], json!("claimed"));
    assert_eq!(claim["execution_input_digest"], Value::Null);

    // Only the claim: no run record, no other job, no reservation or spend.
    assert!(fixture.artifacts(RUN_SCHEMA).await.is_empty());
    let jobs = jobs(&fixture).await;
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|job| job["operation"] == OPERATION));
    assert_eq!(fixture.root().await, (0, 0));
    let audits = fixture.audits();
    for expected in [
        ("monitoring.development_cycle.complete", &second.cycle_id),
        ("monitoring.consolidation.claim", &claim_id),
        ("management.job.finish", &second_status.id),
    ] {
        assert!(
            audits.contains(&(expected.0.to_owned(), expected.1.clone())),
            "{expected:?} in {audits:?}"
        );
    }
    assert_eq!(
        serde_json::to_value(&second_status.committed_side_effects).unwrap(),
        json!([{
            "kind": "consolidation_triggered",
            "cycle_id": second.cycle_id,
            "scope_id": second.scope_id,
            "claim_id": claim_id,
        }])
    );
}

#[tokio::test]
async fn the_same_report_under_another_key_adds_no_cycle_or_claim() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let first_fact = fixture.stage(1, false).await;
    let second_fact = fixture.stage(2, false).await;
    triggered(&trigger(&fixture, &dispatcher, "trigger-1", &first_fact).await);
    let original_status = trigger(&fixture, &dispatcher, "trigger-2", &second_fact).await;
    let original = triggered(&original_status);
    assert_eq!(original.claim_state.as_deref(), Some("no_contrast"));
    let scope = fixture.raw(&original.scope_id).await;
    let claim = fixture.raw(original.claim_id.as_deref().unwrap()).await;

    let again = triggered(&trigger(&fixture, &dispatcher, "trigger-2-again", &second_fact).await);
    assert_eq!(
        again.trigger_state,
        ManagementConsolidationTriggerState::AlreadyClaimed
    );
    assert_eq!(again.cycle_id, original.cycle_id);
    assert_eq!(again.claim_id, original.claim_id);
    assert_eq!(again.claim_state.as_deref(), Some("no_contrast"));
    assert_eq!(business_rows(&fixture).await, (2, 1, 1));
    assert_eq!(fixture.raw(&original.scope_id).await, scope);
    assert_eq!(
        fixture.raw(original.claim_id.as_deref().unwrap()).await,
        claim
    );

    // The same key and payload replay the original job.
    let replay = dispatcher
        .submit(
            &fixture.admin,
            OPERATION,
            fixture.payload("trigger-2", &second_fact),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, original_status.id);
}

#[tokio::test]
async fn a_revoked_source_or_watermark_writes_no_business_row() {
    // A revoked source refuses the submit at its dependency gate.
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let fact = fixture.stage(1, true).await;
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-1".into(),
        },
        "source revoked",
        500,
    )
    .await
    .unwrap();
    let rows = fixture.rows().await;
    let refused = dispatcher
        .submit(&fixture.admin, OPERATION, fixture.payload("revoked", &fact))
        .await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message.contains("run source-1")),
        "{refused:?}"
    );
    assert_eq!(fixture.rows().await, rows);

    // A watermark that moved after the request froze it fails the job before
    // its first business write.
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let fact = fixture.stage(1, true).await;
    let mut session = fixture.store.session().await.unwrap();
    session
        .bump_watermark(&fixture.admin, &d("moved-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let failed = trigger(&fixture, &dispatcher, "moved-watermark", &fact).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert!(failed.result.is_none());
    assert_eq!(business_rows(&fixture).await, (0, 0, 0));
}

#[tokio::test]
async fn a_claim_failure_rolls_the_cycle_back() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let first_fact = fixture.stage(1, true).await;
    let second_fact = fixture.stage(2, true).await;
    triggered(&trigger(&fixture, &dispatcher, "trigger-1", &first_fact).await);
    // Every claim slot is taken, so the claim of generation one is refused
    // after the second cycle's writes in the same transaction.
    let mut session = fixture.store.session().await.unwrap();
    for slot in 0..250 {
        let id = format!("occupied-claim-slot-{slot}");
        session
            .put(
                &fixture.worker,
                "artifact",
                &id,
                fixture.worker.actor(),
                &json!({"id": id, "schema_version": CLAIM_SCHEMA}),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let scope_id = fixture.scope_id().await;
    let scope = fixture.raw(&scope_id).await;
    let failed = trigger(&fixture, &dispatcher, "trigger-2", &second_fact).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert!(failed.result.is_none());
    let cycles = fixture.artifacts(CYCLE_SCHEMA).await;
    assert_eq!(
        cycles.len(),
        1,
        "the second cycle rolled back with the claim"
    );
    assert_ne!(cycles[0]["report_fact_id"], json!(second_fact));
    assert_eq!(fixture.raw(&scope_id).await, scope);
    assert!(
        !fixture
            .audits()
            .iter()
            .any(|(operation, _)| operation == "monitoring.consolidation.claim")
    );
}

#[tokio::test]
async fn the_cycle_claim_and_terminal_job_state_share_one_commit() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let first_fact = fixture.stage(1, true).await;
    let second_fact = fixture.stage(2, true).await;
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_trigger_job_finish BEFORE INSERT ON objects WHEN NEW.id LIKE 'management-job-%' AND json_extract(NEW.body,'$.state')='succeeded' BEGIN SELECT RAISE(ABORT,'injected-job-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = trigger(&fixture, &dispatcher, "atomic-1", &first_fact).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert_eq!(business_rows(&fixture).await, (0, 0, 0));
    fixture.sqlite(
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_trigger_job_finish')\nc.commit()\nprint('ok')",
    );
    triggered(&trigger(&fixture, &dispatcher, "atomic-2", &first_fact).await);
    let claimed = triggered(&trigger(&fixture, &dispatcher, "atomic-3", &second_fact).await);
    assert_eq!(
        claimed.trigger_state,
        ManagementConsolidationTriggerState::Claimed
    );
    assert_eq!(business_rows(&fixture).await, (2, 1, 1));
}

#[tokio::test]
async fn the_trigger_role_and_wire_are_strict() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let fact = fixture.stage(1, true).await;
    let rows = fixture.rows().await;
    for role in [Role::Worker, Role::Host, Role::Evaluator] {
        let ctx = context("someone", role);
        let refused = dispatcher
            .submit(&ctx, OPERATION, fixture.payload("role", &fact))
            .await;
        assert!(matches!(refused, Err(Error::Forbidden)), "{role:?}");
    }
    let mut unknown = fixture.payload("wire", &fact);
    unknown["actor"] = json!("admin");
    let mut missing = fixture.payload("wire", &fact);
    missing.as_object_mut().unwrap().remove("source_ids");
    let mut typed = fixture.payload("wire", &fact);
    typed["revoke_watermark"] = json!("1");
    for payload in [unknown, missing, typed] {
        let refused = dispatcher.submit(&fixture.admin, OPERATION, payload).await;
        assert!(
            matches!(&refused, Err(Error::Invalid(message)) if message == "invalid management payload"),
            "{refused:?}"
        );
    }
    let mut schema = fixture.payload("wire", &fact);
    schema["schema_version"] = json!(TRIGGER_SCHEMA.replace(".v1", ".v2"));
    let refused = dispatcher.submit(&fixture.admin, OPERATION, schema).await;
    assert!(
        matches!(&refused, Err(Error::Invalid(message)) if message == "unsupported management payload schema"),
        "{refused:?}"
    );
    assert_eq!(fixture.rows().await, rows);
}

#[tokio::test]
async fn cancelling_before_the_effect_creates_no_business_rows() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let job = seeded_job(&fixture.admin, "cancel-before", ManagementJobState::Queued);
    put_job(&fixture, &job).await;
    let cancelled = dispatcher.cancel(&fixture.admin, &job.id).await.unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert_eq!(business_rows(&fixture).await, (0, 0, 0));
}

#[tokio::test]
async fn cancelling_after_the_effect_keeps_the_cycle_and_claim() {
    let fixture = Fixture::new().await;
    let dispatcher = new_dispatcher(&fixture);
    let first_fact = fixture.stage(1, true).await;
    let second_fact = fixture.stage(2, true).await;
    triggered(&trigger(&fixture, &dispatcher, "trigger-1", &first_fact).await);
    let completed = trigger(&fixture, &dispatcher, "trigger-2", &second_fact).await;
    let result = triggered(&completed);
    let claim_id = result.claim_id.clone().unwrap();
    let cycle = fixture.raw(&result.cycle_id).await;
    let scope = fixture.raw(&result.scope_id).await;
    let claim = fixture.raw(&claim_id).await;
    let audits = fixture.audits();
    let cancelled = dispatcher
        .cancel(&fixture.admin, &completed.id)
        .await
        .unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Succeeded);
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(&completed.result).unwrap()
    );
    assert_eq!(fixture.raw(&result.cycle_id).await, cycle);
    assert_eq!(fixture.raw(&result.scope_id).await, scope);
    assert_eq!(fixture.raw(&claim_id).await, claim);
    assert_eq!(fixture.root().await, (0, 0));
    let mut expected = audits;
    expected.push(("management.job.cancel".to_owned(), completed.id.clone()));
    assert_eq!(fixture.audits(), expected);
}

#[tokio::test]
async fn a_stale_lease_cannot_write_and_a_restarted_trigger_fails_closed_without_draining() {
    let fixture = Fixture::new().await;
    let job = seeded_job(&fixture.admin, "restarted", ManagementJobState::Running);
    put_job(&fixture, &job).await;
    let lease = job.lease_token.clone().unwrap();
    let mut session = fixture.store.session().await.unwrap();
    for stale in [
        evo_storage::ManagementJobFence {
            job_id: &job.id,
            operation: OPERATION,
            generation: 0,
            lease_token: &lease,
        },
        evo_storage::ManagementJobFence {
            job_id: &job.id,
            operation: OPERATION,
            generation: 1,
            lease_token: "stale-lease",
        },
    ] {
        assert!(matches!(
            session
                .ensure_management_job_fence(&fixture.admin, &stale)
                .await,
            Err(Error::Conflict(_))
        ));
    }
    session.commit().await.unwrap();

    // A new process owns the data directory: the Running trigger left no
    // effect, so it is NotCommitted and fails closed, never Draining.
    let restarted = new_dispatcher(&fixture);
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let failed = terminal(&restarted, &fixture.admin, &job.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.step, "worker_exited_before_commit");
    assert_eq!(
        failed.error_code.as_deref(),
        Some("management_worker_exited")
    );
    assert!(failed.result.is_none());
    assert_eq!(business_rows(&fixture).await, (0, 0, 0));
    assert_eq!(fixture.root().await, (0, 0));
}
