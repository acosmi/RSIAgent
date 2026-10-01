//! Real loopback HTTP process tests.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::evidence::Purpose;
use evo_core::replay::*;
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Role, hash};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_http::{AuthIdentity, AuthRegistry, HttpState, router};
use evo_storage::Store;
use evo_storage::replay::{register_replay_pool, seal_replay_world};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const AGENT_A_TOKEN: &str = "agent-a-token-0123456789";
const AGENT_B_TOKEN: &str = "agent-b-token-0123456789";
const ADMIN_TOKEN: &str = "admin-token-01234567890";
const HOST_TOKEN: &str = "host-token-0123456789";
const EVALUATOR_TOKEN: &str = "evaluator-token-0123456789";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

async fn state(auth: AuthRegistry) -> (tempfile::TempDir, HttpState) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("http.sqlite3")).await.unwrap();
    let host = Context::new("tenant-a", "gateway-host", Role::Host).unwrap();
    let admin = Context::new("tenant-a", "admin", Role::Admin).unwrap();
    ReleaseStore::register_host_surface(
        &admin,
        &store,
        "reference-surface",
        HostSurfaceManifest {
            schema_version: "rsia.host_surface.v1".into(),
            host: "reference-host".into(),
            host_version: "1.0.0".into(),
            adapter_version: "1.0.0".into(),
            source_digest: d("surface"),
            items: vec![SurfaceItem {
                name: "model".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("host.model".into()),
                consumer: Some("reference-host".into()),
                reason: "fixture".into(),
            }],
        },
        vec!["model".into()],
    )
    .await
    .unwrap();
    let prepare_config = HostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "profile-a".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: vec!["read_config".into()],
            mandatory_context_digest: d("mandatory"),
        },
        host_surface_id: "reference-surface".into(),
        host_capabilities: HostCapabilities {
            available: BTreeSet::new(),
            granted: BTreeSet::new(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    };
    let service = HostService::new(store, host).unwrap();
    let management = evo_engine::dispatch::ManagementDispatcher::new(
        service.store().clone(),
        auth.management_contexts().unwrap(),
    )
    .unwrap();
    (
        dir,
        HttpState {
            service,
            prepare_config,
            auth,
            management,
        },
    )
}

fn registry() -> AuthRegistry {
    AuthRegistry::from_plaintext(vec![
        (
            AGENT_A_TOKEN.into(),
            AuthIdentity::new("tenant-a", "agent-a", Role::Agent).unwrap(),
        ),
        (
            AGENT_B_TOKEN.into(),
            AuthIdentity::new("tenant-a", "agent-b", Role::Agent).unwrap(),
        ),
        (
            ADMIN_TOKEN.into(),
            AuthIdentity::new("tenant-a", "admin", Role::Admin).unwrap(),
        ),
        (
            HOST_TOKEN.into(),
            AuthIdentity::new("tenant-a", "gateway-host", Role::Host).unwrap(),
        ),
        (
            EVALUATOR_TOKEN.into(),
            AuthIdentity::new("tenant-a", "evaluator", Role::Evaluator).unwrap(),
        ),
    ])
    .unwrap()
}

async fn spawn(state: HttpState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> reqwest::Response {
    let mut request = client.post(format!("{base}{path}")).json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap()
}

async fn raw_post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: &str,
    body: &str,
) -> reqwest::Response {
    client
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
}

async fn wait_job(client: &reqwest::Client, base: &str, token: &str, job_id: &str) -> Value {
    for _ in 0..100 {
        let response = client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let job: Value = response.json().await.unwrap();
        if matches!(
            job["state"].as_str(),
            Some("succeeded" | "failed" | "cancelled" | "blocked")
        ) {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("management job did not finish");
}

#[tokio::test]
async fn real_http_process_enforces_auth_identity_and_idempotency() {
    let (_dir, state) = state(registry()).await;
    let (base, task) = spawn(state).await;
    let client = reqwest::Client::new();
    let prepare = json!({"request_key":"prepare-1","goal":"locate config","capabilities":[]});

    assert_eq!(
        post(&client, &base, "/v1/tools/prepare", None, prepare.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/tools/prepare",
            Some("wrong-token-0123456789"),
            prepare.clone()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let injected_header = client
        .post(format!("{base}/v1/tools/prepare"))
        .bearer_auth(AGENT_A_TOKEN)
        .header("x-role", "admin")
        .json(&prepare)
        .send()
        .await
        .unwrap();
    assert_eq!(injected_header.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/tools/prepare",
            Some(AGENT_A_TOKEN),
            json!({"request_key":"p-role","goal":"x","capabilities":[],"role":"admin"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let missing_status = post(
        &client,
        &base,
        "/v1/manage/evaluation.status",
        Some(EVALUATOR_TOKEN),
        json!({"schema_version":"rsia.management.evaluation_status.v1","request_key":"status-missing","ticket_id":"missing-ticket"}),
    )
    .await;
    assert_eq!(missing_status.status(), StatusCode::OK);
    let missing_status: Value = missing_status.json().await.unwrap();
    let missing_job_id = missing_status["id"].as_str().unwrap();
    let failed = wait_job(&client, &base, EVALUATOR_TOKEN, missing_job_id).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error_code"], "not_found");
    let failed_text = serde_json::to_string(&failed).unwrap();
    assert!(!failed_text.contains("missing-ticket"));
    assert!(!failed_text.contains("rsia.management.evaluation_status.v1"));
    let live_missing = client
        .get(format!("{base}/v1/manage/jobs/{missing_job_id}"))
        .bearer_auth(EVALUATOR_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(live_missing.status(), StatusCode::OK);
    let live_missing: Value = live_missing.json().await.unwrap();
    assert_eq!(live_missing["state"], "failed");
    assert_eq!(live_missing["error_code"], "not_found");
    assert_eq!(live_missing["step"], failed["step"]);
    let absent_job = client
        .get(format!("{base}/v1/manage/jobs/job-does-not-exist"))
        .bearer_auth(EVALUATOR_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(absent_job.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/tools/prepare",
            Some(AGENT_A_TOKEN),
            json!({"request_key":"p-extra","goal":"x","capabilities":[],"extra":true})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        raw_post(
            &client,
            &base,
            "/v1/tools/prepare",
            AGENT_A_TOKEN,
            r#"{"request_key":"prepare-1","request_key":"prepare-1","goal":"poison","capabilities":[]}"#,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw_post(
            &client,
            &base,
            "/v1/tools/prepare",
            AGENT_A_TOKEN,
            r#"{"request_key":"identity-dup","goal":"x","capabilities":[],"role":"agent","role":"admin"}"#,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw_post(
            &client,
            &base,
            "/v1/tools/prepare",
            AGENT_A_TOKEN,
            r#"{"request_key":"identity-dup","goal":"x","capabilities":[],"namespace":"a","namespace":"b"}"#,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw_post(
            &client,
            &base,
            "/v1/host/trace",
            HOST_TOKEN,
            r#"{"schema_version":"rsia.optimization.source.v1","record":{"id":"trace-a","id":"trace-b"}}"#,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let identity_after_rejections = post(
        &client,
        &base,
        "/v1/tools/prepare",
        Some(AGENT_A_TOKEN),
        json!({"request_key":"identity-dup","goal":"x","capabilities":[]}),
    )
    .await;
    assert_eq!(identity_after_rejections.status(), StatusCode::OK);

    let response = post(
        &client,
        &base,
        "/v1/tools/prepare",
        Some(AGENT_A_TOKEN),
        prepare.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let prepared: Value = response.json().await.unwrap();
    assert_eq!(prepared["skills"], json!([]));
    let run_id = prepared["run"]["id"].as_str().unwrap();

    assert_eq!(
        post(
            &client,
            &base,
            "/v1/tools/prepare",
            Some(AGENT_A_TOKEN),
            json!({"request_key":"prepare-1","goal":"different","capabilities":[]})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/tools/inspect",
            Some(AGENT_B_TOKEN),
            json!({"kind":"run","id":run_id})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for trace_id in ["trace-a", "trace-b"] {
        assert_eq!(
            post(
                &client,
                &base,
                "/v1/tools/inspect",
                Some(HOST_TOKEN),
                json!({"kind":"run","id":trace_id})
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/evaluation.start",
            Some(ADMIN_TOKEN),
            json!({})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let blocked = post(
        &client,
        &base,
        "/v1/manage/meta.start",
        Some(ADMIN_TOKEN),
        json!({"schema_version":"rsia.management.meta_start.v1","request_key":"blocked-1"}),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::OK);
    let blocked: Value = blocked.json().await.unwrap();
    let blocked = wait_job(&client, &base, ADMIN_TOKEN, blocked["id"].as_str().unwrap()).await;
    assert_eq!(blocked["state"], "blocked");
    let job_id = blocked["id"].as_str().unwrap();
    let status = client
        .get(format!("{base}/v1/manage/jobs/{job_id}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let cancel = post(
        &client,
        &base,
        &format!("/v1/manage/jobs/{job_id}/cancel"),
        Some(ADMIN_TOKEN),
        json!({}),
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(cancel.json::<Value>().await.unwrap()["state"], "blocked");
    task.abort();
}

#[tokio::test]
async fn no_auth_registry_refuses_sensitive_routes() {
    let (_dir, state) = state(AuthRegistry::default()).await;
    let (base, task) = spawn(state).await;
    let response = post(
        &reqwest::Client::new(),
        &base,
        "/v1/tools/prepare",
        Some(AGENT_A_TOKEN),
        json!({"request_key":"prepare-1","goal":"x","capabilities":[]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    task.abort();
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
async fn authenticated_async_replay_flow_and_role_rejection() {
    let (_dir, state) = state(registry()).await;
    let store = state.service.store().clone();
    let admin = Context::new("tenant-a", "admin", Role::Admin).unwrap();
    let pool = setup_sealed_pool(&admin, &store).await;
    let (base, task) = spawn(state).await;
    let client = reqwest::Client::new();

    let profile = sample_profile(pool.pool_digest.clone());
    let payload = json!({
        "schema_version": "rsia.management.replay_run.v1",
        "request_key": "http-replay-k1",
        "pool_digest": pool.pool_digest,
        "partition": "select",
        "policy": ElasticPolicyV1::default(),
        "profile": profile,
        "caps": ExplorationCapsV1::online(),
    });

    // 1. Unauthenticated -> 401 Unauthorized
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/replay.run",
            None,
            payload.clone()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    // 2. Role rejection: Agent -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/replay.run",
            Some(AGENT_A_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 3. Role rejection: Evaluator -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/replay.run",
            Some(EVALUATOR_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 4. Authenticated Admin -> 200 OK, queued job
    let response = post(
        &client,
        &base,
        "/v1/manage/replay.run",
        Some(ADMIN_TOKEN),
        payload.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let queued: Value = response.json().await.unwrap();
    assert_eq!(queued["state"], "queued");
    let job_id = queued["id"].as_str().unwrap();

    // 5. Wait for terminal state -> succeeded with ReplayStored
    let terminal = wait_job(&client, &base, ADMIN_TOKEN, job_id).await;
    assert_eq!(terminal["state"], "succeeded");
    assert_eq!(terminal["step"], "replay_stored");

    // 6. GET job status via HTTP returns verified result
    let status_res = client
        .get(format!("{base}/v1/manage/jobs/{job_id}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(status_res.status(), StatusCode::OK);
    let status: Value = status_res.json().await.unwrap();
    assert_eq!(status["state"], "succeeded");
    assert_eq!(status["result"]["result"], "replay_stored");
    assert_eq!(status["result"]["pool_digest"], pool.pool_digest);
    assert!(status["result"]["report_id"].as_str().is_some());

    // 7. Non-management role cannot access management job endpoint (403 Forbidden)
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(AGENT_A_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    // 8. Management role that does not own the job gets 404 NotFound
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(EVALUATOR_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    task.abort();
}

// ---------------------------------------------------------------------------
// curriculum.step through the real listener (E07 management adapter).
// Fixtures replicate crates/evo-engine/tests/curriculum_v41.rs inline.
// ---------------------------------------------------------------------------

const CURRICULUM_PROFILE_ID: &str = "pure_function_test_proposal.v1";
const CURRICULUM_STATE_ID: &str = "learner-state";

async fn setup_curriculum(admin: &Context, store: &Store) {
    use evo_core::curriculum::{CurriculumControlProfileV1, LearnerStateV2};
    use evo_core::evaluation::DataUse;
    use evo_engine::curriculum::{
        CurriculumSourceArtifactV1, CurriculumSourceKindV1, PersistentCurriculumCoordinator,
    };
    use evo_engine::curriculum_profiles::{
        RegisteredPureFunctionProfileV1, registered_profile_source_bodies,
    };
    let source =
        |id: &str, source_kind, subject_digest: String, body: Value| CurriculumSourceArtifactV1 {
            schema_version: CurriculumSourceArtifactV1::SCHEMA.into(),
            id: id.into(),
            source_kind,
            data_use: DataUse::Development,
            subject_digest,
            body_digest: evo_core::fingerprint(&body).unwrap(),
            body,
            dependency_ids: vec![],
        };
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let profile = CurriculumControlProfileV1::offline_default(
        CURRICULUM_PROFILE_ID,
        d("task-space"),
        registered.oracle_digest.clone(),
        registered.runner_digest.clone(),
    )
    .unwrap();
    let state = LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: CURRICULUM_STATE_ID.into(),
        profile_id: CURRICULUM_PROFILE_ID.into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("environment"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest.clone(),
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec![
            registered.oracle_source_id.clone(),
            registered.runner_source_id.clone(),
            registered.target_source_id.clone(),
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
    };
    let coordinator =
        PersistentCurriculumCoordinator::new(store.clone(), admin.clone(), admin.actor()).unwrap();
    coordinator
        .register_source(source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets":["clamp-boundary"]}),
        ))
        .await
        .unwrap();
    for (id, body) in registered_profile_source_bodies() {
        let kind = if id.contains("target") || id.contains("source") {
            CurriculumSourceKindV1::TargetSpec
        } else if id.contains("oracle") {
            CurriculumSourceKindV1::OracleSpec
        } else {
            CurriculumSourceKindV1::RunnerSpec
        };
        coordinator
            .register_source(source(
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
        .register_profile_and_state(profile, state)
        .await
        .unwrap();
}

#[tokio::test]
async fn authenticated_async_curriculum_step_flow_and_role_rejection() {
    let (_dir, state) = state(registry()).await;
    let store = state.service.store().clone();
    let admin = Context::new("tenant-a", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let (base, task) = spawn(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "schema_version": "rsia.management.curriculum_step.v1",
        "request_key": "http-curriculum-k1",
        "profile_id": CURRICULUM_PROFILE_ID,
        "state_id": CURRICULUM_STATE_ID,
        "root_budget_limit_micros": 1_000_000u64,
    });

    // 1. Unauthenticated -> 401 Unauthorized
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            None,
            payload.clone()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    // 2. Role rejection: Agent -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            Some(AGENT_A_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 3. Role rejection: Evaluator -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            Some(EVALUATOR_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 4. Identity injection in the body is refused by the HTTP layer (403);
    // any other unknown field is refused by the typed request (400).
    let mut injected = payload.clone();
    injected["role"] = json!("admin");
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            Some(ADMIN_TOKEN),
            injected
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut unknown = payload.clone();
    unknown["extra_field"] = json!("unexpected");
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            Some(ADMIN_TOKEN),
            unknown
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    // 5. Authenticated Admin -> 200 OK, queued job with an immediate id
    let response = post(
        &client,
        &base,
        "/v1/manage/curriculum.step",
        Some(ADMIN_TOKEN),
        payload.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let queued: Value = response.json().await.unwrap();
    assert_eq!(queued["state"], "queued");
    let job_id = queued["id"].as_str().unwrap().to_string();

    // 6. Wait for terminal state -> succeeded with ProbeScheduled
    let terminal = wait_job(&client, &base, ADMIN_TOKEN, &job_id).await;
    assert_eq!(terminal["state"], "succeeded");
    assert_eq!(terminal["step"], "probe_scheduled");
    assert_eq!(terminal["result"]["result"], "probe_scheduled");
    assert_eq!(terminal["result"]["terminal"], "budget_exhausted");
    let probe_job_id = terminal["result"]["probe_job_id"].as_str().unwrap();
    assert!(probe_job_id.starts_with("probe-"));
    assert_eq!(
        terminal["result"]["trigger_digest"].as_str().unwrap().len(),
        64
    );

    // 7. GET job status via HTTP returns the re-verified result
    let status_res = client
        .get(format!("{base}/v1/manage/jobs/{job_id}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(status_res.status(), StatusCode::OK);
    let status: Value = status_res.json().await.unwrap();
    assert_eq!(status["state"], "succeeded");
    assert_eq!(status["result"], terminal["result"]);

    // 8. Same request reconnects to the same job (reconnect never re-charges)
    let same = post(
        &client,
        &base,
        "/v1/manage/curriculum.step",
        Some(ADMIN_TOKEN),
        payload.clone(),
    )
    .await;
    assert_eq!(same.status(), StatusCode::OK);
    let same: Value = same.json().await.unwrap();
    assert_eq!(same["id"], job_id);
    assert_eq!(same["state"], "succeeded");

    // 9. Same key, different payload -> 409 Conflict
    let mut different = payload.clone();
    different["root_budget_limit_micros"] = json!(2_000_000u64);
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/curriculum.step",
            Some(ADMIN_TOKEN),
            different
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    // 10. Non-management role cannot read the job (403); a management role
    // that does not own it gets 404.
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(AGENT_A_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(EVALUATOR_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // 11. Revocation gate (plan §11.4): bumping the source watermark makes the
    // read side refuse with 409 while the persisted terminal stays succeeded.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "curriculum-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let mut session = store.session().await.unwrap();
    let persisted: Value = session.need(&admin, "job", &job_id).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(persisted["state"], "succeeded");
    assert_eq!(persisted["result"], terminal["result"]);

    task.abort();
}

const EXPLORATION_WORLD_ID: &str = "world-http-1";

/// Same world as `exploration_v41::world()`, signed for `source_watermark == 1`.
fn exploration_world() -> evo_engine::exploration::ExplorationWorldV1 {
    use evo_core::strategy::SimulationContext;
    use evo_engine::exploration::{
        ExplorationDependency, ExplorationWorldV1, RootOpportunity, WorldState,
    };
    let parent_skill = d("parent-skill");
    let parent_bundle = d("parent-bundle");
    let environment = d("environment");
    let model = d("model");
    let tools = d("tools");
    let grader = d("grader");
    let rules = d("rules");
    let context_signature = evo_core::fingerprint(&(
        &parent_skill,
        &parent_bundle,
        &environment,
        &model,
        &tools,
        &grader,
        &rules,
        1u64,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: EXPLORATION_WORLD_ID.into(),
        approved_parent_digest: d("approved-parent"),
        context_signature,
        parent_skill_digest: parent_skill,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark: 1,
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
        dependencies: vec![
            ExplorationDependency {
                kind: "run".into(),
                id: "run-failure".into(),
            },
            ExplorationDependency {
                kind: "run".into(),
                id: "run-success".into(),
            },
        ],
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

/// Seeds the trusted source closure like `exploration_v41`: two Host-issued
/// trace authorities, a source selection grant and one watermark bump.
async fn setup_exploration(host: &Context, store: &Store) {
    use evo_core::evidence::{ExecutionAttestation, SourceSelection, TaskOrigin};
    use evo_core::optimization::{
        OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
    };
    use evo_core::skill_edit::EvidenceRef;
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
    };
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        let trace = OptimizationTrace {
            run_id: id.into(),
            parent_family: family.into(),
            source_digest: d(id),
            purpose: Purpose::Development,
            outcome,
            diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
                kind: SkillFailureKind::SkillDefect,
                skill_id: "skill".into(),
                bundle_digest: d("bundle"),
                request_digest: d("request"),
                rule_id: Some("rule".into()),
                support: vec![EvidenceRef {
                    id: id.into(),
                    digest: d(id),
                }],
                counterexamples: vec![],
                reason: "fixture".into(),
            }),
            excerpt: id.into(),
            seed: 1,
        };
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
            trace,
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(store, host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        store,
        host,
        &SourceSelection {
            roots: vec![],
            run_ids: vec!["run-failure".into(), "run-success".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn authenticated_async_exploration_start_flow_and_role_rejection() {
    let (_dir, state) = state(registry()).await;
    let store = state.service.store().clone();
    let host = Context::new("tenant-a", "gateway-host", Role::Host).unwrap();
    let admin = Context::new("tenant-a", "admin", Role::Admin).unwrap();
    setup_exploration(&host, &store).await;
    let (base, task) = spawn(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": "http-exploration-k1",
        "world": serde_json::to_value(exploration_world()).unwrap(),
    });

    // 1. Unauthenticated -> 401 Unauthorized
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            None,
            payload.clone()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    // 2. Role rejection: Agent -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(AGENT_A_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 3. Role rejection: Evaluator -> 403 Forbidden
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(EVALUATOR_TOKEN),
            payload.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // 4. Identity injection in the body is refused by the HTTP layer (403);
    // any other unknown field, at any nesting level, is refused by the typed
    // request (400).
    let mut injected = payload.clone();
    injected["role"] = json!("admin");
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(ADMIN_TOKEN),
            injected
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut unknown = payload.clone();
    unknown["extra_field"] = json!("unexpected");
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(ADMIN_TOKEN),
            unknown
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut unknown_world = payload.clone();
    unknown_world["world"]["extra_field"] = json!("unexpected");
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(ADMIN_TOKEN),
            unknown_world
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    // 5. Authenticated Admin -> 200 OK, queued job with an immediate id
    let response = post(
        &client,
        &base,
        "/v1/manage/exploration.start",
        Some(ADMIN_TOKEN),
        payload.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let queued: Value = response.json().await.unwrap();
    assert_eq!(queued["state"], "queued");
    let job_id = queued["id"].as_str().unwrap().to_string();

    // 6. Wait for terminal state -> succeeded with ExplorationStarted
    let terminal = wait_job(&client, &base, ADMIN_TOKEN, &job_id).await;
    assert_eq!(terminal["state"], "succeeded");
    assert_eq!(terminal["step"], "exploration_started");
    assert_eq!(terminal["result"]["result"], "exploration_started");
    assert_eq!(terminal["result"]["world_id"], EXPLORATION_WORLD_ID);
    assert_eq!(
        terminal["result"]["context_signature"],
        exploration_world().context_signature
    );
    assert_eq!(
        terminal["result"]["prefix_digest"].as_str().unwrap().len(),
        64
    );
    assert_eq!(
        terminal["result"]["legal_actions_digest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(terminal["result"]["action"]["decision"], "dispatch");
    assert_eq!(terminal["result"]["action"]["action_seqs"], json!([1]));

    // 7. GET job status via HTTP returns the re-verified result
    let status_res = client
        .get(format!("{base}/v1/manage/jobs/{job_id}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(status_res.status(), StatusCode::OK);
    let status: Value = status_res.json().await.unwrap();
    assert_eq!(status["state"], "succeeded");
    assert_eq!(status["result"], terminal["result"]);

    // 8. Same request reconnects to the same job (reconnect never re-charges)
    let same = post(
        &client,
        &base,
        "/v1/manage/exploration.start",
        Some(ADMIN_TOKEN),
        payload.clone(),
    )
    .await;
    assert_eq!(same.status(), StatusCode::OK);
    let same: Value = same.json().await.unwrap();
    assert_eq!(same["id"], job_id);
    assert_eq!(same["state"], "succeeded");

    // 9. Same key, different world -> 409 Conflict
    let mut different = payload.clone();
    different["world"]["remaining_root_micros"] = json!(2_000u64);
    assert_eq!(
        post(
            &client,
            &base,
            "/v1/manage/exploration.start",
            Some(ADMIN_TOKEN),
            different
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    // 10. Non-management role cannot read the job (403); a management role
    // that does not own it gets 404.
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(AGENT_A_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(EVALUATOR_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // 11. Revocation gate (V017/V056): bumping the source watermark makes the
    // read side refuse with 409 while the persisted terminal stays succeeded.
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "e09-source-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        client
            .get(format!("{base}/v1/manage/jobs/{job_id}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let mut session = store.session().await.unwrap();
    let persisted: Value = session.need(&admin, "job", &job_id).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(persisted["state"], "succeeded");
    assert_eq!(persisted["result"], terminal["result"]);

    task.abort();
}
