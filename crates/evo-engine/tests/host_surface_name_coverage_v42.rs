use evo_core::contract::{CapabilityLevel, HostCapabilities, HostSurfaceManifest, SystemSnapshot};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::release_store::{
    HOST_SURFACE_RECORD_SCHEMA, HostSurfaceRecord, PrepareRunRequest, ReleaseStore,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const REFERENCE_JSON: &str = include_str!("../../../fixtures/reference-host/host_surface.v1.json");
const REFERENCE_DIGEST: &str = "e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1";
const SURFACE_ID: &str = "surface-name-coverage";
const RUN_ID: &str = "new-name-coverage-run";

fn context(role: Role) -> Context {
    Context::new("name-coverage", "trusted-actor", role).unwrap()
}

fn surface() -> HostSurfaceManifest {
    serde_json::from_str(REFERENCE_JSON).unwrap()
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|name| (*name).into()).collect()
}

fn request() -> PrepareRunRequest {
    PrepareRunRequest {
        run_id: RUN_ID.into(),
        profile_id: "reference-profile".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "reference-profile".into(),
            host_id: "reference".into(),
            host_version: "0.1.0".into(),
            model_id: "reference-model".into(),
            tools: vec!["reference-tool".into()],
            mandatory_context_digest: hash(b"mandatory-context"),
        },
        host_surface_id: SURFACE_ID.into(),
        host_capabilities: HostCapabilities {
            available: BTreeSet::new(),
            granted: BTreeSet::new(),
        },
        task_input_digest: hash(b"task-input"),
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    }
}

async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn counts(store: &Store) -> (u64, u64, usize) {
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    let artifacts = session
        .namespace_object_count(&admin, "artifact")
        .await
        .unwrap();
    let runs = session.namespace_object_count(&admin, "run").await.unwrap();
    session.commit().await.unwrap();
    (artifacts, runs, store.verify_audit(&admin).await.unwrap())
}

fn assert_invalid<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == expected),
        "expected Invalid({expected:?}), got {result:?}"
    );
}

async fn assert_registration_rejects(extracted: &[&str], missing: &str) {
    // Every case owns a new database, so a successful baseline write is observable.
    let (_dir, store) = database().await;
    let admin = context(Role::Admin);
    let manifest = surface();
    assert_eq!(fingerprint(&manifest).unwrap(), REFERENCE_DIGEST);
    let extracted = names(extracted);
    assert_eq!(counts(&store).await, (0, 0, 0));
    let result = ReleaseStore::register_host_surface(
        &admin,
        &store,
        SURFACE_ID,
        manifest.clone(),
        extracted.clone(),
    )
    .await;
    let after = counts(&store).await;
    let mut session = store.session().await.unwrap();
    let stored: Option<Value> = session.get(&admin, "artifact", SURFACE_ID).await.unwrap();
    session.commit().await.unwrap();
    println!(
        "registration manifest_json={} manifest_digest={REFERENCE_DIGEST} extracted_json={} result={result:?} after_artifacts_runs_audit={after:?} stored_json={}",
        serde_json::to_string(&manifest).unwrap(),
        serde_json::to_string(&extracted).unwrap(),
        serde_json::to_string(&stored).unwrap(),
    );
    assert_invalid(
        result,
        &format!("host surface field {missing} is missing from extraction"),
    );
    assert_eq!(after, (0, 0, 0));
    assert!(stored.is_none());
}

async fn seed_legacy_fixture(
    store: &Store,
    manifest: &HostSurfaceManifest,
    extracted: &[String],
) -> Value {
    // This raw old-shape fixture models a pre-fix persisted record. It deliberately
    // uses storage, not a claim that current registration accepts missing names.
    let raw = json!({
        "id": SURFACE_ID,
        "schema_version": HOST_SURFACE_RECORD_SCHEMA,
        "manifest": manifest,
        "manifest_digest": fingerprint(manifest).unwrap(),
        "extracted": extracted,
    });
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", SURFACE_ID, admin.actor(), &raw)
        .await
        .unwrap();
    session
        .audit(&admin, "fixture.seed_legacy_host_surface", SURFACE_ID)
        .await
        .unwrap();
    session.commit().await.unwrap();
    raw
}

async fn assert_fixture_unchanged(store: &Store, raw: &Value) {
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .raw_object(&admin, "artifact", SURFACE_ID)
            .await
            .unwrap(),
        *raw
    );
    session.commit().await.unwrap();
}

async fn assert_new_run_rejects_legacy_fixture(extracted: &[&str], missing: &str) {
    let (dir, store) = database().await;
    let manifest = surface();
    let raw = seed_legacy_fixture(&store, &manifest, &names(extracted)).await;
    assert_eq!(counts(&store).await, (1, 0, 1));
    store.close().await;
    // Reopen the on-disk database before the production consumer reads the record.
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    let record: HostSurfaceRecord = session.need(&admin, "artifact", SURFACE_ID).await.unwrap();
    assert_eq!(record.manifest_digest, REFERENCE_DIGEST);
    assert_eq!(fingerprint(&record.manifest).unwrap(), REFERENCE_DIGEST);
    session.commit().await.unwrap();
    let result = ReleaseStore::prepare_run(&context(Role::Host), &store, request()).await;
    let after = counts(&store).await;
    let mut session = store.session().await.unwrap();
    let snapshot: Option<Value> = session
        .get(&admin, "artifact", &format!("run-snapshot-{RUN_ID}"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_fixture_unchanged(&store, &raw).await;
    println!(
        "legacy_fixture_json={} manifest_digest={REFERENCE_DIGEST} prepare_result={result:?} after_artifacts_runs_audit={after:?} snapshot_json={}",
        serde_json::to_string(&raw).unwrap(),
        serde_json::to_string(&snapshot).unwrap(),
    );
    assert_invalid(
        result,
        &format!("host surface field {missing} is missing from extraction"),
    );
    assert_eq!(after, (1, 0, 1));
    assert!(snapshot.is_none());
}

#[tokio::test]
async fn registration_rejects_missing_supported_name_before_writing() {
    assert_registration_rejects(&["model", "theme"], "instruction").await;
}

#[tokio::test]
async fn registration_rejects_missing_runtime_owned_name_before_writing() {
    assert_registration_rejects(&["instruction", "theme"], "model").await;
}

#[tokio::test]
async fn registration_rejects_missing_unsupported_name_before_writing() {
    assert_registration_rejects(&["instruction", "model"], "theme").await;
}

#[tokio::test]
async fn registration_rejects_multiple_deletions_before_writing() {
    assert_registration_rejects(&["model"], "instruction").await;
}

#[tokio::test]
async fn registration_rejects_nonempty_truncation_before_writing() {
    assert_registration_rejects(&["instruction"], "model").await;
}

#[tokio::test]
async fn registration_rejects_rename_to_classified_name_before_writing() {
    assert_registration_rejects(&["model", "model", "theme"], "instruction").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_missing_supported_name_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["model", "theme"], "instruction").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_missing_runtime_owned_name_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["instruction", "theme"], "model").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_missing_unsupported_name_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["instruction", "model"], "theme").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_multiple_deletions_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["model"], "instruction").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_nonempty_truncation_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["instruction"], "model").await;
}

#[tokio::test]
async fn new_run_rejects_legacy_rename_to_classified_name_without_writing() {
    assert_new_run_rejects_legacy_fixture(&["model", "model", "theme"], "instruction").await;
}

#[tokio::test]
async fn complete_registration_is_idempotent_and_changed_content_conflicts() {
    let (_dir, store) = database().await;
    let admin = context(Role::Admin);
    let manifest = surface();
    let extracted = names(&["theme", "instruction", "model"]);
    let first = ReleaseStore::register_host_surface(
        &admin,
        &store,
        SURFACE_ID,
        manifest.clone(),
        extracted.clone(),
    )
    .await
    .unwrap();
    assert_eq!(first.manifest_digest, REFERENCE_DIGEST);
    assert_eq!(
        serde_json::to_string(&first.manifest).unwrap(),
        serde_json::to_string(&manifest).unwrap()
    );
    let second = ReleaseStore::register_host_surface(
        &admin,
        &store,
        SURFACE_ID,
        manifest.clone(),
        extracted,
    )
    .await
    .unwrap();
    assert_eq!(fingerprint(&first).unwrap(), fingerprint(&second).unwrap());
    assert_eq!(counts(&store).await, (1, 0, 1));
    let mut changed = manifest.clone();
    changed.items[0].reason = "changed mapping explanation".into();
    for (manifest, extracted) in [
        (changed, names(&["theme", "instruction", "model"])),
        (manifest, names(&["instruction", "model", "theme"])),
    ] {
        let result =
            ReleaseStore::register_host_surface(&admin, &store, SURFACE_ID, manifest, extracted)
                .await;
        assert!(
            matches!(result, Err(Error::Conflict(ref message)) if message == "host surface id reused with different content")
        );
        assert_eq!(counts(&store).await, (1, 0, 1));
    }
}

#[tokio::test]
async fn complete_duplicate_names_keep_registration_behavior() {
    let (_dir, store) = database().await;
    let mut manifest = surface();
    manifest.items.push(manifest.items[0].clone());
    let expected_digest = fingerprint(&manifest).unwrap();
    let record = ReleaseStore::register_host_surface(
        &context(Role::Admin),
        &store,
        SURFACE_ID,
        manifest,
        names(&["instruction", "model", "theme", "model"]),
    )
    .await
    .unwrap();
    assert_eq!(record.manifest_digest, expected_digest);
    assert_eq!(counts(&store).await, (1, 0, 1));
}

#[tokio::test]
async fn complete_legacy_record_read_from_disk_allows_a_new_run() {
    let (dir, store) = database().await;
    let raw = seed_legacy_fixture(
        &store,
        &surface(),
        &names(&["theme", "instruction", "model"]),
    )
    .await;
    store.close().await;
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let snapshot = ReleaseStore::prepare_run(&context(Role::Host), &store, request())
        .await
        .unwrap();
    assert_eq!(snapshot.host_surface_digest, REFERENCE_DIGEST);
    assert!(snapshot.projection.instructions.is_empty());
    assert_eq!(snapshot.projection.tools, vec!["reference-tool"]);
    assert_eq!(counts(&store).await, (2, 0, 2));
    assert_fixture_unchanged(&store, &raw).await;
}

#[tokio::test]
async fn role_errors_precede_missing_names_for_registration_and_prepare() {
    let (_dir, store) = database().await;
    for role in [Role::Agent, Role::Host, Role::Evaluator, Role::Worker] {
        let result = ReleaseStore::register_host_surface(
            &context(role),
            &store,
            SURFACE_ID,
            surface(),
            names(&["model", "theme"]),
        )
        .await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{role:?}: {result:?}"
        );
        assert_eq!(counts(&store).await, (0, 0, 0));
    }
    let raw = seed_legacy_fixture(&store, &surface(), &names(&["model", "theme"])).await;
    for role in [Role::Agent, Role::Admin, Role::Evaluator, Role::Worker] {
        let result = ReleaseStore::prepare_run(&context(role), &store, request()).await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{role:?}: {result:?}"
        );
        assert_eq!(counts(&store).await, (1, 0, 1));
        assert_fixture_unchanged(&store, &raw).await;
    }
}

fn earlier_validation_cases() -> Vec<(HostSurfaceManifest, Vec<String>, &'static str)> {
    let mut cases = Vec::new();
    let mut manifest = surface();
    manifest.schema_version = "unknown".into();
    cases.push((
        manifest,
        names(&["model", "theme"]),
        "unsupported host surface schema",
    ));
    let mut manifest = surface();
    manifest.items[0].consumer = None;
    cases.push((
        manifest,
        names(&["model", "theme"]),
        "supported item needs a consumer",
    ));
    let mut manifest = surface();
    manifest.items[0].mapped_field = Some("unknown.field".into());
    cases.push((
        manifest,
        names(&["model", "theme"]),
        "no consumer for field unknown.field",
    ));
    cases.push((
        surface(),
        vec![],
        "empty extraction is not a successful cover",
    ));
    cases.push((
        surface(),
        names(&["new_name"]),
        "extracted field new_name is unclassified",
    ));
    let mut manifest = surface();
    manifest.items[1].mapped_field = Some("skill.content".into());
    cases.push((
        manifest,
        names(&["instruction", "theme"]),
        "runtime_owned/unsupported items cannot map candidate fields",
    ));
    let mut manifest = surface();
    manifest.items[1].reason.clear();
    cases.push((
        manifest,
        names(&["instruction", "theme"]),
        "reason: expected nonempty text <= 512 bytes",
    ));
    cases
}

#[tokio::test]
async fn registration_keeps_claude_and_existing_validation_error_priority() {
    let mut cases = earlier_validation_cases();
    for (host, version) in [
        ("Claude-Code", "0.1.0"),
        ("claude-code-mcp-tool-only:v1", "0.1.0"),
        ("vendor-claude-bridge", "0.1.0"),
        ("reference", "Unverified-version"),
    ] {
        let mut manifest = surface();
        manifest.host = host.into();
        manifest.host_version = version.into();
        manifest.schema_version = "unknown".into();
        cases.push((
            manifest,
            names(&["model", "theme"]),
            "unverified host surface candidate cannot be registered as supported",
        ));
    }
    for (manifest, extracted, expected) in cases {
        let (_dir, store) = database().await;
        let result = ReleaseStore::register_host_surface(
            &context(Role::Admin),
            &store,
            SURFACE_ID,
            manifest,
            extracted,
        )
        .await;
        assert_invalid(result, expected);
        assert_eq!(counts(&store).await, (0, 0, 0));
    }
}

#[tokio::test]
async fn legacy_record_revalidation_keeps_existing_error_priority() {
    for (manifest, extracted, expected) in earlier_validation_cases() {
        let (dir, store) = database().await;
        let raw = seed_legacy_fixture(&store, &manifest, &extracted).await;
        store.close().await;
        let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
            .await
            .unwrap();
        let result = ReleaseStore::prepare_run(&context(Role::Host), &store, request()).await;
        assert_invalid(result, expected);
        assert_eq!(counts(&store).await, (1, 0, 1));
        assert_fixture_unchanged(&store, &raw).await;
    }
}
