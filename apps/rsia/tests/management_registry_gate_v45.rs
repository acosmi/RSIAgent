//! Actual CLI process controls for the single catalog's static manage gate.
use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const TOKEN: &str = "management-gate-fixture-secret-0123456789";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rsia-manage-gate-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("rsia.sqlite3"),
            b"client sentinel, never opened as a database",
        )
        .unwrap();
        Self(root)
    }
    fn contents(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().into(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect()
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rsia"));
        command
            .current_dir(&self.0)
            .env("RSIA_MANAGEMENT_TOKEN", TOKEN)
            .env_remove("RSIA_HTTP_TOKEN")
            .env_remove("RSIA_HOST_TOKEN")
            .env_remove("RSIA_ADMIN_TOKEN")
            .env_remove("RSIA_EVALUATOR_TOKEN");
        command
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn assert_token_hidden(output: &Output) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
}
fn static_refusal(names: &[&str], reason: &str) {
    let scratch = Scratch::new();
    let before = scratch.contents();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    for name in names {
        for request_file in [None, Some("absent-management-request.json")] {
            let mut command = scratch.command();
            command.args(["manage", name, "--url", &url]);
            if let Some(path) = request_file {
                command.args(["--request-file", path]);
            }
            let output = command.output().unwrap();
            assert_token_hidden(&output);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8(output.stderr).unwrap();
            let body: Value = serde_json::from_str(stderr.lines().next().unwrap()).unwrap();
            assert_eq!(
                body,
                json!({"operation":name,"status":"unsupported","reason":reason})
            );
            assert!(!stderr.contains("--request-file is required"));
            assert!(!stderr.contains("No such file"));
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_eq!(scratch.contents(), before);
        }
    }
    // Missing credentials retain precedence over even a name that is unknown.
    let mut command = scratch.command();
    command.env_remove("RSIA_MANAGEMENT_TOKEN").args([
        "manage",
        "unknown.management",
        "--url",
        &url,
    ]);
    let output = command.output().unwrap();
    assert_token_hidden(&output);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("RSIA_MANAGEMENT_TOKEN is required"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("unknown_management_operation"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(scratch.contents(), before);
}

#[test]
fn manage_unknown_name_refuses_before_request_file_and_http() {
    static_refusal(
        &[
            "unknown.management",
            "evo_prepare",
            "evo_feedback",
            "evo_propose",
            "evo_inspect",
            "control.review.extra",
        ],
        "unknown_management_operation",
    );
}

#[test]
fn manage_other_plane_refuses_before_request_file_and_http() {
    let names: Vec<_> = evo_core::management_registry::formal_names()
        .iter()
        .chain(evo_core::management_registry::offline_names())
        .copied()
        .collect();
    static_refusal(&names, "management_operation_plane_mismatch");
}

#[derive(Debug)]
struct CapturedRequest {
    method: Method,
    path: String,
    body: Vec<u8>,
    authorized: bool,
}
async fn capture(
    State(requests): State<Arc<Mutex<Vec<CapturedRequest>>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    requests.lock().unwrap().push(CapturedRequest {
        method,
        path: uri.path().to_owned(),
        body: body.to_vec(),
        authorized: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some(format!("Bearer {TOKEN}").as_str()),
    });
    match uri.path() {
        "/v1/manage/source.revoke" | "/v1/manage/development.probe_compose.enable" => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"management_operation_not_wired"}})),
        ),
        "/v1/manage/control.review" => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"invalid_review_payload"}})),
        ),
        _ => (
            StatusCode::OK,
            Json(json!({"id":"existing-job","state":"blocked"})),
        ),
    }
}

#[tokio::test]
async fn manage_registered_dispatch_keeps_forwarding_and_dedicated_job_routes() {
    let scratch = Scratch::new();
    let request =
        json!({"schema_version":"rsia.management.meta_start.v1","request_key":"cli-forward"});
    std::fs::write(
        scratch.0.join("request.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let before = scratch.contents();
    let requests = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let router = axum::Router::new()
        .fallback(capture)
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for name in [
        "meta.start",
        "source.revoke",
        "development.probe_compose.enable",
        "control.review",
        "job.status",
        "job.cancel",
    ] {
        let mut command = scratch.command();
        command.args(["manage", name, "--url", &url]);
        let (expected_method, expected_path, expected_body, expected_response) = match name {
            "job.status" => {
                command.args(["--job-id", "existing-job"]);
                (
                    Method::GET,
                    "/v1/manage/jobs/existing-job".to_owned(),
                    Vec::new(),
                    json!({"id":"existing-job","state":"blocked"}),
                )
            }
            "job.cancel" => {
                command.args(["--job-id", "existing-job"]);
                (
                    Method::POST,
                    "/v1/manage/jobs/existing-job/cancel".to_owned(),
                    b"{}".to_vec(),
                    json!({"id":"existing-job","state":"blocked"}),
                )
            }
            _ => {
                command.args(["--request-file", "request.json"]);
                let response = match name {
                    "meta.start" => json!({"id":"existing-job","state":"blocked"}),
                    "control.review" => json!({"error":{"code":"invalid_review_payload"}}),
                    _ => json!({"error":{"code":"management_operation_not_wired"}}),
                };
                (
                    Method::POST,
                    format!("/v1/manage/{name}"),
                    serde_json::to_vec(&request).unwrap(),
                    response,
                )
            }
        };
        let output = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert_token_hidden(&output);
        assert_eq!(
            output.status.success(),
            matches!(name, "meta.start" | "job.status" | "job.cancel")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected_response
        );
        let records = requests.lock().unwrap();
        let captured = records.last().unwrap();
        assert_eq!(captured.method, expected_method);
        assert_eq!(captured.path, expected_path);
        assert_eq!(captured.body, expected_body);
        assert!(captured.authorized);
        assert_eq!(scratch.contents(), before);
    }
    assert_eq!(requests.lock().unwrap().len(), 6);
    server.abort();
}
