//! Real rmcp stdio handshake and tool-call tests.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::{Context, Role, hash};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_mcp::{McpHost, serve_strict};
use evo_storage::Store;
use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

async fn server() -> (tempfile::TempDir, McpHost) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mcp.sqlite3")).await.unwrap();
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
    let service = HostService::new(store, host).unwrap();
    let caller = Context::new("tenant-a", "stdio-agent", Role::Agent).unwrap();
    let config = HostPrepareConfig {
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
    (dir, McpHost::new(service, caller, config).unwrap())
}

fn params(name: &'static str, value: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name).with_arguments(value.as_object().unwrap().clone())
}

#[tokio::test]
async fn real_stdio_handshake_and_four_tool_calls() {
    let (_dir, server) = server().await;
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let (reader, writer) = tokio::io::split(server_transport);
        serve_strict(server, reader, writer).await.unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();

    let listed = client.list_tools(None).await.unwrap();
    let names = listed
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 4);
    assert!(names.contains(&"evo_prepare"));
    assert!(names.contains(&"evo_feedback"));
    assert!(names.contains(&"evo_propose"));
    assert!(names.contains(&"evo_inspect"));
    assert!(!names.contains(&"evaluation.start"));

    let prepared = client
        .call_tool(params(
            "evo_prepare",
            json!({"request_key":"prepare-1","goal":"locate config","capabilities":[]}),
        ))
        .await
        .unwrap();
    assert_ne!(prepared.is_error, Some(true));
    let prepared = prepared.structured_content.unwrap();
    assert_eq!(prepared["skills"], json!([]));
    let run_id = prepared["run"]["id"].as_str().unwrap().to_string();
    let snapshot_id = prepared["run"]["snapshot_id"].as_str().unwrap().to_string();

    let feedback = client
        .call_tool(params(
            "evo_feedback",
            json!({
                "request_key":"feedback-1",
                "run_id":run_id,
                "outcome":"failure",
                "details":"self report",
                "failure_class":"reasoning"
            }),
        ))
        .await
        .unwrap();
    assert_ne!(feedback.is_error, Some(true));
    let feedback = feedback.structured_content.unwrap();
    assert_eq!(feedback["verification"], "self_reported_unverified");
    let feedback_id = feedback["id"].as_str().unwrap().to_string();

    let proposal = client
        .call_tool(params(
            "evo_propose",
            json!({
                "request_key":"proposal-1",
                "run_id":run_id,
                "kind":"skill",
                "parent_snapshot":snapshot_id,
                "hypothesis":"config precedence helps",
                "applicability":"reference host",
                "counterexample":"unrelated host",
                "content":"read the higher priority config first",
                "evidence_refs":[feedback_id],
                "required_capabilities":[],
                "dependencies":[]
            }),
        ))
        .await
        .unwrap();
    assert_ne!(proposal.is_error, Some(true));
    let proposal = proposal.structured_content.unwrap();
    assert_eq!(proposal["state"], "proposed");
    let candidate_id = proposal["id"].as_str().unwrap().to_string();

    let inspected = client
        .call_tool(params(
            "evo_inspect",
            json!({"kind":"candidate","id":candidate_id}),
        ))
        .await
        .unwrap();
    assert_ne!(inspected.is_error, Some(true));
    assert_eq!(inspected.structured_content.unwrap()["state"], "proposed");

    let injected = client
        .call_tool(params(
            "evo_prepare",
            json!({"request_key":"bad-role","goal":"x","capabilities":[],"role":"admin"}),
        ))
        .await
        .unwrap();
    assert_eq!(injected.is_error, Some(true));

    client.cancel().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn raw_duplicate_key_is_rejected_before_dispatch_and_writes_nothing() {
    let (_dir, first_server) = server().await;
    let retry_server = first_server.clone();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let first_task = tokio::spawn(async move {
        let (reader, writer) = tokio::io::split(server_transport);
        serve_strict(first_server, reader, writer).await
    });
    let (client_reader, mut client_writer) = tokio::io::split(client_transport);
    let mut client_reader = BufReader::new(client_reader);
    client_writer
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"raw-test","version":"1.0.0"}}}
"#,
        )
        .await
        .unwrap();
    let mut initialized = String::new();
    client_reader.read_line(&mut initialized).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&initialized).unwrap()["id"],
        1
    );
    client_writer
        .write_all(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#,
        )
        .await
        .unwrap();
    client_writer
        .write_all(
            br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"evo_prepare","arguments":{"request_key":"raw-dup","request_key":"raw-dup","goal":"poison","capabilities":[]}}}
"#,
        )
        .await
        .unwrap();
    drop(client_writer);
    let rejected = first_task.await.unwrap();
    assert!(rejected.is_err());

    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let retry_task = tokio::spawn(async move {
        let (reader, writer) = tokio::io::split(server_transport);
        serve_strict(retry_server, reader, writer).await.unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();
    let prepared = client
        .call_tool(params(
            "evo_prepare",
            json!({"request_key":"raw-dup","goal":"clean","capabilities":[]}),
        ))
        .await
        .unwrap();
    assert_ne!(prepared.is_error, Some(true));
    assert_eq!(prepared.structured_content.unwrap()["run"]["goal"], "clean");
    client.cancel().await.unwrap();
    retry_task.await.unwrap();
}
