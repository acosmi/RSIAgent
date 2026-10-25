//! Real CLI credential-channel checks; all HTTP traffic stays on loopback.
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const TOKEN_KEYS: [&str; 5] = [
    "RSIA_HTTP_TOKEN",
    "RSIA_HOST_TOKEN",
    "RSIA_ADMIN_TOKEN",
    "RSIA_EVALUATOR_TOKEN",
    "RSIA_MANAGEMENT_TOKEN",
];
const SECRET: &str = "credential-argv-rejection-secret-0123456789";
const AGENT: &str = "credential-env-agent-secret-0123456789";
const HOST: &str = "credential-env-host-secret-0123456789";
const ADMIN: &str = "credential-env-admin-secret-0123456789";
const EVALUATOR: &str = "credential-env-evaluator-secret-0123456789";

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rsia-credential-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(scratch: &Scratch) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rsia"));
    command.current_dir(&scratch.0).stdin(Stdio::null());
    for key in TOKEN_KEYS {
        command.env_remove(key);
    }
    command
}
fn hidden(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for secret in [SECRET, AGENT, HOST, ADMIN, EVALUATOR] {
        assert!(!text.contains(secret), "credential value was rendered");
    }
}
fn rejected(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "credential argv must be rejected during parsing"
    );
    hidden(&output.stdout);
    hidden(&output.stderr);
}

#[test]
fn serve_rejects_all_credential_flags_without_echo_or_data_writes() {
    let scratch = Scratch::new("serve-rejection");
    let data = scratch.0.join("uncreated").join("rsia.sqlite3");
    let anchor = scratch.0.join("absent-anchor.sqlite3");
    for flag in [
        "--auth-token",
        "--host-token",
        "--admin-token",
        "--evaluator-token",
    ] {
        for arguments in [
            vec![flag.to_string(), SECRET.to_string()],
            vec![format!("{flag}={SECRET}")],
        ] {
            let output = command(&scratch)
                .arg("serve")
                .arg("--data")
                .arg(&data)
                .arg("--trusted-revocations-db")
                .arg(&anchor)
                .args(arguments)
                .output()
                .unwrap();
            rejected(&output);
            assert_eq!(
                fs::read_dir(&scratch.0).unwrap().count(),
                0,
                "rejection wrote a data directory"
            );
        }
    }
}

#[test]
fn manage_and_review_reject_auth_token_flags_without_echo() {
    let scratch = Scratch::new("client-rejection");
    for prefix in [
        vec!["manage", "unknown-credential-operation"],
        vec![
            "review",
            "--kind",
            "typed_candidate",
            "--id",
            "missing",
            "--url",
            "file:///unavailable",
        ],
    ] {
        for arguments in [
            vec!["--auth-token".to_string(), SECRET.to_string()],
            vec![format!("--auth-token={SECRET}")],
        ] {
            rejected(
                &command(&scratch)
                    .args(&prefix)
                    .args(arguments)
                    .output()
                    .unwrap(),
            );
        }
    }
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
}

#[test]
fn credential_values_cannot_become_positional_arguments() {
    let scratch = Scratch::new("positionals");
    for arguments in [
        vec![
            "serve",
            "--trusted-revocations-db",
            "missing-anchor",
            SECRET,
        ],
        vec!["manage", "job.status", SECRET],
        vec![
            "review",
            "--kind",
            "typed_candidate",
            "--id",
            "missing",
            SECRET,
        ],
    ] {
        rejected(&command(&scratch).args(arguments).output().unwrap());
    }
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        // Reap only the child this fixture started, never other RSIA processes.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn check_files(path: &Path) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_files(&path);
        } else {
            hidden(&fs::read(path).unwrap());
        }
    }
}

#[tokio::test]
async fn environment_credentials_keep_http_roles_and_stay_out_of_argv_and_outputs() {
    let scratch = Scratch::new("environment");
    for subcommand in ["serve", "manage", "review", "mcp"] {
        let mut help = command(&scratch);
        help.args([subcommand, "--help"]);
        for key in TOKEN_KEYS {
            help.env(key, SECRET);
        }
        let output = help.output().unwrap();
        assert!(output.status.success(), "help failed");
        hidden(&output.stdout);
        hidden(&output.stderr);
    }
    assert!(
        command(&scratch)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
    for prefix in [
        vec!["manage", "job.status"],
        vec!["review", "--kind", "typed_candidate", "--id", "missing"],
    ] {
        let output = command(&scratch).args(&prefix).output().unwrap();
        assert!(
            !output.status.success(),
            "missing management environment was accepted"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("RSIA_MANAGEMENT_TOKEN is required")
        );
        hidden(&output.stdout);
        hidden(&output.stderr);
        for invalid in ["short", "credential-invalid-line-secret\n0123456789"] {
            let output = command(&scratch)
                .args(&prefix)
                .env("RSIA_MANAGEMENT_TOKEN", invalid)
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "invalid management environment was accepted"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("invalid authentication environment")
            );
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains(invalid),
                "invalid credential was rendered"
            );
        }
    }
    let data_directory = scratch.0.join("data");
    let logs = scratch.0.join("server.stderr");
    let stdout = scratch.0.join("server.stdout");
    let mut serve = command(&scratch);
    serve
        .args([
            "serve",
            "--bind",
            "127.0.0.1:0",
            "--namespace",
            "credential-scope",
            "--actor",
            "credential-agent",
        ])
        .arg("--data")
        .arg(data_directory.join("rsia.sqlite3"))
        .env("RSIA_HTTP_TOKEN", AGENT)
        .env("RSIA_HOST_TOKEN", HOST)
        .env("RSIA_ADMIN_TOKEN", ADMIN)
        .env("RSIA_EVALUATOR_TOKEN", EVALUATOR)
        .stdout(File::create(&stdout).unwrap())
        .stderr(File::create(&logs).unwrap());
    let mut server = Server(serve.spawn().unwrap());
    let mut address = None;
    for _ in 0..500 {
        let text = fs::read_to_string(&logs).unwrap();
        hidden(text.as_bytes());
        if let Some(line) = text
            .lines()
            .find_map(|line| line.strip_prefix("rsia http listening on "))
        {
            address = Some(format!("http://{line}"));
            break;
        }
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "environment-configured service exited early"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let base = address.expect("environment-configured service did not listen");
    let process = Command::new("ps")
        .args(["-ww", "-p", &server.0.id().to_string(), "-o", "command="])
        .output()
        .unwrap();
    assert!(
        process.status.success(),
        "could not inspect owned service argv"
    );
    hidden(&process.stdout);
    hidden(&process.stderr);
    assert!(String::from_utf8_lossy(&process.stdout).contains("serve"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let prepared = client
        .post(format!("{base}/v1/tools/prepare"))
        .bearer_auth(AGENT)
        .json(
            &json!({"request_key":"credential-prepare", "goal":"local fixture", "capabilities":[]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        prepared.status().as_u16(),
        200,
        "agent environment identity failed"
    );
    let prepared: Value = prepared.json().await.unwrap();
    let run_id = prepared["run"]["id"].as_str().unwrap();
    for (token, expected) in [(HOST, 200), (AGENT, 403)] {
        let response = client
            .post(format!("{base}/v1/host/snapshot"))
            .bearer_auth(token)
            .json(&json!({"run_id":run_id}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "host environment role boundary changed"
        );
        hidden(response.bytes().await.unwrap().as_ref());
    }
    for (token, expected) in [(ADMIN, 404), (EVALUATOR, 404), (AGENT, 403), (HOST, 403)] {
        let response = client
            .get(format!("{base}/v1/manage/jobs/missing"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "management environment role boundary changed"
        );
        hidden(response.bytes().await.unwrap().as_ref());
    }
    let manage = command(&scratch)
        .args([
            "manage",
            "job.status",
            "--job-id",
            "missing",
            "--url",
            &base,
        ])
        .env("RSIA_MANAGEMENT_TOKEN", ADMIN)
        .output()
        .unwrap();
    assert_eq!(manage.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&manage.stdout).unwrap()["error"]["code"],
        "not_found"
    );
    hidden(&manage.stdout);
    hidden(&manage.stderr);
    let review = command(&scratch)
        .args([
            "review",
            "--kind",
            "typed_candidate",
            "--id",
            "missing",
            "--url",
            &base,
        ])
        .env("RSIA_MANAGEMENT_TOKEN", ADMIN)
        .output()
        .unwrap();
    assert_eq!(review.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&review.stdout).unwrap()["error"]["code"],
        "review_unavailable"
    );
    hidden(&review.stdout);
    hidden(&review.stderr);
    drop(server);
    check_files(&scratch.0);
}
