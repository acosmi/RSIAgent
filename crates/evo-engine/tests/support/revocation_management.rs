//! AG-162 shared fixture: one store, one Admin dispatcher, hand-written source
//! graphs and a SQL snapshot of the revocation tables. It uses only API that
//! predates AG-162 and reads every new fact as JSON, so the parent-tree probes
//! compile against it too.
#![allow(dead_code)]
use evo_core::{Context, Error, Role, hash, now};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobV2, VersionedManagementJob,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

pub const NS: &str = "revocation-tenant";
pub const REVOKE_SCHEMA: &str = "rsia.management.source_revoke.v1";
pub const CONTINUE_SCHEMA: &str = "rsia.management.cleanup_continue.v1";
/// The reason and clock of the fixture revocations made through the old API.
pub const OLD_REASON: &str = "fixture revocation";
pub const OLD_NOW: i64 = 10;

pub fn context(actor: &str, role: Role) -> Context {
    Context::new(NS, actor, role).unwrap()
}

pub fn revoke_payload(key: &str, kind: &str, id: &str) -> Value {
    json!({
        "schema_version": REVOKE_SCHEMA,
        "request_key": key,
        "source_kind": kind,
        "source_id": id,
    })
}

pub fn continue_payload(key: &str, cleanup_job_id: &str, limit: u64) -> Value {
    json!({
        "schema_version": CONTINUE_SCHEMA,
        "request_key": key,
        "cleanup_job_id": cleanup_job_id,
        "edge_page_limit": limit,
    })
}

/// A stage fact the cleanup classifies and redacts.
pub fn fact(id: &str) -> Value {
    json!({"id": id, "schema_version": "rsia.optimization.stage_fact.v1",
        "kind": "terminal_rejected", "payload": format!("SECRET-{id}")})
}

/// A node of a schema the cleanup does not classify: it fails the cleanup.
pub fn unknown(id: &str) -> Value {
    json!({"id": id, "schema_version": "rsia.unknown.v9", "payload": format!("SECRET-{id}")})
}

/// The one artifact `begin_revoke` accepts as a source: a strict E16
/// `import_source`.
pub fn import_source(id: &str, owner: &str) -> Value {
    json!({
        "schema_version": "rsia.e16.import_source.v1",
        "id": id,
        "namespace": NS,
        "owner_actor": owner,
        "request_key": format!("request-{id}"),
        "input_digest": hash(format!("input-{id}").as_bytes()),
        "created_at": 1,
        "updated_at": 1,
        "source_refs": [],
        "revoke_watermark": 0,
        "payload": {"raw_blob_digest": null, "status": "prepared"},
    })
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: std::path::PathBuf,
    pub store: Store,
    pub admin: Context,
    pub dispatcher: ManagementDispatcher,
}

impl Fixture {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("revocation.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let admin = context("admin", Role::Admin);
        let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
        Self {
            dir,
            database,
            store,
            admin,
            dispatcher,
        }
    }

    pub async fn put(&self, kind: &str, id: &str, body: &Value, deps: &[(&str, &str)]) {
        let mut session = self.store.session().await.unwrap();
        session
            .put(&self.admin, kind, id, self.admin.actor(), body)
            .await
            .unwrap();
        for (dst_kind, dst_id) in deps {
            session
                .put_edge(&self.admin, kind, id, dst_kind, dst_id)
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
    }

    pub async fn get(&self, kind: &str, id: &str) -> Option<Value> {
        let mut session = self.store.session().await.unwrap();
        let value = session.get(&self.admin, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn run(&self, id: &str) {
        self.put(
            "run",
            id,
            &json!({"id": id, "schema_version": "rsia.optimization.source.v1", "body": "run-body"}),
            &[],
        )
        .await;
    }

    /// A run with three direct dependents and one second-level dependent, so
    /// a one-edge page needs several steps.
    pub async fn graph(&self, source: &str) -> Vec<String> {
        self.run(source).await;
        let ids: Vec<String> = (1..=3).map(|n| format!("{source}-fact-{n}")).collect();
        for id in &ids {
            self.put("artifact", id, &fact(id), &[("run", source)])
                .await;
        }
        let nested = format!("{source}-fact-nested");
        self.put(
            "artifact",
            &nested,
            &fact(&nested),
            &[("artifact", &ids[0])],
        )
        .await;
        let mut all = ids;
        all.push(nested);
        all
    }

    /// The old storage API revocation, the fixture's way to make a cleanup.
    pub async fn old_revoke(&self, source: &str) -> CleanupStatus {
        LifecycleStore::begin_revoke(
            &self.admin,
            &self.store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            OLD_REASON,
            OLD_NOW,
        )
        .await
        .unwrap()
    }

    pub async fn old_step(&self, job_id: &str, limit: usize, now: i64) -> CleanupStatus {
        LifecycleStore::cleanup_step(&self.admin, &self.store, job_id, limit, now)
            .await
            .unwrap()
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

    /// The status view as JSON once terminal; no observed state is Draining.
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
                assert_ne!(
                    status["state"],
                    json!("draining"),
                    "a revocation management job never drains: {status}"
                );
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
        .expect("revocation management job reaches a terminal state")
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

    pub fn sqlite(&self, script: &str) -> String {
        let output = std::process::Command::new("python3")
            .args(["-B", "-c", script])
            .arg(&self.database)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "script failed: {}\n{}",
            script,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Every revocation fact: watermark, tombstones, cleanup jobs, frontier,
    /// event kinds and the bodies of all non-management objects. Clock
    /// columns are left out so a page driven by the dispatcher's clock
    /// compares with a twin driven by a fixed one.
    pub fn snapshot(&self) -> Value {
        let script = r#"
import json, sqlite3, sys
c = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)
ns = sys.argv[2]
def rows(q):
    return [list(r) for r in c.execute(q, (ns,))]
print(json.dumps({
  "watermark": rows("SELECT seq FROM revoke_watermark WHERE namespace=?"),
  "jobs": rows("SELECT job_id,source_kind,source_id,watermark_seq,state,processed_nodes FROM revoke_cleanup_jobs WHERE namespace=? ORDER BY job_id"),
  "frontier": rows("SELECT seq,job_id,node_kind,node_id,expanded,cursor_src_kind,cursor_src_id FROM revoke_cleanup_frontier WHERE namespace=? ORDER BY seq"),
  "events": rows("SELECT job_id,event_kind FROM revoke_cleanup_events WHERE namespace=? ORDER BY seq"),
  "objects": rows("SELECT kind,id,body FROM objects WHERE namespace=? AND kind NOT IN ('job','tombstone') AND id NOT LIKE 'management-%' ORDER BY kind,id"),
  "tombstones": rows("SELECT id,json_extract(body,'$.source_kind'),json_extract(body,'$.reason'),json_extract(body,'$.watermark_seq') FROM objects WHERE namespace=? AND kind='tombstone' ORDER BY id"),
}))
"#;
        let output = std::process::Command::new("python3")
            .args(["-B", "-c", script])
            .arg(&self.database)
            .arg(NS)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// The number of cleanup events of one kind.
    pub fn events(&self, kind: &str) -> usize {
        self.snapshot()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event[1] == json!(kind))
            .count()
    }

    pub async fn put_job(&self, job: &ManagementJobV2) {
        let mut session = self.store.session().await.unwrap();
        session
            .put(&self.admin, "job", &job.id, self.admin.actor(), job)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    pub async fn concurrent_dispatches(&self) -> u64 {
        let mut session = self.store.session().await.unwrap();
        let used = session
            .capacity_usage_v41(now().unwrap() as u64)
            .await
            .unwrap()
            .concurrent_dispatches;
        session.commit().await.unwrap();
        used
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
        payload_digest: hash(b"seeded-revocation-fixture"),
        owner_actor: ctx.actor().into(),
        owner_role: Role::Admin,
        state,
        step: if claimed { "claimed" } else { "accepted" }.into(),
        private_input_ref: format!("management-input-{}", &digest[..24]),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: running.then(|| hash(b"seeded-revocation-lease")),
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
