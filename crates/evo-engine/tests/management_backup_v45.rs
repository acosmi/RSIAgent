//! Real authenticated management jobs and real Store artifacts; no paid provider.
use evo_core::{Context, Error, Role, hash, now};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJobState, VersionedManagementJob};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetExecutionProvenance, BudgetStage,
    ModelCallSettlementEvidence, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{BackupManifest, read_control_plane_facts};
use evo_storage::sqlx::{self, Connection};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    admin: Context,
    dispatcher: ManagementDispatcher,
}

impl Fixture {
    async fn new() -> Self {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("rsia.sqlite3");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let store = Store::open(&path).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let host = Context::new("n", "host", Role::Host).unwrap();
        let other = Context::new("other", "other-admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        for ctx in [&admin, &other] {
            session
                .bump_watermark(ctx, &hash(ctx.namespace().as_bytes()))
                .await
                .unwrap();
            session.put(ctx, "artifact", "seed", ctx.actor(), &json!({"id":"seed","schema_version":"test.backup.seed.v1","body":ctx.namespace()})).await.unwrap();
        }
        session.commit().await.unwrap();
        // Controlled raw source bytes; AG157 first-publication is outside this fixture.
        let bytes = b"management backup real blob";
        let blob = hash(bytes);
        let blobs = root.join("blobs");
        let namespace = blobs.join(hash(host.namespace().as_bytes()));
        for directory in [&blobs, &namespace] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .unwrap();
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(namespace.join(&blob))
            .unwrap()
            .write_all(bytes)
            .unwrap();
        assert_eq!(store.put_blob(&host, bytes).await.unwrap(), blob);
        let at = now().unwrap() - 10;
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "backup-root".into(),
                    billing_scope: "backup-scope".into(),
                    allowed_namespaces: vec!["n".into()],
                    currency: "USD".into(),
                    pricing_version: "fixture-price".into(),
                    payment_subject: "fixture-only".into(),
                    authorization_receipt_digest: hash(b"authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1000,
                    created_at: at,
                },
            )
            .await
            .unwrap();
        for (index, call_id) in ["settled", "uncertain"].into_iter().enumerate() {
            let input =
                BudgetArtifact::from_serializable("test.backup.input.v1", &json!({"call":call_id}))
                    .unwrap();
            let call = store
                .reserve_budget_call(
                    &host,
                    &BudgetCallReservation {
                        billing_scope: "backup-scope".into(),
                        call_id: call_id.into(),
                        dispatch_group_id: "backup-group".into(),
                        stage: BudgetStage::Reflection,
                        actual_input_digest: input.digest.clone(),
                        request_artifact: Some(input),
                        max_cost_micros: 20,
                        lease_token: format!("fixture-{index}"),
                        lease_until: at + 3600,
                        now: at + 1,
                    },
                )
                .await
                .unwrap();
            let fence = BudgetCallFence {
                billing_scope: call.billing_scope.clone(),
                call_id: call.call_id.clone(),
                actual_input_digest: call.actual_input_digest.clone(),
                lease_token: call.lease_token.clone(),
                lease_epoch: call.lease_epoch,
                now: at + 2,
            };
            let call = store
                .begin_budget_dispatch(&host, &fence)
                .await
                .unwrap()
                .call;
            assert_eq!(call.state, evo_storage::budget::BudgetCallState::Dispatched);
            assert!(call.dispatch_id.is_some());
            let mut fence = fence;
            fence.now = at + 3;
            if call_id == "uncertain" {
                store
                    .mark_budget_call_uncertain(&host, &fence, "fixture provider receipt pending")
                    .await
                    .unwrap();
            } else {
                let output = BudgetArtifact::from_serializable(
                    "test.backup.output.v1",
                    &json!({"output":"complete"}),
                )
                .unwrap();
                let settled = store
                    .settle_model_budget_call(
                        &host,
                        &fence,
                        &UsageCharge {
                            amount_micros: 9,
                            currency: "USD".into(),
                            pricing_version: "fixture-price".into(),
                            provider_request_id: "fixture-provider".into(),
                            usage_record_id: "fixture-usage".into(),
                            output_digest: output.digest.clone(),
                        },
                        &ModelCallSettlementEvidence {
                            provenance: BudgetExecutionProvenance::Fixture,
                            actual_model_digest: Some(hash(b"fixture-model")),
                            transport_artifact: output.clone(),
                            usable_response: Some(output.clone()),
                            blocked_response: output,
                            forced_block_reason: None,
                        },
                    )
                    .await
                    .unwrap()
                    .call;
                store
                    .close_budget_call_execution(
                        &host,
                        "backup-scope",
                        call_id,
                        settled.dispatch_id.as_deref().unwrap(),
                        "fixture execution finished",
                        at + 4,
                    )
                    .await
                    .unwrap();
            }
        }
        let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
        let dispatcher =
            ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator]).unwrap();
        Self {
            _directory: directory,
            root,
            store,
            admin,
            dispatcher,
        }
    }

    async fn create(&self, key: &str, name: &str) -> VersionedManagementJob {
        self.dispatcher
            .submit(&self.admin, "backup.create", request(key, name))
            .await
            .unwrap()
    }

    async fn terminal(&self, id: &str) -> VersionedManagementJob {
        for _ in 0..1000 {
            let job = self.dispatcher.status(&self.admin, id).await.unwrap();
            if matches!(
                job.identity().state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("backup job did not reach a real terminal");
    }
}

fn request(key: &str, name: &str) -> Value {
    json!({"schema_version":"rsia.management.backup_create.v1","request_key":key,"relative_name":name})
}

fn manifest(path: &Path) -> (Vec<u8>, BackupManifest) {
    let raw = std::fs::read(path.join("backup-manifest.json")).unwrap();
    let manifest = serde_json::from_slice(&raw).unwrap();
    (raw, manifest)
}

#[tokio::test]
async fn admin_backup_create_writes_one_complete_real_snapshot() {
    let fixture = Fixture::new().await;
    let before = read_control_plane_facts(&fixture.root.join("rsia.sqlite3"))
        .await
        .unwrap();
    let accepted = fixture.create("complete", "complete").await;
    let job = fixture.terminal(accepted.identity().id).await;
    assert_eq!(
        job.identity().state,
        ManagementJobState::Succeeded,
        "{job:?}"
    );
    assert_eq!(job.identity().schema_version, "rsia.management_job.v2");
    let path = fixture.root.join("backups/complete");
    let (raw, saved) = manifest(&path);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(saved.schema_version, "rsia.backup.v2");
    assert!(saved.complete && !saved.blobs.is_empty());
    let database_bytes = std::fs::read(path.join("rsia.sqlite3")).unwrap();
    assert_eq!(
        &database_bytes[18..20],
        &[1, 1],
        "fresh VACUUM INTO image uses rollback journal header"
    );
    for member in std::iter::once(&saved.database).chain(&saved.blobs) {
        let bytes = std::fs::read(path.join(&member.path)).unwrap();
        assert_eq!(hash(&bytes), member.sha256);
        assert_eq!(bytes.len() as u64, member.bytes);
    }
    let snapshot = read_control_plane_facts(&path.join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(snapshot.root_budget_tables, before.root_budget_tables);
    assert_eq!(snapshot.watermarks, before.watermarks);
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path.join("rsia.sqlite3"))
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let amounts: (i64, i64) = sqlx::query_as(
        "SELECT spent_micros,reserved_micros FROM root_budgets WHERE billing_scope='backup-scope'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(amounts, (9, 20));
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM root_budget_calls ORDER BY call_id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(states, ["finalized", "uncertain"]);
    connection.close().await.unwrap();
    let view = serde_json::to_value(
        fixture
            .dispatcher
            .status_view(&fixture.admin, job.identity().id)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(view["schema_version"], "rsia.management_job_status.v1");
    assert_eq!(view["fee_knowledge"]["state"], "known_none");
    let serialized = serde_json::to_string(&view).unwrap();
    assert!(
        !serialized.contains("snapshot_lease_token")
            && !serialized.contains("temporary_name")
            && !serialized.contains(fixture.root.to_str().unwrap())
    );
    let mut wire = serde_json::to_value(&job).unwrap();
    wire["backup"] = Value::Null;
    assert!(evo_storage::decode_management_job(&serde_json::to_string(&wire).unwrap()).is_err());
    let mut wire = serde_json::to_value(&job).unwrap();
    wire["operation"] = json!("meta.start");
    assert!(evo_storage::decode_management_job(&serde_json::to_string(&wire).unwrap()).is_err());
    fixture.store.close().await;
}

#[tokio::test]
async fn backup_same_key_replay_returns_original_refs_without_new_directory() {
    let fixture = Fixture::new().await;
    let accepted = fixture.create("replay", "original").await;
    let job = fixture.terminal(accepted.identity().id).await;
    assert_eq!(job.identity().state, ManagementJobState::Succeeded);
    let before = std::fs::read(fixture.root.join("backups/original/backup-manifest.json")).unwrap();
    let repeated = fixture.create("replay", "original").await;
    assert_eq!(
        serde_json::to_value(&job).unwrap(),
        serde_json::to_value(&repeated).unwrap()
    );
    assert_eq!(
        std::fs::read_dir(fixture.root.join("backups"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(fixture.root.join("backups/original/backup-manifest.json")).unwrap(),
        before
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn backup_same_key_changed_name_conflicts_before_fs() {
    let fixture = Fixture::new().await;
    let accepted = fixture.create("changed", "original").await;
    let job = fixture.terminal(accepted.identity().id).await;
    assert_eq!(job.identity().state, ManagementJobState::Succeeded);
    let before = read_control_plane_facts(&fixture.root.join("rsia.sqlite3"))
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .dispatcher
            .submit(
                &fixture.admin,
                "backup.create",
                request("changed", "different")
            )
            .await,
        Err(Error::Conflict(_))
    ));
    let after = read_control_plane_facts(&fixture.root.join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(before, after);
    assert!(!fixture.root.join("backups/different").exists());
    assert_eq!(
        std::fs::read_dir(fixture.root.join("backups"))
            .unwrap()
            .count(),
        1
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn backup_cancel_after_publish_preserves_snapshot_and_result() {
    let fixture = Fixture::new().await;
    let accepted = fixture.create("cancel-after", "committed").await;
    let job = fixture.terminal(accepted.identity().id).await;
    assert_eq!(job.identity().state, ManagementJobState::Succeeded);
    let raw = std::fs::read(fixture.root.join("backups/committed/backup-manifest.json")).unwrap();
    let cancelled = fixture
        .dispatcher
        .cancel(&fixture.admin, job.identity().id)
        .await
        .unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Succeeded);
    assert_eq!(
        serde_json::to_value(job.result()).unwrap(),
        serde_json::to_value(cancelled.result()).unwrap()
    );
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        std::fs::read(fixture.root.join("backups/committed/backup-manifest.json")).unwrap(),
        raw
    );
    fixture.store.close().await;
}

#[tokio::test]
async fn foreign_namespace_admin_cannot_export_whole_instance() {
    let fixture = Fixture::new().await;
    let foreign = Context::new("other", fixture.admin.actor(), Role::Admin).unwrap();
    for body in [
        request("foreign", "foreign"),
        json!({"schema_version":0,"relative_name":"../escape"}),
    ] {
        assert!(matches!(
            fixture
                .dispatcher
                .submit(&foreign, "backup.create", body)
                .await,
            Err(Error::Forbidden)
        ));
    }
    assert!(!fixture.root.join("backups").exists());
    let mut session = fixture.store.session().await.unwrap();
    assert!(
        session
            .object_rows_page(&fixture.admin, "job", None)
            .await
            .unwrap()
            .is_empty()
    );
    session.commit().await.unwrap();
    fixture.store.close().await;
}

#[tokio::test]
async fn restored_snapshot_running_backup_job_never_reexecutes() {
    let fixture = Fixture::new().await;
    let accepted = fixture.create("restored", "source").await;
    let completed = fixture.terminal(accepted.identity().id).await;
    assert_eq!(completed.identity().state, ManagementJobState::Succeeded);
    let backup = fixture.root.join("backups/source");
    let (raw, saved) = manifest(&backup);
    let delta = fixture.root.join("delta.json");
    std::fs::write(&delta, serde_json::to_vec(&json!({"schema_version":"rsia.revoke_delta.v1","base_manifest_sha256":hash(&raw),"namespaces":saved.watermarks.iter().map(|mark| json!({"namespace":mark.namespace,"base_seq":mark.seq,"base_digest":mark.digest,"events":[],"latest_seq":mark.seq,"latest_digest":mark.digest})).collect::<Vec<_>>()})).unwrap()).unwrap();
    let destination = fixture.root.join("restored");
    let latest_refused = fixture.root.join("refused-latest-anchor");
    let refusal = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/restore_backup.py"
        ))
        .args([
            "--backup",
            backup.to_str().unwrap(),
            "--dest",
            latest_refused.to_str().unwrap(),
            "--revoke-delta",
            delta.to_str().unwrap(),
            "--trusted-revocations-db",
            fixture.root.join("rsia.sqlite3").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(refusal.status.code(), Some(1));
    assert!(!latest_refused.exists());
    assert!(
        String::from_utf8_lossy(&refusal.stderr)
            .contains("consumed query/alpha/dispatch/accounting facts changed")
    );
    // The independent anchor below is a bounded software fixture of the real
    // Running image. It is not represented as the newer live control plane.
    let anchor = fixture.root.join("independent-running-anchor.sqlite3");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&anchor)
            .unwrap()
            .write_all(&std::fs::read(backup.join("rsia.sqlite3")).unwrap())
            .unwrap();
    }
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/restore_backup.py"
        ))
        .args([
            "--backup",
            backup.to_str().unwrap(),
            "--dest",
            destination.to_str().unwrap(),
            "--revoke-delta",
            delta.to_str().unwrap(),
            "--trusted-revocations-db",
            anchor.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision = evo_engine::startup_gate::StartupGate::new(
        &destination.join("rsia.sqlite3"),
        Some(&anchor),
    )
    .evaluate()
    .await
    .unwrap();
    assert_eq!(
        decision.posture(),
        evo_engine::startup_gate::RecoveryPosture::RestoredVerified
    );
    let restored = Store::open(&destination.join("rsia.sqlite3"))
        .await
        .unwrap();
    decision.admit().unwrap();
    let dispatcher =
        ManagementDispatcher::new(restored.clone(), vec![fixture.admin.clone()]).unwrap();
    assert_eq!(dispatcher.recover_pending().await.unwrap(), 1);
    let mut final_job = None;
    for _ in 0..1000 {
        let job = dispatcher
            .status(&fixture.admin, completed.identity().id)
            .await
            .unwrap();
        if job.identity().state == ManagementJobState::Blocked {
            final_job = Some(job);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let final_job = final_job.expect("restored backup must close without re-execution");
    assert_eq!(
        final_job.error_code(),
        Some("backup_publication_unverified")
    );
    assert!(!destination.join("backups").exists());
    assert_eq!(
        std::fs::read(backup.join("backup-manifest.json")).unwrap(),
        raw
    );
    assert_eq!(
        read_control_plane_facts(&destination.join("rsia.sqlite3"))
            .await
            .unwrap()
            .root_budget_tables,
        read_control_plane_facts(&backup.join("rsia.sqlite3"))
            .await
            .unwrap()
            .root_budget_tables
    );
    restored.close().await;
    fixture.store.close().await;
}
