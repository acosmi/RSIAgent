//! Real CLI startup controls in private, single-UID filesystem fixtures.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const ADMIN_TOKEN: &str = "permissions-admin-fixture-0123456789";
const EVALUATOR_TOKEN: &str = "permissions-evaluator-fixture-0123456789";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rsia-permissions-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> &Path { &self.0 }
}
impl Drop for Scratch {
    fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    mode: u32,
    uid: u32,
    links: u64,
    target: Option<PathBuf>,
    bytes: Vec<u8>,
}

fn image(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Entry>) {
        let info = fs::symlink_metadata(path).unwrap();
        let link = info.file_type().is_symlink();
        result.insert(path.strip_prefix(root).unwrap().to_path_buf(), Entry {
            mode: info.mode(), uid: info.uid(), links: info.nlink(),
            target: if link { Some(fs::read_link(path).unwrap()) } else { None },
            bytes: if info.is_file() && !link { fs::read(path).unwrap() } else { Vec::new() },
        });
        if info.is_dir() && !link {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn command(operation: &str, database: &Path, working: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rsia"));
    command.current_dir(working).env_clear()
        .args([operation, "--namespace", "permissions", "--actor", "agent", "--data"])
        .arg(database).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true);
    if operation == "serve" {
        command.args(["--bind", "127.0.0.1:0"])
            .env("RSIA_ADMIN_TOKEN", ADMIN_TOKEN)
            .env("RSIA_EVALUATOR_TOKEN", EVALUATOR_TOKEN);
    }
    command
}

async fn assert_permission_refusal(operation: &str, database: &Path, working: &Path) {
    let before = image(working);
    let output = timeout(Duration::from_secs(20), command(operation, database, working).output())
        .await.unwrap().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "refusal polluted the protocol channel");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.lines().any(|line| line == "Error: startup_rejected: data_directory_insecure"), "{stderr}");
    assert!(!stderr.contains(ADMIN_TOKEN));
    assert!(!stderr.contains(EVALUATOR_TOKEN));
    assert_eq!(image(working), before);
}

#[tokio::test]
async fn serve_insecure_data_refuses_before_bootstrap_and_bind() {
    let root = Scratch::new();
    let directory = root.path().join("data");
    fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
    let database = directory.join("serve.sqlite3");
    let mut healthy = command("serve", &database, root.path()).spawn().unwrap();
    let mut lines = BufReader::new(healthy.stderr.take().unwrap()).lines();
    let address = timeout(Duration::from_secs(20), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("server exited before its real listen point");
            if let Some(address) = line.strip_prefix("rsia http listening on ") {
                break address.to_owned();
            }
        }
    }).await.unwrap();
    // Listening is after the actual Current bootstrap. Exercise the real
    // authenticated status route to its ordinary missing-job business result.
    let response = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
        .get(format!("http://{address}/v1/manage/jobs/permission-missing"))
        .bearer_auth(ADMIN_TOKEN).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"]["code"].is_string());
    assert!(fs::read(&database).unwrap().starts_with(b"SQLite format 3\0"));
    // Stop only this owned fixture process and wait before snapshotting it.
    healthy.kill().await.unwrap();
    healthy.wait().await.unwrap();
    fs::set_permissions(&directory, Permissions::from_mode(0o755)).unwrap();
    assert_permission_refusal("serve", &database, root.path()).await;
}

#[tokio::test]
async fn mcp_insecure_data_refuses_before_bootstrap_and_protocol_output() {
    let root = Scratch::new();
    let directory = root.path().join("data");
    fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
    let database = directory.join("mcp.sqlite3");
    let mut healthy = command("mcp", &database, root.path()).spawn().unwrap();
    let mut input = healthy.stdin.take().unwrap();
    let mut lines = BufReader::new(healthy.stdout.take().unwrap()).lines();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"permission-fixture","version":"1.0.0"}}});
    input.write_all(format!("{initialize}\n").as_bytes()).await.unwrap();
    let line = timeout(Duration::from_secs(20), lines.next_line()).await.unwrap().unwrap().unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(reply["result"]["serverInfo"]["name"], "rsia");
    input.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n").await.unwrap();
    let line = timeout(Duration::from_secs(20), lines.next_line()).await.unwrap().unwrap().unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["id"], 2);
    let names: Vec<_> = reply["result"]["tools"].as_array().unwrap().iter()
        .map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["evo_prepare", "evo_feedback", "evo_propose", "evo_inspect"]);
    drop(input);
    let status = timeout(Duration::from_secs(20), healthy.wait()).await.unwrap().unwrap();
    assert!(status.success());
    let mut remainder = Vec::new();
    lines.into_inner().read_to_end(&mut remainder).await.unwrap();
    assert!(remainder.is_empty());
    assert!(fs::read(&database).unwrap().starts_with(b"SQLite format 3\0"));
    fs::set_permissions(&directory, Permissions::from_mode(0o755)).unwrap();
    assert_permission_refusal("mcp", &database, root.path()).await;
}
