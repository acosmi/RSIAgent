use evo_core::evidence::Purpose;
use evo_core::hash;
use evo_core::replay::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Error, Job, JobState, Role};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_storage::Store;
use evo_storage::replay::{register_replay_pool, replay_pool_storage_id, seal_replay_world};
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
        "schema_version":"rsia.management.curriculum_step.v1",
        "request_key":"blocked-1"
    });
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(&admin, "curriculum.step", request.clone())
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
        Some("curriculum.step_consumer_unavailable")
    );
    let reconnect = reopened_dispatcher
        .submit(&admin, "curriculum.step", request)
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
