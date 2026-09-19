//! Real loopback HTTP process tests.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::{Context, Role, hash};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_http::{AuthIdentity, AuthRegistry, HttpState, router};
use evo_storage::Store;
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
        "/v1/manage/replay.run",
        Some(ADMIN_TOKEN),
        json!({"schema_version":"rsia.management.replay_run.v1","request_key":"blocked-1"}),
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
