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
    (
        dir,
        HttpState {
            service: HostService::new(store, host).unwrap(),
            prepare_config,
            auth,
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
        StatusCode::NOT_IMPLEMENTED
    );
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
