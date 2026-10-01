//! AG-032 (E10/E07/E08; plan §11.3-§11.5, V017/V069): the replay store names a
//! redacted record, and the management submit check matches a tombstone's source
//! kind. Real SQLite store, no model, no provider, zero monetary cost.
//!
//! Read side. After a source revocation the cleanup replaces a replay world, pool
//! and report with an `rsia.redacted.v1` tombstone. That is the expected state of a
//! revoked record, not corruption, so every replay read that meets one answers a
//! `Conflict` that names the record kind and id (`replay <record kind> <id> was
//! redacted because its source was revoked`), never `Internal`. Before this change
//! a pool or report read answered `Internal` (the tombstone does not decode as an
//! envelope) and a world read answered `Invalid`; only the management dispatcher's
//! own status gate kept callers from seeing it. A body that is neither a tombstone
//! nor decodable is still corruption and keeps the classification it had.
//!
//! Submit side. `begin_revoke` keys a tombstone by the id of its source alone, and
//! a source is a run or an artifact, so a run and an artifact that share an id
//! share one tombstone. The submit check of a new management request now compares
//! the `source_kind` in the tombstone with the kind of the dependency: a revoked
//! run no longer refuses a live artifact that carries its id, a revoked run or a
//! revoked artifact still refuses a dependency of its own kind, and a tombstone
//! that cannot be read fails closed.
//!
//! The fixtures below are copied from `tests/management_cleanup_v42.rs` (replay
//! pool, exploration world), `tests/replay_v41.rs` and `tests/replay_economics.rs`
//! (test crates cannot import each other); the originals are untouched.

use evo_core::evidence::Purpose;
use evo_core::replay::{ReplayPoolManifestV1, StoredReplayReportV1, WorldPartition};
use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1};
use evo_core::{Context, Error, Role};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_engine::replay::{
    run_and_persist_pool_replay, run_persisted_replay, verified_replay_report_view,
};
use evo_engine::replay_experiment::PersistentReplayEconomicCoordinator;
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, RevokeTombstone};
use evo_storage::replay::{
    REPLAY_POOL_RECORD_KIND, REPLAY_REPORT_RECORD_KIND, REPLAY_STORE_ENVELOPE_SCHEMA,
    load_live_replay_pool, load_live_replay_report, load_live_replay_world, put_replay_report,
    register_replay_pool, replay_pool_storage_id,
};
use serde_json::{Value, json};
use std::time::Duration;

use common::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

mod common {
    use super::*;
    use evo_core::hash;
    use evo_storage::lifecycle::{CleanupStatus, TypedObjectRef};

    pub const REDACTED: &str = "rsia.redacted.v1";
    pub const WORLD_KIND: &str = "replay_world";

    pub fn d(label: &str) -> String {
        hash(label.as_bytes())
    }

    pub async fn open(name: &str) -> (tempfile::TempDir, Store, Context) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(name)).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        (dir, store, admin)
    }

    /// The stored body of one object, straight from the store, so the assertions
    /// do not depend on any consumer's read path.
    pub async fn raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
        let mut session = store.session().await.unwrap();
        let value = session.need(ctx, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn try_raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Option<Value> {
        let mut session = store.session().await.unwrap();
        let value = session.get(ctx, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn put_raw(store: &Store, ctx: &Context, kind: &str, id: &str, body: &Value) {
        let mut session = store.session().await.unwrap();
        session.put(ctx, kind, id, "admin", body).await.unwrap();
        session.commit().await.unwrap();
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

    /// Drives the cleanup job until it stops moving.
    pub async fn drive(store: &Store, admin: &Context, mut status: CleanupStatus) -> CleanupStatus {
        for now in 501..2_500 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(admin, store, &status.job_id, 8, now)
                .await
                .unwrap();
        }
        status
    }

    pub async fn revoke_and_clean(store: &Store, admin: &Context, run: &str) -> CleanupStatus {
        let status = begin(store, admin, "run", run).await;
        drive(store, admin, status).await
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

    /// The `Conflict` message, failing on any other outcome (above all on
    /// `Internal`, which is what an expected state must never be reported as).
    pub fn expect_conflict<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) -> String {
        match result {
            Err(Error::Conflict(message)) => message,
            other => panic!("{what}: expected a Conflict, got {other:?}"),
        }
    }

    pub fn expect_internal<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) {
        match result {
            Err(Error::Internal) => {}
            other => panic!("{what}: expected Internal, got {other:?}"),
        }
    }

    /// The message a read of a redacted replay record answers.
    pub fn redacted_message(record_kind: &str, id: &str) -> String {
        format!("replay {record_kind} {id} was redacted because its source was revoked")
    }

    /// The shape the cleanup leaves in place of a replay pool or report
    /// (`redact_replay_store_envelope` in the storage crate's lifecycle module).
    pub fn redacted_store_row(id: &str, record_kind: &str) -> Value {
        json!({
            "id": id,
            "schema_version": REDACTED,
            "state": "source_revoked",
            "original_kind": "artifact",
            "original_schema": REPLAY_STORE_ENVELOPE_SCHEMA,
            "original_digest": d(id),
            "metadata": {"record_kind": record_kind},
        })
    }

    /// The shape the cleanup leaves in `replay_worlds.manifest` of a world.
    pub fn redacted_world_manifest(label: &str) -> Value {
        json!({
            "schema_version": REDACTED,
            "state": "source_revoked",
            "original_schema": "rsia.replay_world.v2",
            "original_digest": d(label),
            "sealed_digest": d("sealed"),
            "metadata": {},
            "historical_transitions": [],
        })
    }

    /// A tombstone exactly as `begin_revoke` writes it.
    pub fn tombstone(id: &str, source_kind: &str) -> RevokeTombstone {
        RevokeTombstone {
            id: id.into(),
            schema_version: evo_storage::lifecycle::REVOKE_TOMBSTONE_SCHEMA.into(),
            source_kind: source_kind.into(),
            reason: "fixture".into(),
            watermark_seq: 1,
            watermark_digest: d("watermark"),
            created_at: 1,
        }
    }

    /// Jobs and artifacts of the namespace: what a refused submission must leave
    /// as it found it.
    pub async fn counts(store: &Store, ctx: &Context) -> (u64, u64) {
        let mut session = store.session().await.unwrap();
        let jobs = session.namespace_object_count(ctx, "job").await.unwrap();
        let artifacts = session
            .namespace_object_count(ctx, "artifact")
            .await
            .unwrap();
        session.commit().await.unwrap();
        (jobs, artifacts)
    }

    /// No idempotency row: a half-recorded submission would answer differently.
    pub async fn assert_no_idempotency_row(
        store: &Store,
        ctx: &Context,
        operation: &str,
        key: &str,
    ) {
        let mut session = store.session().await.unwrap();
        let row = session
            .cached::<ManagementJob, _>(ctx, operation, key, &json!({}))
            .await;
        session.commit().await.unwrap();
        assert!(matches!(row, Ok(None)), "{operation} {key}: {row:?}");
    }
}

// ---------------------------------------------------------------------------
// Fixture: trusted runs and world of `exploration.start` (copied from
// tests/management_cleanup_v42.rs; only what the run-dependency cases use)
// ---------------------------------------------------------------------------

mod exploration_fx {
    use super::*;
    use evo_core::evidence::{ExecutionAttestation, SourceSelection, TaskOrigin};
    use evo_core::fingerprint;
    use evo_core::hash;
    use evo_core::optimization::{
        OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
    };
    use evo_core::skill_edit::EvidenceRef;
    use evo_core::strategy::SimulationContext;
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
    };
    use evo_engine::exploration::{
        ExplorationDependency, ExplorationWorldV1, RootOpportunity, WorldState,
    };

    pub const RUNS: [&str; 2] = ["run-failure", "run-success"];

    pub fn world(id: &str, source_watermark: u64) -> ExplorationWorldV1 {
        let parent_skill = hash(b"parent-skill");
        let parent_bundle = hash(b"parent-bundle");
        let environment = hash(b"environment");
        let model = hash(b"model");
        let tools = hash(b"tools");
        let grader = hash(b"grader");
        let rules = hash(b"rules");
        let context_signature = fingerprint(&(
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
            dependencies: RUNS
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

    fn trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
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

    pub async fn store_run(store: &Store, id: &str, family: &str, outcome: TraceOutcome) {
        let host = Context::new("n", "host", Role::Host).unwrap();
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
            trace: trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(store, &host, &authority)
            .await
            .unwrap();
    }

    pub fn selection() -> SourceSelection {
        SourceSelection {
            roots: vec![],
            run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        }
    }

    /// Two Host-issued trace authorities, a source selection grant and one
    /// watermark bump so `source_watermark == 1`.
    pub async fn setup(store: &Store) {
        let host = Context::new("n", "host", Role::Host).unwrap();
        store_run(store, "run-failure", "family-a", TraceOutcome::TaskFailure).await;
        store_run(store, "run-success", "family-b", TraceOutcome::Success).await;
        store_source_selection(store, &host, &selection())
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        session.bump_watermark(&host, "e09-initial").await.unwrap();
        session.commit().await.unwrap();
    }

    pub fn start_request(request_key: &str, world: &ExplorationWorldV1) -> Value {
        json!({
            "schema_version": "rsia.management.exploration_start.v1",
            "request_key": request_key,
            "world": serde_json::to_value(world).unwrap(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture: sealed replay worlds, pool and report of `replay.run` (copied from
// tests/management_cleanup_v42.rs and tests/replay_v41.rs; the pool setup is
// split so a case can register the pool after a revocation)
// ---------------------------------------------------------------------------

mod replay_fx {
    use super::*;
    use evo_core::replay::*;
    use evo_core::strategy::{ActionKindV1, ObservedStatus};
    use evo_storage::replay::{register_replay_pool, seal_replay_world};

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

    pub fn sample_profile(pool_digest: &str) -> ReplaySimulationProfile {
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
    pub fn worlds() -> [ReplayWorldV2; 2] {
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

    /// The pool manifest of `worlds`, computed without a store: the same builder
    /// `register_replay_pool` runs, so the digest (and with it the storage id of
    /// the pool row) is known before anything is registered.
    pub fn manifest(worlds: &[ReplayWorldV2; 2]) -> ReplayPoolManifestV1 {
        ReplayPoolManifestV1::build(worlds).unwrap()
    }

    /// The trusted runs the worlds' source closures name.
    pub async fn store_sources(ctx: &Context, store: &Store, worlds: &[ReplayWorldV2; 2]) {
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

    pub async fn seal_and_register(
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

    pub fn policy() -> ElasticPolicyV1 {
        ElasticPolicyV1::default()
    }

    pub fn caps() -> ExplorationCapsV1 {
        ExplorationCapsV1::online()
    }

    /// The Select report of the pool, stored through the engine's own path.
    pub async fn store_report(
        ctx: &Context,
        store: &Store,
        pool: &ReplayPoolManifestV1,
    ) -> StoredReplayReportV1 {
        run_and_persist_pool_replay(
            ctx,
            store,
            &pool.pool_digest,
            WorldPartition::Select,
            &policy(),
            &sample_profile(&pool.pool_digest),
            &caps(),
        )
        .await
        .unwrap()
    }

    pub fn run_request(request_key: &str, pool_digest: &str) -> Value {
        json!({
            "schema_version": "rsia.management.replay_run.v1",
            "request_key": request_key,
            "pool_digest": pool_digest,
            "partition": "select",
            "policy": policy(),
            "profile": sample_profile(pool_digest),
            "caps": caps(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture: the replay-economic experiment of `register` (copied from
// tests/replay_economics.rs; `register` reads the selection report first)
// ---------------------------------------------------------------------------

mod economic_fx {
    use super::*;
    use evo_core::fingerprint;
    use evo_core::replay_economics::*;

    fn budget_binding() -> EconomicBudgetBindingV1 {
        let bindings = [
            (
                CostComponentKind::HistoryCollection,
                "group-history",
                EconomicBudgetStage::HistoryCollection,
            ),
            (
                CostComponentKind::PolicyGeneration,
                "group-policy",
                EconomicBudgetStage::CandidateGeneration,
            ),
            (
                CostComponentKind::ReplayCpu,
                "group-replay-cpu",
                EconomicBudgetStage::StorageCpu,
            ),
            (
                CostComponentKind::ReplayStorage,
                "group-replay-storage",
                EconomicBudgetStage::StorageCpu,
            ),
            (
                CostComponentKind::OnlineDevelopmentFixed,
                "group-fixed",
                EconomicBudgetStage::DevelopmentExecution,
            ),
            (
                CostComponentKind::OnlineDevelopmentReplaySelected,
                "group-replay",
                EconomicBudgetStage::DevelopmentExecution,
            ),
            (
                CostComponentKind::IndependentAcceptance,
                "group-acceptance",
                EconomicBudgetStage::FormalEvaluation,
            ),
            (
                CostComponentKind::CanaryOperations,
                "group-canary",
                EconomicBudgetStage::GrayOperations,
            ),
        ]
        .into_iter()
        .map(|(component, group, stage)| ComponentBillingBindingV1 {
            component,
            source: ComponentBillingSourceV1::BudgetCall {
                dispatch_group_id: group.into(),
                stage,
            },
        });
        EconomicBudgetBindingV1 {
            billing_scope: "economic-scope".into(),
            root_budget_id: "root-1".into(),
            currency: "usd".into(),
            pricing_version: "price-v1".into(),
            payment_subject: "payer-1".into(),
            components: bindings
                .chain(std::iter::once(ComponentBillingBindingV1 {
                    component: CostComponentKind::HumanReview,
                    source: ComponentBillingSourceV1::AdminMeasurement {
                        source_id: "admin-review-receipt".into(),
                    },
                }))
                .collect(),
        }
    }

    pub fn experiment(report: &StoredReplayReportV1) -> ReplayEconomicExperimentV1 {
        ReplayEconomicExperimentV1 {
            schema_version: EXPERIMENT_SCHEMA.into(),
            id: "economic-exp-1".into(),
            profile_id: "profile-1".into(),
            evaluator_actor: "evaluator".into(),
            executor_actor: "executor".into(),
            proposer_actor: "proposer".into(),
            approver_actor: "approver".into(),
            primary_claim: EconomicClaim::NoninferiorSavings,
            min_gain_micros: 20_000,
            noninferiority_margin_micros: 10_000,
            preregistration_digest: d("preregistered"),
            paired_ticket_id: "paired-ticket".into(),
            paired_ticket_digest: d("paired-ticket"),
            dataset_epoch: "epoch-1".into(),
            tasks: vec![PairedTaskRef {
                ordinal: 1,
                task_id: "task-1".into(),
                task_digest: d("task-1"),
                cluster_id: "cluster-1".into(),
                arm_order: ArmOrder::FixedThenReplaySelected,
            }],
            runtime: MatchedRuntimeContract {
                w_online: 1,
                fixed_policy_digest: d("fixed-policy"),
                replay_selected_policy_digest: report.policy_digest.clone(),
                generation_strategy_digest: d("generation"),
                candidate_bundle_digest: d("candidate-bundle"),
                baseline_bundle_digest: d("baseline-bundle"),
                environment_digest: d("environment"),
                model_digest: d("model"),
                tools_digest: d("tools"),
                runner_digest: d("runner"),
                grader_digest: d("scorer"),
                guidance_digest: d("guidance"),
                rules_digest: d("rules"),
                context_signature: d("context"),
                target_runtime_profile: "online-w1".into(),
                per_arm_node_budget: 12,
                per_arm_root_budget_micros: 1_000,
            },
            replay_selection: ReplaySelectionRef {
                report_artifact_id: report.report_id.clone(),
                report_digest: fingerprint(report).unwrap(),
                semantic_digest: report.semantic_reports_digest.clone(),
                world_pool_digest: report.pool_digest.clone(),
                policy_digest: report.policy_digest.clone(),
                profile_digest: report.profile_digest.clone(),
                caps_digest: report.caps_digest.clone(),
                selection_rule_version: REPLAY_SELECTION_RULE_V1.into(),
                selected_with_w_sim: 1,
                target_w_online: 1,
                simulation_only: vec![SimulationOnlyDiagnosticRef {
                    w_sim: 2,
                    report_id: "simulation-w2".into(),
                    report_digest: d("simulation-w2"),
                }],
            },
            cost_plan: FullCostPlanV1 {
                component_kinds: CostComponentKind::ALL.into(),
                currency: "usd".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
            },
            budget_binding: budget_binding(),
            sources: vec![EconomicSourceRefV1 {
                id: "source-1".into(),
                digest: fingerprint(&json!({"schema_version":"fixture.source.v1","id":"source-1"}))
                    .unwrap(),
            }],
            source_watermark: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Read side: a redacted replay record is named, not reported as Internal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reads_of_a_cleaned_replay_chain_name_the_redacted_record() {
    let (_dir, store, admin) = open("cleaned-chain.sqlite3").await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let pool_row = replay_pool_storage_id(&pool.pool_digest).unwrap();
    // The chain as `replay.run` builds it: job -> private input -> pool -> report.
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-chain", &pool.pool_digest),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    let Some(ManagementResult::ReplayStored { report_id, .. }) = &done.result else {
        panic!("unexpected result: {:?}", done.result);
    };

    // Control: before the revocation every read of the chain works.
    load_live_replay_report(&admin, &store, report_id)
        .await
        .unwrap();
    load_live_replay_pool(&evaluator, &store, &pool.pool_digest)
        .await
        .unwrap();
    load_live_replay_world(&admin, &store, "world-1", Purpose::Development)
        .await
        .unwrap();
    verified_replay_report_view(&evaluator, &store, report_id)
        .await
        .unwrap();

    // `source-2` is a source of `world-1`: the cleanup redacts the world, the pool,
    // the report and the job's private input that hang off it, and deletes the run.
    let cleanup = revoke_and_clean(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_eq!(cleanup.last_error, None);
    assert_eq!(cleanup.pending_nodes, 0);
    for id in [&pool_row, report_id, &done.private_input_ref] {
        let body = raw(&store, &admin, "artifact", id).await;
        assert_eq!(body["schema_version"], REDACTED, "{body}");
    }
    let mut session = store.session().await.unwrap();
    let (_, world) = session.get_world(&admin, "world-1").await.unwrap().unwrap();
    session.commit().await.unwrap();
    assert_eq!(world["schema_version"], REDACTED, "{world}");

    let report = redacted_message(REPLAY_REPORT_RECORD_KIND, report_id);
    let pool_message = redacted_message(REPLAY_POOL_RECORD_KIND, &pool_row);
    let world_message = redacted_message(WORLD_KIND, "world-1");

    // The report read, directly and through the verified view.
    assert_eq!(
        expect_conflict(
            load_live_replay_report(&admin, &store, report_id).await,
            "load_live_replay_report"
        ),
        report
    );
    for (reader, who) in [(&evaluator, "evaluator"), (&admin, "admin")] {
        assert_eq!(
            expect_conflict(
                verified_replay_report_view(reader, &store, report_id).await,
                &format!("verified_replay_report_view as {who}")
            ),
            report
        );
    }
    // The pool read, directly and through the engine paths that load the pool.
    assert_eq!(
        expect_conflict(
            load_live_replay_pool(&evaluator, &store, &pool.pool_digest).await,
            "load_live_replay_pool"
        ),
        pool_message
    );
    assert_eq!(
        expect_conflict(
            run_and_persist_pool_replay(
                &admin,
                &store,
                &pool.pool_digest,
                WorldPartition::Select,
                &replay_fx::policy(),
                &replay_fx::sample_profile(&pool.pool_digest),
                &replay_fx::caps(),
            )
            .await,
            "run_and_persist_pool_replay"
        ),
        pool_message
    );
    assert_eq!(
        expect_conflict(
            run_persisted_replay(
                &admin,
                &store,
                "world-1",
                Purpose::Development,
                &replay_fx::policy(),
                &replay_fx::sample_profile(&pool.pool_digest),
                &replay_fx::caps(),
            )
            .await,
            "run_persisted_replay"
        ),
        pool_message
    );
    // The world read, directly and through the registration of a pool over it.
    assert_eq!(
        expect_conflict(
            load_live_replay_world(&admin, &store, "world-1", Purpose::Development).await,
            "load_live_replay_world"
        ),
        world_message
    );
    assert_eq!(
        expect_conflict(
            register_replay_pool(
                &admin,
                &store,
                &["world-1".to_string(), "world-train".to_string()]
            )
            .await,
            "register_replay_pool"
        ),
        world_message
    );
    // The Train world was not redacted; its read stops on the moved watermark
    // instead, and says so (the tombstone check is specific to the redacted record).
    assert_eq!(
        expect_conflict(
            load_live_replay_world(&admin, &store, "world-train", Purpose::Development).await,
            "load_live_replay_world of the live world"
        ),
        "source revoke watermark changed"
    );
    // The dispatcher's own gate on the job still answers first, and agrees.
    let gate = expect_conflict(
        dispatcher.status(&admin, &done.id).await,
        "status of the job after the cleanup",
    );
    assert!(gate.contains("redacted"), "{gate}");
    assert!(gate.contains(&done.private_input_ref), "{gate}");
}

#[tokio::test]
async fn the_economic_coordinator_propagates_the_named_conflict_of_a_redacted_selection() {
    // `PersistentReplayEconomicCoordinator::register` reads the selection report
    // through `verified_replay_report_view` before it looks at anything else, and
    // `load_live_selection` hands that error on unchanged (the call sits in
    // replay_experiment.rs). No gate stands in front of it, so before this change
    // a redacted report surfaced here as `Internal`.
    let (_dir, store, admin) = open("economic-selection.sqlite3").await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let stored = replay_fx::store_report(&admin, &store, &pool).await;
    let experiment = economic_fx::experiment(&stored);
    experiment.validate().unwrap();

    let cleanup = revoke_and_clean(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");

    let message = expect_conflict(
        PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment).await,
        "economic register over a redacted selection report",
    );
    assert_eq!(
        message,
        redacted_message(REPLAY_REPORT_RECORD_KIND, &stored.report_id)
    );
}

#[tokio::test]
async fn each_replay_read_point_names_the_record_that_was_redacted() {
    // The cleanup redacts a world, its pool and its report together, so a read
    // that stops on the first one never reaches the later ones. Each record is
    // redacted on its own here, in the shape the cleanup leaves, so every read
    // point is exercised by itself.
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();

    // Report only: the pool and the worlds are live.
    let (_dir, store, admin) = open("only-report.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let stored = replay_fx::store_report(&admin, &store, &pool).await;
    put_raw(
        &store,
        &admin,
        "artifact",
        &stored.report_id,
        &redacted_store_row(&stored.report_id, REPLAY_REPORT_RECORD_KIND),
    )
    .await;
    let report = redacted_message(REPLAY_REPORT_RECORD_KIND, &stored.report_id);
    assert_eq!(
        expect_conflict(
            load_live_replay_report(&evaluator, &store, &stored.report_id).await,
            "load_live_replay_report"
        ),
        report
    );
    // `put_replay_report` finds the report already stored (the read of an existing
    // report) and `run_and_persist_pool_replay` looks it up before it recomputes.
    assert_eq!(
        expect_conflict(
            put_replay_report(&admin, &store, &stored).await,
            "put_replay_report over a redacted report"
        ),
        report
    );
    assert_eq!(
        expect_conflict(
            run_and_persist_pool_replay(
                &admin,
                &store,
                &pool.pool_digest,
                WorldPartition::Select,
                &replay_fx::policy(),
                &replay_fx::sample_profile(&pool.pool_digest),
                &replay_fx::caps(),
            )
            .await,
            "run_and_persist_pool_replay over a redacted report"
        ),
        report
    );
    assert_eq!(
        expect_conflict(
            verified_replay_report_view(&evaluator, &store, &stored.report_id).await,
            "verified_replay_report_view"
        ),
        report
    );
    // The pool itself is still readable.
    load_live_replay_pool(&evaluator, &store, &pool.pool_digest)
        .await
        .unwrap();

    // Pool only: the report and the worlds are live.
    let (_dir, store, admin) = open("only-pool.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let stored = replay_fx::store_report(&admin, &store, &pool).await;
    let pool_row = replay_pool_storage_id(&pool.pool_digest).unwrap();
    put_raw(
        &store,
        &admin,
        "artifact",
        &pool_row,
        &redacted_store_row(&pool_row, REPLAY_POOL_RECORD_KIND),
    )
    .await;
    let pool_message = redacted_message(REPLAY_POOL_RECORD_KIND, &pool_row);
    assert_eq!(
        expect_conflict(
            load_live_replay_pool(&evaluator, &store, &pool.pool_digest).await,
            "load_live_replay_pool"
        ),
        pool_message
    );
    // The live report loads its pool: the redacted pool is named there too.
    assert_eq!(
        expect_conflict(
            load_live_replay_report(&evaluator, &store, &stored.report_id).await,
            "load_live_replay_report over a redacted pool"
        ),
        pool_message
    );
    assert_eq!(
        expect_conflict(
            put_replay_report(&admin, &store, &stored).await,
            "put_replay_report over a redacted pool"
        ),
        pool_message
    );
    // `register_replay_pool` finds the pool already stored: the read of an
    // existing pool, after its worlds were loaded live.
    assert_eq!(
        expect_conflict(
            register_replay_pool(
                &admin,
                &store,
                &["world-1".to_string(), "world-train".to_string()]
            )
            .await,
            "register_replay_pool over a redacted pool"
        ),
        pool_message
    );

    // A world on its own, sealed or still a draft.
    let (_dir, store, admin) = open("only-world.sqlite3").await;
    for (id, sealed) in [
        ("world-redacted-sealed", true),
        ("world-redacted-draft", false),
    ] {
        let mut session = store.session().await.unwrap();
        session
            .put_world(&admin, id, sealed, &redacted_world_manifest(id))
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert_eq!(
            expect_conflict(
                load_live_replay_world(&admin, &store, id, Purpose::Development).await,
                &format!("load_live_replay_world of {id}")
            ),
            redacted_message(WORLD_KIND, id),
            "a redacted draft world is named as redacted, not as unsealed"
        );
    }
    // A pool registration over a redacted world names that world.
    assert_eq!(
        expect_conflict(
            register_replay_pool(&admin, &store, &["world-redacted-sealed".to_string()]).await,
            "register_replay_pool over a redacted world"
        ),
        redacted_message(WORLD_KIND, "world-redacted-sealed")
    );
}

#[tokio::test]
async fn a_replay_body_that_is_not_a_tombstone_and_does_not_decode_keeps_its_classification() {
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    // Bodies that are not the `rsia.redacted.v1` tombstone and are not envelopes:
    // a damaged envelope, a tombstone of another version, a body without a
    // schema, one whose schema is not a string, and a body that is no object.
    let damaged = |id: &str| {
        vec![
            json!({"schema_version": REPLAY_STORE_ENVELOPE_SCHEMA, "id": id, "unexpected": true}),
            json!({"schema_version": "rsia.redacted.v2", "id": id}),
            json!({"id": id}),
            json!({"schema_version": 7}),
            json!([1, 2, 3]),
        ]
    };

    // Report row.
    let (_dir, store, admin) = open("damaged-report.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let stored = replay_fx::store_report(&admin, &store, &pool).await;
    for body in damaged(&stored.report_id) {
        put_raw(&store, &admin, "artifact", &stored.report_id, &body).await;
        expect_internal(
            load_live_replay_report(&evaluator, &store, &stored.report_id).await,
            &format!("load_live_replay_report over {body}"),
        );
        expect_internal(
            put_replay_report(&admin, &store, &stored).await,
            &format!("put_replay_report over {body}"),
        );
        expect_internal(
            verified_replay_report_view(&evaluator, &store, &stored.report_id).await,
            &format!("verified_replay_report_view over {body}"),
        );
    }

    // Pool row.
    let (_dir, store, admin) = open("damaged-pool.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let stored = replay_fx::store_report(&admin, &store, &pool).await;
    let pool_row = replay_pool_storage_id(&pool.pool_digest).unwrap();
    for body in damaged(&pool_row) {
        put_raw(&store, &admin, "artifact", &pool_row, &body).await;
        expect_internal(
            load_live_replay_pool(&evaluator, &store, &pool.pool_digest).await,
            &format!("load_live_replay_pool over {body}"),
        );
        expect_internal(
            load_live_replay_report(&evaluator, &store, &stored.report_id).await,
            &format!("load_live_replay_report over a pool of {body}"),
        );
        expect_internal(
            register_replay_pool(
                &admin,
                &store,
                &["world-1".to_string(), "world-train".to_string()],
            )
            .await,
            &format!("register_replay_pool over {body}"),
        );
    }

    // World row: a sealed body that is not a `ReplayWorldV2` was `Invalid` before
    // (the world read never answered `Internal`) and is `Invalid` still.
    let (_dir, store, admin) = open("damaged-world.sqlite3").await;
    let mut session = store.session().await.unwrap();
    session
        .put_world(&admin, "legacy-world", true, &json!({"version": "legacy"}))
        .await
        .unwrap();
    session.commit().await.unwrap();
    match load_live_replay_world(&admin, &store, "legacy-world", Purpose::Development).await {
        Err(Error::Invalid(message)) => assert_eq!(message, "sealed world is not ReplayWorldV2"),
        other => panic!("a sealed body that is no world: expected Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. Submit side: the tombstone's source kind decides, per dependency kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_revoked_run_does_not_refuse_a_live_artifact_that_shares_its_id() {
    // A trusted run is given the id the pool's storage row will have, revoked and
    // cleaned: `begin_revoke` leaves a tombstone (source_kind "run") under that id.
    // The pool is then registered as an *artifact* of the same id and is live. A
    // `replay.run` request depends on that artifact, not on the run.
    let (_dir, store, admin) = open("shared-id.sqlite3").await;
    let worlds = replay_fx::worlds();
    let pool = replay_fx::manifest(&worlds);
    let pool_row = replay_pool_storage_id(&pool.pool_digest).unwrap();
    replay_fx::store_sources(&admin, &store, &worlds).await;
    put_raw(
        &store,
        &admin,
        "run",
        &pool_row,
        &json!({"schema_version": "fixture.run.v1", "id": pool_row}),
    )
    .await;
    let cleanup = revoke_and_clean(&store, &admin, &pool_row).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_eq!(cleanup.last_error, None);
    // The revocation bumped the watermark to 1, the one the worlds are sealed for.
    let registered = replay_fx::seal_and_register(&admin, &store, &worlds).await;
    assert_eq!(registered.pool_digest, pool.pool_digest);

    let tombstone: RevokeTombstone =
        serde_json::from_value(raw(&store, &admin, "tombstone", &pool_row).await).unwrap();
    assert_eq!(tombstone.source_kind, "run");
    assert!(try_raw(&store, &admin, "run", &pool_row).await.is_none());
    let artifact = raw(&store, &admin, "artifact", &pool_row).await;
    assert_ne!(artifact["schema_version"], REDACTED);

    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-shared-id", &pool.pool_digest),
        )
        .await
        .expect("a revoked run must not refuse a live artifact of the same id");
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "replay_stored");
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());
}

#[tokio::test]
async fn a_revoked_run_still_refuses_a_request_that_depends_on_that_run() {
    let (_dir, store, admin) = open("run-refused.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    // Control: over live runs the request is accepted and runs.
    let first = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request("exploration-live", &exploration_fx::world("world-1", 1)),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &first.id).await.state,
        ManagementJobState::Succeeded
    );

    let cleanup = revoke_and_clean(&store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    let tombstone: RevokeTombstone =
        serde_json::from_value(raw(&store, &admin, "tombstone", "run-failure").await).unwrap();
    assert_eq!(tombstone.source_kind, "run");

    let before = counts(&store, &admin).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "exploration.start",
                exploration_fx::start_request(
                    "exploration-after-cleanup",
                    &exploration_fx::world("world-2", 2),
                ),
            )
            .await,
        "submit over a revoked run",
    );
    assert_eq!(
        message,
        "management request depends on run run-failure, whose source was revoked"
    );
    assert_eq!(counts(&store, &admin).await, before);
    assert_no_idempotency_row(
        &store,
        &admin,
        "exploration.start",
        "exploration-after-cleanup",
    )
    .await;
}

fn import_source_envelope(id: &str) -> Value {
    // The one artifact `begin_revoke` accepts as a source: a strict E16
    // `import_source` envelope.
    json!({
        "schema_version": "rsia.e16.import_source.v1",
        "id": id,
        "namespace": "n",
        "owner_actor": "admin",
        "request_key": "import-source-fixture",
        "input_digest": d("import-input"),
        "created_at": 1,
        "updated_at": 1,
        "source_refs": [],
        "revoke_watermark": 0,
        "payload": {},
    })
}

#[tokio::test]
async fn a_revoked_artifact_source_still_refuses_a_request_that_depends_on_that_artifact() {
    // The artifact itself is the revoked source (a strict E16 import source stored
    // under the id of a replay pool row; only `begin_revoke` ran, so the artifact
    // is not redacted yet and the tombstone alone decides).
    let (_dir, store, admin) = open("artifact-refused.sqlite3").await;
    let pool_digest = d("pool-whose-row-is-a-revoked-artifact");
    let pool_row = replay_pool_storage_id(&pool_digest).unwrap();
    put_raw(
        &store,
        &admin,
        "artifact",
        &pool_row,
        &import_source_envelope(&pool_row),
    )
    .await;
    let status = begin(&store, &admin, "artifact", &pool_row).await;
    assert_eq!(status.state, CleanupState::Pending, "{status:?}");
    let tombstone: RevokeTombstone =
        serde_json::from_value(raw(&store, &admin, "tombstone", &pool_row).await).unwrap();
    assert_eq!(tombstone.source_kind, "artifact");
    assert_ne!(
        raw(&store, &admin, "artifact", &pool_row).await["schema_version"],
        REDACTED
    );

    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let before = counts(&store, &admin).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "replay.run",
                replay_fx::run_request("replay-revoked-artifact", &pool_digest),
            )
            .await,
        "submit over a revoked artifact",
    );
    assert_eq!(
        message,
        format!("management request depends on artifact {pool_row}, whose source was revoked")
    );
    assert_eq!(counts(&store, &admin).await, before);
    assert_no_idempotency_row(&store, &admin, "replay.run", "replay-revoked-artifact").await;
}

/// Tombstone bodies `begin_revoke` never writes: the shape older tests leave (an
/// id and nothing else), a body that is not an object, a decodable body missing a
/// field, one with an unknown field, and one whose source kind is neither `run`
/// nor `artifact`.
fn unreadable_tombstones(id: &str) -> Vec<Value> {
    let unknown_kind = serde_json::to_value(tombstone(id, "release")).unwrap();
    let mut extra_field = serde_json::to_value(tombstone(id, "run")).unwrap();
    extra_field["unexpected"] = json!(true);
    let mut missing_field = serde_json::to_value(tombstone(id, "run")).unwrap();
    missing_field.as_object_mut().unwrap().remove("reason");
    vec![
        json!({"id": id}),
        json!("revoked"),
        json!([id]),
        missing_field,
        extra_field,
        unknown_kind,
    ]
}

#[tokio::test]
async fn an_unreadable_tombstone_fails_the_submit_check_closed() {
    // An artifact dependency: a live pool, and the tombstone under its row id.
    let (_dir, store, admin) = open("unreadable-artifact.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let pool_row = replay_pool_storage_id(&pool.pool_digest).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let before = counts(&store, &admin).await;
    for (index, body) in unreadable_tombstones(&pool_row).into_iter().enumerate() {
        put_raw(&store, &admin, "tombstone", &pool_row, &body).await;
        let key = format!("replay-unreadable-{index}");
        let message = expect_conflict(
            dispatcher
                .submit(
                    &admin,
                    "replay.run",
                    replay_fx::run_request(&key, &pool.pool_digest),
                )
                .await,
            &format!("submit under the tombstone {body}"),
        );
        assert_eq!(
            message,
            format!(
                "management request depends on artifact {pool_row}, whose revocation tombstone cannot be read; the source is treated as revoked"
            ),
            "{body}"
        );
        assert_eq!(counts(&store, &admin).await, before, "{body}");
        assert_no_idempotency_row(&store, &admin, "replay.run", &key).await;
    }
    // Control: the same store and request under a well-formed tombstone of a
    // run (a different object that shares the id) is accepted and runs.
    put_raw(
        &store,
        &admin,
        "tombstone",
        &pool_row,
        &serde_json::to_value(tombstone(&pool_row, "run")).unwrap(),
    )
    .await;
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-readable", &pool.pool_digest),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");

    // A run dependency: live runs, and the tombstone under one run id.
    let (_dir, store, admin) = open("unreadable-run.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let before = counts(&store, &admin).await;
    for (index, body) in unreadable_tombstones("run-failure").into_iter().enumerate() {
        put_raw(&store, &admin, "tombstone", "run-failure", &body).await;
        let key = format!("exploration-unreadable-{index}");
        let message = expect_conflict(
            dispatcher
                .submit(
                    &admin,
                    "exploration.start",
                    exploration_fx::start_request(&key, &exploration_fx::world("world-1", 1)),
                )
                .await,
            &format!("submit under the tombstone {body}"),
        );
        assert_eq!(
            message,
            "management request depends on run run-failure, whose revocation tombstone cannot be read; the source is treated as revoked",
            "{body}"
        );
        assert_eq!(counts(&store, &admin).await, before, "{body}");
        assert_no_idempotency_row(&store, &admin, "exploration.start", &key).await;
    }
}
