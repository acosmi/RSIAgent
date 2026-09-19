//! Immutable replay contract tests.
use evo_core::evidence::Purpose;
use evo_core::hash;
use evo_core::replay::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};

fn digest(label: &str) -> String {
    hash(label.as_bytes())
}

fn refresh_evidence_digests(world: &mut ReplayWorldV2) {
    let digest = replay_baseline_observation_digest(&world.manifest).unwrap();
    let source_id = world.manifest.baseline_observation_source_id.clone();
    let source = world
        .manifest
        .source_closure
        .iter_mut()
        .find(|source| source.source_id == source_id)
        .unwrap();
    source.content_digest = digest;
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

fn manifest(partition: WorldPartition, cluster: &str) -> ReplayWorldManifestV2 {
    ReplayWorldManifestV2 {
        schema_version: REPLAY_MANIFEST_SCHEMA.into(),
        world_id: format!("world-{cluster}"),
        cluster_id: cluster.into(),
        partition,
        purpose: Purpose::Development,
        generation_signature: digest("generation-v1"),
        world_context_signature: digest("world-context"),
        baseline_context_signature: digest("baseline-context"),
        approved_parent_digest: digest("approved-parent"),
        model_digest: digest("model-digest"),
        tools_digest: digest("tools-digest"),
        scorer_digest: digest("scorer-digest"),
        guidance_digest: digest("guidance-digest"),
        repair_template_digest: digest("repair-digest"),
        input_order_digest: digest("input-order"),
        initial_baseline_quality_micros: 100_000,
        baseline_observation_source_id: "baseline-source".into(),
        source_closure: vec![
            ReplaySourceRef {
                source_id: "source-1".into(),
                content_digest: digest("source-1"),
            },
            ReplaySourceRef {
                source_id: "baseline-source".into(),
                content_digest: digest("pending-baseline"),
            },
        ],
        revoke_watermark: 1,
        prefix_coverage: vec![PrefixCoverageV1 {
            context_signature: digest("next-context"),
            exhausted: true,
        }],
        action_catalog: vec![ReplayActionSpecV1 {
            record_seq: 1,
            generation_signature: digest("generation-v1"),
            parent_context_signature: digest("baseline-context"),
            branch_seq: 1,
            target_depth: 1,
            action_kind: ActionKindV1::Widen { root_slot: 1 },
            estimated_cost_upper_micros: Some(10),
            writes_shared_workspace: false,
        }],
    }
}

fn world(partition: WorldPartition, cluster: &str) -> ReplayWorldV2 {
    let transition = ReplayTransitionV2 {
        record_id: "opaque-a".into(),
        record_seq: 1,
        generation_signature: digest("generation-v1"),
        parent_context_signature: digest("baseline-context"),
        action_kind: ActionKindV1::Widen { root_slot: 1 },
        next_context_signature: digest("next-context"),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: 500_000,
            },
        },
        actual_usage: HistoricalUsage::default(),
        source_ids: vec!["source-1".into()],
        observation_source_id: "source-1".into(),
    };
    let mut world_manifest = manifest(partition, cluster);
    world_manifest.source_closure[1].content_digest =
        replay_baseline_observation_digest(&world_manifest).unwrap();
    world_manifest.source_closure[0].content_digest =
        replay_observation_digest(&world_manifest, &transition).unwrap();
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: world_manifest,
        transitions: vec![transition],
        sealed_digest: None,
    };
    world.seal().unwrap();
    world
}

#[test]
fn seal_binds_all_content_and_cannot_be_resealed() {
    let mut world = world(WorldPartition::Train, "c1");
    assert!(world.validate_sealed().is_ok());
    assert!(world.seal().is_err());
    world.transitions[0].record_id = "renamed".into();
    assert!(world.validate_sealed().is_err());
}

#[test]
fn baseline_quality_cannot_change_without_matching_observation_material() {
    let mut world = world(WorldPartition::Train, "baseline-proof");
    world.sealed_digest = None;
    world.manifest.initial_baseline_quality_micros = 1_000_000;
    assert!(world.seal().is_err());
}

#[test]
fn transition_must_match_preregistered_action_signature() {
    let mut world = world(WorldPartition::Train, "c1");
    world.sealed_digest = None;
    world.transitions[0].parent_context_signature = "future-context".into();
    assert!(world.seal().is_err());
}

#[test]
fn whole_cluster_cannot_cross_train_select() {
    let train = world(WorldPartition::Train, "same");
    let mut select = world(WorldPartition::Select, "same");
    select.manifest.world_id = "world-select".into();
    select.sealed_digest = None;
    select.seal().unwrap();
    assert!(validate_world_partitions(&[train, select]).is_err());
}

#[test]
fn simulation_profile_is_fixed_and_bounded() {
    let caps = ExplorationCapsV1::online();
    for width in [1, 2, 4] {
        assert!(
            ReplaySimulationProfile::default_v2(width, digest("pool"))
                .validate(&caps)
                .is_ok()
        );
    }
    assert!(
        ReplaySimulationProfile::default_v2(3, digest("pool"))
            .validate(&caps)
            .is_err()
    );
    let mut profile = ReplaySimulationProfile::default_v2(1, digest("pool"));
    profile.lambda_work_micros = 0;
    assert!(profile.validate(&caps).is_err());
}

fn report(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    cpu: u64,
) -> ReplayReportV2 {
    ReplayReportV2 {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest: world.sealed_digest.clone().unwrap(),
        cluster_id: world.manifest.cluster_id.clone(),
        partition: world.manifest.partition,
        policy_digest: evo_core::fingerprint(policy).unwrap(),
        profile_digest: evo_core::fingerprint(profile).unwrap(),
        objective_version: profile.objective.version().into(),
        batches: vec![],
        probe_best_quality_micros: vec![],
        round_best_quality_micros: vec![],
        final_quality_micros: world.manifest.initial_baseline_quality_micros,
        probes: 0,
        simulated_rounds: 0,
        parallel_penalty: None,
        no_probe: true,
        attainment_auc_micros: Some(world.manifest.initial_baseline_quality_micros),
        q_auc_sim_micros: Some(world.manifest.initial_baseline_quality_micros),
        score_v2_micros: Some(i64::from(world.manifest.initial_baseline_quality_micros)),
        historical_usage: HistoricalUsage::default(),
        replay_cpu_nanos: cpu,
        coverage: ReplayCoverage::default(),
        terminal: ReplayTerminal::PolicyStop,
        terminal_reason: "fixture stop".into(),
        development_only: true,
        revealed_prefix: None,
    }
}

#[test]
fn replay_pool_is_content_derived_partitioned_and_order_independent() {
    let mut train = world(WorldPartition::Train, "train");
    train.manifest.world_context_signature = digest("train-workspace");
    train.sealed_digest = None;
    refresh_evidence_digests(&mut train);
    train.seal().unwrap();
    let mut select = world(WorldPartition::Select, "select");
    select.manifest.world_context_signature = digest("select-workspace");
    select.sealed_digest = None;
    refresh_evidence_digests(&mut select);
    select.seal().unwrap();
    let first = ReplayPoolManifestV1::build(&[train.clone(), select.clone()]).unwrap();
    let reversed = ReplayPoolManifestV1::build(&[select.clone(), train.clone()]).unwrap();
    assert_eq!(first, reversed);
    assert!(
        first
            .validate_against(&[select.clone(), train.clone()])
            .is_ok()
    );
    assert!(ReplayPoolManifestV1::build(std::slice::from_ref(&train)).is_err());

    let mut incompatible = select;
    incompatible.manifest.model_digest = digest("other-model");
    incompatible.sealed_digest = None;
    refresh_evidence_digests(&mut incompatible);
    incompatible.seal().unwrap();
    assert!(ReplayPoolManifestV1::build(&[train, incompatible]).is_err());
}

#[test]
fn stored_report_binds_complete_partition_and_semantic_digest_only_ignores_cpu() {
    let train = world(WorldPartition::Train, "train");
    let select = world(WorldPartition::Select, "select");
    let pool = ReplayPoolManifestV1::build(&[train.clone(), select]).unwrap();
    let caps = ExplorationCapsV1::online();
    let policy = ElasticPolicyV1::default();
    let profile = ReplaySimulationProfile::default_v2(1, pool.pool_digest.clone());
    let stored = StoredReplayReportV1::build(
        "tenant",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        caps.clone(),
        vec![report(&train, &policy, &profile, 10)],
    )
    .unwrap();
    stored.validate_static(&pool).unwrap();
    let mut cpu_changed = stored.member_reports.clone();
    cpu_changed[0].report.replay_cpu_nanos = 99;
    assert_eq!(
        replay_report_semantic_digest(&stored.member_reports).unwrap(),
        replay_report_semantic_digest(&cpu_changed).unwrap()
    );
    assert_ne!(
        evo_core::fingerprint(&stored.member_reports).unwrap(),
        evo_core::fingerprint(&cpu_changed).unwrap()
    );
    cpu_changed[0].report.final_quality_micros += 1;
    assert_ne!(
        replay_report_semantic_digest(&stored.member_reports).unwrap(),
        replay_report_semantic_digest(&cpu_changed).unwrap()
    );

    let wrong_profile = ReplaySimulationProfile::default_v2(1, digest("invented-pool"));
    assert!(wrong_profile.validate_for_pool(&caps, &pool).is_err());
    assert!(
        StoredReplayReportV1::build(
            "tenant",
            WorldPartition::Select,
            &pool,
            policy,
            profile,
            caps,
            vec![],
        )
        .is_err()
    );
}
