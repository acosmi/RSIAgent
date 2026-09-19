//! Persistent seal, restart, purpose, and revocation tests.
use evo_core::evidence::Purpose;
use evo_core::replay::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::replay::{
    load_live_replay_pool, load_live_replay_report, load_live_replay_world, put_replay_report,
    put_replay_world_draft, register_replay_pool, seal_replay_world,
};
use serde_json::json;

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn world() -> ReplayWorldV2 {
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: "world-1".into(),
            cluster_id: "cluster-1".into(),
            partition: WorldPartition::Train,
            purpose: Purpose::Development,
            generation_signature: d("generation"),
            world_context_signature: d("world-context"),
            baseline_context_signature: d("baseline"),
            approved_parent_digest: d("approved"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            scorer_digest: d("scorer"),
            guidance_digest: d("guidance"),
            repair_template_digest: d("repair"),
            input_order_digest: d("order"),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-source-world-1".into(),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: "source-1".into(),
                    content_digest: d("evidence-source-1"),
                },
                ReplaySourceRef {
                    source_id: "source-2".into(),
                    content_digest: d("evidence-source-2"),
                },
                ReplaySourceRef {
                    source_id: "baseline-source-world-1".into(),
                    content_digest: d("pending-baseline"),
                },
            ],
            revoke_watermark: 1,
            prefix_coverage: vec![PrefixCoverageV1 {
                context_signature: d("next"),
                exhausted: true,
            }],
            action_catalog: vec![ReplayActionSpecV1 {
                record_seq: 1,
                generation_signature: d("generation"),
                parent_context_signature: d("baseline"),
                branch_seq: 1,
                target_depth: 1,
                action_kind: ActionKindV1::Widen { root_slot: 1 },
                estimated_cost_upper_micros: Some(1),
                writes_shared_workspace: false,
            }],
        },
        transitions: vec![ReplayTransitionV2 {
            record_id: "opaque".into(),
            record_seq: 1,
            generation_signature: d("generation"),
            parent_context_signature: d("baseline"),
            action_kind: ActionKindV1::Widen { root_slot: 1 },
            next_context_signature: d("next"),
            outcome: ReplayTransitionOutcome::Observed {
                status: ObservedStatus::Valid {
                    quality_micros: 500_000,
                },
            },
            actual_usage: HistoricalUsage::default(),
            source_ids: vec!["source-1".into(), "source-2".into()],
            observation_source_id: "source-1".into(),
        }],
        sealed_digest: None,
    };
    world.manifest.source_closure[2].content_digest =
        replay_baseline_observation_digest(&world.manifest).unwrap();
    world.manifest.source_closure[0].content_digest =
        replay_observation_digest(&world.manifest, &world.transitions[0]).unwrap();
    world
}

fn sealed_world(id: &str, cluster: &str, partition: WorldPartition) -> ReplayWorldV2 {
    let mut value = world();
    value.manifest.world_id = id.into();
    value.manifest.cluster_id = cluster.into();
    value.manifest.partition = partition;
    value.manifest.world_context_signature = d(&format!("workspace-{id}"));
    value.manifest.input_order_digest = d(&format!("order-{id}"));
    let observation_source_id = format!("observation-source-{id}");
    value.manifest.source_closure[0].source_id = observation_source_id.clone();
    value.transitions[0].source_ids[0] = observation_source_id.clone();
    value.transitions[0].observation_source_id = observation_source_id;
    value.manifest.baseline_observation_source_id = format!("baseline-source-{id}");
    value.manifest.source_closure[2].source_id = format!("baseline-source-{id}");
    value.manifest.source_closure[2].content_digest =
        replay_baseline_observation_digest(&value.manifest).unwrap();
    value.manifest.source_closure[0].content_digest =
        replay_observation_digest(&value.manifest, &value.transitions[0]).unwrap();
    value.seal().unwrap();
    value
}

fn report(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    quality: u32,
) -> ReplayReportV2 {
    ReplayReportV2 {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest: world.sealed_digest.clone().unwrap(),
        cluster_id: world.manifest.cluster_id.clone(),
        partition: world.manifest.partition,
        policy_digest: fingerprint(policy).unwrap(),
        profile_digest: fingerprint(profile).unwrap(),
        objective_version: profile.objective.version().into(),
        batches: vec![],
        probe_best_quality_micros: vec![],
        round_best_quality_micros: vec![],
        final_quality_micros: quality,
        probes: 0,
        simulated_rounds: 0,
        parallel_penalty: None,
        no_probe: true,
        attainment_auc_micros: Some(quality),
        q_auc_sim_micros: Some(quality),
        score_v2_micros: Some(i64::from(quality)),
        historical_usage: HistoricalUsage::default(),
        replay_cpu_nanos: 7,
        coverage: ReplayCoverage::default(),
        terminal: ReplayTerminal::PolicyStop,
        terminal_reason: "fixture stop".into(),
        development_only: true,
        revealed_prefix: None,
    }
}

async fn registered_pool(
    store: &Store,
    ctx: &Context,
) -> (ReplayWorldV2, ReplayWorldV2, ReplayPoolManifestV1) {
    let train = sealed_world("world-train", "cluster-train", WorldPartition::Train);
    let select = sealed_world("world-select", "cluster-select", WorldPartition::Select);
    for world in [&train, &select] {
        put_trusted_development_source(
            store,
            ctx,
            &world.manifest.baseline_observation_source_id,
            replay_baseline_observation_bytes(&world.manifest).unwrap(),
        )
        .await;
        put_trusted_development_source(
            store,
            ctx,
            &world.transitions[0].observation_source_id,
            replay_observation_bytes(&world.manifest, &world.transitions[0]).unwrap(),
        )
        .await;
    }
    seal_replay_world(ctx, store, &train).await.unwrap();
    seal_replay_world(ctx, store, &select).await.unwrap();
    let pool = register_replay_pool(
        ctx,
        store,
        &[
            select.manifest.world_id.clone(),
            train.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap();
    (train, select, pool)
}

async fn put_trusted_development_source(store: &Store, ctx: &Context, id: &str, body: Vec<u8>) {
    let mut session = store.session().await.unwrap();
    session
        .put(
            ctx,
            "run",
            id,
            "host",
            &json!({
                "schema_version":"rsia.optimization.source.v1",
                "record": {
                    "id": id,
                    "body": body.clone(),
                    "parent_family": format!("family-{id}"),
                    "task_origin": "trusted_run",
                    "execution_attestation": "trusted_host",
                    "purpose": "development"
                },
                "trace": {
                    "run_id": id,
                    "parent_family": format!("family-{id}"),
                    "source_digest": hash(&body),
                    "purpose": "development",
                    "outcome": "success",
                    "diagnosis": null,
                    "excerpt": String::from_utf8(body.clone()).unwrap(),
                    "seed": 1
                },
                "excerpt_start": 0,
                "excerpt_end": body.len()
            }),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn setup() -> (tempfile::TempDir, Store, Context) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("replay.sqlite3"))
        .await
        .unwrap();
    let ctx = Context::new("n", "worker", Role::Worker).unwrap();
    let fixture = world();
    for id in ["source-1", "source-2", "baseline-source-world-1"] {
        let body = if id == "source-1" {
            replay_observation_bytes(&fixture.manifest, &fixture.transitions[0]).unwrap()
        } else if id == fixture.manifest.baseline_observation_source_id {
            replay_baseline_observation_bytes(&fixture.manifest).unwrap()
        } else {
            format!("evidence-{id}").into_bytes()
        };
        put_trusted_development_source(&store, &ctx, id, body).await;
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx, &d("watermark-1"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    (dir, store, ctx)
}

#[tokio::test]
async fn draft_seal_is_idempotent_and_reopen_preserves_immutable_world() {
    let (dir, store, ctx) = setup().await;
    let draft = world();
    put_replay_world_draft(&ctx, &store, &draft).await.unwrap();
    let mut sealed = draft.clone();
    sealed.seal().unwrap();
    seal_replay_world(&ctx, &store, &sealed).await.unwrap();
    seal_replay_world(&ctx, &store, &sealed).await.unwrap();
    let mut changed = sealed.clone();
    changed.transitions[0].record_id = "changed".into();
    assert!(seal_replay_world(&ctx, &store, &changed).await.is_err());
    store.close().await;

    let reopened = Store::open(&dir.path().join("replay.sqlite3"))
        .await
        .unwrap();
    let loaded = load_live_replay_world(&ctx, &reopened, "world-1", Purpose::Development)
        .await
        .unwrap();
    assert_eq!(loaded.sealed_digest, sealed.sealed_digest);
    assert!(matches!(
        load_live_replay_world(&ctx, &reopened, "world-1", Purpose::Inspection).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn changed_q0_is_rejected_when_trusted_baseline_source_body_does_not_match() {
    let (_dir, store, ctx) = setup().await;
    let mut forged = world();
    forged.manifest.initial_baseline_quality_micros = 1_000_000;
    let changed_digest = replay_baseline_observation_digest(&forged.manifest).unwrap();
    let baseline_source_id = forged.manifest.baseline_observation_source_id.clone();
    forged
        .manifest
        .source_closure
        .iter_mut()
        .find(|source| source.source_id == baseline_source_id)
        .unwrap()
        .content_digest = changed_digest;
    forged.manifest.source_closure[0].content_digest =
        replay_observation_digest(&forged.manifest, &forged.transitions[0]).unwrap();
    forged.seal().unwrap();

    // Pure sealing binds the claimed material. Persistence additionally
    // requires the digest to equal the existing TrustedHost source body.
    assert!(seal_replay_world(&ctx, &store, &forged).await.is_err());
}

#[tokio::test]
async fn old_observation_body_rejects_changed_guidance_and_reduced_source_closure() {
    let (_dir, store, ctx) = setup().await;

    let mut changed_guidance = world();
    changed_guidance.manifest.world_id = "world-changed-guidance".into();
    changed_guidance.manifest.guidance_digest = d("changed-guidance");
    changed_guidance.manifest.source_closure[0].content_digest =
        replay_observation_digest(&changed_guidance.manifest, &changed_guidance.transitions[0])
            .unwrap();
    changed_guidance.seal().unwrap();
    assert!(
        seal_replay_world(&ctx, &store, &changed_guidance)
            .await
            .is_err()
    );

    let mut reduced_sources = world();
    reduced_sources.manifest.world_id = "world-reduced-sources".into();
    reduced_sources
        .manifest
        .source_closure
        .retain(|source| source.source_id != "source-2");
    reduced_sources.transitions[0]
        .source_ids
        .retain(|source_id| source_id != "source-2");
    reduced_sources.manifest.source_closure[0].content_digest =
        replay_observation_digest(&reduced_sources.manifest, &reduced_sources.transitions[0])
            .unwrap();
    reduced_sources.seal().unwrap();
    assert!(
        seal_replay_world(&ctx, &store, &reduced_sources)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revoked_baseline_source_blocks_live_world() {
    let (_dir, store, ctx) = setup().await;
    let mut sealed = world();
    sealed.seal().unwrap();
    seal_replay_world(&ctx, &store, &sealed).await.unwrap();
    let baseline_source_id = sealed.manifest.baseline_observation_source_id.clone();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &ctx,
            "tombstone",
            &baseline_source_id,
            ctx.actor(),
            &json!({"id":baseline_source_id}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&ctx, &d("baseline-revoked"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        load_live_replay_world(&ctx, &store, "world-1", Purpose::Development)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn secondary_source_revoke_blocks_load_and_edges_are_indexed() {
    let (_dir, store, ctx) = setup().await;
    let mut sealed = world();
    sealed.seal().unwrap();
    seal_replay_world(&ctx, &store, &sealed).await.unwrap();
    let mut session = store.session().await.unwrap();
    let dependents = session.dependents(&ctx, "run", "source-2").await.unwrap();
    assert!(dependents.contains(&("replay_world".into(), "world-1".into())));
    session
        .put(
            &ctx,
            "tombstone",
            "source-2",
            "worker",
            &json!({"id":"source-2"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&ctx, &d("watermark-2"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        load_live_replay_world(&ctx, &store, "world-1", Purpose::Development)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sealed_legacy_schema_is_not_v2_authority() {
    let (_dir, store, ctx) = setup().await;
    let mut session = store.session().await.unwrap();
    session
        .put_world(&ctx, "legacy", true, &json!({"version":"legacy"}))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .put_world(&ctx, "legacy", false, &json!({"version":"changed"}))
            .await
            .is_err()
    );
    drop(session);
    assert!(
        load_live_replay_world(&ctx, &store, "legacy", Purpose::Development)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pool_registration_derives_content_and_evaluator_is_read_only() {
    let (_dir, store, worker) = setup().await;
    let (train, select, pool) = registered_pool(&store, &worker).await;
    let again = register_replay_pool(
        &worker,
        &store,
        &[
            train.manifest.world_id.clone(),
            select.manifest.world_id.clone(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(again, pool);
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let loaded = load_live_replay_pool(&evaluator, &store, &pool.pool_digest)
        .await
        .unwrap();
    assert_eq!(loaded.manifest, pool);
    assert_eq!(loaded.worlds.len(), 2);
    assert!(
        register_replay_pool(
            &evaluator,
            &store,
            &["world-train".into(), "world-select".into()]
        )
        .await
        .is_err()
    );
    let profile = ReplaySimulationProfile::default_v2(1, pool.pool_digest.clone());
    assert!(
        profile
            .validate_for_pool(&ExplorationCapsV1::online(), &pool)
            .is_ok()
    );
}

#[tokio::test]
async fn baseline_source_revocation_blocks_pool_and_stored_report_view() {
    let (_dir, store, worker) = setup().await;
    let (train, _select, pool) = registered_pool(&store, &worker).await;
    let policy = ElasticPolicyV1::default();
    let profile = ReplaySimulationProfile::default_v2(1, pool.pool_digest.clone());
    let record = StoredReplayReportV1::build(
        "n",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        ExplorationCapsV1::online(),
        vec![report(&train, &policy, &profile, 100_000)],
    )
    .unwrap();
    put_replay_report(&worker, &store, &record).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &worker,
            "tombstone",
            &train.manifest.baseline_observation_source_id,
            worker.actor(),
            &json!({"id":train.manifest.baseline_observation_source_id}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&worker, &d("watermark-2"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    assert!(
        load_live_replay_pool(&evaluator, &store, &pool.pool_digest)
            .await
            .is_err()
    );
    assert!(
        load_live_replay_report(&evaluator, &store, &record.report_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reports_require_complete_partition_and_are_immutable() {
    let (_dir, store, worker) = setup().await;
    let (train, select, pool) = registered_pool(&store, &worker).await;
    let policy = ElasticPolicyV1::default();
    let profile = ReplaySimulationProfile::default_v2(1, pool.pool_digest.clone());
    let caps = ExplorationCapsV1::online();
    assert!(
        StoredReplayReportV1::build(
            "n",
            WorldPartition::Select,
            &pool,
            policy.clone(),
            profile.clone(),
            caps.clone(),
            vec![],
        )
        .is_err()
    );
    let first = StoredReplayReportV1::build(
        "n",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        caps.clone(),
        vec![report(&train, &policy, &profile, 100_000)],
    )
    .unwrap();
    put_replay_report(&worker, &store, &first).await.unwrap();
    put_replay_report(&worker, &store, &first).await.unwrap();
    let changed = StoredReplayReportV1::build(
        "n",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        caps,
        vec![report(&train, &policy, &profile, 200_000)],
    )
    .unwrap();
    assert_eq!(first.report_id, changed.report_id);
    assert!(put_replay_report(&worker, &store, &changed).await.is_err());
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let loaded = load_live_replay_report(&evaluator, &store, &first.report_id)
        .await
        .unwrap();
    assert_eq!(loaded.reports_body_digest, first.reports_body_digest);
    assert!(put_replay_report(&evaluator, &store, &first).await.is_err());

    let mut leaking_select = select;
    leaking_select.manifest.world_id = "world-leaking-select".into();
    leaking_select.manifest.cluster_id = "cluster-train".into();
    leaking_select.manifest.baseline_observation_source_id = "baseline-source-leaking".into();
    leaking_select.manifest.source_closure[2].source_id = "baseline-source-leaking".into();
    leaking_select.manifest.source_closure[0].source_id = "observation-source-leaking".into();
    leaking_select.transitions[0].source_ids[0] = "observation-source-leaking".into();
    leaking_select.transitions[0].observation_source_id = "observation-source-leaking".into();
    leaking_select.manifest.source_closure[2].content_digest =
        replay_baseline_observation_digest(&leaking_select.manifest).unwrap();
    leaking_select.manifest.source_closure[0].content_digest =
        replay_observation_digest(&leaking_select.manifest, &leaking_select.transitions[0])
            .unwrap();
    leaking_select.sealed_digest = None;
    leaking_select.seal().unwrap();
    put_trusted_development_source(
        &store,
        &worker,
        &leaking_select.manifest.baseline_observation_source_id,
        replay_baseline_observation_bytes(&leaking_select.manifest).unwrap(),
    )
    .await;
    put_trusted_development_source(
        &store,
        &worker,
        &leaking_select.transitions[0].observation_source_id,
        replay_observation_bytes(&leaking_select.manifest, &leaking_select.transitions[0]).unwrap(),
    )
    .await;
    seal_replay_world(&worker, &store, &leaking_select)
        .await
        .unwrap();
    assert!(
        register_replay_pool(
            &worker,
            &store,
            &[train.manifest.world_id, leaking_select.manifest.world_id]
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn replay_store_envelope_must_match_row_namespace_kind_and_id() {
    let (_dir, store, worker) = setup().await;
    let train = sealed_world("strict-train", "strict-c1", WorldPartition::Train);
    let select = sealed_world("strict-select", "strict-c2", WorldPartition::Select);
    for world in [&train, &select] {
        put_trusted_development_source(
            &store,
            &worker,
            &world.manifest.baseline_observation_source_id,
            replay_baseline_observation_bytes(&world.manifest).unwrap(),
        )
        .await;
        put_trusted_development_source(
            &store,
            &worker,
            &world.transitions[0].observation_source_id,
            replay_observation_bytes(&world.manifest, &world.transitions[0]).unwrap(),
        )
        .await;
    }
    seal_replay_world(&worker, &store, &train).await.unwrap();
    seal_replay_world(&worker, &store, &select).await.unwrap();
    let manifest = ReplayPoolManifestV1::build(&[train.clone(), select.clone()]).unwrap();
    let row_id = format!("replay-pool-{}", manifest.pool_digest);
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .put(
                &worker,
                "artifact",
                &row_id,
                worker.actor(),
                &json!({
                    "schema_version":"rsia.replay.store_envelope.v1",
                    "id":"wrong-row-id",
                    "namespace":"n",
                    "record_kind":"replay_pool_manifest_v1",
                    "payload":manifest,
                }),
            )
            .await
            .is_err()
    );
}
