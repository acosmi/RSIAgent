//! AG-166c management controls for `seed.install` (plan v4.5 §6.1.2 L828,
//! §11.1–11.1.1, E16.3).
//!
//! Real Store, real Admin contexts, real registered blobs, the real
//! compiled-in builtin catalog and the real `ManagementDispatcher` job
//! lifecycle. The strict wire carries only a request key, the transient
//! input-blob digest and the three material blob digests; paths, roots,
//! publishers, roles, owners, namespaces, markers, verification carriers,
//! permission booleans and body clocks are refused before any session.
//! Results carry only the committed install identity, a closed classification
//! and three digests.
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobStatusV1, ManagementResultV2,
    ManagementSeedClassification, ManagementSeedResultV2,
};
use evo_engine::packages::{E16_STAGED_ASSET_SCHEMA, E16Envelope, E16SourceRef};
use evo_engine::seeds::{
    BUILTIN_I0_BYTES, E16_CURRENT_SEED_INSTALL_SCHEMA, PersistentSeedStore, SeedIdentity,
    SeedInstallPayloadV2, SeedStatus, builtin_seed_catalog,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("seed-management.sqlite3"))
            .await
            .unwrap();
        let admin = admin("tenant", "admin");
        let body = json!({"schema_version":"fixture.seed_source.v1","id":"seed-source"});
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "artifact", "seed-source", admin.actor(), &body)
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
                kind: "artifact".into(),
                id: "seed-source".into(),
                digest: fingerprint(&body).unwrap(),
            }],
            i0_digest,
            environment_digest: hash(b"environment"),
            compiler_version: "builtin-i0-v2".into(),
        }
    }

    fn input_bytes(&self, catalog_asset_id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": "rsia.management.seed_install_input.v1",
            "catalog_asset_id": catalog_asset_id,
            "source_refs": self.sources,
            "environment_digest": self.environment_digest,
            "compiler_version": self.compiler_version,
        }))
        .unwrap()
    }

    async fn input_digest(&self, catalog_asset_id: &str) -> String {
        self.store
            .put_blob_bounded(
                &self.admin,
                &self.input_bytes(catalog_asset_id),
                64 * 1024 * 1024,
            )
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

    fn install_id(&self) -> String {
        let identity = SeedIdentity {
            publisher: "rsia.builtin".into(),
            asset_id: "improver-i0".into(),
            kind: "improver".into(),
        };
        format!(
            "e16-seed-install-{}",
            fingerprint(&(
                E16_CURRENT_SEED_INSTALL_SCHEMA,
                self.admin.namespace(),
                identity
            ))
            .unwrap()
        )
    }

    async fn terminal(
        &self,
        dispatcher: &ManagementDispatcher,
        job_id: &str,
    ) -> ManagementJobStatusV1 {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
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

    async fn seed_records(&self, ctx: &Context) -> Vec<Value> {
        let mut session = self.store.session().await.unwrap();
        let artifacts: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
        session.commit().await.unwrap();
        artifacts
            .into_iter()
            .filter(|body| {
                body["schema_version"] == E16_CURRENT_SEED_INSTALL_SCHEMA
                    || body["schema_version"] == E16_STAGED_ASSET_SCHEMA
            })
            .collect()
    }

    async fn job_count(&self) -> usize {
        let mut session = self.store.session().await.unwrap();
        let jobs: Vec<Value> = session.list(&self.admin, "job").await.unwrap();
        session.commit().await.unwrap();
        jobs.len()
    }

    async fn shape(&self) -> Value {
        let mut session = self.store.session().await.unwrap();
        let jobs: Vec<Value> = session.list(&self.admin, "job").await.unwrap();
        let artifacts: Vec<Value> = session.list(&self.admin, "artifact").await.unwrap();
        let watermark = session.watermark(&self.admin).await.unwrap();
        session.commit().await.unwrap();
        let audit = self.store.verify_audit(&self.admin).await.unwrap();
        json!({
            "jobs": jobs,
            "artifacts": artifacts,
            "watermark": watermark,
            "audit": audit,
            "files": blob_files(self.dir.path()),
        })
    }
}

fn blob_files(root: &std::path::Path) -> BTreeMap<String, String> {
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

fn seed_result(status: &ManagementJobStatusV1) -> &ManagementSeedResultV2 {
    match status.result.as_ref() {
        Some(ManagementResultV2::Seed(result)) => result,
        other => panic!("expected Seed result, got {other:?}"),
    }
}

async fn assert_no_listed_effects(store: &Store, ctx: &Context) {
    let mut session = store.session().await.unwrap();
    for kind in [
        "candidate",
        "release",
        "pointer",
        "evaluation",
        "budget",
        "reservation",
    ] {
        let rows: Vec<Value> = session.list(ctx, kind).await.unwrap();
        assert!(rows.is_empty(), "{kind}: {rows:?}");
    }
    session.commit().await.unwrap();
}

fn collect_exposed(value: &Value, keys: &mut Vec<String>, texts: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                keys.push(key.clone());
                collect_exposed(nested, keys, texts);
            }
        }
        Value::Array(items) => {
            for nested in items {
                collect_exposed(nested, keys, texts);
            }
        }
        Value::String(text) => texts.push(text.clone()),
        _ => {}
    }
}

/// Admin is checked before the payload is parsed or any source is observed.
/// The held Session proves no connection is acquired by the refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_management_role_precedes_payload_and_source_observation() {
    let fixture = Fixture::new().await;
    let evaluator = Context::new("tenant", "evaluator", Role::Evaluator).unwrap();
    let dispatcher = ManagementDispatcher::new(
        fixture.store.clone(),
        vec![fixture.admin.clone(), evaluator.clone()],
    )
    .unwrap();
    let blocker = fixture.store.session().await.unwrap();
    for payload in [Value::Null, json!(["not a business DTO"])] {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            dispatcher.submit(&evaluator, "seed.install", payload),
        )
        .await
        .expect("the role gate must not acquire the held connection")
        .unwrap_err();
        assert!(matches!(error, Error::Forbidden), "{error:?}");
    }
    blocker.commit().await.unwrap();
    let untrusted = Context::new("tenant", "agent", Role::Agent).unwrap();
    assert!(matches!(
        dispatcher
            .submit(&untrusted, "seed.install", Value::Null)
            .await
            .unwrap_err(),
        Error::Forbidden
    ));
    assert_eq!(fixture.job_count().await, 0);
    assert!(fixture.seed_records(&fixture.admin).await.is_empty());
    fixture.store.close().await;
}

/// The real blob/catalog identity: B=L=U=I0, a real v2 Installed record, real
/// published bytes, source edges, and no activation or private effects.
#[tokio::test]
async fn seed_install_uses_real_blob_and_catalog_identity() {
    let fixture = Fixture::new().await;
    let catalog = builtin_seed_catalog().unwrap();
    let entry = &catalog.entries[0];
    assert_eq!(entry.publisher, "rsia.builtin");
    assert_eq!(entry.asset_id, "improver-i0");
    assert_eq!(entry.kind, "improver");
    assert_eq!(hash(BUILTIN_I0_BYTES), entry.content_sha256);
    let input_digest = fixture.input_digest("improver-i0").await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let status = fixture
        .done(
            &dispatcher,
            fixture.payload("first-managed-copy", &input_digest),
        )
        .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "seed_installed");
    assert_eq!(status.owner_actor, "admin");
    assert_eq!(status.owner_role, Role::Admin);
    let install_id = fixture.install_id();
    match seed_result(&status) {
        ManagementSeedResultV2::SeedInstalled {
            install_id: result_id,
            classification,
            baseline_digest,
            local_digest,
            upstream_digest,
            staged_asset_id,
        } => {
            assert_eq!(result_id, &install_id);
            assert_eq!(*classification, ManagementSeedClassification::Unmodified);
            assert_eq!(baseline_digest, &entry.content_sha256);
            assert_eq!(local_digest, &entry.content_sha256);
            assert_eq!(
                upstream_digest.as_deref(),
                Some(entry.content_sha256.as_str())
            );
            assert!(staged_asset_id.is_none());
        }
    }
    let install: E16Envelope<SeedInstallPayloadV2> =
        PersistentSeedStore::read_install(&fixture.admin, &fixture.store, &install_id)
            .await
            .unwrap();
    assert_eq!(install.payload.status, SeedStatus::Installed);
    assert_eq!(install.payload.catalog_version, Some(entry.version));
    assert_eq!(install.payload.publisher, entry.publisher);
    assert_eq!(install.payload.asset_id, entry.asset_id);
    assert_eq!(install.payload.kind, entry.kind);
    assert_eq!(
        install.payload.environment_digest,
        fixture.environment_digest
    );
    assert_eq!(install.payload.compiler_version, fixture.compiler_version);
    assert_eq!(install.source_refs, fixture.sources);
    assert_eq!(install.owner_actor, fixture.admin.actor());
    // The local bytes are never replaced by the install: all three material
    // digests are the registered I0 content.
    let published = fixture
        .store
        .read_blob(&fixture.admin, &fixture.i0_digest, 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(published, BUILTIN_I0_BYTES);
    assert_eq!(hash(&published), entry.content_sha256);
    assert_eq!(
        PersistentSeedStore::read_install(&fixture.admin, &fixture.store, &install_id)
            .await
            .unwrap()
            .payload
            .local_digest,
        entry.content_sha256
    );
    assert_no_listed_effects(&fixture.store, &fixture.admin).await;
    assert_eq!(fixture.job_count().await, 1);
    assert!(fixture.store.verify_audit(&fixture.admin).await.unwrap() > 0);
    fixture.store.close().await;
}

/// Paths, roots, publishers, roles, owners, namespaces, markers, verification
/// carriers, permission booleans, content, merged previews and clocks are not
/// part of the wire; every one is refused before any session.
#[tokio::test]
async fn seed_management_rejects_paths_roles_markers_and_private_carriers() {
    let fixture = Fixture::new().await;
    let input_digest = fixture.input_digest("improver-i0").await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let before = fixture.shape().await;
    for (field, value) in [
        ("path", json!("/tmp/seed")),
        ("root", json!("/tmp/root")),
        ("publisher", json!("attacker.publisher")),
        ("role", json!("admin")),
        ("owner", json!("admin")),
        ("namespace", json!("tenant")),
        ("marker", json!(true)),
        ("local_marked", json!(true)),
        ("verified", json!(true)),
        ("authority", json!("trusted")),
        ("permissions", json!({"write": true})),
        ("content", json!("baseline bytes")),
        ("merged_preview", json!("private merged text")),
        ("created_at", json!(1)),
        ("install_input", json!({"path": "/tmp/seed"})),
    ] {
        let mut payload = fixture.payload("forbidden-fields", &input_digest);
        payload[field] = value;
        let error = dispatcher
            .submit(&fixture.admin, "seed.install", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "invalid management payload"),
            "{field}: {error:?}"
        );
    }
    assert_eq!(
        fixture.shape().await,
        before,
        "forbidden fields wrote nothing"
    );
    // The transient input carrier is strict too: an unknown path field is
    // refused at execution, before any install row is written.
    let mut input: Value = serde_json::from_slice(&fixture.input_bytes("improver-i0")).unwrap();
    input["path"] = json!("/tmp/seed");
    let bad_input = fixture
        .store
        .put_blob_bounded(
            &fixture.admin,
            &serde_json::to_vec(&input).unwrap(),
            64 * 1024,
        )
        .await
        .unwrap();
    let failed = fixture
        .done(
            &dispatcher,
            fixture.payload("input-path-refused", &bad_input),
        )
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("invalid_input"));
    assert!(fixture.seed_records(&fixture.admin).await.is_empty());
    fixture.store.close().await;
}

/// Only a registered builtin catalog entry with its compiled-in content and
/// license can be installed; unregistered selectors and wrong material write
/// nothing.
#[tokio::test]
async fn seed_install_requires_real_builtin_identity_and_license() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let catalog = builtin_seed_catalog().unwrap();
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(catalog.entries[0].license.name, "Source-Available");
    assert_eq!(catalog.entries[0].license.path, "LICENSE");
    let unregistered = fixture.input_digest("org.rsia/not-a-seed").await;
    let failed = fixture
        .done(
            &dispatcher,
            fixture.payload("unregistered-catalog", &unregistered),
        )
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("invalid_input"));
    assert!(fixture.seed_records(&fixture.admin).await.is_empty());
    // A material blob whose bytes are not the catalog content cannot inherit
    // authority from the digest or from the request key.
    let foreign = fixture
        .store
        .put_blob_bounded(&fixture.admin, b"not the registered I0 bytes", 1024 * 1024)
        .await
        .unwrap();
    let input_digest = fixture.input_digest("improver-i0").await;
    let mut payload = fixture.payload("foreign-material", &input_digest);
    payload["baseline_blob_digest"] = json!(foreign);
    let failed = fixture.done(&dispatcher, payload).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("invalid_input"));
    assert!(fixture.seed_records(&fixture.admin).await.is_empty());
    fixture.store.close().await;
}

/// Namespace and owner are authentication facts: another namespace cannot
/// reach the blobs, and another Admin actor of the same namespace cannot
/// replace or re-own the committed install.
#[tokio::test]
async fn seed_management_namespace_and_owner_are_not_spoofable() {
    let fixture = Fixture::new().await;
    let input_digest = fixture.input_digest("improver-i0").await;
    let payload = fixture.payload("owner-install", &input_digest);
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let installed = fixture.done(&dispatcher, payload.clone()).await;
    assert_eq!(installed.state, ManagementJobState::Succeeded);
    let install_id = fixture.install_id();
    let before_record = {
        let mut session = fixture.store.session().await.unwrap();
        let body: Value = session
            .need(&fixture.admin, "artifact", &install_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        body
    };

    // Another namespace: its own blob directory has no such digests.
    let other_namespace = admin("other-tenant", "admin");
    let other_dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![other_namespace.clone()]).unwrap();
    let queued = other_dispatcher
        .submit(&other_namespace, "seed.install", payload.clone())
        .await
        .unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let status = other_dispatcher
                .status_view(&other_namespace, queued.identity().id)
                .await
                .unwrap();
            if status.state == ManagementJobState::Failed {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("cross-namespace refusal reaches a terminal state");
    assert_eq!(
        status.error_code.as_deref(),
        Some("not_found"),
        "{status:?}"
    );
    let mut session = fixture.store.session().await.unwrap();
    let foreign: Vec<Value> = session.list(&other_namespace, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert!(
        foreign
            .iter()
            .all(|body| body["schema_version"] != E16_CURRENT_SEED_INSTALL_SCHEMA)
    );

    // Another Admin actor of the same namespace is a different owner.
    let second = admin("tenant", "admin-two");
    let second_dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![second.clone()]).unwrap();
    let queued = second_dispatcher
        .submit(&second, "seed.install", payload)
        .await
        .unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let status = second_dispatcher
                .status_view(&second, queued.identity().id)
                .await
                .unwrap();
            if status.state == ManagementJobState::Failed {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("foreign owner refusal reaches a terminal state");
    assert_eq!(status.error_code.as_deref(), Some("conflict"), "{status:?}");
    let mut session = fixture.store.session().await.unwrap();
    let bodies: Vec<Value> = session
        .list::<Value>(&fixture.admin, "artifact")
        .await
        .unwrap()
        .into_iter()
        .filter(|body| body["schema_version"] == E16_CURRENT_SEED_INSTALL_SCHEMA)
        .collect();
    let after_record: Value = session
        .need(&fixture.admin, "artifact", &install_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(bodies.len(), 1, "one committed install identity");
    assert_eq!(after_record, before_record);
    assert_eq!(after_record["owner_actor"], "admin");
    fixture.store.close().await;
}

/// The result carries only the committed identifiers, the closed
/// classification and three digests; no baseline/local/upstream content, merged
/// preview, path, root, private seed or fee assertion is ever returned.
#[tokio::test]
async fn seed_management_results_never_return_private_content() {
    let fixture = Fixture::new().await;
    let input_digest = fixture.input_digest("improver-i0").await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let status = fixture
        .done(
            &dispatcher,
            fixture.payload("private-result", &input_digest),
        )
        .await;
    assert_eq!(status.state, ManagementJobState::Succeeded);
    let result = serde_json::to_value(status.result.as_ref().unwrap()).unwrap();
    let object = result.as_object().unwrap();
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "baseline_digest",
            "classification",
            "install_id",
            "local_digest",
            "result",
            "staged_asset_id",
            "upstream_digest",
        ]
    );
    assert_eq!(result["result"], "seed_installed");
    let effects = serde_json::to_value(status.committed_side_effects.as_ref().unwrap()).unwrap();
    let exposed = json!({"result": result, "committed_side_effects": effects});
    let mut keys = Vec::new();
    let mut texts = Vec::new();
    collect_exposed(&exposed, &mut keys, &mut texts);
    for forbidden in [
        "merged_preview",
        "payload",
        "content",
        "path",
        "root",
        "private_seed",
        "cost",
        "fee",
        "refund",
        "local",
        "upstream",
        "baseline",
    ] {
        assert!(
            !keys.iter().any(|key| key == forbidden),
            "forbidden key {forbidden}: {keys:?}"
        );
    }
    // No material excerpt of the compiled-in I0 leaks into the result. The
    // 64-hex digests are content identities, not material, and are exempt from
    // the word scan.
    let excerpt = std::str::from_utf8(&BUILTIN_I0_BYTES[..64]).unwrap();
    for text in &texts {
        if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        assert!(!text.contains(excerpt), "{text}");
        assert!(!text.contains("merged_preview"), "{text}");
    }
    assert_eq!(effects[0]["kind"], "seed_installed");
    assert!(effects[0].get("install_id").is_some());
    assert!(effects[0].get("baseline_digest").is_some());
    assert!(effects[0].get("local_digest").is_some());
    assert!(effects[0].get("path").is_none());
    fixture.store.close().await;
}

/// One request key freezes one intent: a same-key same-wire replay returns the
/// original immutable job and result, changed material under the same key
/// conflicts at submit, and a new key cannot replace the committed first copy.
#[tokio::test]
async fn seed_install_replays_same_key_and_conflicts_on_changed_material() {
    let fixture = Fixture::new().await;
    let input_digest = fixture.input_digest("improver-i0").await;
    let payload = fixture.payload("same-key", &input_digest);
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let installed = fixture.done(&dispatcher, payload.clone()).await;
    assert_eq!(installed.state, ManagementJobState::Succeeded);
    let install_id = fixture.install_id();
    let install_body = {
        let mut session = fixture.store.session().await.unwrap();
        let body: Value = session
            .need(&fixture.admin, "artifact", &install_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        body
    };
    let before = fixture.shape().await;
    let replay = fixture.done(&dispatcher, payload.clone()).await;
    assert_eq!(replay.id, installed.id);
    assert_eq!(
        serde_json::to_value(replay.result.as_ref()).unwrap(),
        serde_json::to_value(installed.result.as_ref()).unwrap()
    );
    assert_eq!(fixture.shape().await, before, "a replay writes nothing");
    assert_eq!(fixture.job_count().await, 1);

    // Same key, different material: the cached intent refuses at submit.
    let other = fixture
        .store
        .put_blob_bounded(&fixture.admin, b"different upstream", 1024 * 1024)
        .await
        .unwrap();
    let mut changed = payload;
    changed["upstream_blob_digest"] = json!(other);
    let error = dispatcher
        .submit(&fixture.admin, "seed.install", changed)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    let after_changed = fixture.shape().await;
    assert_eq!(after_changed["artifacts"], before["artifacts"]);
    assert_eq!(after_changed["jobs"], before["jobs"]);

    // A new key cannot replace the committed first managed copy; the handler
    // refuses on the real existing identity and writes no business row.
    let failed = fixture
        .done(&dispatcher, fixture.payload("second-key", &input_digest))
        .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    let after = fixture.shape().await;
    assert_eq!(
        after["files"], after_changed["files"],
        "install bytes unchanged"
    );
    // The refused job still has its own accepted private input row, but no
    // seed install or staged artifact was added.
    assert_eq!(fixture.seed_records(&fixture.admin).await.len(), 1);
    let after_body = {
        let mut session = fixture.store.session().await.unwrap();
        let body: Value = session
            .need(&fixture.admin, "artifact", &install_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        body
    };
    assert_eq!(after_body, install_body, "the install record is immutable");
    fixture.store.close().await;
}
