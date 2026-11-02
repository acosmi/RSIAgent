//! The three existing lifecycle routes share one actual versioned view.
use evo_core::contract::{CapabilityLevel, HostCapabilities, SurfaceCoverage, SystemSnapshot};
use evo_core::{Context, Role, hash};
use evo_engine::service::{CompleteHostPrepareConfig, HostPrepareConfig, HostService};
use evo_http::{AuthIdentity, AuthRegistry, HttpState, router, typed_route_tool_source};
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
    let source = evo_core::contract::ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: r#"{"schema_version":"rsia.host_reviewed_declarations.v1","host":"reference-host","host_version":"1.0.0","declarations":[{"name":"model","value_type":"string","enum_values":[],"default":{"kind":"value","value":"disabled"}}]}"#.into(),
    };
    let raw = serde_json::to_string(&source).unwrap();
    evo_engine::hosts::register_reviewed_host_surface_json(
        &admin,
        &store,
        "reference-surface",
        &raw,
        evo_core::contract::HostSurfaceRegistrationV2 {
            adapter_version: "1.0.0".into(),
            retired: vec![],
            classifications: vec![evo_core::contract::HostSurfaceClassificationV2 {
                name: "model".into(),
                coverage: SurfaceCoverage::RuntimeOwned,
                mapped_field: None,
                consumer: None,
                reason: "ToolOnly protocol fixture has no native model slot".into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            }],
        },
        None,
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
            tools: vec![],
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
    let prepare_config = CompleteHostPrepareConfig::from_producer(
        &prepare_config,
        typed_route_tool_source().unwrap(),
    )
    .unwrap();
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

async fn spawn(
    state: HttpState,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{address}"), stop, task)
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

#[tokio::test]
async fn submit_status_cancel_share_versioned_job_view() {
    let (_dir, state) = state(registry()).await;
    let (base, stop, server) = spawn(state).await;
    let client = reqwest::Client::new();
    let request =
        json!({"schema_version":"rsia.management.meta_start.v1","request_key":"one-versioned-job"});
    let response = post(
        &client,
        &base,
        "/v1/manage/meta.start",
        Some(ADMIN_TOKEN),
        request.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: Value = response.json().await.unwrap();
    assert_eq!(accepted["schema_version"], "rsia.management_job_status.v1");
    assert_eq!(accepted["job_schema_version"], "rsia.management_job.v2");
    assert_eq!(accepted["state"], "queued");
    assert!(accepted.get("cancel_arrival").is_none());
    let id = accepted["id"].as_str().unwrap();
    let mut final_view = Value::Null;
    for _ in 0..500 {
        let read: Value = client
            .get(format!("{base}/v1/manage/jobs/{id}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if read["state"] == "blocked" {
            final_view = read;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(final_view["state"], "blocked");
    assert_eq!(final_view["schema_version"], accepted["schema_version"]);
    assert_eq!(
        final_view["job_schema_version"],
        accepted["job_schema_version"]
    );
    let cancelled = post(
        &client,
        &base,
        &format!("/v1/manage/jobs/{id}/cancel"),
        Some(ADMIN_TOKEN),
        json!({}),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let unchanged: Value = cancelled.json().await.unwrap();
    assert_eq!(unchanged, final_view);
    let replay: Value = post(
        &client,
        &base,
        "/v1/manage/meta.start",
        Some(ADMIN_TOKEN),
        request,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(replay, final_view);
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn job_lifecycle_routes_keep_auth_identity_and_no_new_job() {
    let (_dir, state) = state(registry()).await;
    let store = state.service.store().clone();
    let (base, stop, server) = spawn(state).await;
    let client = reqwest::Client::new();
    let path = "/v1/manage/jobs/management-job-absent";
    assert_eq!(
        client
            .get(format!("{base}{path}"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{base}{path}"))
            .bearer_auth(AGENT_A_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let bad=post(&client,&base,"/v1/manage/meta.start",Some(ADMIN_TOKEN),json!({"schema_version":"rsia.management.meta_start.v1","request_key":"identity-body","actor":"other"})).await;
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    let admin = Context::new("tenant-a", "admin", Role::Admin).unwrap();
    let mut s = store.session().await.unwrap();
    assert!(s.list::<Value>(&admin, "job").await.unwrap().is_empty());
    s.commit().await.unwrap();
    assert_eq!(
        client
            .get(format!("{base}{path}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let missing = post(
        &client,
        &base,
        &format!("{path}/cancel"),
        Some(ADMIN_TOKEN),
        json!({}),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let mut s = store.session().await.unwrap();
    assert!(s.list::<Value>(&admin, "job").await.unwrap().is_empty());
    s.commit().await.unwrap();
    stop.send(()).unwrap();
    server.await.unwrap();
}
