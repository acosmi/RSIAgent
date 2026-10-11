//! Real backup/restore and public accounting APIs; faults alter only the
//! historical E16 ref. Equal monetary rows cannot license a different ref.
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{
    BackupManifest, CleanupState, LifecycleStore, TypedObjectRef, read_control_plane_facts,
};
use evo_storage::typed_budget::{E16_BUDGET_REQUEST_SCHEMA, E16BudgetRequest, e16_budget_ref_id};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RESTORE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/restore_backup.py"
);
fn admin() -> Context {
    Context::new("n", "import-admin", Role::Admin).unwrap()
}
fn host() -> Context {
    Context::new("n", "model-host", Role::Host).unwrap()
}
fn fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}
fn charge() -> UsageCharge {
    UsageCharge {
        amount_micros: 9,
        currency: "USD".into(),
        pricing_version: "price".into(),
        provider_request_id: "provider-1".into(),
        usage_record_id: "usage-1".into(),
        output_digest: hash(b"output"),
    }
}
fn evidence() -> ModelCallSettlementEvidence {
    ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            "test.transport.v1",
            &json!({"content":"TRANSPORT-MARKER"}),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                "test.response.v1",
                &json!({"content":"RESPONSE-MARKER"}),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "test.response.v1",
            &json!({"content":"BLOCKED-MARKER"}),
        )
        .unwrap(),
        forced_block_reason: None,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    live: PathBuf,
    call: BudgetCallRecord,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let live = root.join("rsia.sqlite3");
        let store = Store::open(&live).await.unwrap();
        store
            .authorize_root_budget(
                &admin(),
                &RootBudgetAuthorization {
                    root_budget_id: "root".into(),
                    billing_scope: "scope".into(),
                    allowed_namespaces: vec!["n".into()],
                    currency: "USD".into(),
                    pricing_version: "price".into(),
                    payment_subject: "payer".into(),
                    authorization_receipt_digest: hash(b"authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let mut s = store.session().await.unwrap();
        s.bump_watermark(&admin(), &hash(b"initial-watermark"))
            .await
            .unwrap();
        s.put(&admin(),"artifact","S",admin().actor(),&json!({"schema_version":"rsia.e16.import_source.v1","id":"S","namespace":"n","owner_actor":"import-admin",
            "request_key":"import-1","input_digest":hash(b"source"),"created_at":1,"updated_at":1,"source_refs":[],"revoke_watermark":1,
            "payload":{"status":"prepared","raw_blob_digest":null}})).await.unwrap();
        s.commit().await.unwrap();
        let refs = store
            .snapshot_e16_budget_sources(
                &host(),
                &[TypedObjectRef {
                    kind: "artifact".into(),
                    id: "S".into(),
                }],
            )
            .await
            .unwrap();
        let input = BudgetArtifact::from_serializable(
            "test.input.v1",
            &json!({"content":"REQUEST-MARKER"}),
        )
        .unwrap();
        let artifact = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "n".into(),
            billing_scope: "scope".into(),
            call_id: "call".into(),
            object_refs: refs,
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        let call = store
            .reserve_e16_budget_call(
                &host(),
                &BudgetCallReservation {
                    billing_scope: "scope".into(),
                    call_id: "call".into(),
                    dispatch_group_id: "group".into(),
                    stage: BudgetStage::Reflection,
                    actual_input_digest: input.digest,
                    request_artifact: Some(artifact),
                    max_cost_micros: 20,
                    lease_token: "lease".into(),
                    lease_until: 1000,
                    now: 2,
                },
            )
            .await
            .unwrap();
        let call = store
            .begin_budget_dispatch(&host(), &fence(&call, 3))
            .await
            .unwrap()
            .call;
        Self {
            _dir: dir,
            root,
            store,
            live,
            call,
        }
    }
    async fn settle(&mut self) {
        self.call = self
            .store
            .settle_model_budget_call(&host(), &fence(&self.call, 4), &charge(), &evidence())
            .await
            .unwrap()
            .call;
        assert_eq!(self.call.actual_cost_micros, Some(9));
        assert_eq!(self.call.response_usable, Some(true));
    }
    async fn close(&mut self) {
        self.call = self
            .store
            .close_budget_call_execution(
                &host(),
                "scope",
                "call",
                self.call.dispatch_id.as_deref().unwrap(),
                "execution finished",
                5,
            )
            .await
            .unwrap();
    }
    async fn reference(&self) -> Value {
        let mut s = self.store.session().await.unwrap();
        let value = s
            .need(
                &admin(),
                "artifact",
                &e16_budget_ref_id("n", "scope", "call").unwrap(),
            )
            .await
            .unwrap();
        s.commit().await.unwrap();
        value
    }
    async fn write_ref(&self, value: Option<&Value>) {
        let mut s = self.store.session().await.unwrap();
        let id = e16_budget_ref_id("n", "scope", "call").unwrap();
        if let Some(v) = value {
            s.put(&admin(), "artifact", &id, host().actor(), v)
                .await
                .unwrap();
        } else {
            s.delete(&admin(), "artifact", &id).await.unwrap();
        }
        s.commit().await.unwrap();
    }
    async fn backup(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        self.store.backup(&path).await.unwrap();
        path
    }
    fn restore(&self, backup: &Path, name: &str) -> (PathBuf, Output) {
        let raw = std::fs::read(backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&raw).unwrap();
        let delta = self.root.join(format!("{name}-delta.json"));
        std::fs::write(&delta,serde_json::to_vec(&json!({"schema_version":"rsia.revoke_delta.v1","base_manifest_sha256":hash(&raw),
            "namespaces":manifest.watermarks.iter().map(|m|json!({"namespace":m.namespace,"base_seq":m.seq,"base_digest":m.digest,"events":[],"latest_seq":m.seq,"latest_digest":m.digest})).collect::<Vec<_>>()})).unwrap()).unwrap();
        let dest = self.root.join(name);
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--backup")
            .arg(backup)
            .arg("--dest")
            .arg(&dest)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(&self.live)
            .output()
            .unwrap();
        (dest, output)
    }
}
fn restored_ok(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("RESTORE_OK events=0"));
}
fn isolated(dest: &Path, output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "consumed query/alpha/dispatch/accounting facts changed; make a fresh backup"
        )
    );
    assert!(!dest.exists());
    assert!(
        !std::fs::read_dir(dest.parent().unwrap()).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&format!(".{}", dest.file_name().unwrap().to_string_lossy())))
    );
}

#[tokio::test]
async fn identical_finalized_and_closed_budget_and_e16_ref_restore_without_replaying_cost() {
    let mut f = Fixture::new().await;
    f.settle().await;
    f.close().await;
    let reference = f.reference().await;
    let backup = f.backup("backup").await;
    let original = read_control_plane_facts(&f.live).await.unwrap();
    assert_eq!(
        original.protected_objects[&(
            "n".into(),
            "artifact".into(),
            e16_budget_ref_id("n", "scope", "call").unwrap()
        )],
        reference
    );
    let (dest, out) = f.restore(&backup, "restored");
    restored_ok(&out);
    let facts = read_control_plane_facts(&dest.join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(facts, original);
    let store = Store::open(&dest.join("rsia.sqlite3")).await.unwrap();
    let call = store
        .budget_call(&host(), "scope", "call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call, f.call);
    assert!(
        !store
            .begin_budget_dispatch(&host(), &fence(&call, 6))
            .await
            .unwrap()
            .new_dispatch
    );
    assert_eq!(
        store
            .consume_e16_budget_response(&host(), "scope", "call")
            .await
            .unwrap(),
        call.response_artifact.clone().unwrap()
    );
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        9
    );
    assert_eq!(
        read_control_plane_facts(&dest.join("rsia.sqlite3"))
            .await
            .unwrap(),
        facts
    );
    store.close().await;
}

#[tokio::test]
async fn equal_ledger_with_changed_or_missing_e16_ref_is_not_an_admissible_backup() {
    for fault in ["request_digest", "source_digest", "missing"] {
        let mut f = Fixture::new().await;
        f.settle().await;
        let backup = f.backup("backup").await;
        let before = read_control_plane_facts(&f.live).await.unwrap();
        let mut reference = f.reference().await;
        match fault {
            "request_digest" => reference["request_digest"] = json!(hash(b"different request")),
            "source_digest" => {
                reference["object_refs"][0]["storage_body_digest"] =
                    json!(hash(b"different storage body"))
            }
            _ => {}
        }
        f.write_ref(if fault == "missing" {
            None
        } else {
            Some(&reference)
        })
        .await;
        let after = read_control_plane_facts(&f.live).await.unwrap();
        assert_eq!(
            before.root_budget_tables, after.root_budget_tables,
            "{fault}"
        );
        assert_eq!(before.watermarks, after.watermarks);
        assert_ne!(before.protected_objects, after.protected_objects);
        let (dest, out) = f.restore(&backup, "isolated");
        isolated(&dest, &out);
        assert_eq!(
            read_control_plane_facts(&f.live).await.unwrap(),
            after,
            "restore must not rewrite history"
        );
    }
}

#[tokio::test]
async fn backup_missing_only_ref_is_isolated_from_equal_ledger_with_original_ref() {
    let mut f = Fixture::new().await;
    f.settle().await;
    let reference = f.reference().await;
    let original = read_control_plane_facts(&f.live).await.unwrap();
    f.write_ref(None).await;
    let backup = f.backup("missing-ref-backup").await;
    f.write_ref(Some(&reference)).await;
    let current = read_control_plane_facts(&f.live).await.unwrap();
    assert_eq!(original, current);
    let old = read_control_plane_facts(&backup.join("rsia.sqlite3"))
        .await
        .unwrap();
    assert_eq!(old.root_budget_tables, current.root_budget_tables);
    assert_ne!(old.protected_objects, current.protected_objects);
    let (dest, out) = f.restore(&backup, "isolated");
    isolated(&dest, &out);
}

#[tokio::test]
async fn redacted_three_bodies_and_preserved_fee_ref_and_watermark_restore_without_content() {
    let mut f = Fixture::new().await;
    f.settle().await;
    f.close().await;
    let status = LifecycleStore::begin_revoke(
        &admin(),
        &f.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "S".into(),
        },
        "source revoked",
        6,
    )
    .await
    .unwrap();
    let mut complete = None;
    for now in 7..500 {
        let next = LifecycleStore::cleanup_step(&admin(), &f.store, &status.job_id, 1, now)
            .await
            .unwrap();
        assert_ne!(next.state, CleanupState::Failed, "{next:?}");
        if next.state == CleanupState::Complete {
            complete = Some(next);
            break;
        }
    }
    assert!(complete.is_some());
    let call = f
        .store
        .budget_call(&host(), "scope", "call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(9));
    assert_eq!(call.dispatch_id, f.call.dispatch_id);
    assert!(call.execution_closed);
    for a in [
        &call.request_artifact,
        &call.transport_artifact,
        &call.response_artifact,
    ] {
        let a = a.as_ref().unwrap();
        assert_eq!(a.schema_version, "rsia.redacted.v1");
        for marker in [
            "REQUEST-MARKER",
            "TRANSPORT-MARKER",
            "RESPONSE-MARKER",
            "BLOCKED-MARKER",
        ] {
            assert!(!a.body.contains(marker));
        }
    }
    let reference = f.reference().await;
    let backup = f.backup("redacted-backup").await;
    let facts = read_control_plane_facts(&f.live).await.unwrap();
    assert_eq!(facts.watermarks["n"].seq, 2);
    let (dest, out) = f.restore(&backup, "redacted-restored");
    restored_ok(&out);
    assert_eq!(
        read_control_plane_facts(&dest.join("rsia.sqlite3"))
            .await
            .unwrap(),
        facts
    );
    let store = Store::open(&dest.join("rsia.sqlite3")).await.unwrap();
    assert_eq!(
        store
            .budget_call(&host(), "scope", "call")
            .await
            .unwrap()
            .unwrap(),
        call
    );
    let mut s = store.session().await.unwrap();
    assert_eq!(
        s.need::<Value>(
            &admin(),
            "artifact",
            &e16_budget_ref_id("n", "scope", "call").unwrap()
        )
        .await
        .unwrap(),
        reference
    );
    s.commit().await.unwrap();
    assert!(
        matches!(store.consume_e16_budget_response(&host(),"scope","call").await,Err(Error::Conflict(code)) if code=="e16_budget_sources_unavailable")
    );
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        9
    );
    store.close().await;
}

#[tokio::test]
async fn restored_unknown_usage_remains_uncertain_until_real_admin_reconciliation_and_close() {
    let mut f = Fixture::new().await;
    f.call = f
        .store
        .mark_budget_call_uncertain(&host(), &fence(&f.call, 4), "provider receipt pending")
        .await
        .unwrap();
    let backup = f.backup("uncertain-backup").await;
    let (dest, out) = f.restore(&backup, "uncertain-restored");
    restored_ok(&out);
    let store = Store::open(&dest.join("rsia.sqlite3")).await.unwrap();
    let call = store
        .budget_call(&host(), "scope", "call")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call, f.call);
    assert_eq!(call.state, BudgetCallState::Uncertain);
    assert!(!call.execution_closed);
    assert_eq!(call.actual_cost_micros, None);
    let root = store.root_budget(&admin(), "scope").await.unwrap().unwrap();
    assert_eq!((root.reserved_micros, root.spent_micros), (20, 0));
    assert!(
        store
            .consume_e16_budget_response(&host(), "scope", "call")
            .await
            .is_err()
    );
    let settled = store
        .reconcile_budget_call_cost(&admin(), "scope", "call", &charge(), 6)
        .await
        .unwrap();
    assert_eq!(settled.actual_cost_micros, Some(9));
    assert_eq!(settled.dispatch_id, call.dispatch_id);
    assert!(!settled.execution_closed);
    let closed = store
        .close_budget_call_execution(
            &host(),
            "scope",
            "call",
            call.dispatch_id.as_deref().unwrap(),
            "receipt reconciled",
            7,
        )
        .await
        .unwrap();
    assert!(closed.execution_closed);
    let root = store.root_budget(&admin(), "scope").await.unwrap().unwrap();
    assert_eq!((root.reserved_micros, root.spent_micros), (0, 9));
    store.close().await;
}
