use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::{Context, Role, hash};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{HostPrepareConfig, HostService};
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

#[derive(Debug, Parser)]
#[command(name = "rsia", version, about = "Bounded RSIA host service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    },
    /// Run the four MCP tools over stdio with a startup-fixed Agent identity.
    Mcp {
        #[arg(long, default_value = "rsia.sqlite3")]
        data: PathBuf,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "stdio-agent")]
        actor: String,
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
        } => {
            let _data_lock = DataDirectoryLock::acquire(&data)?;
            let (service, prepare_config) = bootstrap(&data, &namespace).await?;
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
        } => {
            let _data_lock = DataDirectoryLock::acquire(&data)?;
            let (service, prepare_config) = bootstrap(&data, &namespace).await?;
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

struct DataDirectoryLock {
    _file: File,
}

impl DataDirectoryLock {
    fn acquire(data: &Path) -> Result<Self> {
        let parent = data.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create data directory {}", parent.display()))?;
        let parent = parent
            .canonicalize()
            .with_context(|| format!("failed to resolve data directory {}", parent.display()))?;
        let lock_path = parent.join(".rsia.lock");
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
                    parent.display()
                );
            }
            return Err(error)
                .with_context(|| format!("failed to lock data directory {}", parent.display()));
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
