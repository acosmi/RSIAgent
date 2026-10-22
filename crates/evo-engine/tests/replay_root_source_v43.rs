//! AG147 current replay collection over a real temporary Store.
//! The existing Host issuer is a controlled local schema fixture. These tests
//! grant no real OS/Provider/paid authority and do not claim a real Host run.
//! They use the actual source issuer, E09 registration, collector, seal and
//! reopen paths; no direct DB authority rewrite or test-only production bypass.
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::replay::*;
use evo_core::strategy::{
    ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus, SimulationContext,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::{ExplorationStartRequest, ManagementDispatcher, ManagementJobState};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::replay::{
    ReplayWorldV3Seed, collect_replay_transition_v3, collect_replay_world_v3_draft,
    prepare_replay_world_v3, seal_replay_world_v3,
};
use evo_storage::replay::load_replay_world_v3;
use evo_storage::{Store, lifecycle::read_control_plane_facts};

fn d(label: &str) -> String {
    hash(label.as_bytes())
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    host: Context,
    registered: ExplorationWorldV1,
}
impl Fixture {
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("replay-v3.sqlite3")
    }
}
async fn issue_controlled_source(fixture: &Fixture, id: &str, body: Vec<u8>) {
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.clone(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(&body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: String::from_utf8(body.clone()).unwrap(),
            seed: 7,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(&fixture.store, &fixture.host, &authority)
        .await
        .unwrap();
}
async fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("replay-v3.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let mut registered = ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: "e09-original".into(),
        approved_parent_digest: d("controlled-parent"),
        context_signature: String::new(),
        parent_skill_digest: d("skill"),
        parent_bundle_digest: d("bundle"),
        environment_digest: d("environment"),
        model_digest: d("model"),
        tools_digest: d("tools"),
        grader_digest: d("grader"),
        rules_digest: d("rules"),
        source_watermark: 1,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        // Preserve the original order; the new material separately sorts slots.
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
        dependencies: vec![ExplorationDependency {
            kind: "run".into(),
            id: "registered-input".into(),
        }],
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 100_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 1,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    };
    registered.context_signature = fingerprint(&(
        &registered.parent_skill_digest,
        &registered.parent_bundle_digest,
        &registered.environment_digest,
        &registered.model_digest,
        &registered.tools_digest,
        &registered.grader_digest,
        &registered.rules_digest,
        registered.source_watermark,
    ))
    .unwrap();
    let fixture = Fixture {
        dir,
        store,
        admin,
        host,
        registered,
    };
    issue_controlled_source(
        &fixture,
        "registered-input",
        b"actual controlled S0 input".to_vec(),
    )
    .await;
    let mut session = fixture.store.session().await.unwrap();
    session
        .bump_watermark(&fixture.host, &d("initial-source"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    PersistentCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
        .unwrap()
        .register_world(fixture.registered.clone())
        .await
        .unwrap();
    fixture
}
fn seed(fixture: &Fixture) -> ReplayWorldV3Seed {
    ReplayWorldV3Seed {
        cluster_id: "cluster".into(),
        partition: WorldPartition::Train,
        generation_signature: d("generation"),
        world_context_signature: fixture.registered.context_signature.clone(),
        baseline_context_signature: d("baseline-context"),
        approved_parent_digest: fixture.registered.approved_parent_digest.clone(),
        model_digest: fixture.registered.model_digest.clone(),
        tools_digest: fixture.registered.tools_digest.clone(),
        scorer_digest: fixture.registered.grader_digest.clone(),
        guidance_digest: d("frozen-guidance"),
        repair_template_digest: d("frozen-repair"),
        input_order_digest: d("frozen-order"),
        initial_baseline_quality_micros: 100_000,
        baseline_observation_source_id: "baseline-v2".into(),
        prefix_coverage: vec![PrefixCoverageV1 {
            context_signature: d("baseline-context"),
            exhausted: false,
        }],
        action_catalog: vec![],
    }
}
async fn collecting(fixture: &Fixture) -> ReplayWorldV3 {
    let world = prepare_replay_world_v3(
        &fixture.admin,
        &fixture.store,
        &fixture.registered.id,
        seed(fixture),
    )
    .await
    .unwrap();
    assert_eq!(
        world.manifest.root_source.registration_digest,
        fixture.registered.registration_fingerprint().unwrap()
    );
    assert_eq!(
        world
            .manifest
            .root_source
            .roots
            .iter()
            .map(|root| root.root_slot)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    issue_controlled_source(
        fixture,
        &world.manifest.baseline_observation_source_id,
        replay_baseline_observation_bytes_v2(&world.manifest).unwrap(),
    )
    .await;
    collect_replay_world_v3_draft(&fixture.admin, &fixture.store, &world)
        .await
        .unwrap();
    world
}
fn observed(world: &ReplayWorldV3) -> (ReplayActionSpecV1, ReplayTransitionV2) {
    let action = ReplayActionSpecV1 {
        record_seq: 1,
        generation_signature: world.manifest.generation_signature.clone(),
        parent_context_signature: world.manifest.baseline_context_signature.clone(),
        branch_seq: 1,
        target_depth: 1,
        action_kind: ActionKindV1::Widen { root_slot: 1 },
        estimated_cost_upper_micros: Some(10),
        writes_shared_workspace: false,
    };
    let transition = ReplayTransitionV2 {
        record_id: "opaque-observation".into(),
        record_seq: 1,
        generation_signature: action.generation_signature.clone(),
        parent_context_signature: action.parent_context_signature.clone(),
        action_kind: action.action_kind.clone(),
        next_context_signature: d("next-context"),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: 500_000,
            },
        },
        actual_usage: HistoricalUsage {
            input_tokens: 1,
            output_tokens: 2,
            cost_micros: Some(10),
            latency_millis: Some(3),
        },
        source_ids: vec!["observation-v3".into()],
        observation_source_id: "observation-v3".into(),
    };
    (action, transition)
}
async fn counts(fixture: &Fixture) -> (u64, Vec<u64>) {
    let mut session = fixture.store.session().await.unwrap();
    let worlds = session.replay_world_count().await.unwrap();
    let mut counts = Vec::new();
    for kind in ["run", "artifact", "job", "tombstone"] {
        counts.push(
            session
                .namespace_object_count(&fixture.admin, kind)
                .await
                .unwrap(),
        );
    }
    session.commit().await.unwrap();
    (worlds, counts)
}

#[tokio::test]
async fn actual_collecting_seal_reopen_preserves_original_sources_and_full_binding() {
    let fixture = setup().await;
    let before_registered = fixture.registered.registration_fingerprint().unwrap();
    let world = collecting(&fixture).await;
    assert!(matches!(
        load_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            Purpose::Development
        )
        .await,
        Err(Error::Conflict(_))
    ));
    let (action, transition) = observed(&world);
    // Canonical preparation needs no placeholder digest for its own future run.
    let bytes = replay_observation_bytes_v3(&world.manifest, &transition).unwrap();
    issue_controlled_source(&fixture, &transition.observation_source_id, bytes).await;
    let updated = collect_replay_transition_v3(
        &fixture.admin,
        &fixture.store,
        &world.manifest.world_id,
        &action,
        &transition,
    )
    .await
    .unwrap();
    assert_eq!(updated.transitions.len(), 1);
    assert_eq!(updated.transitions[0].actual_usage.cost_micros, Some(10));
    let sealed = seal_replay_world_v3(&fixture.admin, &fixture.store, &world.manifest.world_id)
        .await
        .unwrap();
    sealed.validate_sealed().unwrap();
    assert_eq!(
        fixture.registered.registration_fingerprint().unwrap(),
        before_registered
    );
    assert_eq!(sealed.manifest.root_source, world.manifest.root_source);
    let path = fixture.path();
    fixture.store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let loaded = load_replay_world_v3(
        &worker,
        &reopened,
        &sealed.manifest.world_id,
        Purpose::Development,
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(loaded.world()).unwrap(),
        fingerprint(&sealed).unwrap()
    );
    let mut session = reopened.session().await.unwrap();
    loaded
        .revalidate_in_session(&mut session, &worker)
        .await
        .unwrap();
    session.commit().await.unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn missing_or_legacy_baseline_never_creates_a_world_or_consumed_fact() {
    for legacy_body in [false, true] {
        let fixture = setup().await;
        let world = prepare_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &fixture.registered.id,
            seed(&fixture),
        )
        .await
        .unwrap();
        if legacy_body {
            issue_controlled_source(&fixture, "baseline-v2",
                b"{\"schema_version\":\"rsia.replay_baseline_observation.v1\",\"observation_kind\":\"observed\"}".to_vec()).await;
        }
        let facts = read_control_plane_facts(&fixture.path()).await.unwrap();
        let before_counts = counts(&fixture).await;
        let result = collect_replay_world_v3_draft(&fixture.admin, &fixture.store, &world).await;
        if legacy_body {
            assert!(matches!(result, Err(Error::Invalid(_))));
        } else {
            assert!(matches!(result, Err(Error::NotFound)));
        }
        assert_eq!(counts(&fixture).await, before_counts);
        assert_eq!(
            read_control_plane_facts(&fixture.path()).await.unwrap(),
            facts
        );
        let mut session = fixture.store.session().await.unwrap();
        assert!(
            session
                .get_world(&fixture.admin, &world.manifest.world_id)
                .await
                .unwrap()
                .is_none()
        );
        session.commit().await.unwrap();
        fixture.store.close().await;
    }
}

#[tokio::test]
async fn caller_role_and_s0_are_not_authority_and_seal_requires_an_actual_draft() {
    let fixture = setup().await;
    let mut wrong_seed = seed(&fixture);
    wrong_seed.model_digest = d("other-model");
    assert!(matches!(
        prepare_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &fixture.registered.id,
            wrong_seed
        )
        .await,
        Err(Error::Conflict(_))
    ));
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    assert!(matches!(
        prepare_replay_world_v3(
            &worker,
            &fixture.store,
            &fixture.registered.id,
            seed(&fixture)
        )
        .await,
        Err(Error::Forbidden)
    ));
    let world = prepare_replay_world_v3(
        &fixture.admin,
        &fixture.store,
        &fixture.registered.id,
        seed(&fixture),
    )
    .await
    .unwrap();
    issue_controlled_source(
        &fixture,
        "baseline-v2",
        replay_baseline_observation_bytes_v2(&world.manifest).unwrap(),
    )
    .await;
    let before = counts(&fixture).await;
    assert!(matches!(
        collect_replay_world_v3_draft(&worker, &fixture.store, &world).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        seal_replay_world_v3(&fixture.admin, &fixture.store, &world.manifest.world_id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(counts(&fixture).await, before);
    fixture.store.close().await;
}

#[tokio::test]
async fn existing_observation_cannot_be_rewritten_or_appended_after_seal() {
    let fixture = setup().await;
    let world = collecting(&fixture).await;
    let (action, transition) = observed(&world);
    issue_controlled_source(
        &fixture,
        "observation-v3",
        replay_observation_bytes_v3(&world.manifest, &transition).unwrap(),
    )
    .await;
    let updated = collect_replay_transition_v3(
        &fixture.admin,
        &fixture.store,
        &world.manifest.world_id,
        &action,
        &transition,
    )
    .await
    .unwrap();
    let before = fingerprint(&updated).unwrap();
    let mut changed = transition.clone();
    changed.actual_usage.cost_micros = Some(0);
    assert!(matches!(
        collect_replay_transition_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            &action,
            &changed
        )
        .await,
        Err(Error::Conflict(_))
    ));
    let sealed = seal_replay_world_v3(&fixture.admin, &fixture.store, &world.manifest.world_id)
        .await
        .unwrap();
    assert_eq!(fingerprint(&updated).unwrap(), before);
    assert!(matches!(
        collect_replay_transition_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            &action,
            &transition
        )
        .await,
        Err(Error::Conflict(_))
    ));
    let loaded = load_replay_world_v3(
        &fixture.admin,
        &fixture.store,
        &world.manifest.world_id,
        Purpose::Development,
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(loaded.world()).unwrap(),
        fingerprint(&sealed).unwrap()
    );
    let other = Context::new("other", "worker", Role::Worker).unwrap();
    assert!(matches!(
        load_replay_world_v3(
            &other,
            &fixture.store,
            &world.manifest.world_id,
            Purpose::Development
        )
        .await,
        Err(Error::NotFound)
    ));
    fixture.store.close().await;
}

async fn add_original(fixture: &Fixture, key: &str) -> String {
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let job = dispatcher
        .submit(
            &fixture.admin,
            "exploration.start",
            serde_json::to_value(ExplorationStartRequest {
                schema_version: "rsia.management.exploration_start.v1".into(),
                request_key: key.into(),
                world: fixture.registered.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    for _ in 0..1000 {
        let current = dispatcher.status(&fixture.admin, &job.id).await.unwrap();
        if current.state == ManagementJobState::Succeeded {
            return job.id;
        }
        assert!(!matches!(
            current.state,
            ManagementJobState::Failed | ManagementJobState::Cancelled
        ));
        tokio::task::yield_now().await;
    }
    panic!("actual exploration registration job did not complete");
}
#[tokio::test]
async fn later_valid_original_keeps_the_existing_genuine_proof_choice() {
    for management_first in [false, true] {
        let fixture = setup().await;
        let first = if management_first {
            Some(add_original(&fixture, "first").await)
        } else {
            None
        };
        let world = collecting(&fixture).await;
        let sealed = seal_replay_world_v3(&fixture.admin, &fixture.store, &world.manifest.world_id)
            .await
            .unwrap();
        let key = if let Some(first) = first {
            (0..1000)
                .map(|i| format!("later-{i}"))
                .find(|key| {
                    let digest = hash(format!("n\0admin\0exploration.start\0{key}").as_bytes());
                    format!("management-job-{}", &digest[..24]) < first
                })
                .unwrap()
        } else {
            "later".into()
        };
        add_original(&fixture, &key).await;
        let loaded = load_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            Purpose::Development,
        )
        .await
        .unwrap();
        assert_eq!(
            loaded.world().manifest.root_source,
            sealed.manifest.root_source
        );
        assert_eq!(
            loaded.world().registration_image_utf8,
            sealed.registration_image_utf8
        );
        fixture.store.close().await;
    }
}

#[tokio::test]
async fn actual_baseline_source_cannot_cross_partition_even_with_identical_s0() {
    let fixture = setup().await;
    let train = prepare_replay_world_v3(
        &fixture.admin,
        &fixture.store,
        &fixture.registered.id,
        seed(&fixture),
    )
    .await
    .unwrap();
    let mut select_seed = seed(&fixture);
    select_seed.partition = WorldPartition::Select;
    let select = prepare_replay_world_v3(
        &fixture.admin,
        &fixture.store,
        &fixture.registered.id,
        select_seed,
    )
    .await
    .unwrap();
    assert_ne!(train.manifest.world_id, select.manifest.world_id);
    let train_bytes = replay_baseline_observation_bytes_v2(&train.manifest).unwrap();
    assert_ne!(
        train_bytes,
        replay_baseline_observation_bytes_v2(&select.manifest).unwrap()
    );
    issue_controlled_source(
        &fixture,
        &train.manifest.baseline_observation_source_id,
        train_bytes,
    )
    .await;
    let before = counts(&fixture).await;
    let facts = read_control_plane_facts(&fixture.path()).await.unwrap();
    assert!(matches!(
        collect_replay_world_v3_draft(&fixture.admin, &fixture.store, &select).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(counts(&fixture).await, before);
    assert_eq!(
        read_control_plane_facts(&fixture.path()).await.unwrap(),
        facts
    );
    // The original partition still has the real source and remains healthy.
    collect_replay_world_v3_draft(&fixture.admin, &fixture.store, &train)
        .await
        .unwrap();
    fixture.store.close().await;
}

#[tokio::test]
async fn actual_current_collecting_and_sealed_world_cleanup_removes_original_root_material() {
    use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
    for sealed in [false, true] {
        let fixture = setup().await;
        let draft = collecting(&fixture).await;
        if sealed {
            let world =
                seal_replay_world_v3(&fixture.admin, &fixture.store, &draft.manifest.world_id)
                    .await
                    .unwrap();
            let read = load_replay_world_v3(
                &fixture.admin,
                &fixture.store,
                &world.manifest.world_id,
                Purpose::Development,
            )
            .await
            .unwrap();
            assert_eq!(
                fingerprint(read.world()).unwrap(),
                fingerprint(&world).unwrap()
            );
        }
        let before = read_control_plane_facts(&fixture.path()).await.unwrap();
        let mut status = LifecycleStore::begin_revoke(
            &fixture.admin,
            &fixture.store,
            TypedObjectRef {
                kind: "run".into(),
                id: draft.manifest.baseline_observation_source_id.clone(),
            },
            "controlled Current baseline revocation",
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
        for _ in 0..128 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(
                &fixture.admin,
                &fixture.store,
                &status.job_id,
                64,
                evo_core::now().unwrap(),
            )
            .await
            .unwrap();
        }
        assert_eq!(status.state, CleanupState::Complete, "{status:?}");
        let mut session = fixture.store.session().await.unwrap();
        let (_, raw) = session
            .get_world(&fixture.admin, &draft.manifest.world_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(raw["schema_version"], "rsia.redacted.v1");
        assert_eq!(raw["original_schema"], REPLAY_WORLD_SCHEMA_V3);
        assert!(raw.get("manifest").is_none());
        assert!(raw.get("registration_image_utf8").is_none());
        assert_eq!(raw["historical_transitions"], serde_json::json!([]));
        session.commit().await.unwrap();
        match load_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &draft.manifest.world_id,
            Purpose::Development,
        )
        .await
        {
            Err(Error::Conflict(reason)) => assert_eq!(
                reason,
                format!(
                    "replay replay_world {} was redacted because its source was revoked",
                    draft.manifest.world_id
                )
            ),
            other => panic!("cleaned current world remained usable: {other:?}"),
        }
        let after = read_control_plane_facts(&fixture.path()).await.unwrap();
        assert_eq!(after.protected_objects, before.protected_objects);
        assert_eq!(after.root_budget_tables, before.root_budget_tables);
        fixture.store.close().await;
    }
}

#[tokio::test]
async fn actual_current_world_backup_restore_reuses_the_full_loader_in_a_new_store() {
    let fixture = setup().await;
    let world = collecting(&fixture).await;
    let sealed = seal_replay_world_v3(&fixture.admin, &fixture.store, &world.manifest.world_id)
        .await
        .unwrap();
    let root = fixture.dir.path().canonicalize().unwrap();
    let source_path = root.join("replay-v3.sqlite3");
    let backup = root.join("qualified-world-backup");
    fixture.store.backup(&backup).await.unwrap();
    let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: evo_storage::lifecycle::BackupManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();
    let base = manifest
        .watermarks
        .iter()
        .find(|row| row.namespace == "n")
        .unwrap();
    let delta = root.join("equal-world-delta.json");
    std::fs::write(
        &delta,
        serde_json::to_vec(&serde_json::json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&manifest_bytes),
            "namespaces":[{
                "namespace":"n","base_seq":base.seq,"base_digest":base.digest,
                "events":[],"latest_seq":base.seq,"latest_digest":base.digest
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fixture.store.close().await;
    let destination = root.join("restored-qualified-world");
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/restore_backup.py"
    );
    let result = std::process::Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(script)
        .arg("--backup")
        .arg(&backup)
        .arg("--dest")
        .arg(&destination)
        .arg("--revoke-delta")
        .arg(&delta)
        .arg("--trusted-revocations-db")
        .arg(&source_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "real restore failed: {:?}\n{}\n{}",
        result.status.code(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let restored_path = destination.join("rsia.sqlite3");
    let decision = evo_engine::startup_gate::StartupGate::new(&restored_path, Some(&source_path))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(
        decision.posture(),
        evo_engine::startup_gate::RecoveryPosture::RestoredVerified
    );
    let restored = Store::open(&restored_path).await.unwrap();
    decision.admit().unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let actual = load_replay_world_v3(
        &worker,
        &restored,
        &sealed.manifest.world_id,
        Purpose::Development,
    )
    .await
    .unwrap();
    assert_eq!(
        fingerprint(actual.world()).unwrap(),
        fingerprint(&sealed).unwrap()
    );
    let mut session = restored.session().await.unwrap();
    actual
        .revalidate_in_session(&mut session, &worker)
        .await
        .unwrap();
    session.commit().await.unwrap();
    restored.close().await;
}

#[tokio::test]
async fn committed_root_slot_branch_and_action_identity_are_each_checked_before_append() {
    for mutation in ["slot", "branch", "action"] {
        let fixture = setup().await;
        let world = collecting(&fixture).await;
        let (mut action, mut transition) = observed(&world);
        transition.observation_source_id = "uncommitted-observation".into();
        transition.source_ids = vec![transition.observation_source_id.clone()];
        match mutation {
            "slot" => action.action_kind = ActionKindV1::Widen { root_slot: 999 },
            "branch" => action.branch_seq = 999,
            "action" => action.record_seq = 999,
            _ => unreachable!(),
        }
        transition.action_kind = action.action_kind.clone();
        transition.record_seq = action.record_seq;
        issue_controlled_source(
            &fixture,
            &transition.observation_source_id,
            replay_observation_bytes_v3(&world.manifest, &transition).unwrap(),
        )
        .await;
        let before = fixture.dir.path().join("before-uncommitted-append");
        fixture.store.backup(&before).await.unwrap();
        let result = collect_replay_transition_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            &action,
            &transition,
        )
        .await;
        match (mutation, result) {
            ("slot", Err(Error::Invalid(reason))) => {
                assert_eq!(reason, "replay Widen is outside registered roots");
            }
            ("branch" | "action", Err(Error::Conflict(reason))) => {
                assert_eq!(reason, "replay Widen differs from registered root");
            }
            (_, other) => {
                panic!("{mutation}: expected exact typed root mapping refusal, got {other:?}");
            }
        }
        let after = fixture.dir.path().join("after-uncommitted-append");
        fixture.store.backup(&after).await.unwrap();
        assert_eq!(
            std::fs::read(before.join("rsia.sqlite3")).unwrap(),
            std::fs::read(after.join("rsia.sqlite3")).unwrap(),
            "{mutation}: rejected append changed objects/worlds/edges/audit"
        );
        let (valid_action, valid_transition) = observed(&world);
        issue_controlled_source(
            &fixture,
            &valid_transition.observation_source_id,
            replay_observation_bytes_v3(&world.manifest, &valid_transition).unwrap(),
        )
        .await;
        let healthy = collect_replay_transition_v3(
            &fixture.admin,
            &fixture.store,
            &world.manifest.world_id,
            &valid_action,
            &valid_transition,
        )
        .await
        .unwrap();
        assert_eq!(healthy.transitions.len(), 1);
        assert_eq!(healthy.transitions[0].record_seq, 1);
        assert_eq!(healthy.manifest.root_source, world.manifest.root_source);
        fixture.store.close().await;
    }
}
