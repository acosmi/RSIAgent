use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::{Context, Role, hash};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_engine::startup_gate::{DATA_LOCK_FILE, GateDecision, StartupGate, data_directory};
use evo_http::{AuthIdentity, AuthRegistry, DEFAULT_BIND, HttpState};
use evo_storage::Store;
use fs2::FileExt;
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    net::SocketAddr,
    path::{Path, PathBuf},
};

const GATEWAY_HOST_ACTOR: &str = "reference-host-gateway";
const REFERENCE_SURFACE_ID: &str = "reference-host-surface-v1";

async fn review_client(
    kind: ReviewKindArgument,
    id: String,
    detail: ReviewDetailArgument,
    url: String,
    auth_token: String,
) -> Result<()> {
    let mut base =
        reqwest::Url::parse(&url).map_err(|_| anyhow::anyhow!("invalid review service URL"))?;
    let host = base.host_str().unwrap_or("").trim_matches(['[', ']']);
    let local = host == "localhost"
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
            std::net::IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.segments()[0] & 0xfe00 == 0xfc00
                    || ip.segments()[0] & 0xffc0 == 0xfe80
            }
        });
    if !local
        || !matches!(base.scheme(), "http" | "https")
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || !matches!(base.path(), "" | "/")
    {
        bail!("review service URL must identify an authorized local or private HTTP service");
    }
    if auth_token.len() < 16 || auth_token.len() > 4096 || auth_token.chars().any(char::is_control)
    {
        bail!("invalid review authentication");
    }
    let request = evo_engine::review::ReviewRequest {
        kind: match kind {
            ReviewKindArgument::TypedCandidate => evo_engine::review::ReviewKind::TypedCandidate,
            ReviewKindArgument::StageFact => evo_engine::review::ReviewKind::StageFact,
            ReviewKindArgument::FormalReport => evo_engine::review::ReviewKind::FormalReport,
        },
        id,
        detail: match detail {
            ReviewDetailArgument::Metadata => evo_engine::review::ReviewDetail::Metadata,
            ReviewDetailArgument::Exact => evo_engine::review::ReviewDetail::Exact,
        },
    };
    base.set_path("/v1/review");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| anyhow::anyhow!("review HTTP client unavailable"))?;
    let response = client
        .post(base)
        .bearer_auth(&auth_token)
        .json(&request)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("review HTTP request failed"))?;
    let status = response.status();
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("invalid review HTTP response"))?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    if !status.is_success() {
        bail!("review HTTP request was not successful");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ReviewKindArgument {
    TypedCandidate,
    StageFact,
    FormalReport,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ReviewDetailArgument {
    Metadata,
    Exact,
}

#[derive(Debug, Parser)]
#[command(name = "rsia", version, about = "Bounded RSIA host service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Review stored material through the authenticated HTTP service.
    Review {
        #[arg(long, value_enum)]
        kind: ReviewKindArgument,
        #[arg(long)]
        id: String,
        #[arg(long, value_enum, default_value = "metadata")]
        detail: ReviewDetailArgument,
        #[arg(long, default_value = "http://127.0.0.1:7788")]
        url: String,
        #[arg(long, env = "RSIA_MANAGEMENT_TOKEN", hide_env_values = true)]
        auth_token: String,
    },
    /// Serve the authenticated HTTP API. No token means every sensitive route returns 503.
    Serve {
        #[arg(long, default_value = DEFAULT_BIND)]
        bind: SocketAddr,
        #[arg(long, default_value = "rsia.sqlite3")]
        data: PathBuf,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "http-agent")]
        actor: String,
        #[arg(long, default_value = "evaluator")]
        evaluator_actor: String,
        #[arg(long, env = "RSIA_HTTP_TOKEN", hide_env_values = true)]
        auth_token: Option<String>,
        #[arg(long, env = "RSIA_HOST_TOKEN", hide_env_values = true)]
        host_token: Option<String>,
        #[arg(long, env = "RSIA_ADMIN_TOKEN", hide_env_values = true)]
        admin_token: Option<String>,
        #[arg(long, env = "RSIA_EVALUATOR_TOKEN", hide_env_values = true)]
        evaluator_token: Option<String>,
        /// Current control-plane database that vouches for a restored data directory.
        /// Accepted only for a restored directory that has not been admitted yet.
        #[arg(long)]
        trusted_revocations_db: Option<PathBuf>,
    },
    /// Run the four MCP tools over stdio with a startup-fixed Agent identity.
    Mcp {
        #[arg(long, default_value = "rsia.sqlite3")]
        data: PathBuf,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "stdio-agent")]
        actor: String,
        /// Current control-plane database that vouches for a restored data directory.
        /// Accepted only for a restored directory that has not been admitted yet.
        #[arg(long)]
        trusted_revocations_db: Option<PathBuf>,
    },
    /// Submit or inspect authenticated management jobs through the HTTP service.
    Manage {
        operation: String,
        #[arg(long, default_value = "http://127.0.0.1:7788")]
        url: String,
        #[arg(long, env = "RSIA_MANAGEMENT_TOKEN", hide_env_values = true)]
        auth_token: String,
        #[arg(long)]
        request_file: Option<PathBuf>,
        #[arg(long)]
        job_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Review {
            kind,
            id,
            detail,
            url,
            auth_token,
        } => {
            review_client(kind, id, detail, url, auth_token).await?;
        }
        Command::Serve {
            bind,
            data,
            namespace,
            actor,
            evaluator_actor,
            auth_token,
            host_token,
            admin_token,
            evaluator_token,
            trusted_revocations_db,
        } => {
            let (_data_lock, gate) = mount_gate(&data, trusted_revocations_db.as_deref()).await?;
            let (service, prepare_config) = bootstrap(&data, &namespace).await?;
            gate.admit()?;
            let mut entries = Vec::new();
            if let Some(token) = auth_token {
                entries.push((token, AuthIdentity::new(&namespace, &actor, Role::Agent)?));
            }
            if let Some(token) = host_token {
                entries.push((
                    token,
                    AuthIdentity::new(&namespace, GATEWAY_HOST_ACTOR, Role::Host)?,
                ));
            }
            if let Some(token) = admin_token {
                entries.push((
                    token,
                    AuthIdentity::new(&namespace, "service-admin", Role::Admin)?,
                ));
            }
            if let Some(token) = evaluator_token {
                entries.push((
                    token,
                    AuthIdentity::new(&namespace, &evaluator_actor, Role::Evaluator)?,
                ));
            }
            let auth = AuthRegistry::from_plaintext(entries)?;
            let management = evo_engine::dispatch::ManagementDispatcher::new(
                service.store().clone(),
                auth.management_contexts()?,
            )?;
            management.recover_pending().await?;
            if !auth.is_configured() {
                eprintln!("authentication is not configured; sensitive routes will return 503");
            }
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("failed to bind {bind}"))?;
            eprintln!("rsia http listening on {}", listener.local_addr()?);
            axum::serve(
                listener,
                evo_http::router(HttpState {
                    service,
                    prepare_config,
                    auth,
                    management,
                }),
            )
            .await?;
        }
        Command::Mcp {
            data,
            namespace,
            actor,
            trusted_revocations_db,
        } => {
            let (_data_lock, gate) = mount_gate(&data, trusted_revocations_db.as_deref()).await?;
            let (service, prepare_config) = bootstrap(&data, &namespace).await?;
            gate.admit()?;
            let caller = Context::new(&namespace, &actor, Role::Agent)?;
            let server = evo_mcp::McpHost::new(service, caller, prepare_config)?;
            evo_mcp::serve_stdio(server)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        }
        Command::Manage {
            operation,
            url,
            auth_token,
            request_file,
            job_id,
        } => {
            let client = reqwest::Client::new();
            let base = url.trim_end_matches('/');
            let response = match operation.as_str() {
                "job.status" => {
                    let job_id = job_id.context("--job-id is required for job.status")?;
                    client
                        .get(format!("{base}/v1/manage/jobs/{job_id}"))
                        .bearer_auth(&auth_token)
                        .send()
                        .await?
                }
                "job.cancel" => {
                    let job_id = job_id.context("--job-id is required for job.cancel")?;
                    client
                        .post(format!("{base}/v1/manage/jobs/{job_id}/cancel"))
                        .bearer_auth(&auth_token)
                        .json(&serde_json::json!({}))
                        .send()
                        .await?
                }
                _ => {
                    if !evo_core::contract::admin_ops().contains(&operation.as_str()) {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "operation": operation,
                                "status": "unsupported",
                                "reason": "unknown_management_operation"
                            })
                        );
                        bail!("unknown management operation");
                    }
                    let path = request_file.context("--request-file is required")?;
                    let body = tokio::fs::read(path).await?;
                    client
                        .post(format!("{base}/v1/manage/{operation}"))
                        .bearer_auth(&auth_token)
                        .header("content-type", "application/json")
                        .body(body)
                        .send()
                        .await?
                }
            };
            let status = response.status();
            let body = response.text().await?;
            println!("{body}");
            if !status.is_success() {
                bail!("management request failed with HTTP {status}");
            }
        }
    }
    Ok(())
}

/// E16.5 startup recovery gate, then the data-directory lock.
///
/// The gate runs twice on purpose. The first evaluation happens before the lock
/// exists so that a refusal writes nothing at all (taking the lock creates
/// `.rsia.lock`); the second runs under the lock so the decision that is acted on
/// cannot be raced by another RSIA process. Only a restored directory that is
/// being verified for the first time pays for the second, database-reading pass.
/// The caller admits a verified directory with `GateDecision::admit` after the
/// store is bootstrapped and before anything recovers or serves.
async fn mount_gate(
    data: &Path,
    trusted_revocations_db: Option<&Path>,
) -> Result<(DataDirectoryLock, GateDecision)> {
    let gate = StartupGate::new(data, trusted_revocations_db);
    gate.evaluate().await?;
    let lock = DataDirectoryLock::acquire(data)?;
    let decision = gate.evaluate().await?;
    // stderr only: stdout is the MCP JSON-RPC channel.
    eprintln!("{}", decision.startup_line());
    Ok((lock, decision))
}

/// The directory the data-directory lock is taken in, before it is created and
/// canonicalized. It is the startup gate's own definition of the data directory,
/// not a second computation of it: a bare file name (the `--data` default) is the
/// current directory, where the gate also looks for the restore receipt. An empty
/// parent must never reach `canonicalize`, which rejects it.
fn lock_directory(data: &Path) -> PathBuf {
    data_directory(data)
}

struct DataDirectoryLock {
    _file: File,
}

impl DataDirectoryLock {
    fn acquire(data: &Path) -> Result<Self> {
        let directory = lock_directory(data);
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("failed to create data directory {}", directory.display()))?;
        let directory = directory
            .canonicalize()
            .with_context(|| format!("failed to resolve data directory {}", directory.display()))?;
        let lock_path = directory.join(DATA_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open data lock {}", lock_path.display()))?;
        if let Err(error) = FileExt::try_lock_exclusive(&file) {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                bail!(
                    "data directory is already locked by another RSIA process: {}",
                    directory.display()
                );
            }
            return Err(error)
                .with_context(|| format!("failed to lock data directory {}", directory.display()));
        }
        Ok(Self { _file: file })
    }
}

async fn bootstrap(data: &Path, namespace: &str) -> Result<(HostService, HostPrepareConfig)> {
    let store = Store::open(data).await?;
    let host = Context::new(namespace, GATEWAY_HOST_ACTOR, Role::Host)?;
    let admin = Context::new(namespace, "service-admin", Role::Admin)?;
    let manifest = HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "reference-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "1.0.0".into(),
        source_digest: hash(b"rsia-reference-host-surface-v1"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("reference-host".into()),
            reason: "fixed disabled-model reference adapter".into(),
        }],
    };
    ReleaseStore::register_host_surface(
        &admin,
        &store,
        REFERENCE_SURFACE_ID,
        manifest,
        vec!["model".into()],
    )
    .await?;
    let prepare_config = HostPrepareConfig {
        profile_id: "default".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "default".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: vec!["read_config".into()],
            mandatory_context_digest: hash(b"reference-host-mandatory-context-v1"),
        },
        host_surface_id: REFERENCE_SURFACE_ID.into(),
        host_capabilities: HostCapabilities {
            available: ["fs_read".into()].into_iter().collect::<BTreeSet<_>>(),
            granted: ["fs_read".into()].into_iter().collect::<BTreeSet<_>>(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    };
    Ok((HostService::new(store, host)?, prepare_config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use evo_engine::startup_gate::{
        RESTORE_ADMISSION_FILE, RESTORE_RECEIPT_FILE, RecoveryPosture, StartupGateError,
        deployment_config,
    };

    const FORBIDDEN_SWITCHES: [&str; 5] = [
        "--allow-code-execution",
        "--sandbox",
        "--code-execution",
        "--enable-code-execution",
        "--allow-external-network",
    ];

    /// A unique scratch directory (this binary crate has no tempfile dev-dependency).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "rsia-main-test-{label}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn listing(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn serve_and_mcp_accept_no_code_execution_sandbox_or_network_switch() {
        for command in ["serve", "mcp"] {
            assert!(Cli::try_parse_from(["rsia", command]).is_ok(), "{command}");
            for switch in FORBIDDEN_SWITCHES {
                assert!(
                    Cli::try_parse_from(["rsia", command, switch]).is_err(),
                    "{command} {switch}"
                );
                assert!(
                    Cli::try_parse_from(["rsia", command, switch, "true"]).is_err(),
                    "{command} {switch} true"
                );
                assert!(
                    Cli::try_parse_from(["rsia", command, &format!("{switch}=true")]).is_err(),
                    "{command} {switch}=true"
                );
            }
        }
    }

    #[test]
    fn no_serve_or_mcp_argument_or_environment_variable_can_name_code_execution() {
        let command = Cli::command();
        let mut inspected = 0;
        for subcommand in command
            .get_subcommands()
            .filter(|subcommand| matches!(subcommand.get_name(), "serve" | "mcp"))
        {
            for argument in subcommand.get_arguments() {
                inspected += 1;
                let names = [
                    argument.get_id().as_str().to_ascii_lowercase(),
                    argument.get_long().unwrap_or_default().to_ascii_lowercase(),
                    argument
                        .get_env()
                        .map(|name| name.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default(),
                ];
                for name in names {
                    for word in ["code", "exec", "sandbox", "network"] {
                        assert!(
                            !name.contains(word),
                            "{} exposes {name:?}",
                            subcommand.get_name()
                        );
                    }
                }
            }
        }
        assert!(inspected > 8, "the serve and mcp arguments were inspected");
    }

    #[test]
    fn trusted_revocations_db_is_a_serve_and_mcp_option_only() {
        for command in ["serve", "mcp"] {
            let parsed = Cli::try_parse_from([
                "rsia",
                command,
                "--trusted-revocations-db",
                "/anchor/rsia.sqlite3",
            ])
            .unwrap_or_else(|error| panic!("{command}: {error}"));
            let anchor = match parsed.command {
                Command::Serve {
                    trusted_revocations_db,
                    ..
                }
                | Command::Mcp {
                    trusted_revocations_db,
                    ..
                } => trusted_revocations_db,
                Command::Manage { .. } | Command::Review { .. } => panic!("unexpected subcommand"),
            };
            assert_eq!(anchor, Some(PathBuf::from("/anchor/rsia.sqlite3")));
            // absent by default
            match Cli::try_parse_from(["rsia", command]).unwrap().command {
                Command::Serve {
                    trusted_revocations_db,
                    ..
                }
                | Command::Mcp {
                    trusted_revocations_db,
                    ..
                } => assert_eq!(trusted_revocations_db, None),
                Command::Manage { .. } | Command::Review { .. } => panic!("unexpected subcommand"),
            }
        }
        let manage = ["rsia", "manage", "job.status", "--auth-token", "token"];
        assert!(Cli::try_parse_from(manage).is_ok());
        assert!(
            Cli::try_parse_from(
                manage
                    .into_iter()
                    .chain(["--trusted-revocations-db", "/anchor/rsia.sqlite3"])
            )
            .is_err(),
            "manage is a pure HTTP client and takes no anchor"
        );
    }

    #[test]
    fn the_startup_deployment_config_never_allows_code_execution() {
        for anchor in [None, Some(Path::new("/anchor/rsia.sqlite3"))] {
            let config = deployment_config(anchor);
            assert!(!config.allow_code_execution);
            assert!(!config.sandbox_enabled);
            assert!(!config.allow_external_network);
        }
    }

    #[test]
    fn the_lock_directory_is_the_startup_gate_data_directory() {
        // (`--data` path, its data directory): a bare file name is the current directory
        for (data, directory) in [
            ("rsia.sqlite3", "."),
            ("./rsia.sqlite3", "."),
            ("dir/x.sqlite3", "dir"),
            ("a/b/x.sqlite3", "a/b"),
            ("/var/lib/rsia/rsia.sqlite3", "/var/lib/rsia"),
            ("/rsia.sqlite3", "/"),
        ] {
            let data = Path::new(data);
            assert_eq!(lock_directory(data), Path::new(directory), "{data:?}");
            // one definition: the lock cannot drift from what the startup gate reads
            assert_eq!(lock_directory(data), data_directory(data), "{data:?}");
        }
    }

    #[test]
    fn a_bare_file_name_locks_the_current_directory() {
        // `Path::parent` of a bare file name is empty and `canonicalize` rejects an
        // empty path, so the lock directory must be the resolvable current directory.
        let directory = lock_directory(Path::new("rsia.sqlite3"));
        assert_eq!(
            directory.canonicalize().unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
    }

    #[test]
    fn the_default_data_path_of_serve_and_mcp_locks_the_current_directory() {
        for command in ["serve", "mcp"] {
            let data = match Cli::try_parse_from(["rsia", command]).unwrap().command {
                Command::Serve { data, .. } | Command::Mcp { data, .. } => data,
                Command::Manage { .. } | Command::Review { .. } => panic!("unexpected subcommand"),
            };
            assert_eq!(data, Path::new("rsia.sqlite3"), "{command}");
            assert_eq!(lock_directory(&data), Path::new("."), "{command}");
        }
    }

    #[test]
    fn the_lock_is_taken_in_the_directory_the_startup_gate_reads() {
        let scratch = Scratch::new("lock-directory");
        // the data directory does not exist yet
        let data = scratch.0.join("nested").join("x.sqlite3");
        let lock = DataDirectoryLock::acquire(&data).unwrap();
        let directory = data_directory(&data);
        assert_eq!(directory, scratch.0.join("nested"));
        assert!(directory.join(DATA_LOCK_FILE).is_file());

        // the lock guards the directory, not one database name or one spelling of its path
        let other = directory.join(".").join("y.sqlite3");
        let error = DataDirectoryLock::acquire(&other).err().unwrap();
        assert!(
            error
                .to_string()
                .starts_with("data directory is already locked by another RSIA process: "),
            "{error}"
        );
        drop(lock);
        assert!(DataDirectoryLock::acquire(&other).is_ok());
    }

    #[tokio::test]
    async fn an_ordinary_directory_mounts_under_the_lock_and_reports_its_posture() {
        let scratch = Scratch::new("normal");
        let data = scratch.0.join("rsia.sqlite3");
        let (lock, gate) = mount_gate(&data, None).await.unwrap();
        assert_eq!(gate.posture(), RecoveryPosture::Normal);
        assert_eq!(
            gate.startup_line(),
            "rsia startup: code_execution=disabled sandbox=unavailable recovery=normal"
        );
        gate.admit().unwrap();
        assert_eq!(scratch.listing(), [DATA_LOCK_FILE]);
        assert!(
            DataDirectoryLock::acquire(&data).is_err(),
            "the mounted directory stays locked"
        );
        drop(lock);
        assert!(DataDirectoryLock::acquire(&data).is_ok());
    }

    #[tokio::test]
    async fn a_refused_startup_writes_nothing_not_even_the_lock_file() {
        // an anchor for a directory that was never restored (and does not exist)
        let scratch = Scratch::new("refused");
        let missing = scratch.0.join("never-created");
        let error = mount_gate(
            &missing.join("rsia.sqlite3"),
            Some(Path::new("/anchor/rsia.sqlite3")),
        )
        .await
        .err()
        .unwrap();
        assert!(
            matches!(
                error.downcast_ref::<StartupGateError>(),
                Some(StartupGateError::Rejected(_))
            ),
            "{error}"
        );
        assert!(error.to_string().starts_with("startup_rejected: "));
        assert!(!missing.exists());

        // a restored directory without an anchor is quarantined before any lock exists
        let restored = scratch.0.join("restored");
        std::fs::create_dir(&restored).unwrap();
        std::fs::write(restored.join(RESTORE_RECEIPT_FILE), b"{}").unwrap();
        std::fs::write(restored.join("rsia.sqlite3"), b"not opened").unwrap();
        let before = std::fs::read_dir(&restored).unwrap().count();
        let error = mount_gate(&restored.join("rsia.sqlite3"), None)
            .await
            .err()
            .unwrap();
        assert!(
            error.to_string().starts_with("recovery_quarantine: ")
                && error.to_string().ends_with("; not mounting"),
            "{error}"
        );
        assert_eq!(std::fs::read_dir(&restored).unwrap().count(), before);
        assert!(!restored.join(DATA_LOCK_FILE).exists());
        assert!(!restored.join(RESTORE_ADMISSION_FILE).exists());
    }
}
