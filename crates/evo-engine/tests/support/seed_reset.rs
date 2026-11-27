//! AG-166c-reset shared fixture: one store, one Admin dispatcher, one
//! Host-written live run and one seed install labelled with that run's
//! environment digest. It uses only API that predates the wiring and reads
//! every new fact as JSON, so the parent-tree probes compile against it too.
#![allow(dead_code)]
#[path = "trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshotV3, ToolSourceV1,
};
use evo_core::{Context, Error, Role, fingerprint, hash, now};
use evo_engine::capacity::unix_now_secs;
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobV2, VersionedManagementJob,
};
use evo_engine::packages::E16SourceRef;
use evo_engine::release_store::{CompletePrepareRunRequest, ReleaseStore};
use evo_engine::seeds::{
    E16_CURRENT_SEED_INSTALL_SCHEMA, InstallCurrentSeedRequest, PersistentSeedStore, SeedIdentity,
    SeedUpstream,
};
use evo_storage::Store;
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const NS: &str = "tenant";
pub const ADMIN: &str = "admin";
pub const RESET_SCHEMA: &str = "rsia.management.seed_reset_stage.v1";
pub const RUN_ID: &str = "seed-reset-run";
pub const BASELINE: &[u8] = b"baseline seed";
pub const COMPILER_VERSION: &str = "compiler-v1";

pub fn reset_payload(key: &str, install_id: &str, run_id: &str) -> Value {
    json!({
        "schema_version": RESET_SCHEMA,
        "request_key": key,
        "install_id": install_id,
        "run_id": run_id,
    })
}

/// The staged asset id the seed kernel derives from the namespace, the owner
/// and the request key.
pub fn staged_id(key: &str) -> String {
    format!(
        "e16-staged-asset-{}",
        fingerprint(&(NS, ADMIN, key)).unwrap()
    )
}

/// The install id of the fixture's seed identity.
pub fn install_id() -> String {
    let identity = SeedIdentity {
        publisher: "org.rsia".into(),
        asset_id: "reasoning-seed".into(),
        kind: "skill".into(),
    };
    format!(
        "e16-seed-install-{}",
        fingerprint(&(E16_CURRENT_SEED_INSTALL_SCHEMA, NS, identity)).unwrap()
    )
}

fn surface() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "seed-reset-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: hash(b"seed-reset-surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::RuntimeOwned,
            mapped_field: None,
            consumer: None,
            reason: "fixture projection".into(),
        }],
    }
}

fn system() -> SystemSnapshotV3 {
    let source = ToolSourceV1 {
        schema_version: evo_core::contract::TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "../fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    };
    SystemSnapshotV3 {
        schema_version: "rsia.system_snapshot.v3".into(),
        profile_id: "profile".into(),
        host_id: "seed-reset-host".into(),
        host_version: "1.0.0".into(),
        model_id: "model-v1".into(),
        tools: source.names().unwrap(),
        tool_source: source,
        mandatory_context_digest: hash(b"seed-reset-mandatory"),
    }
}

/// A job row of `operation` as the dispatcher derives its ids, for the
/// queued-cancel, restart and Draining fixtures.
pub fn seeded_job(
    ctx: &Context,
    operation: &str,
    key: &str,
    state: ManagementJobState,
) -> ManagementJobV2 {
    let digest =
        hash(format!("{}\0{}\0{operation}\0{key}", ctx.namespace(), ctx.actor()).as_bytes());
    let claimed = state != ManagementJobState::Queued;
    let running = state == ManagementJobState::Running;
    ManagementJobV2 {
        id: format!("management-job-{}", &digest[..24]),
        schema_version: "rsia.management_job.v2".into(),
        operation: operation.into(),
        request_key: key.into(),
        payload_digest: hash(b"seeded-seed-reset-fixture"),
        owner_actor: ctx.actor().into(),
        owner_role: Role::Admin,
        state,
        step: if claimed { "claimed" } else { "accepted" }.into(),
        private_input_ref: format!("management-input-{}", &digest[..24]),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: running.then(|| hash(b"seeded-seed-reset-lease")),
        lease_until: if running { now().unwrap() + 60 } else { 0 },
        generation: u64::from(claimed),
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    }
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: std::path::PathBuf,
    pub store: Store,
    pub admin: Context,
    pub dispatcher: ManagementDispatcher,
    /// The environment digest of the live Host-written run.
    pub run_environment_digest: String,
    /// The environment label the install was stored with.
    pub install_environment_digest: String,
    pub install_id: String,
    pub baseline_blob_digest: String,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: String,
}

impl Fixture {
    /// The install is labelled with the live run's environment digest.
    pub async fn new() -> Self {
        Self::installed(None).await
    }

    /// `label` replaces the live run's digest as the install's environment
    /// label: an install that can never match the environment it is reset in.
    pub async fn installed(label: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("seed-reset.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let admin = Context::new(NS, ADMIN, Role::Admin).unwrap();
        let host = Context::new(NS, "host", Role::Host).unwrap();
        // The install's one source is a run, so it can be revoked through the
        // lifecycle API.
        let source = json!({"schema_version":"fixture.seed_source.v1","id":"seed-source"});
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "run", "seed-source", admin.actor(), &source)
            .await
            .unwrap();
        session
            .bump_watermark(&admin, "seed-reset-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        trusted_compiler_inputs::register_fixture_host_surface(
            &admin,
            &store,
            "surface",
            surface(),
            vec!["model".into()],
        )
        .await
        .unwrap();
        let snapshot = ReleaseStore::prepare_complete_run(
            &host,
            &store,
            CompletePrepareRunRequest {
                run_id: RUN_ID.into(),
                profile_id: "profile".into(),
                system_snapshot: system(),
                host_surface_id: "surface".into(),
                host_capabilities: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                task_input_digest: hash(b"seed-reset-task-input"),
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            },
        )
        .await
        .unwrap();
        let identity = SeedIdentity {
            publisher: "org.rsia".into(),
            asset_id: "reasoning-seed".into(),
            kind: "skill".into(),
        };
        // B, L and U all differ: the local copy is edited, so the reset must
        // stage B alone and only copy the L and U digests.
        let installed = PersistentSeedStore::install_current(
            &admin,
            &store,
            InstallCurrentSeedRequest {
                request_key: "install-key".into(),
                baseline_identity: Some(identity.clone()),
                local_identity: Some(identity),
                local_marked: true,
                baseline_bytes: Some(BASELINE.to_vec()),
                local_bytes: Some(b"local seed".to_vec()),
                upstream: Some(SeedUpstream {
                    path: "seeds/upstream.txt".into(),
                    content_digest: hash(b"upstream seed"),
                    bytes: b"upstream seed".to_vec(),
                }),
                environment_digest: label
                    .map_or_else(|| snapshot.environment_digest.clone(), str::to_owned),
                source_refs: vec![E16SourceRef {
                    kind: "run".into(),
                    id: "seed-source".into(),
                    digest: fingerprint(&source).unwrap(),
                }],
                compiler_version: COMPILER_VERSION.into(),
                confirmed_bytes: None,
            },
        )
        .await
        .unwrap();
        assert!(installed.staged.is_none(), "the install stages nothing");
        let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
        Self {
            dispatcher,
            run_environment_digest: snapshot.environment_digest.clone(),
            install_environment_digest: installed.install.payload.environment_digest,
            baseline_blob_digest: installed.install.payload.baseline_blob_digest,
            baseline_digest: installed.install.payload.baseline_digest,
            local_digest: installed.install.payload.local_digest,
            upstream_digest: installed.install.payload.upstream_digest.unwrap(),
            install_id: installed.install.id,
            store,
            admin,
            database,
            dir,
        }
    }

    pub async fn submit(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<VersionedManagementJob, Error> {
        self.dispatcher
            .submit(&self.admin, operation, payload)
            .await
    }

    /// The status view as JSON once terminal.
    pub async fn terminal(&self, job_id: &str) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(90), async {
            loop {
                let status = serde_json::to_value(
                    self.dispatcher
                        .status_view(&self.admin, job_id)
                        .await
                        .unwrap(),
                )
                .unwrap();
                if matches!(
                    status["state"].as_str(),
                    Some("succeeded" | "failed" | "cancelled" | "blocked")
                ) {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the management job reaches a terminal state")
    }

    /// Submits and waits; a parent-tree refusal of the registered but
    /// unwired name panics with the expected-failure marker.
    pub async fn run_job(&self, operation: &str, payload: Value) -> Value {
        let queued = match self.submit(operation, payload).await {
            Ok(queued) => queued,
            Err(Error::Invalid(code)) if code == "management_operation_not_wired" => {
                panic!("PARENT-EXPECTED-FAILURE: {operation} is registered but not wired ({code})")
            }
            Err(error) => panic!("unexpected {operation} refusal: {error:?}"),
        };
        self.terminal(queued.identity().id).await
    }

    pub async fn get(&self, kind: &str, id: &str) -> Option<Value> {
        let mut session = self.store.session().await.unwrap();
        let value = session.get(&self.admin, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    /// The raw stored body of one object, byte for byte.
    pub async fn raw_body(&self, kind: &str, id: &str) -> String {
        let mut session = self.store.session().await.unwrap();
        let body = session
            .object_row(&self.admin, kind, id)
            .await
            .unwrap()
            .unwrap()
            .body;
        session.commit().await.unwrap();
        body
    }

    /// The direct dependents of one object.
    pub async fn dependents(&self, kind: &str, id: &str) -> Vec<(String, String)> {
        let mut session = self.store.session().await.unwrap();
        let mut dependents = session.dependents(&self.admin, kind, id).await.unwrap();
        session.commit().await.unwrap();
        dependents.sort();
        dependents
    }

    pub async fn staged_packages(&self) -> u64 {
        let mut session = self.store.session().await.unwrap();
        let usage = session
            .capacity_usage_v41(unix_now_secs().unwrap())
            .await
            .unwrap();
        session.commit().await.unwrap();
        usage.staged_packages
    }

    pub async fn concurrent_dispatches(&self) -> u64 {
        let mut session = self.store.session().await.unwrap();
        let usage = session
            .capacity_usage_v41(unix_now_secs().unwrap())
            .await
            .unwrap();
        session.commit().await.unwrap();
        usage.concurrent_dispatches
    }

    pub async fn put_job(&self, job: &ManagementJobV2) {
        let mut session = self.store.session().await.unwrap();
        session
            .put(&self.admin, "job", &job.id, self.admin.actor(), job)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    /// The number of stored management job rows of the namespace.
    pub async fn job_count(&self) -> usize {
        let mut session = self.store.session().await.unwrap();
        let jobs: Vec<Value> = session.list(&self.admin, "job").await.unwrap();
        session.commit().await.unwrap();
        jobs.len()
    }

    /// Revokes the install's one source run through the lifecycle API.
    pub async fn revoke_install_source(&self) {
        LifecycleStore::begin_revoke(
            &self.admin,
            &self.store,
            TypedObjectRef {
                kind: "run".into(),
                id: "seed-source".into(),
            },
            "seed reset fixture",
            2,
        )
        .await
        .unwrap();
    }

    fn exec_sql(&self, statement: &str) {
        let output = std::process::Command::new("python3")
            .args([
                "-B",
                "-c",
                "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(sys.argv[2])\nc.commit()\n",
            ])
            .arg(&self.database)
            .arg(statement)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "statement failed: {statement}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A trigger that makes the database refuse the insert of any staged
    /// asset row: the fault the atomicity control injects.
    pub fn arm_staged_insert_trigger(&self, name: &str) {
        self.exec_sql(&format!(
            "CREATE TRIGGER {name} BEFORE INSERT ON objects WHEN json_extract(NEW.body,'$.schema_version')='rsia.e16.staged_asset.v1' BEGIN SELECT RAISE(ABORT,'injected-{name}'); END"
        ));
    }

    pub fn drop_trigger(&self, name: &str) {
        self.exec_sql(&format!("DROP TRIGGER {name}"));
    }

    /// How many audit records of one operation name one entity.
    pub fn audit_count(&self, operation: &str, entity: &str) -> usize {
        self.audit()
            .iter()
            .filter(|(recorded, subject)| recorded == operation && subject == entity)
            .count()
    }

    /// Whether any staging audit record exists at all.
    pub fn has_staging_audit(&self) -> bool {
        self.business_audit()
            .iter()
            .any(|(operation, _)| operation == "e16.seed_reset.stage")
    }

    /// Puts one object and its edges in a plain session.
    pub async fn put(&self, kind: &str, id: &str, body: &Value) {
        let mut session = self.store.session().await.unwrap();
        session
            .put(&self.admin, kind, id, self.admin.actor(), body)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    fn sqlite(&self, script: &str, argument: &str) -> String {
        let output = std::process::Command::new("python3")
            .args(["-B", "-c", script])
            .arg(&self.database)
            .arg(argument)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "script failed: {script}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Every stored object of the namespace except the management job and
    /// private-input rows: kind, id and body, ordered.
    pub fn objects(&self) -> Value {
        self.sqlite(
            r#"
import json, sqlite3, sys
c = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)
rows = [list(r) for r in c.execute("SELECT kind,id,body FROM objects WHERE namespace=? AND id NOT LIKE 'management-%' ORDER BY kind,id", (sys.argv[2],))]
print(json.dumps(rows))
"#,
            NS,
        )
        .parse()
        .unwrap()
    }

    /// The audit chain as (operation, entity) pairs, in order.
    pub fn audit(&self) -> Vec<(String, String)> {
        let rows: Value = self
            .sqlite(
                r#"
import json, sqlite3, sys
c = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)
rows = [list(r) for r in c.execute("SELECT json_extract(payload,'$.operation'),json_extract(payload,'$.entity') FROM audit WHERE namespace=? ORDER BY seq", (sys.argv[2],))]
print(json.dumps(rows))
"#,
                NS,
            )
            .parse()
            .unwrap();
        serde_json::from_value(rows).unwrap()
    }

    /// The audit records that are not the management job lifecycle.
    pub fn business_audit(&self) -> Vec<(String, String)> {
        self.audit()
            .into_iter()
            .filter(|(operation, _)| !operation.starts_with("management."))
            .collect()
    }

    /// The stored objects, the business audit records and the blob files: a
    /// refusal or a replay must leave all three unchanged.
    pub async fn business_shape(&self) -> Value {
        json!({
            "objects": self.objects(),
            "audit": self.business_audit(),
            "files": blob_files(self.dir.path()),
        })
    }
}

/// The content-addressed blob files under one data directory, by relative
/// path and content digest.
pub fn blob_files(root: &std::path::Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    fn walk(root: &std::path::Path, path: &std::path::Path, files: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hash(&std::fs::read(&path).unwrap()),
                );
            }
        }
    }
    let blobs = root.join("blobs");
    if blobs.exists() {
        walk(root, &blobs, &mut files);
    }
    files
}
