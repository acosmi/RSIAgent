//! AG-146 real CLI controls for the four budget management operations. The CLI
//! remains a pure HTTP client: these tests use a loopback mock server and a
//! sentinel database file that must never be opened.
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
    sync::{Arc, Mutex},
};

const TOKEN: &str = "budget-cli-fixture-secret-0123456789";

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rsia-budget-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
        "/v1/manage/jobs/existing-budget-job" => (
            StatusCode::OK,
            Json(json!({
                "id":"existing-budget-job",
                "operation":"budget.calls.list_open",
                "state":"succeeded",
                "result":{"result":"open_calls_listed"}
            })),
        ),
        _ => (
            StatusCode::OK,
            Json(json!({
                "id":"budget-job",
                "state":"queued",
                "job_schema_version":"rsia.management_job.v2"
            })),
        ),
    }
}

fn payload(operation: &str) -> Value {
    match operation {
        "budget.root.authorize" => json!({
            "schema_version":"rsia.management.budget_root_authorize.v1",
            "request_key":"cli-root-authorize",
            "root_budget_id":"root-cli",
            "billing_scope":"scope-cli",
            "allowed_namespaces":["n"],
            "currency":"USD",
            "pricing_version":"price-v1",
            "payment_subject":"payer-1",
            "authorization_receipt_digest":"a".repeat(64),
            "per_call_cap_micros":10,
            "total_limit_micros":100
        }),
        "budget.stage_plan.bind" => json!({
            "schema_version":"rsia.management.budget_stage_plan_bind.v1",
            "request_key":"cli-stage-bind",
            "billing_scope":"scope-cli",
            "expected_seq":0,
            "plan":{"schema_version":"rsia.root_stage_budget_plan.v1"}
        }),
        "budget.calls.list_open" => json!({
            "schema_version":"rsia.management.budget_calls_list_open.v1",
            "request_key":"cli-list-open",
            "billing_scope":"scope-cli",
            "page_size":10
        }),
        "budget.call.reconcile" => json!({
            "schema_version":"rsia.management.budget_call_reconcile.v1",
            "request_key":"cli-reconcile",
            "billing_scope":"scope-cli",
            "call_id":"call-cli",
            "provider_request_id":"provider-cli",
            "usage_record_id":"usage-cli",
            "output_digest":"b".repeat(64),
            "amount_micros":5,
            "currency":"USD",
            "pricing_version":"price-v1"
        }),
        _ => panic!("unexpected operation {operation}"),
    }
}

#[tokio::test]
async fn budget_operations_forward_exact_request_files_and_responses() {
    let scratch = Scratch::new();
    let requests = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let router = axum::Router::new()
        .fallback(capture)
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let before = scratch.contents();
    for operation in [
        "budget.root.authorize",
        "budget.stage_plan.bind",
        "budget.calls.list_open",
        "budget.call.reconcile",
    ] {
        let request = payload(operation);
        std::fs::write(
            scratch.0.join("request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let mut command = scratch.command();
        command
            .args(["manage", operation, "--url", &url])
            .args(["--request-file", "request.json"]);
        let output = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert_token_hidden(&output);
        assert!(output.status.success(), "{operation}: {output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({
                "id":"budget-job",
                "state":"queued",
                "job_schema_version":"rsia.management_job.v2"
            }),
            "{operation}"
        );
        let records = requests.lock().unwrap();
        let captured = records.last().unwrap();
        assert_eq!(captured.method, Method::POST);
        assert_eq!(captured.path, format!("/v1/manage/{operation}"));
        assert_eq!(captured.body, serde_json::to_vec(&request).unwrap());
        assert!(captured.authorized);
    }
    assert_eq!(requests.lock().unwrap().len(), 4);
    // The request file is test input; every other byte is unchanged and the
    // sentinel database was never opened.
    std::fs::remove_file(scratch.0.join("request.json")).unwrap();
    assert_eq!(scratch.contents(), before);
    server.abort();
}

#[tokio::test]
async fn budget_job_status_reconnects_and_never_opens_the_local_store() {
    let scratch = Scratch::new();
    let requests = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let router = axum::Router::new()
        .fallback(capture)
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let before = scratch.contents();
    for _ in 0..2 {
        let mut command = scratch.command();
        command
            .args(["manage", "job.status", "--job-id", "existing-budget-job"])
            .args(["--url", &url]);
        let output = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert_token_hidden(&output);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({
                "id":"existing-budget-job",
                "operation":"budget.calls.list_open",
                "state":"succeeded",
                "result":{"result":"open_calls_listed"}
            })
        );
        let records = requests.lock().unwrap();
        let captured = records.last().unwrap();
        assert_eq!(captured.method, Method::GET);
        assert_eq!(captured.path, "/v1/manage/jobs/existing-budget-job");
        assert!(captured.body.is_empty());
        assert!(captured.authorized);
    }
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert_eq!(scratch.contents(), before);
    server.abort();
}
