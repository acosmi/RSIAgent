//! AG-035 (E08/E07/§11, V017): a revocation cleanup completes only at a closure
//! fixpoint, so a management request accepted while the cleanup runs is cleaned
//! before the job reports `Complete`. Real SQLite store, no model, no provider,
//! zero monetary cost.
//!
//! The submit check of a management request refuses a dependency that was
//! already redacted, but until the cleanup reaches the replay pool a request over
//! it is accepted: its private input (the whole request payload) is stored with an
//! edge to the pool and its job then fails closed on the operation's own watermark
//! check. That edge is written after `begin_revoke`. The cleanup pages the
//! dependents of the pool through a forward key-set cursor and expands the pool
//! once; an input whose id sorts before the cursor, or that arrives after the
//! pool was expanded, was never visited, and the job still reported `Complete`.
//! The id of a private input is a hash of its request key, so the position
//! relative to the cursor is arbitrary, and with the request keys below eight of
//! the twenty accepted inputs stayed in plaintext
//! (`rsia.management_private_input.v1`) at `Complete`.
//!
//! The fixtures below are copied from `tests/replay_redacted_reads_v42.rs`
//! (themselves copied from `tests/management_cleanup_v42.rs` and
//! `tests/replay_v41.rs`); test crates cannot import each other, and the originals
//! are untouched.

use evo_core::{Context, Role};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::Value;
use std::time::Duration;

use common::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

mod common {
    use super::*;
    use evo_core::hash;

    pub const REDACTED: &str = "rsia.redacted.v1";

    pub fn d(label: &str) -> String {
        hash(label.as_bytes())
    }

    pub async fn open(name: &str) -> (tempfile::TempDir, Store, Context) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(name)).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        (dir, store, admin)
    }

    /// The stored body of one object, `None` when there is none, straight from
    /// the store so the assertions do not depend on any consumer's read path.
    pub async fn try_raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Option<Value> {
        let mut session = store.session().await.unwrap();
        let value = session.get(ctx, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn begin(store: &Store, admin: &Context, kind: &str, id: &str) -> CleanupStatus {
        LifecycleStore::begin_revoke(
            admin,
            store,
            TypedObjectRef {
                kind: kind.into(),
                id: id.into(),
            },
            "source revoked",
            500,
        )
        .await
        .unwrap()
    }

    /// One cleanup step of `limit` edges; `number` numbers the steps of a test so
    /// every `now` is distinct.
    pub async fn step(
        store: &Store,
        admin: &Context,
        status: &CleanupStatus,
        limit: usize,
        number: i64,
    ) -> CleanupStatus {
        LifecycleStore::cleanup_step(admin, store, &status.job_id, limit, 501 + number)
            .await
            .unwrap()
    }

    /// Steps until the job stops moving; returns it and the number of steps.
    pub async fn drive(
        store: &Store,
        admin: &Context,
        mut status: CleanupStatus,
        limit: usize,
        mut steps: i64,
    ) -> (CleanupStatus, i64) {
        while !matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            assert!(steps < 5_000, "the cleanup did not finish: {status:?}");
            status = step(store, admin, &status, limit, steps).await;
            steps += 1;
        }
        (status, steps)
    }

    /// Polls the stored job until it is terminal.
    pub async fn wait_job(store: &Store, ctx: &Context, job_id: &str) -> ManagementJob {
        for _ in 0..2_000 {
            let mut session = store.session().await.unwrap();
            let job: ManagementJob = session.need(ctx, "job", job_id).await.unwrap();
            session.commit().await.unwrap();
            if matches!(
                job.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("management job did not finish");
    }
}

// ---------------------------------------------------------------------------
// Fixture: sealed replay worlds, pool and report of `replay.run` (copied from
// tests/replay_redacted_reads_v42.rs)
// ---------------------------------------------------------------------------

mod replay_fx {
    use super::*;
    use evo_core::evidence::Purpose;
    use evo_core::replay::*;
    use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
    use evo_storage::replay::{register_replay_pool, seal_replay_world};
    use serde_json::json;

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

    fn sample_profile(pool_digest: &str) -> ReplaySimulationProfile {
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
            pool_digest: pool_digest.into(),
            purpose: Purpose::Development,
            target_runtime_profile: "simulation-only".into(),
        }
    }

    /// The Select world `world-1` (sources `source-1`, `source-2`, and the
    /// baseline source) and the Train world `world-train` (its own runs), both
    /// sealed for revoke watermark 1.
    fn worlds() -> [ReplayWorldV2; 2] {
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
        [persisted, train]
    }

    /// The trusted runs the worlds' source closures name.
    async fn store_sources(ctx: &Context, store: &Store, worlds: &[ReplayWorldV2; 2]) {
        let mut session = store.session().await.unwrap();
        for replay_world in worlds {
            for source in &replay_world.manifest.source_closure {
                let body =
                    if source.source_id == replay_world.manifest.baseline_observation_source_id {
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
        session.commit().await.unwrap();
    }

    async fn seal_and_register(
        ctx: &Context,
        store: &Store,
        worlds: &[ReplayWorldV2; 2],
    ) -> ReplayPoolManifestV1 {
        for replay_world in worlds {
            seal_replay_world(ctx, store, replay_world).await.unwrap();
        }
        register_replay_pool(
            ctx,
            store,
            &[
                worlds[0].manifest.world_id.clone(),
                worlds[1].manifest.world_id.clone(),
            ],
        )
        .await
        .unwrap()
    }

    /// Two sealed worlds over their runs, one watermark bump (so the store's
    /// watermark is the worlds' 1) and the pool that registers both.
    pub async fn setup_sealed_pool(ctx: &Context, store: &Store) -> ReplayPoolManifestV1 {
        let worlds = worlds();
        store_sources(ctx, store, &worlds).await;
        let mut session = store.session().await.unwrap();
        session.bump_watermark(ctx, &d("watermark")).await.unwrap();
        session.commit().await.unwrap();
        seal_and_register(ctx, store, &worlds).await
    }

    pub fn run_request(request_key: &str, pool_digest: &str) -> Value {
        json!({
            "schema_version": "rsia.management.replay_run.v1",
            "request_key": request_key,
            "pool_digest": pool_digest,
            "partition": "select",
            "policy": ElasticPolicyV1::default(),
            "profile": sample_profile(pool_digest),
            "caps": ExplorationCapsV1::online(),
        })
    }
}

/// A replay chain of `source-2` stored by a `replay.run` job: the pool, its
/// report and the job with its private input all depend on the source.
async fn stored_chain(
    store: &Store,
    admin: &Context,
    key: &str,
) -> (ManagementDispatcher, evo_core::replay::ReplayPoolManifestV1) {
    let pool = replay_fx::setup_sealed_pool(admin, store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            admin,
            "replay.run",
            replay_fx::run_request(key, &pool.pool_digest),
        )
        .await
        .unwrap();
    let done = wait_job(store, admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    (dispatcher, pool)
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// `replay.run` is submitted before every one of the first twenty cleanup steps
/// (one edge per step) of the revocation of `source-2`. The pool is not redacted
/// yet, so each submission is accepted and its job fails closed (`conflict`: the
/// source revoke watermark changed). At `Complete` no private input of an accepted
/// submission may be left in plaintext; the jobs stay as the record of the action.
#[tokio::test]
async fn management_inputs_accepted_during_the_cleanup_are_redacted_at_complete() {
    let (_dir, store, admin) = open("fixpoint-p3.sqlite3").await;
    let (dispatcher, pool) = stored_chain(&store, &admin, "ctrl-p3-chain").await;

    let mut status = begin(&store, &admin, "run", "source-2").await;
    let mut accepted = Vec::new();
    for round in 0..20 {
        // The request keys are those of the controller's probe, so that the ids
        // (and with them the position of each input relative to the page cursor)
        // are the ones the 8 plaintext inputs were observed with.
        let key = format!("ctrl-p3-during-{round}");
        let job = dispatcher
            .submit(
                &admin,
                "replay.run",
                replay_fx::run_request(&key, &pool.pool_digest),
            )
            .await
            .unwrap_or_else(|error| panic!("round {round}: the submission was refused: {error:?}"));
        let job = wait_job(&store, &admin, &job.id).await;
        assert_eq!(
            job.state,
            ManagementJobState::Failed,
            "round {round}: {job:?}"
        );
        assert_eq!(job.error_code.as_deref(), Some("conflict"), "round {round}");
        accepted.push(job);
        status = step(&store, &admin, &status, 1, round).await;
    }
    let (status, _) = drive(&store, &admin, status, 1, 20).await;

    let mut plaintext = Vec::new();
    for (round, job) in accepted.iter().enumerate() {
        let input = try_raw(&store, &admin, "artifact", &job.private_input_ref)
            .await
            .unwrap_or_else(|| panic!("round {round}: the private input is gone"));
        if input["schema_version"] != REDACTED {
            plaintext.push((round, input["schema_version"].clone()));
        }
    }
    assert!(
        plaintext.is_empty(),
        "{} of {} private inputs are still plaintext at {:?}: {plaintext:?}",
        plaintext.len(),
        accepted.len(),
        status.state
    );
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.pending_nodes, 0);
    assert_eq!(status.last_error, None);
    // The jobs are preserved as the record of the action, still failed closed.
    for (round, job) in accepted.iter().enumerate() {
        let kept = wait_job(&store, &admin, &job.id).await;
        assert_eq!(kept.state, ManagementJobState::Failed, "round {round}");
        assert_eq!(
            kept.error_code.as_deref(),
            Some("conflict"),
            "round {round}"
        );
    }
}

/// `(edge_page_limit, steps to Complete, processed_nodes)` of the cleanup of a
/// stored replay chain, every write done before `begin_revoke`, as measured on the
/// code before AG-035 (commit 04e6309). The recheck of a closed graph finds
/// nothing, so neither number changes.
const CHAIN_BEFORE: [(usize, i64, u64); 2] = [(1, 11, 8), (8, 1, 8)];

#[tokio::test]
async fn a_replay_chain_without_late_writes_takes_the_steps_it_took_before() {
    let mut measured = Vec::new();
    for (limit, _, _) in CHAIN_BEFORE {
        let (_dir, store, admin) = open("fixpoint-chain.sqlite3").await;
        let (_dispatcher, _pool) = stored_chain(&store, &admin, "chain-regression").await;
        let status = begin(&store, &admin, "run", "source-2").await;
        let (status, steps) = drive(&store, &admin, status, limit, 0).await;
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "limit {limit}: {status:?}"
        );
        assert_eq!(status.last_error, None, "limit {limit}");
        measured.push((limit, steps, status.processed_nodes));
    }
    assert_eq!(measured, CHAIN_BEFORE.to_vec());
}
