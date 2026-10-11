//! Actual persistent import writer and all three local readers. Budget snapshot
//! metadata is not E16 business authorization or a completed learning chain.
use evo_core::evidence::Purpose;
use evo_core::{Context, Role, fingerprint, hash};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, IMPORT_RESULT_SCHEMA, IMPORT_SOURCE_SCHEMA,
    ImportRegistrationRequest, ImportRetentionScope, ImportSourceReadStatus, ImportSourceRecord,
    ImportSourceSpec, PersistentImportService, SOURCE_SELECTION_SCHEMA, SourceFormat,
};
use evo_engine::startup_gate::{RecoveryPosture, StartupGate, StartupGateError};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{BackupManifest, TypedObjectRef, read_control_plane_facts};
use evo_storage::typed_budget::{
    E16_BUDGET_REQUEST_SCHEMA, E16BudgetRequest, E16BudgetSourceRef, e16_budget_ref_id,
};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn admin() -> Context {
    Context::new("n", "import-admin", Role::Admin).unwrap()
}
fn host() -> Context {
    Context::new("n", "model-host", Role::Host).unwrap()
}
struct Imported {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    live: PathBuf,
    keys: Vec<TypedObjectRef>,
    source_ids: Vec<String>,
    result_id: String,
}
impl Imported {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let live = root.join("rsia.sqlite3");
        let store = Store::open(&live).await.unwrap();
        let mut s = store.session().await.unwrap();
        s.bump_watermark(&admin(), &hash(b"initial-import-watermark"))
            .await
            .unwrap();
        s.commit().await.unwrap();
        let samples = [
            (
                "native.json",
                SourceFormat::RsiaTraceV1,
                r#"{"schema_version":"rsia.trace.v1","run_id":"reader-run","events":[{"role":"user","kind":"chat","content":"native imported observation"}]}"#,
            ),
            (
                "pi.json",
                SourceFormat::RsihPiFixture,
                r#"{"format":"rsih.pi.fixture","prompts":[{"role":"user","text":"pi imported observation"}]}"#,
            ),
            (
                "claude.jsonl",
                SourceFormat::ClaudeFixture,
                r#"{"role":"user","content":"claude imported observation"}"#,
            ),
        ];
        let mut sources = Vec::new();
        for (i, (name, reader, body)) in samples.iter().enumerate() {
            let path = root.join(name);
            std::fs::write(&path, body).unwrap();
            sources.push(ImportSourceSpec {
                source_id: format!("source-{i}"),
                path: path.to_string_lossy().into_owned(),
                reader: *reader,
                expected_digest: hash(body.as_bytes()),
            });
        }
        let service = PersistentImportService::new(store.clone());
        let selection = service
            .register(
                &admin(),
                ImportRegistrationRequest {
                    schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                    request_key: "three-real-readers".into(),
                    roots: vec![root.to_string_lossy().into_owned()],
                    purpose: Purpose::Development,
                    allow_model_excerpts: true,
                    outbound_authorized: true,
                    retention_scope: ImportRetentionScope::LocalWithAuthorizedExcerpts,
                    sources,
                },
            )
            .await
            .unwrap();
        let result = service.execute(&admin(), &selection.id).await.unwrap();
        assert_eq!(result.payload.aggregate_summary.total_sources, 3);
        assert_eq!(result.payload.aggregate_summary.total_events, 3);
        assert_eq!(
            result.payload.generation_status,
            "blocked_external_generation_conditions_unavailable"
        );
        let source_ids = selection.payload.import_source_ids;
        let mut keys = vec![
            TypedObjectRef {
                kind: "artifact".into(),
                id: selection.id,
            },
            TypedObjectRef {
                kind: "artifact".into(),
                id: result.id.clone(),
            },
        ];
        keys.extend(source_ids.iter().map(|id| TypedObjectRef {
            kind: "artifact".into(),
            id: id.clone(),
        }));
        Self {
            _dir: dir,
            root,
            store,
            live,
            keys,
            source_ids,
            result_id: result.id,
        }
    }
    async fn reserve(&self, id: &str) -> BudgetCallRecord {
        self.store
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
        let refs = self
            .store
            .snapshot_e16_budget_sources(&host(), &self.keys)
            .await
            .unwrap();
        let input = BudgetArtifact::from_serializable(
            "test.future_content_input.v1",
            &json!({"content":"REQUEST-MARKER"}),
        )
        .unwrap();
        let request = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "n".into(),
            billing_scope: "scope".into(),
            call_id: id.into(),
            object_refs: refs,
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        self.store
            .reserve_e16_budget_call(
                &host(),
                &BudgetCallReservation {
                    billing_scope: "scope".into(),
                    call_id: id.into(),
                    dispatch_group_id: format!("group-{id}"),
                    stage: BudgetStage::Reflection,
                    actual_input_digest: input.digest,
                    request_artifact: Some(request),
                    max_cost_micros: 20,
                    lease_token: format!("lease-{id}"),
                    lease_until: 1000,
                    now: 2,
                },
            )
            .await
            .unwrap()
    }
}
fn raw_digest(database: &Path, id: &str) -> String {
    let output=Command::new("python3").arg("-c").arg("import sqlite3,hashlib,sys\nc=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)\nb=c.execute(\"SELECT body FROM objects WHERE namespace='n' AND kind='artifact' AND id=?\",(sys.argv[2],)).fetchone()[0]\nprint(hashlib.sha256(b.encode('utf-8')).hexdigest())\nc.close()")
        .arg(database).arg(id).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
struct ReverseFields<'a>(&'a Value);
impl Serialize for ReverseFields<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let object = self.0.as_object().unwrap();
        let mut map = serializer.serialize_map(Some(object.len()))?;
        for (k, v) in object.iter().rev() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}
fn assert_refs_actual(imported: &Imported, refs: &[E16BudgetSourceRef]) {
    assert_eq!(refs.len(), 5);
    let schemas = refs
        .iter()
        .map(|r| r.schema_version.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        schemas
            .iter()
            .filter(|s| **s == IMPORT_SOURCE_SCHEMA)
            .count(),
        3
    );
    assert!(schemas.contains(&SOURCE_SELECTION_SCHEMA));
    assert!(schemas.contains(&IMPORT_RESULT_SCHEMA));
    for r in refs {
        assert_eq!(r.namespace, "n");
        assert_eq!(r.kind, "artifact");
        assert_eq!(r.owner_actor, admin().actor());
        assert_ne!(r.owner_actor, host().actor());
        assert_eq!(r.storage_body_digest, raw_digest(&imported.live, &r.id));
    }
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
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.insert(key, "dir".into());
                walk(root, &path, out)
            } else if meta.file_type().is_symlink() {
                out.insert(
                    key,
                    format!("symlink:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else {
                out.insert(key, hash(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[tokio::test]
async fn real_three_readers_writer_keep_original_owner_and_snapshot_actual_raw_order() {
    let f = Imported::new().await;
    let refs = f
        .store
        .snapshot_e16_budget_sources(&host(), &f.keys)
        .await
        .unwrap();
    assert_refs_actual(&f, &refs);
    let mut s = f.store.session().await.unwrap();
    let mut readers = Vec::new();
    let mut first = None;
    for id in &f.source_ids {
        let source: ImportSourceRecord = s.need(&admin(), "artifact", id).await.unwrap();
        assert_eq!(source.payload.status, ImportSourceReadStatus::Ready);
        assert!(source.payload.blob_published);
        readers.push(source.payload.reader);
        if first.is_none() {
            first = Some(source);
        }
    }
    s.commit().await.unwrap();
    for reader in [
        SourceFormat::RsiaTraceV1,
        SourceFormat::RsihPiFixture,
        SourceFormat::ClaudeFixture,
    ] {
        assert!(readers.contains(&reader));
    }
    let source = first.unwrap();
    let business_digest = fingerprint(&source).unwrap();
    let service = PersistentImportService::new(f.store.clone());
    let before = service
        .load_live_result(&admin(), &f.result_id)
        .await
        .unwrap();
    assert_eq!(
        before
            .source_refs
            .iter()
            .find(|r| r.id == source.id)
            .unwrap()
            .content_digest,
        business_digest
    );
    // Only serialization order changes. Typed import data, owner and business
    // content fingerprint remain valid; raw storage bytes are now distinct.
    let value = serde_json::to_value(&source).unwrap();
    let mut s = f.store.session().await.unwrap();
    s.put(
        &admin(),
        "artifact",
        &source.id,
        admin().actor(),
        &ReverseFields(&value),
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    let updated = f
        .store
        .snapshot_e16_budget_sources(&host(), &f.keys)
        .await
        .unwrap();
    assert_refs_actual(&f, &updated);
    let raw = &updated
        .iter()
        .find(|r| r.id == source.id)
        .unwrap()
        .storage_body_digest;
    assert_ne!(raw, &business_digest);
    let mut s = f.store.session().await.unwrap();
    let loaded: ImportSourceRecord = s.need(&admin(), "artifact", &source.id).await.unwrap();
    assert_eq!(fingerprint(&loaded).unwrap(), business_digest);
    s.commit().await.unwrap();
    let still_live = service
        .load_live_result(&admin(), &f.result_id)
        .await
        .unwrap();
    assert_eq!(
        still_live
            .source_refs
            .iter()
            .find(|r| r.id == source.id)
            .unwrap()
            .content_digest,
        business_digest
    );
    let call = f.reserve("writer-call").await;
    assert_eq!(call.namespace, "n");
    assert!(
        f.store
            .begin_budget_dispatch(&host(), &fence(&call, 3))
            .await
            .unwrap()
            .new_dispatch
    );
}

#[tokio::test]
async fn startup_gate_detects_only_new_ref_drift_with_identical_real_ledger_and_never_writes() {
    for missing in [false, true] {
        let f = Imported::new().await;
        let call = f.reserve("gate-call").await;
        let call = f
            .store
            .begin_budget_dispatch(&host(), &fence(&call, 3))
            .await
            .unwrap()
            .call;
        let charge = UsageCharge {
            amount_micros: 9,
            currency: "USD".into(),
            pricing_version: "price".into(),
            provider_request_id: "provider".into(),
            usage_record_id: "usage".into(),
            output_digest: hash(b"output"),
        };
        let response = BudgetArtifact::from_serializable(
            "test.response.v1",
            &json!({"content":"RESPONSE-MARKER"}),
        )
        .unwrap();
        let evidence = ModelCallSettlementEvidence {
            provenance: BudgetExecutionProvenance::Fixture,
            actual_model_digest: Some(hash(b"model")),
            transport_artifact: BudgetArtifact::from_serializable(
                "test.transport.v1",
                &json!({"content":"TRANSPORT-MARKER"}),
            )
            .unwrap(),
            usable_response: Some(response),
            blocked_response: BudgetArtifact::from_serializable(
                "test.response.v1",
                &json!({"content":"BLOCKED-MARKER"}),
            )
            .unwrap(),
            forced_block_reason: None,
        };
        let settled = f
            .store
            .settle_model_budget_call(&host(), &fence(&call, 4), &charge, &evidence)
            .await
            .unwrap();
        assert_eq!(settled.call.actual_cost_micros, Some(9));
        let backup = f.root.join("backup");
        f.store.backup(&backup).await.unwrap();
        let raw = std::fs::read(backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&raw).unwrap();
        let delta = f.root.join("delta.json");
        std::fs::write(&delta,serde_json::to_vec(&json!({"schema_version":"rsia.revoke_delta.v1","base_manifest_sha256":hash(&raw),"namespaces":manifest.watermarks.iter().map(|m|json!({"namespace":m.namespace,"base_seq":m.seq,"base_digest":m.digest,"events":[],"latest_seq":m.seq,"latest_digest":m.digest})).collect::<Vec<_>>()})).unwrap()).unwrap();
        let restored = f.root.join("restored");
        let output = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scripts/restore_backup.py"
            ))
            .arg("--backup")
            .arg(&backup)
            .arg("--dest")
            .arg(&restored)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(&f.live)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let data = restored.join("rsia.sqlite3");
        let before = snapshot(&restored);
        let gate = StartupGate::new(&data, Some(&f.live))
            .evaluate()
            .await
            .unwrap();
        assert_eq!(gate.posture(), RecoveryPosture::RestoredVerified);
        assert_eq!(snapshot(&restored), before);
        let facts = read_control_plane_facts(&f.live).await.unwrap();
        let id = e16_budget_ref_id("n", "scope", "gate-call").unwrap();
        let mut s = f.store.session().await.unwrap();
        if missing {
            s.delete(&admin(), "artifact", &id).await.unwrap();
        } else {
            let mut value: Value = s.need(&admin(), "artifact", &id).await.unwrap();
            value["request_digest"] = json!(hash(b"different request"));
            s.put(&admin(), "artifact", &id, host().actor(), &value)
                .await
                .unwrap();
        }
        s.commit().await.unwrap();
        let changed = read_control_plane_facts(&f.live).await.unwrap();
        assert_eq!(facts.root_budget_tables, changed.root_budget_tables);
        assert_ne!(facts.protected_objects, changed.protected_objects);
        let before = snapshot(&restored);
        let error = StartupGate::new(&data, Some(&f.live))
            .evaluate()
            .await
            .unwrap_err();
        assert!(matches!(error, StartupGateError::Quarantine(_)), "{error}");
        assert_eq!(snapshot(&restored), before);
    }
}
