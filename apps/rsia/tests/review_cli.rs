//! Real binary + authenticated local HTTP, with an untouched client directory.
use evo_core::contract::{
    CapabilityLevel, CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch,
    SkillSnapshot, SystemSnapshot, compile_bundle,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Role, Strategy, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_engine::review::{ReviewDetail, ReviewKind, ReviewRequest};
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_http::{AuthIdentity, AuthRegistry, HttpState};
use evo_storage::Store;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const NS: &str = "cli-review-fixture";
const TOKEN: &str = "cli-review-admin-fixture-secret-0123456789";
static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Scratch {
    root: PathBuf,
    client: PathBuf,
}
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rsia-review-cli-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let client = root.join("client");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(
            client.join("rsia.sqlite3"),
            b"client sentinel; never opened as SQLite",
        )
        .unwrap();
        Self { root, client }
    }
    fn client_contents(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn read(path: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    read(&path, base, out);
                } else {
                    out.insert(
                        path.strip_prefix(base).unwrap().to_path_buf(),
                        std::fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        read(&self.client, &self.client, &mut out);
        out
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn d(label: &str) -> String {
    hash(label.as_bytes())
}
fn ctx(actor: &str, role: Role) -> Context {
    Context::new(NS, actor, role).unwrap()
}

async fn fixture(scratch: &Scratch) -> HttpState {
    let service_path = scratch.root.join("service");
    std::fs::create_dir(&service_path).unwrap();
    let store = Store::open(&service_path.join("stored.sqlite3"))
        .await
        .unwrap();
    let host = ctx("host", Role::Host);
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "cli-run".into(),
            body: b"cli trusted source".to_vec(),
            parent_family: "cli-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "cli-run".into(),
            parent_family: "cli-family".into(),
            source_digest: d("cli trusted source"),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: "cli trusted source".into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: b"cli trusted source".len(),
    };
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&host, &d("cli-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let profile = Profile {
        id: "cli-profile".into(),
        evolution_enabled: true,
        parent_digest: d("parent"),
        baseline_digest: d("baseline"),
    };
    let parent = SkillSnapshot {
        content: "CLI stored full rule 中文🙂".into(),
        applicability: "CLI fixture".into(),
        counterexample: "CLI cannot infer approval".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let bundle = compile_bundle(CompileParts {
        profile: &profile,
        parent: &parent,
        baseline: &SkillSnapshot::empty(),
        parent_strategy: &Strategy::default(),
        baseline_strategy: &Strategy::default(),
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        caps: &caps,
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    let snapshot = SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: "cli-profile".into(),
        host_id: "cli-host".into(),
        host_version: "1.0.0".into(),
        model_id: "disabled".into(),
        tools: vec![],
        mandatory_context_digest: d("mandatory"),
    };
    let proposer = ctx("proposer", Role::Worker);
    ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "cli-candidate".into(),
            bundle,
            environment_digest: snapshot.digest().unwrap(),
            proposer_actor: proposer.actor().into(),
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: authority.record.id.clone(),
                content_digest: authority.trace.source_digest.clone(),
            }],
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let auth = AuthRegistry::from_plaintext(vec![(
        TOKEN.into(),
        AuthIdentity::new(NS, "admin", Role::Admin).unwrap(),
    )])
    .unwrap();
    let service = HostService::new(store, host).unwrap();
    let management = evo_engine::dispatch::ManagementDispatcher::new(
        service.store().clone(),
        auth.management_contexts().unwrap(),
    )
    .unwrap();
    HttpState {
        service,
        prepare_config: HostPrepareConfig {
            profile_id: "cli-profile".into(),
            system_snapshot: snapshot,
            host_surface_id: "unused-review-surface".into(),
            host_capabilities: caps,
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
        auth,
        management,
    }
}
async fn spawn(state: HttpState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, evo_http::router(state))
            .await
            .unwrap();
    });
    (format!("http://{address}"), task)
}
fn command(scratch: &Scratch) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rsia"));
    command
        .current_dir(&scratch.client)
        .env("RSIA_MANAGEMENT_TOKEN", TOKEN)
        .env_remove("RSIA_HTTP_TOKEN")
        .env_remove("RSIA_HOST_TOKEN")
        .env_remove("RSIA_ADMIN_TOKEN")
        .env_remove("RSIA_EVALUATOR_TOKEN");
    command
}
async fn output(mut command: Command) -> Output {
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}
fn assert_token_hidden(result: &Output) {
    assert!(!String::from_utf8_lossy(&result.stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(TOKEN));
}

#[tokio::test]
async fn real_review_cli_outputs_the_complete_http_dto_with_default_metadata_and_exact() {
    let scratch = Scratch::new();
    let state = fixture(&scratch).await;
    let before = scratch.client_contents();
    let expected = state
        .service
        .review(
            &ctx("admin", Role::Admin),
            ReviewRequest {
                kind: ReviewKind::TypedCandidate,
                id: "cli-candidate".into(),
                detail: ReviewDetail::Exact,
            },
        )
        .await
        .unwrap();
    let (url, server) = spawn(state).await;
    let mut cmd = command(&scratch);
    cmd.args([
        "review",
        "--kind",
        "typed_candidate",
        "--id",
        "cli-candidate",
        "--url",
        &url,
    ]);
    let result = output(cmd).await;
    assert_token_hidden(&result);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["schema_version"], "rsia.review.v1");
    assert_eq!(value["detail"], "metadata");
    assert!(value["payload"]["materials"]["bundle"]["skill"]["content"]["display"].is_null());
    let mut cmd = command(&scratch);
    cmd.args([
        "review",
        "--kind",
        "typed_candidate",
        "--id",
        "cli-candidate",
        "--detail",
        "exact",
        "--url",
        &url,
    ]);
    let result = output(cmd).await;
    assert_token_hidden(&result);
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
        scratch.client_contents(),
        before,
        "client must not mount or lock any database"
    );
    server.abort();
}

#[tokio::test]
async fn cli_allowlists_all_three_kinds_and_propagates_fixed_http_failure_without_token() {
    let scratch = Scratch::new();
    let (url, server) = spawn(fixture(&scratch).await).await;
    let before = scratch.client_contents();
    for kind in ["typed_candidate", "stage_fact", "formal_report"] {
        let mut cmd = command(&scratch);
        cmd.args([
            "review",
            "--kind",
            kind,
            "--id",
            "missing-id",
            "--url",
            &url,
        ]);
        let result = output(cmd).await;
        assert_token_hidden(&result);
        assert!(!result.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!({"error":{"code":"review_unavailable"}})
        );
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("review HTTP request was not successful")
        );
    }
    assert_eq!(scratch.client_contents(), before);
    server.abort();
}

#[tokio::test]
async fn cli_rejects_public_credentialed_query_urls_and_client_database_or_identity_flags() {
    let scratch = Scratch::new();
    let before = scratch.client_contents();
    for url in [
        "https://example.com",
        "http://127.0.0.1:1/path",
        "http://user:secret@127.0.0.1:1",
        "http://127.0.0.1:1?token=secret",
        "http://127.0.0.1:1#fragment",
    ] {
        let mut cmd = command(&scratch);
        cmd.args([
            "review",
            "--kind",
            "typed_candidate",
            "--id",
            "candidate",
            "--url",
            url,
        ]);
        let result = output(cmd).await;
        assert_token_hidden(&result);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("authorized local or private HTTP service"));
        assert!(!error.contains(url));
    }
    for flag in ["--data", "--namespace", "--actor", "--role"] {
        let mut cmd = command(&scratch);
        cmd.args([
            "review",
            "--kind",
            "typed_candidate",
            "--id",
            "candidate",
            flag,
            "forged",
        ]);
        let result = output(cmd).await;
        assert_token_hidden(&result);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("unexpected argument"));
    }
    let mut cmd = command(&scratch);
    cmd.args(["review", "--help"]);
    let result = output(cmd).await;
    assert_token_hidden(&result);
    assert!(result.status.success());
    let help = String::from_utf8_lossy(&result.stdout);
    assert!(!help.contains("--data") && !help.contains("--namespace"));
    let mut cmd = command(&scratch);
    cmd.args([
        "review",
        "--kind",
        "typed_candidate",
        "--id",
        "candidate",
        "--url",
        "http://127.0.0.1:1",
    ]);
    let result = output(cmd).await;
    assert_token_hidden(&result);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("review HTTP request failed"));
    assert_eq!(scratch.client_contents(), before);
}
