use evo_core::curriculum::ProbeTerminal;
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::hash;
use evo_core::optimization::{
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::replay::*;
use evo_core::skill_edit::EvidenceRef;
use evo_core::strategy::{
    ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus, SimulationContext,
};
use evo_core::{Context, Error, Job, JobState, Role, now};
use evo_engine::capacity::MAX_ACTIVE_LEASES;
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity, WorldState,
    exploration_world_storage_id,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
};
use evo_storage::replay::{register_replay_pool, replay_pool_storage_id, seal_replay_world};
use serde_json::{Value, json};
use std::time::Duration;

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
        "schema_version":"rsia.management.meta_start.v1",
        "request_key":"blocked-1"
    });
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "meta.start", request.clone())
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
        Some("meta.start_consumer_unavailable")
    );
    let reconnect = reopened_dispatcher
        .submit(&admin, "meta.start", request)
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
    // The background consumer fails the job; poll the store (not `status`)
    // until it is terminal instead of assuming a fixed delay.
    let mut first: Option<ManagementJob> = None;
    for _ in 0..400 {
        let mut session = store.session().await.unwrap();
        let job: ManagementJob = session
            .need(&evaluator, "job", &first_queued.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        if job.state != ManagementJobState::Queued && job.state != ManagementJobState::Running {
            first = Some(job);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let first = first.expect("management job did not fail in the background");
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

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn action(seq: u32, parent: &str, branch: u32, kind: ActionKindV1) -> ReplayActionSpecV1 {
    ReplayActionSpecV1 {
        record_seq: seq,
        generation_signature: d("generation-v1"),
        parent_context_signature: parent.into(),
        branch_seq: branch,
        target_depth: if matches!(kind, ActionKindV1::Widen { .. }) {
            1
        } else {
            2
        },
        action_kind: kind,
        estimated_cost_upper_micros: Some(10),
        writes_shared_workspace: false,
    }
}

fn transition(
    seq: u32,
    id: &str,
    parent: &str,
    next: &str,
    kind: ActionKindV1,
    outcome: ReplayTransitionOutcome,
) -> ReplayTransitionV2 {
    ReplayTransitionV2 {
        record_id: id.into(),
        record_seq: seq,
        generation_signature: d("generation-v1"),
        parent_context_signature: parent.into(),
        action_kind: kind,
        next_context_signature: next.into(),
        outcome,
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_micros: Some(1),
            latency_millis: Some(1),
        },
        source_ids: vec![format!("source-{seq}")],
        observation_source_id: format!("source-{seq}"),
    }
}

fn valid(q: u32) -> ReplayTransitionOutcome {
    ReplayTransitionOutcome::Observed {
        status: ObservedStatus::Valid { quality_micros: q },
    }
}

fn refresh_evidence_digests(world: &mut ReplayWorldV2) {
    let digest = replay_baseline_observation_digest(&world.manifest).unwrap();
    let source_id = world.manifest.baseline_observation_source_id.clone();
    world
        .manifest
        .source_closure
        .iter_mut()
        .find(|source| source.source_id == source_id)
        .unwrap()
        .content_digest = digest;
    let observation_digests: Vec<_> = world
        .transitions
        .iter()
        .map(|transition| {
            (
                transition.observation_source_id.clone(),
                replay_observation_digest(&world.manifest, transition).unwrap(),
            )
        })
        .collect();
    for (source_id, digest) in observation_digests {
        world
            .manifest
            .source_closure
            .iter_mut()
            .find(|source| source.source_id == source_id)
            .unwrap()
            .content_digest = digest;
    }
}

fn sample_world() -> ReplayWorldV2 {
    let actions = vec![
        action(1, &d("baseline"), 1, ActionKindV1::Widen { root_slot: 1 }),
        action(
            2,
            &d("ctx-1"),
            1,
            ActionKindV1::Deepen { parent_node_seq: 1 },
        ),
    ];
    let transitions = vec![
        transition(
            1,
            "opaque-root",
            &d("baseline"),
            &d("ctx-1"),
            ActionKindV1::Widen { root_slot: 1 },
            valid(400_000),
        ),
        transition(
            2,
            "opaque-child",
            &d("ctx-1"),
            &d("ctx-2"),
            ActionKindV1::Deepen { parent_node_seq: 1 },
            valid(800_000),
        ),
    ];
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: "world-1".into(),
            cluster_id: "cluster-1".into(),
            partition: WorldPartition::Select,
            purpose: Purpose::Development,
            generation_signature: d("generation-v1"),
            world_context_signature: d("world-context"),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("approved-parent"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-source-1".into(),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: "source-1".into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: "source-2".into(),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: "baseline-source-1".into(),
                    content_digest: d("pending-baseline"),
                },
            ],
            revoke_watermark: 1,
            prefix_coverage: vec![
                PrefixCoverageV1 {
                    context_signature: d("baseline"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: d("ctx-1"),
                    exhausted: false,
                },
                PrefixCoverageV1 {
                    context_signature: d("ctx-2"),
                    exhausted: true,
                },
            ],
            action_catalog: actions,
        },
        transitions,
        sealed_digest: None,
    };
    refresh_evidence_digests(&mut world);
    world.seal().unwrap();
    world
}

fn sample_profile(pool_digest: String) -> ReplaySimulationProfile {
    ReplaySimulationProfile {
        simulation_version: SIMULATION_VERSION.into(),
        objective: ReplayObjective::ParetoAttainmentV2,
        w_sim: 1,
        probe_budget: 2,
        horizon: 2,
        lambda_work_micros: DEFAULT_LAMBDA_MICROS,
        lambda_round_micros: DEFAULT_LAMBDA_MICROS,
        fixed_seed: 9,
        global_recovery_dispatch_limit: 1,
        pool_digest,
        purpose: Purpose::Development,
        target_runtime_profile: "simulation-only".into(),
    }
}

async fn setup_sealed_pool(ctx: &Context, store: &Store) -> ReplayPoolManifestV1 {
    let mut persisted = sample_world();
    persisted.sealed_digest = None;
    persisted.manifest.revoke_watermark = 1;
    persisted.seal().unwrap();

    let mut train = persisted.clone();
    train.sealed_digest = None;
    train.manifest.world_id = "world-train".into();
    train.manifest.cluster_id = "cluster-train".into();
    train.manifest.partition = WorldPartition::Train;
    train.manifest.baseline_observation_source_id = "baseline-source-train".into();
    train.manifest.source_closure[2].source_id = "baseline-source-train".into();
    for (index, transition) in train.transitions.iter_mut().enumerate() {
        let source_id = format!("train-source-{}", index + 1);
        transition.source_ids = vec![source_id.clone()];
        transition.observation_source_id = source_id.clone();
        train.manifest.source_closure[index].source_id = source_id;
    }
    refresh_evidence_digests(&mut train);
    train.seal().unwrap();

    let mut session = store.session().await.unwrap();
    for replay_world in [&persisted, &train] {
        for source in &replay_world.manifest.source_closure {
            let body = if source.source_id == replay_world.manifest.baseline_observation_source_id {
                replay_baseline_observation_bytes(&replay_world.manifest).unwrap()
            } else {
                let transition = replay_world
                    .transitions
                    .iter()
                    .find(|transition| transition.observation_source_id == source.source_id)
                    .unwrap();
                replay_observation_bytes(&replay_world.manifest, transition).unwrap()
            };
            let excerpt = String::from_utf8(body.clone()).unwrap();
            session
                .put(
                    ctx,
                    "run",
                    &source.source_id,
                    "host",
                    &json!({
                        "schema_version":"rsia.optimization.source.v1",
                        "record":{"id":source.source_id,"body":body.clone(),"parent_family":format!("family-{}",source.source_id),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
                        "trace":{"run_id":source.source_id,"parent_family":format!("family-{}",source.source_id),"source_digest":source.content_digest,"purpose":"development","outcome":"success","diagnosis":null,"excerpt":excerpt,"seed":1},
                        "excerpt_start":0,"excerpt_end":body.len()
                    }),
                )
                .await
                .unwrap();
        }
    }
    session.bump_watermark(ctx, &d("watermark")).await.unwrap();
    session.commit().await.unwrap();

    seal_replay_world(ctx, store, &persisted).await.unwrap();
    seal_replay_world(ctx, store, &train).await.unwrap();
    register_replay_pool(
        ctx,
        store,
        &[
            persisted.manifest.world_id.clone(),
            train.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn replay_run_admin_e2e_and_idempotency() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let pool = setup_sealed_pool(&admin, &store).await;
    let profile = sample_profile(pool.pool_digest.clone());
    let policy = ElasticPolicyV1::default();
    let caps = ExplorationCapsV1::online();

    let request = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-k1",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": policy,
        "profile": profile,
        "caps": caps,
    });

    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "replay.run", request.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);

    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert_eq!(done.step, "replay_stored");

    let status = dispatcher.status(&admin, &done.id).await.unwrap();
    assert_eq!(status.state, ManagementJobState::Succeeded);

    let (report_id, pool_digest, semantic_reports_digest) = match status.result {
        Some(ManagementResult::ReplayStored {
            report_id,
            pool_digest,
            semantic_reports_digest,
        }) => (report_id, pool_digest, semantic_reports_digest),
        other => panic!("unexpected result: {:?}", other),
    };
    assert_eq!(pool_digest, pool.pool_digest);

    // Verify report in store
    let loaded = evo_storage::replay::load_live_replay_report(&admin, &store, &report_id)
        .await
        .unwrap();
    assert_eq!(loaded.report_id, report_id);
    assert_eq!(loaded.pool_digest, pool.pool_digest);
    assert_eq!(loaded.semantic_reports_digest, semantic_reports_digest);

    // Verify dependency edge: job -> private_input -> pool
    let mut session = store.session().await.unwrap();
    let pool_storage_id = replay_pool_storage_id(&pool.pool_digest).unwrap();
    let deps = session
        .dependents(&admin, "artifact", &pool_storage_id)
        .await
        .unwrap();
    assert!(
        deps.iter()
            .any(|(kind, id)| kind == "artifact" && id == &done.private_input_ref)
    );
    session.commit().await.unwrap();

    // Idempotency: same request returns same job
    let same = dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    assert_eq!(same.id, queued.id);

    // Conflict: same request_key with different partition
    let different = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-k1",
        "pool_digest": pool.pool_digest,
        "partition": "train",
        "policy": policy,
        "profile": sample_profile(pool.pool_digest.clone()),
        "caps": caps,
    });
    assert!(matches!(
        dispatcher.submit(&admin, "replay.run", different).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn replay_run_role_unknown_fields_and_validation_rejections() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let pool = setup_sealed_pool(&admin, &store).await;
    let profile = sample_profile(pool.pool_digest.clone());

    let payload = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-auth",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": profile,
        "caps": ExplorationCapsV1::online(),
    });

    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();

    // Agent forbidden
    assert!(matches!(
        dispatcher
            .submit(&agent, "replay.run", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));

    // Evaluator forbidden
    assert!(matches!(
        dispatcher
            .submit(&evaluator, "replay.run", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));

    // Unknown field rejected
    let mut unknown = payload.clone();
    unknown["extra_field"] = json!("unexpected");
    assert!(matches!(
        dispatcher.submit(&admin, "replay.run", unknown).await,
        Err(Error::Invalid(_))
    ));

    // Invalid schema version rejected
    let mut bad_version = payload.clone();
    bad_version["schema_version"] = json!("rsia.management.replay_run.v2");
    assert!(matches!(
        dispatcher.submit(&admin, "replay.run", bad_version).await,
        Err(Error::Invalid(_))
    ));

    // Missing field rejected
    let incomplete = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "incomplete-key"
    });
    assert!(matches!(
        dispatcher.submit(&admin, "replay.run", incomplete).await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn replay_run_missing_pool_fails_cleanly() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let non_existent_pool = d("non-existent-pool");
    let profile = sample_profile(non_existent_pool.clone());

    let request = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-missing-pool",
        "pool_digest": non_existent_pool,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": profile,
        "caps": ExplorationCapsV1::online(),
    });

    let queued = dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);

    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.step, "failed");
    assert_eq!(terminal.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn replay_run_status_revocation_gate_and_terminal_preservation() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let pool = setup_sealed_pool(&admin, &store).await;
    let profile = sample_profile(pool.pool_digest.clone());

    let request = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-revoke-test",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": profile,
        "caps": ExplorationCapsV1::online(),
    });

    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);

    // Reading status succeeds initially
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());

    // Also create a cancelled job
    let request_cancel = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-cancel-test",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": sample_profile(pool.pool_digest.clone()),
        "caps": ExplorationCapsV1::online(),
    });
    let queued_cancel = dispatcher
        .submit(&admin, "replay.run", request_cancel)
        .await
        .unwrap();
    let cancelled = dispatcher.cancel(&admin, &queued_cancel.id).await.unwrap();
    assert!(matches!(
        cancelled.state,
        ManagementJobState::Cancelled | ManagementJobState::Succeeded
    ));

    // Now revoke a source member (source-2) by placing a tombstone
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "source-2",
            "admin",
            &json!({"id": "source-2"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    // Now reading the Succeeded job MUST fail because live source verification fails
    assert!(matches!(
        dispatcher.status(&admin, &done.id).await,
        Err(Error::Forbidden)
    ));

    // But if a job was cancelled before claim, reading its status does not revive or re-evaluate sources
    let cancelled_direct = ManagementJob {
        id: "management-job-cancelled-replay".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "replay.run".into(),
        request_key: "cancelled-replay-k".into(),
        payload_digest: "c".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Cancelled,
        step: "cancelled_before_claim".into(),
        private_input_ref: "missing-ref".into(),
        result: None,
        error_code: Some("cancelled".into()),
        cancel_requested: true,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "job",
            &cancelled_direct.id,
            admin.actor(),
            &cancelled_direct,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let observed_cancelled = dispatcher
        .status(&admin, &cancelled_direct.id)
        .await
        .unwrap();
    assert_eq!(observed_cancelled.state, ManagementJobState::Cancelled);
    assert_eq!(observed_cancelled.step, "cancelled_before_claim");
}

#[tokio::test]
async fn replay_run_crash_recovery_reuses_stored_report() {
    let (dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let pool = setup_sealed_pool(&admin, &store).await;
    let profile = sample_profile(pool.pool_digest.clone());

    let request = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "replay-crash-1",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": profile,
        "caps": ExplorationCapsV1::online(),
    });

    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "replay.run", request.clone())
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);

    // Simulate crash and restart
    store.close().await;
    let reopened = Store::open(&dir.path().join("management.sqlite3"))
        .await
        .unwrap();
    let reopened_dispatcher =
        ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    reopened_dispatcher.recover_pending().await.unwrap();

    let recovered_status = reopened_dispatcher.status(&admin, &done.id).await.unwrap();
    assert_eq!(recovered_status.id, done.id);
    assert_eq!(recovered_status.state, ManagementJobState::Succeeded);
    assert_eq!(
        serde_json::to_value(&recovered_status.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // Submitting again connects to the same job
    let resubmit = reopened_dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    assert_eq!(resubmit.id, done.id);
}

// ---------------------------------------------------------------------------
// curriculum.step (E07 management adapter, program sub-scope)
//
// The fixtures below replicate `tests/curriculum_v41.rs` inline because test
// crates cannot import each other. The registered pure-function profile is
// offline and zero-budget, so every scheduled probe terminates immediately
// with `BudgetExhausted` (first window) or `Cooldown` (later windows).
// ---------------------------------------------------------------------------

const CURRICULUM_PROFILE_ID: &str = "pure_function_test_proposal.v1";
const CURRICULUM_STATE_ID: &str = "learner-state";
const CURRICULUM_ENVELOPE_SCHEMA: &str = "rsia.curriculum_artifact_envelope.v1";
const PROBE_JOB_KIND: &str = "coverage_probe_job_v1";
const PROBE_RECEIPT_KIND: &str = "probe_schedule_receipt_v1";

fn curriculum_profile() -> evo_core::curriculum::CurriculumControlProfileV1 {
    let registered = evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1::clamp_i64();
    registered.validate().unwrap();
    evo_core::curriculum::CurriculumControlProfileV1::offline_default(
        CURRICULUM_PROFILE_ID,
        d("task-space"),
        registered.oracle_digest,
        registered.runner_digest,
    )
    .unwrap()
}

fn curriculum_source(
    id: &str,
    source_kind: evo_engine::curriculum::CurriculumSourceKindV1,
    subject_digest: String,
    body: serde_json::Value,
) -> evo_engine::curriculum::CurriculumSourceArtifactV1 {
    evo_engine::curriculum::CurriculumSourceArtifactV1 {
        schema_version: evo_engine::curriculum::CurriculumSourceArtifactV1::SCHEMA.into(),
        id: id.into(),
        source_kind,
        data_use: evo_core::evaluation::DataUse::Development,
        subject_digest,
        body_digest: evo_core::fingerprint(&body).unwrap(),
        body,
        dependency_ids: vec![],
    }
}

fn learner_state() -> evo_core::curriculum::LearnerStateV2 {
    let registered = evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1::clamp_i64();
    evo_core::curriculum::LearnerStateV2 {
        schema_version: evo_core::curriculum::LearnerStateV2::SCHEMA.into(),
        id: CURRICULUM_STATE_ID.into(),
        profile_id: CURRICULUM_PROFILE_ID.into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("environment"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

async fn setup_curriculum(admin: &Context, store: &Store) {
    use evo_engine::curriculum::CurriculumSourceKindV1;
    let coordinator = evo_engine::curriculum::PersistentCurriculumCoordinator::new(
        store.clone(),
        admin.clone(),
        admin.actor(),
    )
    .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets":["clamp-boundary"]}),
        ))
        .await
        .unwrap();
    for (id, body) in evo_engine::curriculum_profiles::registered_profile_source_bodies() {
        let kind = if id.contains("target") || id.contains("source") {
            CurriculumSourceKindV1::TargetSpec
        } else if id.contains("oracle") {
            CurriculumSourceKindV1::OracleSpec
        } else {
            CurriculumSourceKindV1::RunnerSpec
        };
        coordinator
            .register_source(curriculum_source(
                &id,
                kind,
                evo_core::fingerprint(&body).unwrap(),
                body,
            ))
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(admin, "curriculum-initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    coordinator
        .register_profile_and_state(curriculum_profile(), learner_state())
        .await
        .unwrap();
}

fn curriculum_step_request(request_key: &str) -> serde_json::Value {
    json!({
        "schema_version": "rsia.management.curriculum_step.v1",
        "request_key": request_key,
        "profile_id": CURRICULUM_PROFILE_ID,
        "state_id": CURRICULUM_STATE_ID,
        "root_budget_limit_micros": 1_000_000u64,
    })
}

/// Typed curriculum records of one kind, read straight from the store so the
/// assertions do not depend on the consumer's own read path.
async fn curriculum_records(
    admin: &Context,
    store: &Store,
    record_kind: &str,
) -> Vec<serde_json::Value> {
    let mut session = store.session().await.unwrap();
    let values = session
        .list::<serde_json::Value>(admin, "artifact")
        .await
        .unwrap();
    session.commit().await.unwrap();
    values
        .into_iter()
        .filter(|value| {
            value["schema_version"] == CURRICULUM_ENVELOPE_SCHEMA
                && value["record_kind"] == record_kind
        })
        .map(|value| value["payload"].clone())
        .collect()
}

async fn raw_job(admin: &Context, store: &Store, job_id: &str) -> ManagementJob {
    let mut session = store.session().await.unwrap();
    let job: ManagementJob = session.need(admin, "job", job_id).await.unwrap();
    session.commit().await.unwrap();
    job
}

fn probe_scheduled(job: &ManagementJob) -> (String, String, Option<ProbeTerminal>) {
    match &job.result {
        Some(ManagementResult::ProbeScheduled {
            probe_job_id,
            trigger_digest,
            terminal,
        }) => (probe_job_id.clone(), trigger_digest.clone(), *terminal),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test]
async fn curriculum_step_admin_e2e_and_idempotency() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = curriculum_step_request("curriculum-k1");

    // Immediate job_id (plan §6.1): the submit returns before the consumer runs.
    let queued = dispatcher
        .submit(&admin, "curriculum.step", request.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    assert_eq!(queued.step, "accepted");

    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert_eq!(done.step, "probe_scheduled");
    assert!(done.error_code.is_none());
    let (probe_job_id, trigger_digest, terminal) = probe_scheduled(&done);
    assert_eq!(terminal, Some(ProbeTerminal::BudgetExhausted));
    assert!(probe_job_id.starts_with("probe-"));
    assert_eq!(trigger_digest.len(), 64);

    // The read side re-verifies against the live store and agrees.
    let status = dispatcher.status(&admin, &done.id).await.unwrap();
    assert_eq!(status.state, ManagementJobState::Succeeded);
    assert_eq!(
        probe_scheduled(&status),
        (probe_job_id.clone(), trigger_digest.clone(), terminal)
    );
    let view = evo_engine::curriculum::verified_probe_job_view(&admin, &store, &probe_job_id)
        .await
        .unwrap();
    assert_eq!(view.id, probe_job_id);
    assert_eq!(view.trigger_digest, trigger_digest);
    assert_eq!(view.terminal, Some(ProbeTerminal::BudgetExhausted));
    assert_eq!(view.profile_id, CURRICULUM_PROFILE_ID);
    assert_eq!(view.state_id, CURRICULUM_STATE_ID);
    assert_eq!(view.root_budget_limit_micros, 1_000_000);
    assert_eq!(view.curriculum_share_limit_micros, 200_000);
    assert_eq!(view.effective_monetary_limit_micros, 0);
    assert_eq!(view.provider_dispatch_count, 0);

    // Dependency edges: job -> private_input -> {profile, state} artifacts.
    let mut session = store.session().await.unwrap();
    for storage_id in [
        evo_engine::curriculum::curriculum_profile_storage_id(CURRICULUM_PROFILE_ID).unwrap(),
        evo_engine::curriculum::curriculum_state_storage_id(CURRICULUM_STATE_ID).unwrap(),
    ] {
        let dependents = session
            .dependents(&admin, "artifact", &storage_id)
            .await
            .unwrap();
        assert!(
            dependents
                .iter()
                .any(|(kind, id)| kind == "artifact" && id == &done.private_input_ref),
            "missing private-input edge to {storage_id}"
        );
    }
    session.commit().await.unwrap();

    // Exactly one probe job and one schedule receipt bound to this job id.
    let probes = curriculum_records(&admin, &store, PROBE_JOB_KIND).await;
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0]["id"], probe_job_id);
    assert_eq!(probes[0]["terminal"], "budget_exhausted");
    let receipts = curriculum_records(&admin, &store, PROBE_RECEIPT_KIND).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0]["schema_version"],
        "rsia.probe_schedule_receipt.v1"
    );
    assert_eq!(receipts[0]["idempotency_key"], done.id);
    assert_eq!(receipts[0]["probe_job_id"], probe_job_id);
    assert_eq!(receipts[0]["trigger_digest"], trigger_digest);
    assert_eq!(receipts[0]["profile_id"], CURRICULUM_PROFILE_ID);
    assert_eq!(receipts[0]["state_id"], CURRICULUM_STATE_ID);
    assert_eq!(receipts[0]["root_budget_limit_micros"], 1_000_000u64);
    assert_eq!(receipts[0]["revoke_watermark"], 1u64);

    // Idempotency: the identical request reconnects and never re-charges the
    // consumer (no second probe job, no Cooldown window opened).
    let same = dispatcher
        .submit(&admin, "curriculum.step", request)
        .await
        .unwrap();
    assert_eq!(same.id, queued.id);
    assert_eq!(same.state, ManagementJobState::Succeeded);
    tokio::task::yield_now().await;
    assert_eq!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .len(),
        1
    );
    assert_eq!(
        curriculum_records(&admin, &store, PROBE_RECEIPT_KIND)
            .await
            .len(),
        1
    );

    // Same key with a different payload is a conflict, not a second probe.
    let mut different = curriculum_step_request("curriculum-k1");
    different["root_budget_limit_micros"] = json!(2_000_000u64);
    assert!(matches!(
        dispatcher
            .submit(&admin, "curriculum.step", different)
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn curriculum_step_role_unknown_fields_and_validation_rejections() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let payload = curriculum_step_request("curriculum-auth");

    assert!(matches!(
        dispatcher
            .submit(&agent, "curriculum.step", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        dispatcher
            .submit(&evaluator, "curriculum.step", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));

    let mut unknown = payload.clone();
    unknown["extra_field"] = json!("unexpected");
    assert!(matches!(
        dispatcher.submit(&admin, "curriculum.step", unknown).await,
        Err(Error::Invalid(_))
    ));

    // No top-level actor/role/namespace fields: identity comes from the caller.
    for field in ["actor", "role", "namespace"] {
        let mut injected = payload.clone();
        injected[field] = json!("admin");
        assert!(matches!(
            dispatcher.submit(&admin, "curriculum.step", injected).await,
            Err(Error::Invalid(_))
        ));
    }

    let mut bad_version = payload.clone();
    bad_version["schema_version"] = json!("rsia.management.curriculum_step.v2");
    assert!(matches!(
        dispatcher
            .submit(&admin, "curriculum.step", bad_version)
            .await,
        Err(Error::Invalid(_))
    ));

    let incomplete = json!({
        "schema_version": "rsia.management.curriculum_step.v1",
        "request_key": "incomplete-key"
    });
    assert!(matches!(
        dispatcher
            .submit(&admin, "curriculum.step", incomplete)
            .await,
        Err(Error::Invalid(_))
    ));

    let mut bad_identifier = payload.clone();
    bad_identifier["state_id"] = json!("not a valid identifier");
    assert!(matches!(
        dispatcher
            .submit(&admin, "curriculum.step", bad_identifier)
            .await,
        Err(Error::Invalid(_))
    ));

    // Nothing above reached the consumer.
    assert!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn curriculum_step_missing_profile_or_state_fails_cleanly() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let mut missing_profile = curriculum_step_request("curriculum-missing-profile");
    missing_profile["profile_id"] = json!("missing-profile");
    let queued = dispatcher
        .submit(&admin, "curriculum.step", missing_profile)
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.step, "failed");
    assert_eq!(terminal.error_code.as_deref(), Some("not_found"));
    assert!(terminal.result.is_none());

    let mut missing_state = curriculum_step_request("curriculum-missing-state");
    missing_state["state_id"] = json!("missing-state");
    let queued = dispatcher
        .submit(&admin, "curriculum.step", missing_state)
        .await
        .unwrap();
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.step, "failed");
    assert_eq!(terminal.error_code.as_deref(), Some("not_found"));

    // Failed jobs are never re-verified and never mutated the learner state.
    assert_eq!(
        dispatcher.status(&admin, &queued.id).await.unwrap().state,
        ManagementJobState::Failed
    );
    assert!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .is_empty()
    );
    assert!(
        curriculum_records(&admin, &store, PROBE_RECEIPT_KIND)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn curriculum_step_status_revocation_gate_and_terminal_preservation() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "curriculum.step",
            curriculum_step_request("curriculum-revoke"),
        )
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());

    // Tombstoning one member of the state's source closure (plan §11.4)
    // fails the read side closed with Forbidden ...
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "task-space-source",
            "admin",
            &json!({"id": "task-space-source"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        dispatcher.status(&admin, &done.id).await,
        Err(Error::Forbidden)
    ));
    // ... and the persisted terminal is never rewritten.
    let persisted = raw_job(&admin, &store, &done.id).await;
    assert_eq!(persisted.state, ManagementJobState::Succeeded);
    assert_eq!(persisted.step, "probe_scheduled");
    assert_eq!(
        serde_json::to_value(&persisted.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // A watermark bump (source closure changed) is a Conflict.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "curriculum-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        dispatcher.status(&admin, &done.id).await,
        Err(Error::Conflict(_))
    ));
    let persisted = raw_job(&admin, &store, &done.id).await;
    assert_eq!(persisted.state, ManagementJobState::Succeeded);

    // A new request after revocation fails inside the job, not at submit.
    let queued = dispatcher
        .submit(
            &admin,
            "curriculum.step",
            curriculum_step_request("curriculum-after-revoke"),
        )
        .await
        .unwrap();
    let failed = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        dispatcher.status(&admin, &failed.id).await.unwrap().state,
        ManagementJobState::Failed
    );

    // A job cancelled before claim is never re-verified against sources.
    let cancelled_direct = ManagementJob {
        id: "management-job-cancelled-curriculum".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "curriculum.step".into(),
        request_key: "cancelled-curriculum-k".into(),
        payload_digest: "c".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Cancelled,
        step: "cancelled_before_claim".into(),
        private_input_ref: "missing-ref".into(),
        result: None,
        error_code: Some("cancelled".into()),
        cancel_requested: true,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "job",
            &cancelled_direct.id,
            admin.actor(),
            &cancelled_direct,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let observed = dispatcher
        .status(&admin, &cancelled_direct.id)
        .await
        .unwrap();
    assert_eq!(observed.state, ManagementJobState::Cancelled);
    assert_eq!(observed.step, "cancelled_before_claim");
}

#[tokio::test]
async fn curriculum_step_crash_recovery_reuses_probe_schedule_receipt() {
    let (dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = curriculum_step_request("curriculum-crash-1");
    let queued = dispatcher
        .submit(&admin, "curriculum.step", request.clone())
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let (probe_job_id, trigger_digest, terminal) = probe_scheduled(&done);
    assert_eq!(terminal, Some(ProbeTerminal::BudgetExhausted));

    // Simulate a crash after the consumer committed (probe job, state update
    // and receipt are durable) but before `finish_claim` wrote the terminal.
    let mut crashed = done.clone();
    crashed.state = ManagementJobState::Running;
    crashed.step = "before_curriculum_step".into();
    crashed.result = None;
    crashed.error_code = None;
    crashed.lease_token = Some("dead-process-lease".into());
    crashed.lease_until = 0;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &crashed.id, admin.actor(), &crashed)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let reopened = Store::open(&dir.path().join("management.sqlite3"))
        .await
        .unwrap();
    let reopened_dispatcher =
        ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(reopened_dispatcher.recover_pending().await.unwrap(), 1);
    let recovered = wait_terminal(&reopened_dispatcher, &admin, &done.id).await;
    assert_eq!(recovered.id, done.id);
    assert_eq!(recovered.state, ManagementJobState::Succeeded);
    assert_eq!(recovered.step, "probe_scheduled");
    assert_eq!(recovered.generation, done.generation + 1);
    assert_eq!(
        probe_scheduled(&recovered),
        (probe_job_id.clone(), trigger_digest.clone(), terminal)
    );
    assert_eq!(
        serde_json::to_value(&recovered.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // Recovery continued from the persisted receipt (plan §6.7.4): still
    // exactly one probe job, and no Cooldown job was opened by the re-run.
    let probes = curriculum_records(&admin, &reopened, PROBE_JOB_KIND).await;
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0]["id"], probe_job_id);
    assert_eq!(probes[0]["terminal"], "budget_exhausted");
    let receipts = curriculum_records(&admin, &reopened, PROBE_RECEIPT_KIND).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["idempotency_key"], done.id);
    let view = evo_engine::curriculum::verified_probe_job_view(&admin, &reopened, &probe_job_id)
        .await
        .unwrap();
    assert_eq!(view.terminal, Some(ProbeTerminal::BudgetExhausted));

    // Reconnect after restart still resolves to the same job.
    let resubmit = reopened_dispatcher
        .submit(&admin, "curriculum.step", request)
        .await
        .unwrap();
    assert_eq!(resubmit.id, done.id);

    // A genuinely new request key on the same state opens the next window
    // (Cooldown) — proving the re-run above did not consume that window.
    let next = reopened_dispatcher
        .submit(
            &admin,
            "curriculum.step",
            curriculum_step_request("curriculum-crash-2"),
        )
        .await
        .unwrap();
    let next = wait_terminal(&reopened_dispatcher, &admin, &next.id).await;
    assert_eq!(next.state, ManagementJobState::Succeeded);
    let (next_probe_id, _, next_terminal) = probe_scheduled(&next);
    assert_ne!(next_probe_id, probe_job_id);
    assert_eq!(next_terminal, Some(ProbeTerminal::Cooldown));
    assert_eq!(
        curriculum_records(&admin, &reopened, PROBE_JOB_KIND)
            .await
            .len(),
        2
    );
}

#[tokio::test]
async fn curriculum_step_cancel_before_claim_creates_no_probe_job() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    // The store serializes sessions on one connection, so the cancel issued
    // right after submit is applied before the background worker can claim.
    let queued = dispatcher
        .submit(
            &admin,
            "curriculum.step",
            curriculum_step_request("curriculum-cancel"),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let cancelled = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert_eq!(cancelled.step, "cancelled_before_claim");
    assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
    assert!(cancelled.cancel_requested);

    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Cancelled);
    assert_eq!(terminal.step, "cancelled_before_claim");
    assert!(terminal.result.is_none());
    assert!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .is_empty()
    );
    assert!(
        curriculum_records(&admin, &store, PROBE_RECEIPT_KIND)
            .await
            .is_empty()
    );

    // Cancellation is persisted (plan §6.6): reconnecting with the same key
    // returns the cancelled job instead of scheduling.
    let reconnect = dispatcher
        .submit(
            &admin,
            "curriculum.step",
            curriculum_step_request("curriculum-cancel"),
        )
        .await
        .unwrap();
    assert_eq!(reconnect.id, queued.id);
    assert_eq!(reconnect.state, ManagementJobState::Cancelled);
    tokio::task::yield_now().await;
    assert!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// E16.5 AG-014: MVP capacity gates at the management claim entry point
// ---------------------------------------------------------------------------

/// Like `wait_terminal`, but tolerant of the capacity retry delay.
async fn wait_terminal_slowly(
    dispatcher: &ManagementDispatcher,
    context: &Context,
    job_id: &str,
) -> ManagementJob {
    for _ in 0..200 {
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
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("management job did not finish after the capacity wait");
}

fn live_running_job() -> ManagementJob {
    ManagementJob {
        id: "management-job-holding-the-dispatch".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "meta.start".into(),
        request_key: "holding".into(),
        payload_digest: "a".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: "management-input-holding".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: Some("live-lease".into()),
        lease_until: now().saturating_add(60),
        generation: 1,
        diagnostics: vec![],
        created_at: 1,
    }
}

async fn assert_waiting_for_capacity(
    dispatcher: &ManagementDispatcher,
    admin: &Context,
    job_id: &str,
) -> ManagementJob {
    // Let the spawned claim (and a few retries) run.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let waiting = dispatcher.status(admin, job_id).await.unwrap();
    assert_eq!(waiting.state, ManagementJobState::Queued);
    assert_eq!(waiting.step, ManagementDispatcher::CAPACITY_WAIT_STEP);
    assert!(waiting.lease_token.is_none());
    assert_eq!(waiting.lease_until, 0);
    assert_eq!(waiting.generation, 0);
    assert_eq!(waiting.diagnostics.len(), 1);
    assert_eq!(waiting.diagnostics[0].code, "conflict");
    // Retries keep the job Queued without piling up diagnostics.
    tokio::time::sleep(ManagementDispatcher::CAPACITY_WAIT_RETRY * 2).await;
    let still_waiting = dispatcher.status(admin, job_id).await.unwrap();
    assert_eq!(still_waiting.state, ManagementJobState::Queued);
    assert_eq!(still_waiting.diagnostics.len(), 1);
    still_waiting
}

#[tokio::test]
async fn second_management_dispatch_waits_queued_until_the_live_lease_finishes() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let holding = live_running_job();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &holding.id, admin.actor(), &holding)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "meta.start",
            json!({
                "schema_version":"rsia.management.meta_start.v1",
                "request_key":"waits-for-dispatch-capacity"
            }),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    assert_waiting_for_capacity(&dispatcher, &admin, &queued.id).await;

    // The running job finishes and releases its lease: the deferred claim
    // proceeds on its own and the job runs to its normal terminal state.
    let mut finished = holding.clone();
    finished.state = ManagementJobState::Succeeded;
    finished.step = "finished".into();
    finished.lease_token = None;
    finished.lease_until = 0;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &finished.id, admin.actor(), &finished)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let done = wait_terminal_slowly(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Blocked);
    assert_eq!(
        done.error_code.as_deref(),
        Some("meta.start_consumer_unavailable")
    );
    assert_eq!(done.generation, 1);
    assert_eq!(done.diagnostics.len(), 1);
    assert_eq!(done.diagnostics[0].code, "conflict");
    // The job that held the dispatch was never touched.
    let untouched = dispatcher.status(&admin, &holding.id).await.unwrap();
    assert_eq!(untouched.state, ManagementJobState::Succeeded);
}

/// F24: the one concurrent management dispatch is per instance. A live lease
/// held by a job in ns-a defers a claim in ns-b until that lease is released.
#[tokio::test]
async fn management_dispatch_capacity_cannot_be_split_across_namespaces() {
    let (_dir, store) = store().await;
    let admin_a = Context::new("ns-a", "admin", Role::Admin).unwrap();
    let admin_b = Context::new("ns-b", "admin", Role::Admin).unwrap();
    let holding = live_running_job();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin_a, "job", &holding.id, admin_a.actor(), &holding)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin_b.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin_b,
            "meta.start",
            json!({
                "schema_version":"rsia.management.meta_start.v1",
                "request_key":"waits-behind-another-namespace"
            }),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    assert_waiting_for_capacity(&dispatcher, &admin_b, &queued.id).await;

    let mut finished = holding.clone();
    finished.state = ManagementJobState::Succeeded;
    finished.step = "finished".into();
    finished.lease_token = None;
    finished.lease_until = 0;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin_a, "job", &finished.id, admin_a.actor(), &finished)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let done = wait_terminal_slowly(&dispatcher, &admin_b, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Blocked);
    assert_eq!(
        done.error_code.as_deref(),
        Some("meta.start_consumer_unavailable")
    );
    assert_eq!(done.generation, 1);
    assert_eq!(done.diagnostics.len(), 1);
    // ns-b never saw the ns-a job as content; only its lease counted.
    assert!(matches!(
        dispatcher.status(&admin_b, &holding.id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn capacity_deferred_job_can_still_be_cancelled_while_queued() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let holding = live_running_job();
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &holding.id, admin.actor(), &holding)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "meta.start",
            json!({"schema_version":"rsia.management.meta_start.v1","request_key":"cancel-while-waiting"}),
        )
        .await
        .unwrap();
    assert_waiting_for_capacity(&dispatcher, &admin, &queued.id).await;
    let cancelled = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert_eq!(cancelled.step, "cancelled_before_claim");
    // The retry loop observes the terminal state and stops without a claim.
    tokio::time::sleep(ManagementDispatcher::CAPACITY_WAIT_RETRY * 2).await;
    let observed = dispatcher.status(&admin, &queued.id).await.unwrap();
    assert_eq!(observed.state, ManagementJobState::Cancelled);
    assert_eq!(observed.generation, 0);
}

#[tokio::test]
async fn management_claim_waits_while_ten_budget_leases_are_live() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"admin-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 10_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let clock = now();
    let mut calls = Vec::new();
    for index in 0..MAX_ACTIVE_LEASES {
        let call_id = format!("lease-{index}");
        calls.push(
            store
                .reserve_budget_call(
                    &worker,
                    &BudgetCallReservation {
                        billing_scope: "scope-1".into(),
                        call_id: call_id.clone(),
                        dispatch_group_id: "group-1".into(),
                        stage: BudgetStage::Reflection,
                        actual_input_digest: hash(call_id.as_bytes()),
                        request_artifact: None,
                        max_cost_micros: 10,
                        lease_token: format!("token-{index}"),
                        lease_until: clock.saturating_add(600),
                        now: clock,
                    },
                )
                .await
                .unwrap(),
        );
    }
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "meta.start",
            json!({
                "schema_version":"rsia.management.meta_start.v1",
                "request_key":"waits-for-lease-capacity"
            }),
        )
        .await
        .unwrap();
    assert_waiting_for_capacity(&dispatcher, &admin, &queued.id).await;

    // Releasing one lease is enough: the claim then holds the tenth lease.
    let released = &calls[0];
    store
        .release_undispatched_budget_call(
            &worker,
            &BudgetCallFence {
                billing_scope: released.billing_scope.clone(),
                call_id: released.call_id.clone(),
                actual_input_digest: released.actual_input_digest.clone(),
                lease_token: released.lease_token.clone(),
                lease_epoch: released.lease_epoch,
                now: clock.saturating_add(1),
            },
            "capacity_test_release",
        )
        .await
        .unwrap();
    let done = wait_terminal_slowly(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Blocked);
    assert_eq!(done.generation, 1);
}

// ---------------------------------------------------------------------------
// exploration.start (E07 management adapter over the E09 persistent
// coordinator): register a world and take its first pure decision.
// ---------------------------------------------------------------------------

const EXPLORATION_ENVELOPE_SCHEMA: &str = "rsia.exploration_artifact_envelope.v1";
const EXPLORATION_WORLD_KIND: &str = "exploration_world_v1";
const EXPLORATION_RUNS: [&str; 2] = ["run-failure", "run-success"];

/// Same world as `exploration_v41::world()`, parameterised by id and
/// watermark so the context signature stays consistent with the store.
fn exploration_world(id: &str, source_watermark: u64) -> ExplorationWorldV1 {
    let parent_skill = hash(b"parent-skill");
    let parent_bundle = hash(b"parent-bundle");
    let environment = hash(b"environment");
    let model = hash(b"model");
    let tools = hash(b"tools");
    let grader = hash(b"grader");
    let rules = hash(b"rules");
    let context_signature = evo_core::fingerprint(&(
        &parent_skill,
        &parent_bundle,
        &environment,
        &model,
        &tools,
        &grader,
        &rules,
        source_watermark,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: id.into(),
        approved_parent_digest: hash(b"approved-parent"),
        context_signature,
        parent_skill_digest: parent_skill,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        root_opportunities: vec![
            RootOpportunity {
                root_slot: 2,
                branch_seq: 2,
                action_seq: 2,
                estimated_cost_upper_micros: 10,
            },
            RootOpportunity {
                root_slot: 1,
                branch_seq: 1,
                action_seq: 1,
                estimated_cost_upper_micros: 10,
            },
        ],
        dependencies: EXPLORATION_RUNS
            .iter()
            .map(|run| ExplorationDependency {
                kind: "run".into(),
                id: (*run).into(),
            })
            .collect(),
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 2,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    }
}

fn exploration_trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

/// Seeds the trusted source closure exactly like `exploration_v41`: two
/// Host-issued trace authorities, a source selection grant, and one watermark
/// bump so `source_watermark == 1`.
async fn setup_exploration(store: &Store) {
    let host = Context::new("n", "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: exploration_trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: EXPLORATION_RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
}

fn exploration_start_request(request_key: &str, world: &ExplorationWorldV1) -> Value {
    json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": request_key,
        "world": serde_json::to_value(world).unwrap(),
    })
}

/// Persisted world envelopes, read straight from the store so the assertions
/// do not depend on the consumer's own read path.
async fn exploration_worlds(admin: &Context, store: &Store) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let values = session.list::<Value>(admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    values
        .into_iter()
        .filter(|value| {
            value["schema_version"] == EXPLORATION_ENVELOPE_SCHEMA
                && value["record_kind"] == EXPLORATION_WORLD_KIND
        })
        .map(|value| value["payload"].clone())
        .collect()
}

struct ExplorationStarted {
    world_id: String,
    context_signature: String,
    prefix_digest: String,
    legal_actions_digest: String,
    policy_digest: String,
    caps_digest: String,
    action: Value,
}

fn exploration_started(job: &ManagementJob) -> ExplorationStarted {
    match &job.result {
        Some(ManagementResult::ExplorationStarted {
            world_id,
            context_signature,
            prefix_digest,
            legal_actions_digest,
            policy_digest,
            caps_digest,
            action,
        }) => ExplorationStarted {
            world_id: world_id.clone(),
            context_signature: context_signature.clone(),
            prefix_digest: prefix_digest.clone(),
            legal_actions_digest: legal_actions_digest.clone(),
            policy_digest: policy_digest.clone(),
            caps_digest: caps_digest.clone(),
            action: serde_json::to_value(action).unwrap(),
        },
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test]
async fn exploration_start_admin_e2e_and_idempotency() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let world = exploration_world("world-1", 1);
    let request = exploration_start_request("exploration-k1", &world);

    // Immediate job_id (plan §6.1): the submit returns before the consumer runs.
    let queued = dispatcher
        .submit(&admin, "exploration.start", request.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    assert_eq!(queued.step, "accepted");

    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert_eq!(done.step, "exploration_started");
    assert!(done.error_code.is_none());
    let started = exploration_started(&done);
    assert_eq!(started.world_id, "world-1");
    assert_eq!(started.context_signature, world.context_signature);
    assert_eq!(started.prefix_digest.len(), 64);
    assert_eq!(started.legal_actions_digest.len(), 64);
    // E14: the result carries the digests of the policy and caps the first
    // decision was taken with (the built-in policy and the 12/4/1 caps here).
    assert_eq!(
        started.policy_digest,
        ElasticPolicyV1::default().digest().unwrap()
    );
    assert_eq!(
        started.caps_digest,
        ExplorationCapsV1::online().digest().unwrap()
    );
    // First pure decision over the empty prefix: dispatch the lowest root
    // opportunity (plan §7.1), exactly as `exploration_v41` observes it.
    assert_eq!(started.action["decision"], "dispatch");
    assert_eq!(started.action["action_seqs"], json!([1]));

    // The stored digests equal a direct pure decision on the same world.
    let coordinator = PersistentCoordinator::new(store.clone(), admin.clone(), "admin").unwrap();
    let direct = coordinator.decide_next("world-1").await.unwrap();
    assert_eq!(direct.world_id, "world-1");
    assert_eq!(direct.prefix_digest, started.prefix_digest);
    assert_eq!(direct.legal_actions_digest, started.legal_actions_digest);
    assert_eq!(direct.policy_digest, started.policy_digest);
    assert_eq!(direct.caps_digest, started.caps_digest);
    assert_eq!(
        serde_json::to_value(&direct.action).unwrap(),
        started.action
    );

    // The read side re-verifies against the live store and agrees.
    let status = dispatcher.status(&admin, &done.id).await.unwrap();
    assert_eq!(status.state, ManagementJobState::Succeeded);
    assert_eq!(
        serde_json::to_value(&status.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // Exactly one world envelope, still collecting and empty: the management
    // adapter never dispatches a node.
    let worlds = exploration_worlds(&admin, &store).await;
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0]["id"], "world-1");
    assert_eq!(worlds[0]["state"], "collecting");
    assert_eq!(worlds[0]["node_ids"], json!([]));
    assert_eq!(worlds[0]["dispatch_ids"], json!([]));
    assert_eq!(worlds[0]["decision_round"], 0);

    // Dependency edges: job -> private_input -> {runs, world envelope} and
    // world envelope -> runs.
    let world_storage_id = exploration_world_storage_id("world-1").unwrap();
    let mut session = store.session().await.unwrap();
    for run in EXPLORATION_RUNS {
        let dependents = session.dependents(&admin, "run", run).await.unwrap();
        assert!(
            dependents
                .iter()
                .any(|(kind, id)| kind == "artifact" && id == &done.private_input_ref),
            "missing private-input edge to run {run}"
        );
        assert!(
            dependents
                .iter()
                .any(|(kind, id)| kind == "artifact" && id == &world_storage_id),
            "missing world edge to run {run}"
        );
    }
    let dependents = session
        .dependents(&admin, "artifact", &world_storage_id)
        .await
        .unwrap();
    assert!(
        dependents
            .iter()
            .any(|(kind, id)| kind == "artifact" && id == &done.private_input_ref),
        "missing private-input edge to the world envelope"
    );
    let dependents = session
        .dependents(&admin, "artifact", &done.private_input_ref)
        .await
        .unwrap();
    assert!(
        dependents
            .iter()
            .any(|(kind, id)| kind == "job" && id == &done.id)
    );
    session.commit().await.unwrap();

    // Idempotency: the identical request reconnects and never re-charges the
    // consumer (plan §6.1): still one world.
    let same = dispatcher
        .submit(&admin, "exploration.start", request)
        .await
        .unwrap();
    assert_eq!(same.id, queued.id);
    assert_eq!(same.state, ManagementJobState::Succeeded);
    tokio::task::yield_now().await;
    assert_eq!(exploration_worlds(&admin, &store).await.len(), 1);

    // Same key with a different world is a conflict, not a second world.
    let mut different = exploration_world("world-1", 1);
    different.remaining_root_micros = 2_000;
    assert!(matches!(
        dispatcher
            .submit(
                &admin,
                "exploration.start",
                exploration_start_request("exploration-k1", &different)
            )
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(exploration_worlds(&admin, &store).await.len(), 1);
}

#[tokio::test]
async fn exploration_start_same_world_id_converges_or_conflicts() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let world = exploration_world("world-1", 1);
    let first = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-same-1", &world),
        )
        .await
        .unwrap();
    let first = wait_terminal(&dispatcher, &admin, &first.id).await;
    assert_eq!(first.state, ManagementJobState::Succeeded);

    // A new request key carrying the same registration converges on the
    // persisted world (AlreadyRegistered) and reports the same pure decision.
    let again = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-same-2", &world),
        )
        .await
        .unwrap();
    assert_ne!(again.id, first.id);
    let again = wait_terminal(&dispatcher, &admin, &again.id).await;
    assert_eq!(again.state, ManagementJobState::Succeeded);
    assert_eq!(again.step, "exploration_started");
    assert_eq!(
        serde_json::to_value(&again.result).unwrap(),
        serde_json::to_value(&first.result).unwrap()
    );
    assert_eq!(exploration_worlds(&admin, &store).await.len(), 1);

    // The same world id with a different (but individually valid)
    // registration fails inside the job with a typed conflict and leaves the
    // persisted world untouched.
    let mut other = exploration_world("world-1", 1);
    other.approved_parent_digest = hash(b"other-approved-parent");
    let conflicting = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-same-3", &other),
        )
        .await
        .unwrap();
    let conflicting = wait_terminal(&dispatcher, &admin, &conflicting.id).await;
    assert_eq!(conflicting.state, ManagementJobState::Failed);
    assert_eq!(conflicting.step, "failed");
    assert_eq!(conflicting.error_code.as_deref(), Some("conflict"));
    assert!(conflicting.result.is_none());
    let worlds = exploration_worlds(&admin, &store).await;
    assert_eq!(worlds.len(), 1);
    assert_eq!(
        worlds[0]["approved_parent_digest"],
        json!(world.approved_parent_digest)
    );
}

#[tokio::test]
async fn exploration_start_role_unknown_fields_and_validation_rejections() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let agent = Context::new("n", "agent", Role::Agent).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    setup_exploration(&store).await;
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let world = exploration_world("world-auth", 1);
    let payload = exploration_start_request("exploration-auth", &world);

    assert!(matches!(
        dispatcher
            .submit(&agent, "exploration.start", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        dispatcher
            .submit(&evaluator, "exploration.start", payload.clone())
            .await,
        Err(Error::Forbidden)
    ));

    let mut unknown = payload.clone();
    unknown["extra_field"] = json!("unexpected");
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", unknown)
            .await,
        Err(Error::Invalid(_))
    ));

    // Unknown fields are refused at every nesting level of the world.
    let mut unknown_world = payload.clone();
    unknown_world["world"]["extra_field"] = json!("unexpected");
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", unknown_world)
            .await,
        Err(Error::Invalid(_))
    ));
    let mut unknown_caps = payload.clone();
    unknown_caps["world"]["caps"]["extra_field"] = json!(1);
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", unknown_caps)
            .await,
        Err(Error::Invalid(_))
    ));
    let mut unknown_simulation = payload.clone();
    unknown_simulation["world"]["simulation"]["online"]["extra_field"] = json!(1);
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", unknown_simulation)
            .await,
        Err(Error::Invalid(_))
    ));
    let mut unknown_dependency = payload.clone();
    unknown_dependency["world"]["dependencies"][0]["extra_field"] = json!(1);
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", unknown_dependency)
            .await,
        Err(Error::Invalid(_))
    ));

    // No top-level actor/role/namespace fields: identity comes from the caller.
    for field in ["actor", "role", "namespace"] {
        let mut injected = payload.clone();
        injected[field] = json!("admin");
        assert!(matches!(
            dispatcher
                .submit(&admin, "exploration.start", injected)
                .await,
            Err(Error::Invalid(_))
        ));
    }

    let mut bad_version = payload.clone();
    bad_version["schema_version"] = json!("rsia.management.exploration_start.v2");
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", bad_version)
            .await,
        Err(Error::Invalid(_))
    ));

    let incomplete = json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": "incomplete-key"
    });
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", incomplete)
            .await,
        Err(Error::Invalid(_))
    ));

    // The typed source closure must name trusted runs with valid identifiers.
    let mut bad_kind = payload.clone();
    bad_kind["world"]["dependencies"][0]["kind"] = json!("artifact");
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", bad_kind)
            .await,
        Err(Error::Invalid(_))
    ));
    let mut bad_identifier = payload.clone();
    bad_identifier["world"]["id"] = json!("not a valid identifier");
    assert!(matches!(
        dispatcher
            .submit(&admin, "exploration.start", bad_identifier)
            .await,
        Err(Error::Invalid(_))
    ));

    // The blocked representative keeps its typed payload: a world is not
    // accepted where meta.start expects the blocked request shape.
    let mut meta = payload.clone();
    meta["schema_version"] = json!("rsia.management.meta_start.v1");
    assert!(matches!(
        dispatcher.submit(&admin, "meta.start", meta).await,
        Err(Error::Invalid(_))
    ));

    // Nothing above reached the consumer.
    assert!(exploration_worlds(&admin, &store).await.is_empty());
}

#[tokio::test]
async fn exploration_start_invalid_world_fails_cleanly_without_persisting() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    // Empty source closure: rejected by the world's own validation.
    let mut empty_closure = exploration_world("world-empty-closure", 1);
    empty_closure.dependencies.clear();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-empty-closure", &empty_closure),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.step, "failed");
    assert_eq!(terminal.error_code.as_deref(), Some("invalid_input"));
    assert!(terminal.result.is_none());

    // A world signed for another watermark is not live in this store.
    let stale = exploration_world("world-stale", 2);
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-stale", &stale),
        )
        .await
        .unwrap();
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.error_code.as_deref(), Some("conflict"));

    // A context signature that does not match the S0 digests is a conflict.
    let mut forged = exploration_world("world-forged", 1);
    forged.context_signature = hash(b"forged");
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-forged", &forged),
        )
        .await
        .unwrap();
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.error_code.as_deref(), Some("conflict"));

    // Pre-filled scheduling state is not a registration (plan §7.3: no
    // fabricated branches).
    let mut prefilled = exploration_world("world-prefilled", 1);
    prefilled.node_ids.push("node-1".into());
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-prefilled", &prefilled),
        )
        .await
        .unwrap();
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.error_code.as_deref(), Some("invalid_input"));
    let mut sealed = exploration_world("world-sealed", 1);
    sealed.state = WorldState::Sealed;
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-sealed", &sealed),
        )
        .await
        .unwrap();
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.error_code.as_deref(), Some("invalid_input"));

    // Failed jobs are never re-verified and never persisted a world.
    assert_eq!(
        dispatcher.status(&admin, &terminal.id).await.unwrap().state,
        ManagementJobState::Failed
    );
    assert!(exploration_worlds(&admin, &store).await.is_empty());
}

#[tokio::test]
async fn exploration_start_missing_dependency_run_fails_cleanly() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let mut world = exploration_world("world-missing-run", 1);
    world.dependencies.push(ExplorationDependency {
        kind: "run".into(),
        id: "run-missing".into(),
    });
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-missing-run", &world),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Failed);
    assert_eq!(terminal.step, "failed");
    // `check_world_live` fails closed on the missing trusted run.
    assert_eq!(terminal.error_code.as_deref(), Some("not_found"));
    assert!(terminal.result.is_none());
    assert_eq!(
        dispatcher.status(&admin, &terminal.id).await.unwrap().state,
        ManagementJobState::Failed
    );
    assert!(exploration_worlds(&admin, &store).await.is_empty());
}

#[tokio::test]
async fn exploration_start_status_revocation_gate_and_terminal_preservation() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-revoke", &exploration_world("world-1", 1)),
        )
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());

    // Tombstoning one trusted run of the world's source closure (V017/V056)
    // fails the read side closed with Forbidden ...
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "run-failure",
            "admin",
            &json!({"id": "run-failure"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        dispatcher.status(&admin, &done.id).await,
        Err(Error::Forbidden)
    ));
    // ... and the persisted terminal is never rewritten.
    let persisted = raw_job(&admin, &store, &done.id).await;
    assert_eq!(persisted.state, ManagementJobState::Succeeded);
    assert_eq!(persisted.step, "exploration_started");
    assert_eq!(
        serde_json::to_value(&persisted.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // A new world over the revoked closure is refused at submit (plan §11: a
    // revocation refuses new access at once) with a Conflict that names the
    // revoked run, is never persisted, and leaves no job, private input or
    // idempotency row.
    let (jobs_before, artifacts_before) = {
        let mut session = store.session().await.unwrap();
        let counts = (
            session.namespace_object_count(&admin, "job").await.unwrap(),
            session
                .namespace_object_count(&admin, "artifact")
                .await
                .unwrap(),
        );
        session.commit().await.unwrap();
        counts
    };
    let refused = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request(
                "exploration-after-tombstone",
                &exploration_world("world-2", 1),
            ),
        )
        .await;
    let message = match refused {
        Err(Error::Conflict(message)) => message,
        other => panic!("expected a Conflict at submit, got {other:?}"),
    };
    assert!(message.contains("run run-failure"), "{message}");
    assert!(message.contains("revoked"), "{message}");
    assert_eq!(exploration_worlds(&admin, &store).await.len(), 1);
    {
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session.namespace_object_count(&admin, "job").await.unwrap(),
            jobs_before,
            "a refused submission writes no job"
        );
        assert_eq!(
            session
                .namespace_object_count(&admin, "artifact")
                .await
                .unwrap(),
            artifacts_before,
            "a refused submission writes no private input"
        );
        let row = session
            .cached::<ManagementJob, _>(
                &admin,
                "exploration.start",
                "exploration-after-tombstone",
                &json!({}),
            )
            .await;
        assert!(
            matches!(row, Ok(None)),
            "a refused submission leaves no idempotency row: {row:?}"
        );
        session.commit().await.unwrap();
    }

    // A watermark bump (source closure changed) is a Conflict.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "e09-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        dispatcher.status(&admin, &done.id).await,
        Err(Error::Conflict(_))
    ));
    let persisted = raw_job(&admin, &store, &done.id).await;
    assert_eq!(persisted.state, ManagementJobState::Succeeded);

    // The run is still tombstoned after the watermark bump, so a new world is
    // refused at submit as well, again with nothing left behind.
    let refused = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_start_request("exploration-after-bump", &exploration_world("world-3", 1)),
        )
        .await;
    let message = match refused {
        Err(Error::Conflict(message)) => message,
        other => panic!("expected a Conflict at submit, got {other:?}"),
    };
    assert!(message.contains("run run-failure"), "{message}");
    assert!(message.contains("revoked"), "{message}");
    assert_eq!(exploration_worlds(&admin, &store).await.len(), 1);
    {
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session.namespace_object_count(&admin, "job").await.unwrap(),
            jobs_before,
            "a refused submission writes no job"
        );
        assert_eq!(
            session
                .namespace_object_count(&admin, "artifact")
                .await
                .unwrap(),
            artifacts_before,
            "a refused submission writes no private input"
        );
        let row = session
            .cached::<ManagementJob, _>(
                &admin,
                "exploration.start",
                "exploration-after-bump",
                &json!({}),
            )
            .await;
        assert!(
            matches!(row, Ok(None)),
            "a refused submission leaves no idempotency row: {row:?}"
        );
        session.commit().await.unwrap();
    }

    // A job cancelled before claim is never re-verified against sources.
    let cancelled_direct = ManagementJob {
        id: "management-job-cancelled-exploration".into(),
        schema_version: "rsia.management_job.v1".into(),
        operation: "exploration.start".into(),
        request_key: "cancelled-exploration-k".into(),
        payload_digest: "d".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Cancelled,
        step: "cancelled_before_claim".into(),
        private_input_ref: "missing-ref".into(),
        result: None,
        error_code: Some("cancelled".into()),
        cancel_requested: true,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "job",
            &cancelled_direct.id,
            admin.actor(),
            &cancelled_direct,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let observed = dispatcher
        .status(&admin, &cancelled_direct.id)
        .await
        .unwrap();
    assert_eq!(observed.state, ManagementJobState::Cancelled);
    assert_eq!(observed.step, "cancelled_before_claim");
}

#[tokio::test]
async fn exploration_start_crash_recovery_converges_on_registered_world() {
    let (dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request =
        exploration_start_request("exploration-crash-1", &exploration_world("world-1", 1));
    let queued = dispatcher
        .submit(&admin, "exploration.start", request.clone())
        .await
        .unwrap();
    let done = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let started = exploration_started(&done);

    // Simulate a crash after the world registration committed but before
    // `finish_claim` wrote the terminal.
    let mut crashed = done.clone();
    crashed.state = ManagementJobState::Running;
    crashed.step = "before_exploration_start".into();
    crashed.result = None;
    crashed.error_code = None;
    crashed.lease_token = Some("dead-process-lease".into());
    crashed.lease_until = 0;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "job", &crashed.id, admin.actor(), &crashed)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let reopened = Store::open(&dir.path().join("management.sqlite3"))
        .await
        .unwrap();
    let reopened_dispatcher =
        ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(reopened_dispatcher.recover_pending().await.unwrap(), 1);
    let recovered = wait_terminal(&reopened_dispatcher, &admin, &done.id).await;
    assert_eq!(recovered.id, done.id);
    assert_eq!(recovered.state, ManagementJobState::Succeeded);
    assert_eq!(recovered.step, "exploration_started");
    assert_eq!(recovered.generation, done.generation + 1);
    let again = exploration_started(&recovered);
    assert_eq!(again.world_id, started.world_id);
    assert_eq!(again.context_signature, started.context_signature);
    assert_eq!(again.prefix_digest, started.prefix_digest);
    assert_eq!(again.legal_actions_digest, started.legal_actions_digest);
    assert_eq!(again.policy_digest, started.policy_digest);
    assert_eq!(again.caps_digest, started.caps_digest);
    assert_eq!(again.action, started.action);
    assert_eq!(
        serde_json::to_value(&recovered.result).unwrap(),
        serde_json::to_value(&done.result).unwrap()
    );

    // Recovery converged on the persisted world (plan §6.7.4, compare on
    // conflict): still exactly one world, still empty.
    let worlds = exploration_worlds(&admin, &reopened).await;
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0]["id"], "world-1");
    assert_eq!(worlds[0]["node_ids"], json!([]));
    let view = evo_engine::exploration::verified_world_decision_view(&admin, &reopened, "world-1")
        .await
        .unwrap();
    assert_eq!(view.decision.prefix_digest, started.prefix_digest);
    assert_eq!(view.context_signature, started.context_signature);

    // Reconnect after restart still resolves to the same job.
    let resubmit = reopened_dispatcher
        .submit(&admin, "exploration.start", request)
        .await
        .unwrap();
    assert_eq!(resubmit.id, done.id);
    assert_eq!(resubmit.state, ManagementJobState::Succeeded);
}

#[tokio::test]
async fn exploration_start_cancel_before_claim_creates_no_world() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = exploration_start_request("exploration-cancel", &exploration_world("world-1", 1));

    // The store serializes sessions on one connection, so the cancel issued
    // right after submit is applied before the background worker can claim.
    let queued = dispatcher
        .submit(&admin, "exploration.start", request.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    let cancelled = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert_eq!(cancelled.step, "cancelled_before_claim");
    assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
    assert!(cancelled.cancel_requested);

    let terminal = wait_terminal(&dispatcher, &admin, &queued.id).await;
    assert_eq!(terminal.state, ManagementJobState::Cancelled);
    assert_eq!(terminal.step, "cancelled_before_claim");
    assert!(terminal.result.is_none());
    assert!(exploration_worlds(&admin, &store).await.is_empty());

    // Cancellation is persisted (plan §6.6, V008): reconnecting with the same
    // key returns the cancelled job instead of registering.
    let reconnect = dispatcher
        .submit(&admin, "exploration.start", request)
        .await
        .unwrap();
    assert_eq!(reconnect.id, queued.id);
    assert_eq!(reconnect.state, ManagementJobState::Cancelled);
    tokio::task::yield_now().await;
    assert!(exploration_worlds(&admin, &store).await.is_empty());
}
