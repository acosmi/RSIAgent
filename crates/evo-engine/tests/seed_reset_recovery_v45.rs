//! AG-166c-reset recovery controls for `seed.reset_stage` (plan v4.5
//! §6.1.2 L828, §11.1 L1468, capacity L381/L387, E16.3).
//!
//! The reset kernel shares one staging tail for the public API and the
//! management handler. This file pins the capacity gate of that tail and the
//! job-recovery behaviour of the handler: staging and the terminal job state
//! in one transaction, cancellation before and after the effect, and the way
//! out of Draining.
//!
//! Two-tree probe. It uses only API that predates the wiring, so it compiles
//! on the parent tree, where the public reset has no capacity admission and
//! the probe fails with `PARENT-EXPECTED-FAILURE`:
//! - CPP-C (refusal with zero writes): at 20/20 staged packages the 21st
//!   public reset is refused with the typed staging conflict, while a replay
//!   of an existing key still returns its original image.
//!
//! The head-side recovery controls pin the handler's job behaviour: the
//! staged asset and the successful terminal job state commit together or not
//! at all, cancellation before the effect is zero-effect and after it only
//! records the arrival, and the two ways a claimed job reaches recovery
//! (a restart, a cancel) land on a terminal state instead of staying
//! Draining.
#[path = "support/seed_reset.rs"]
mod seed_reset;

use evo_core::Error;
use evo_engine::dispatch::{ManagementCancelArrival, ManagementDispatcher, ManagementJobState};
use evo_engine::seeds::PersistentSeedStore;
use seed_reset::{COMPILER_VERSION, Fixture, RUN_ID, reset_payload, seeded_job, staged_id};
use serde_json::{Value, json};

#[tokio::test]
async fn cpp_c_public_seed_reset_refuses_the_21st_staging_package() {
    let fixture = Fixture::new().await;
    let environment = fixture.run_environment_digest.clone();
    // Twenty explicit safe-B stagings fill the instance-wide 20-package
    // limit; the install itself stages nothing.
    for index in 0..20 {
        PersistentSeedStore::stage_reset_to_baseline(
            &fixture.admin,
            &fixture.store,
            &fixture.install_id,
            &format!("fill-{index}"),
            &environment,
            COMPILER_VERSION,
        )
        .await
        .unwrap();
    }
    assert_eq!(fixture.staged_packages().await, 20);
    let before = fixture.business_shape().await;

    // At the limit, the replay of an existing key is not a new derivation: it
    // returns the original image and writes nothing.
    let replayed = PersistentSeedStore::stage_reset_to_baseline(
        &fixture.admin,
        &fixture.store,
        &fixture.install_id,
        "fill-0",
        &environment,
        COMPILER_VERSION,
    )
    .await
    .unwrap();
    assert_eq!(replayed.id, staged_id("fill-0"));
    assert_eq!(fixture.business_shape().await, before);

    // The 21st is a new staging package: refused with the typed conflict
    // that names the item, the used count and the limit.
    match PersistentSeedStore::stage_reset_to_baseline(
        &fixture.admin,
        &fixture.store,
        &fixture.install_id,
        "fill-20",
        &environment,
        COMPILER_VERSION,
    )
    .await
    {
        Err(Error::Capacity(detail)) => {
            assert_eq!(
                detail.item,
                evo_core::capacity::CapacityItem::StagingPackage
            );
            assert_eq!((detail.used, detail.limit, detail.delta), (20, 20, 1));
        }
        Err(other) => panic!("unexpected refusal of the 21st staging: {other:?}"),
        Ok(staged) => panic!(
            "PARENT-EXPECTED-FAILURE: the public seed reset admitted a 21st staging package ({})",
            staged.id
        ),
    }
    assert_eq!(fixture.staged_packages().await, 20);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "the refused 21st staging wrote nothing"
    );
    fixture.store.close().await;
}

/// The staged asset, its edge, its audit record and the successful terminal
/// job state share one transaction (§11.1: an interrupted staging is never
/// marked complete). A database fault on the staged asset insert rolls back
/// all of them: the job ends Failed with no result, and no staged row and no
/// staging audit exist. The same key stays that honest failed job; a new key
/// succeeds once the fault is gone.
#[tokio::test]
async fn seed_reset_staging_and_the_job_terminal_state_commit_together() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    fixture.arm_staged_insert_trigger("stop_seed_reset_stage");
    let before = fixture.business_shape().await;
    let failed = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("atomic", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["error_code"], json!("internal_error"));
    assert_eq!(failed["result"], Value::Null);
    assert_eq!(
        fixture.get("artifact", &staged_id("atomic")).await,
        None,
        "the staged row was rolled back with the job's success"
    );
    assert!(!fixture.has_staging_audit());
    assert_eq!(fixture.staged_packages().await, 0);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "the faulted job wrote nothing"
    );

    // The same key is the already accepted intent: it is never re-run into a
    // false success.
    let replay = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("atomic", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(replay["id"], failed["id"]);
    assert_eq!(replay["state"], json!("failed"));
    assert_eq!(replay["result"], Value::Null);
    assert_eq!(fixture.get("artifact", &staged_id("atomic")).await, None);

    fixture.drop_trigger("stop_seed_reset_stage");
    let healthy = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("atomic-healthy", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(healthy["state"], json!("succeeded"), "{healthy}");
    assert_eq!(
        healthy["result"]["staged_asset_id"],
        json!(staged_id("atomic-healthy"))
    );
    let asset = fixture.get("artifact", &staged_id("atomic-healthy")).await;
    assert_eq!(asset.unwrap()["payload"]["state"], json!("staged"));
    assert_eq!(
        fixture.audit_count("e16.seed_reset.stage", &staged_id("atomic-healthy")),
        1
    );
    assert_eq!(fixture.staged_packages().await, 1);
    fixture.store.close().await;
}

/// Cancellation is a persisted request, never a hard stop (§6.1.2). Before
/// the first write it is zero-effect: a queued, never-claimed reset job ends
/// Cancelled with nothing staged. After the committed staging it only records
/// the arrival: the job stays Succeeded with its result, and the staged asset
/// is unchanged.
#[tokio::test]
async fn seed_reset_cancel_before_and_after_the_effect_is_honest() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let before = fixture.business_shape().await;

    // Before the effect: a queued job that was never claimed.
    let queued = seeded_job(
        &fixture.admin,
        "seed.reset_stage",
        "cancel-before",
        ManagementJobState::Queued,
    );
    fixture.put_job(&queued).await;
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, &queued.id)
        .await
        .unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Cancelled,
        "{cancelled:?}"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert_eq!(fixture.business_shape().await, before);
    assert_eq!(fixture.staged_packages().await, 0);

    // After the effect: a committed staging.
    let committed = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("cancel-after", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(committed["state"], json!("succeeded"), "{committed}");
    let after_commit = fixture.business_shape().await;
    let job_id = committed["id"].as_str().unwrap();
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, job_id)
        .await
        .unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Succeeded,
        "a committed staging is an audit fact"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        committed["result"]
    );
    assert_eq!(fixture.business_shape().await, after_commit);
    let asset = fixture.get("artifact", &staged_id("cancel-after")).await;
    assert_eq!(asset.unwrap()["payload"]["state"], json!("staged"));
    fixture.store.close().await;
}

/// A restart finds a claimed job whose worker died with it. The job left no
/// effect, so recovery reads NotCommitted and fails it closed; it never
/// turns Draining.
#[tokio::test]
async fn a_restarted_running_reset_fails_closed_without_draining() {
    let fixture = Fixture::new().await;
    let before = fixture.business_shape().await;
    let running = seeded_job(
        &fixture.admin,
        "seed.reset_stage",
        "restarted",
        ManagementJobState::Running,
    );
    fixture.put_job(&running).await;
    let restarted =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let failed = fixture.terminal(&running.id).await;
    assert_eq!(failed["state"], json!("failed"), "{failed}");
    assert_eq!(failed["step"], json!("worker_exited_before_commit"));
    assert_eq!(failed["error_code"], json!("management_worker_exited"));
    assert_eq!(failed["result"], Value::Null);
    assert_eq!(fixture.business_shape().await, before);
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// The way out of Draining (R10 §1.5). The handler never sets Draining
/// itself; a cancel of a claimed job passes through it on the generic cancel
/// path, and the job then holds the only dispatch slot. Its private input is
/// the tombstone a source revocation leaves, so recovery must not need to
/// read it: the seed short-circuit judges the job by its state and its bound
/// result alone, proves no effect, and lands it on Cancelled. While the job
/// drains, a closure job is still claimed and runs.
#[tokio::test]
async fn a_cancelled_running_reset_leaves_draining_on_cancelled_with_nothing_staged() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    // One committed reset for the closure job to close.
    let committed = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("to-abort", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(committed["state"], json!("succeeded"), "{committed}");
    let running = seeded_job(
        &fixture.admin,
        "seed.reset_stage",
        "cancel-running",
        ManagementJobState::Running,
    );
    fixture
        .put(
            "artifact",
            &running.private_input_ref,
            &json!({
                "id": running.private_input_ref,
                "schema_version": "rsia.redacted.v1",
                "state": "source_revoked",
            }),
        )
        .await;
    fixture.put_job(&running).await;
    let before = fixture.business_shape().await;
    let draining = fixture
        .dispatcher
        .cancel(&fixture.admin, &running.id)
        .await
        .unwrap();
    assert_eq!(
        draining.identity().state,
        ManagementJobState::Draining,
        "{draining:?}"
    );
    assert!(draining.cancel_requested());
    assert_eq!(
        fixture.concurrent_dispatches().await,
        1,
        "the draining job holds the only dispatch slot"
    );

    // A closure job is claimed and runs while the slot is held.
    let closed = fixture
        .run_job(
            "asset.abort",
            json!({
                "schema_version": "rsia.management.asset_abort.v1",
                "request_key": "abort",
                "staged_asset_id": staged_id("to-abort"),
            }),
        )
        .await;
    assert_eq!(closed["state"], json!("succeeded"), "{closed}");
    assert_eq!(closed["result"]["state"], json!("aborted"));
    assert_eq!(
        fixture
            .dispatcher
            .status(&fixture.admin, &running.id)
            .await
            .unwrap()
            .identity()
            .state,
        ManagementJobState::Draining,
        "nothing closes the job before recovery looks at it"
    );

    // A new process owns the data directory and recovers the job.
    let restarted =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let landed = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let job = restarted.status(&fixture.admin, &running.id).await.unwrap();
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
        "{landed:?}"
    );
    assert!(landed.cancel_requested());
    assert_eq!(
        landed.cancel_arrival(),
        Some(ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(landed.result().is_none());
    // Nothing was staged for the cancelled job.
    assert_eq!(
        fixture.get("artifact", &staged_id("cancel-running")).await,
        None
    );
    assert_eq!(
        fixture.audit_count("e16.seed_reset.stage", &staged_id("cancel-running")),
        0
    );
    assert_eq!(
        fixture.staged_packages().await,
        1,
        "only the closed asset of the first reset exists"
    );
    fixture.store.close().await;
}
