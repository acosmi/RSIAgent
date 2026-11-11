//! AG-166c recovery controls for `seed.install` (plan v4.5 §6.1.2 L828,
//! §11.1–11.1.1, E16.3).
//!
//! The install handler commits Prepared, the three registered blobs,
//! Installed, the edges, the audit records and the terminal job state in one
//! transaction. A crash leaves no half-committed install and at most
//! content-addressed bytes; a same-key cached job is never re-run, a new key
//! with the same material converges on the shared kernel, a stale lease cannot
//! finalize, cancellation before the first write is zero-effect, and a source
//! or watermark change blocks every later install attempt.
use evo_core::{Context, Error, Role, fingerprint, hash, now};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobStatusV1, ManagementJobV2,
    ManagementResultV2, ManagementSeedResultV2,
};
use evo_engine::packages::E16SourceRef;
use evo_engine::seeds::{
    BUILTIN_I0_BYTES, E16_CURRENT_SEED_INSTALL_SCHEMA, PersistentSeedStore, SeedStatus,
};
use evo_storage::Store;
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::collections::BTreeMap;

async fn begin_revoke_source(admin: &Context, store: &Store, id: &str) {
    LifecycleStore::begin_revoke(
        admin,
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: id.into(),
        },
        "seed recovery fixture",
        2,
    )
    .await
    .unwrap();
}

fn admin(namespace: &str, actor: &str) -> Context {
    Context::new(namespace, actor, Role::Admin).unwrap()
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    sources: Vec<E16SourceRef>,
    i0_digest: String,
    environment_digest: String,
    compiler_version: String,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(name)).await.unwrap();
        let admin = admin("tenant", "admin");
        let body = json!({"schema_version":"fixture.seed_source.v1","id":"seed-source"});
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "run", "seed-source", admin.actor(), &body)
            .await
            .unwrap();
        session
            .bump_watermark(&admin, "seed-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let i0_digest = store
            .put_blob_bounded(&admin, BUILTIN_I0_BYTES, 64 * 1024 * 1024)
            .await
            .unwrap();
        Self {
            dir,
            store,
            admin,
            sources: vec![E16SourceRef {
                kind: "run".into(),
                id: "seed-source".into(),
                digest: fingerprint(&body).unwrap(),
            }],
            i0_digest,
            environment_digest: hash(b"environment"),
            compiler_version: "builtin-i0-v2".into(),
        }
    }

    fn input_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": "rsia.management.seed_install_input.v1",
            "catalog_asset_id": "improver-i0",
            "source_refs": self.sources,
            "environment_digest": self.environment_digest,
            "compiler_version": self.compiler_version,
        }))
        .unwrap()
    }

    async fn input_digest(&self) -> String {
        self.store
            .put_blob_bounded(&self.admin, &self.input_bytes(), 64 * 1024 * 1024)
            .await
            .unwrap()
    }

    fn payload(&self, key: &str, input_digest: &str) -> Value {
        json!({
            "schema_version": "rsia.management.seed_install.v1",
            "request_key": key,
            "install_input_blob_digest": input_digest,
            "baseline_blob_digest": self.i0_digest,
            "local_blob_digest": self.i0_digest,
            "upstream_blob_digest": self.i0_digest,
        })
    }

    async fn terminal(
        &self,
        dispatcher: &ManagementDispatcher,
        job_id: &str,
    ) -> ManagementJobStatusV1 {
        tokio::time::timeout(std::time::Duration::from_secs(90), async {
            loop {
                let status = dispatcher.status_view(&self.admin, job_id).await.unwrap();
                if matches!(
                    status.state,
                    ManagementJobState::Succeeded
                        | ManagementJobState::Failed
                        | ManagementJobState::Cancelled
                        | ManagementJobState::Blocked
                ) {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("seed.install job reaches a terminal state")
    }

    async fn done(
        &self,
        dispatcher: &ManagementDispatcher,
        payload: Value,
    ) -> ManagementJobStatusV1 {
        let queued = dispatcher
            .submit(&self.admin, "seed.install", payload)
            .await
            .unwrap();
        self.terminal(dispatcher, queued.identity().id).await
    }

    async fn seed_records(&self) -> Vec<Value> {
        let mut session = self.store.session().await.unwrap();
        let artifacts: Vec<Value> = session.list(&self.admin, "artifact").await.unwrap();
        session.commit().await.unwrap();
        artifacts
            .into_iter()
            .filter(|body| body["schema_version"] == E16_CURRENT_SEED_INSTALL_SCHEMA)
            .collect()
    }

    async fn record_body(&self, id: &str) -> Value {
        let mut session = self.store.session().await.unwrap();
        let body = session.need(&self.admin, "artifact", id).await.unwrap();
        session.commit().await.unwrap();
        body
    }
}

fn files_under(root: &std::path::Path) -> BTreeMap<String, String> {
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

fn sqlite(path: &std::path::Path, script: &str) {
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "script failed: {}\n{}",
        script,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn trigger(path: &std::path::Path, name: &str, condition: &str) {
    sqlite(
        path,
        &format!(
            "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER {name} BEFORE INSERT ON objects WHEN {condition} BEGIN SELECT RAISE(ABORT,'injected-{name}'); END\")\nc.commit()\nprint('ok')"
        ),
    );
}

fn drop_trigger(path: &std::path::Path, name: &str) {
    sqlite(
        path,
        &format!(
            "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER {name}')\nc.commit()\nprint('ok')"
        ),
    );
}

fn trigger_finalize(path: &std::path::Path, name: &str) {
    sqlite(
        path,
        &format!(
            "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER {name} BEFORE UPDATE OF body ON objects WHEN json_extract(NEW.body,'$.schema_version')='{E16_CURRENT_SEED_INSTALL_SCHEMA}' AND json_extract(NEW.body,'$.payload.status')='installed' BEGIN SELECT RAISE(FAIL,'injected-{name}'); END\")\nc.commit()\nprint('ok')"
        ),
    );
}

/// Prepared and the terminal job state share one commit: a refusal leaves no
/// install row and no successful job.
#[tokio::test]
async fn seed_prepared_phase_and_job_progress_commit_together() {
    let fixture = Fixture::new("seed-recovery-prepare.sqlite3").await;
    let input_digest = fixture.input_digest().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let path = fixture.dir.path().join("seed-recovery-prepare.sqlite3");
    trigger(
        &path,
        "stop_seed_prepare",
        "json_extract(NEW.body,'$.schema_version')='rsia.e16.seed_install.v2'",
    );
    let failed = fixture
        .done(
            &dispatcher,
            fixture.payload("atomic-prepare", &input_digest),
        )
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert!(fixture.seed_records().await.is_empty(), "no Prepared row");
    let published = fixture
        .store
        .read_blob(&fixture.admin, &fixture.i0_digest, 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(published, BUILTIN_I0_BYTES, "input blobs still readable");
    drop_trigger(&path, "stop_seed_prepare");
    let healthy = fixture
        .done(
            &dispatcher,
            fixture.payload("atomic-prepare-healthy", &input_digest),
        )
        .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    assert_eq!(fixture.seed_records().await.len(), 1);
    fixture.store.close().await;
}

/// A failure after the registered bytes were published rolls back every SQL
/// row; the same request key stays an honest failed job and a new key with the
/// same material converges on the same content-addressed bytes.
#[tokio::test]
async fn seed_blob_publish_then_crash_recovers_same_material() {
    let fixture = Fixture::new("seed-recovery-finalize.sqlite3").await;
    let input_digest = fixture.input_digest().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let path = fixture.dir.path().join("seed-recovery-finalize.sqlite3");
    let before_files = files_under(fixture.dir.path());
    trigger_finalize(&path, "stop_seed_finalize");
    let failed = fixture
        .done(
            &dispatcher,
            fixture.payload("published-crash", &input_digest),
        )
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert!(
        fixture.seed_records().await.is_empty(),
        "no Installed row after the rolled-back finalize"
    );
    assert_eq!(
        files_under(fixture.dir.path()),
        before_files,
        "only the pre-existing content-addressed bytes remain"
    );
    assert_eq!(
        fixture
            .store
            .read_blob(&fixture.admin, &fixture.i0_digest, 64 * 1024 * 1024)
            .await
            .unwrap(),
        BUILTIN_I0_BYTES
    );
    // The same key is the already accepted intent: it is never re-run into a
    // false success.
    let replay = fixture
        .done(
            &dispatcher,
            fixture.payload("published-crash", &input_digest),
        )
        .await;
    assert_eq!(replay.id, failed.id);
    assert_eq!(replay.state, ManagementJobState::Failed);
    assert!(replay.result.is_none());
    drop_trigger(&path, "stop_seed_finalize");
    let healthy = fixture
        .done(
            &dispatcher,
            fixture.payload("published-crash-retry", &input_digest),
        )
        .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    assert_eq!(
        files_under(fixture.dir.path()),
        before_files,
        "the retry reuses the same content-addressed bytes"
    );
    let install_id = match healthy.result.as_ref() {
        Some(ManagementResultV2::Seed(ManagementSeedResultV2::SeedInstalled {
            install_id,
            ..
        })) => install_id.clone(),
        other => panic!("{other:?}"),
    };
    let install: evo_engine::packages::E16Envelope<evo_engine::seeds::SeedInstallPayloadV2> =
        PersistentSeedStore::read_install(&fixture.admin, &fixture.store, &install_id)
            .await
            .unwrap();
    assert_eq!(install.payload.status, SeedStatus::Installed);
    fixture.store.close().await;
}

/// A stale generation, lease or operation can neither finalize nor replace the
/// baseline; only the current claim passes the physical fence.
#[tokio::test]
async fn seed_stale_lease_cannot_finalize_or_replace_baseline() {
    let fixture = Fixture::new("seed-recovery-stale.sqlite3").await;
    let job_id = "management-job-seed-fence";
    let lease = hash(b"current-lease");
    let job = ManagementJobV2 {
        id: job_id.into(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "seed.install".into(),
        request_key: "seed-fence".into(),
        payload_digest: hash(b"payload"),
        owner_actor: fixture.admin.actor().into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: "management-input-seed-fence".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: Some(lease.clone()),
        lease_until: now().unwrap() + 60,
        generation: 1,
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, "job", job_id, fixture.admin.actor(), &job)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before_files = files_under(fixture.dir.path());
    let mut session = fixture.store.registered_blob_write_session().await.unwrap();
    for stale in [
        evo_storage::ManagementJobFence {
            job_id,
            operation: "seed.install",
            generation: 0,
            lease_token: &lease,
        },
        evo_storage::ManagementJobFence {
            job_id,
            operation: "seed.install",
            generation: 1,
            lease_token: "stale-lease",
        },
        evo_storage::ManagementJobFence {
            job_id,
            operation: "seed.reset_stage",
            generation: 1,
            lease_token: &lease,
        },
    ] {
        assert!(matches!(
            session
                .ensure_management_job_fence(&fixture.admin, &stale)
                .await,
            Err(Error::Conflict(_))
        ));
    }
    let current = evo_storage::ManagementJobFence {
        job_id,
        operation: "seed.install",
        generation: 1,
        lease_token: &lease,
    };
    session
        .ensure_management_job_fence(&fixture.admin, &current)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(fixture.seed_records().await.is_empty());
    assert_eq!(files_under(fixture.dir.path()), before_files);
    fixture.store.close().await;
}

/// Cancellation before the first write is zero-effect; after a committed
/// install it only records the arrival and preserves the immutable facts.
#[tokio::test]
async fn seed_cancel_before_and_after_effect_are_honest() {
    let fixture = Fixture::new("seed-recovery-cancel.sqlite3").await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let before = files_under(fixture.dir.path());
    // A queued, never-claimed job fixture for the before-effect control.
    let job_digest = hash(b"tenant\0admin\0seed.install\0cancel-before");
    let job_id = format!("management-job-{}", &job_digest[..24]);
    let job = ManagementJobV2 {
        id: job_id.clone(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "seed.install".into(),
        request_key: "cancel-before".into(),
        payload_digest: hash(b"accepted-fixture"),
        owner_actor: fixture.admin.actor().into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: format!("management-input-{}", &job_digest[..24]),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, "job", &job_id, fixture.admin.actor(), &job)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let cancelled = dispatcher.cancel(&fixture.admin, &job_id).await.unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Cancelled,
        "{cancelled:?}"
    );
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert!(fixture.seed_records().await.is_empty());
    assert_eq!(files_under(fixture.dir.path()), before);

    // The after-effect control: a real committed install.
    let input_digest = fixture.input_digest().await;
    let installed = fixture
        .done(&dispatcher, fixture.payload("cancel-after", &input_digest))
        .await;
    assert_eq!(installed.state, ManagementJobState::Succeeded);
    let install_id = match installed.result.as_ref() {
        Some(ManagementResultV2::Seed(ManagementSeedResultV2::SeedInstalled {
            install_id,
            ..
        })) => install_id.clone(),
        other => panic!("{other:?}"),
    };
    let before_record = fixture.record_body(&install_id).await;
    let before_files = files_under(fixture.dir.path());
    let cancelled = dispatcher
        .cancel(&fixture.admin, &installed.id)
        .await
        .unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Succeeded,
        "a committed install is an audit fact"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(installed.result.as_ref()).unwrap()
    );
    assert_eq!(fixture.record_body(&install_id).await, before_record);
    assert_eq!(files_under(fixture.dir.path()), before_files);
    fixture.store.close().await;
}

/// A revoked source or an advanced watermark blocks every later install
/// attempt with zero business writes; the committed record and bytes survive.
#[tokio::test]
async fn seed_changed_source_or_watermark_blocks_resume() {
    let fixture = Fixture::new("seed-recovery-revoke.sqlite3").await;
    let input_digest = fixture.input_digest().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let installed = fixture
        .done(
            &dispatcher,
            fixture.payload("revoke-install", &input_digest),
        )
        .await;
    assert_eq!(installed.state, ManagementJobState::Succeeded);
    let install_id = match installed.result.as_ref() {
        Some(ManagementResultV2::Seed(ManagementSeedResultV2::SeedInstalled {
            install_id,
            ..
        })) => install_id.clone(),
        other => panic!("{other:?}"),
    };
    let before_record = fixture.record_body(&install_id).await;
    let before_files = files_under(fixture.dir.path());
    begin_revoke_source(&fixture.admin, &fixture.store, &fixture.sources[0].id).await;
    let failed = fixture
        .done(&dispatcher, fixture.payload("revoke-replay", &input_digest))
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("forbidden"));
    assert_eq!(fixture.record_body(&install_id).await, before_record);
    assert_eq!(files_under(fixture.dir.path()), before_files);
    assert_eq!(fixture.seed_records().await.len(), 1);

    // A fresh namespace where the source was revoked before the first install:
    // the handler refuses before its first business write.
    let fresh = Fixture::new("seed-recovery-revoke-fresh.sqlite3").await;
    let fresh_input = fresh.input_digest().await;
    begin_revoke_source(&fresh.admin, &fresh.store, &fresh.sources[0].id).await;
    let dispatcher =
        ManagementDispatcher::new(fresh.store.clone(), vec![fresh.admin.clone()]).unwrap();
    let refused = fresh
        .done(&dispatcher, fresh.payload("revoked-first", &fresh_input))
        .await;
    assert_eq!(refused.state, ManagementJobState::Failed, "{refused:?}");
    assert_eq!(refused.error_code.as_deref(), Some("forbidden"));
    assert!(fresh.seed_records().await.is_empty());
    fresh.store.close().await;
    fixture.store.close().await;
}
