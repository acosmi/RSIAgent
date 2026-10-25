use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceDefaultV2,
    HostSurfaceManifest, HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
    SystemSnapshot, extract_reviewed_host_source,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::release_store::{
    HOST_SURFACE_RECORD_SCHEMA, HOST_SURFACE_RECORD_SCHEMA_V2, HostSurfaceRecord,
    HostSurfaceRecordV2, PrepareRunRequest, RUN_APPLICATION_SCHEMA_V2, ReleaseStore,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
    // v1's exact old name diagnostic remains a readonly contract. It cannot
    // authorize a new current run even if its old manifest is otherwise valid.
    assert_invalid(
        record
            .manifest
            .validate_against_extraction(&record.extracted),
        &format!("host surface field {missing} is missing from extraction"),
    );
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
    assert_eq!(counts(&store).await, (1, 0, 2));
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
        assert_eq!(counts(&store).await, (1, 0, 2));
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
    assert_eq!(counts(&store).await, (1, 0, 2));
}

// Actual frozen parent-producer bytes. Only objects and their original audit
// chain are needed here; migration/FTS internals are created by Store::open.
// Producer base: 800eafc4ccaf39d5e4f3fb1b7891f9c142e6b482.
const ACTUAL_LEGACY_HOST: &str = r#"{"id":"surface-name-coverage","schema_version":"rsia.host_surface_record.v1","manifest":{"schema_version":"rsia.host_surface.v1","host":"reference","host_version":"0.1.0","adapter_version":"0.1.0","source_digest":"reference_host_surface_v1","items":[{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"compiler","reason":"run-level instruction projection"},{"name":"model","coverage":"runtime_owned","mapped_field":null,"consumer":null,"reason":"model identity is host-declared"},{"name":"theme","coverage":"unsupported","mapped_field":null,"consumer":null,"reason":"UI theme is out of scope"}]},"manifest_digest":"e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1","extracted":["theme","instruction","model"]}"#;
const ACTUAL_LEGACY_RUN: &str = r#"{"id":"run-snapshot-new-name-coverage-run","schema_version":"rsia.run_application.v1","run_id":"new-name-coverage-run","profile_id":"reference-profile","pointer_epoch":0,"release_id":null,"bundle_digest":null,"environment_digest":"52fcaa74f196f06a1c10316eb6a8296dc4fa6626aa15e7fab8e5ebc56ecdf8b2","system_snapshot":{"schema_version":"rsia.system_snapshot.v2","profile_id":"reference-profile","host_id":"reference","host_version":"0.1.0","model_id":"reference-model","tools":["reference-tool"],"mandatory_context_digest":"49b8c8f4776661d7b6def910c9ced5658d1490cecdf61d2ae0a64581c287718f"},"host_surface_id":"surface-name-coverage","host_surface_digest":"e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1","host_capabilities_digest":"192a331b817f68dbbab671a8acefa7a286a4dc3bc49c37e2990cac2722d0b691","projection":{"bundle_digest":"","instructions":[],"tools":["reference-tool"]},"request_material":{"mandatory_context_digest":"49b8c8f4776661d7b6def910c9ced5658d1490cecdf61d2ae0a64581c287718f","task_input_digest":"6352c6328ea376ae6e4568db2089e359581c55ea55c4c8306d0097ba1def2478","instructions":[],"tools":["reference-tool"]},"request_digest":"da721de1a55f91b8ab731325fe1fe9a498b92e76920af62b71f3395a8d51fe44","capability_level":"tool_only","evolution_enabled":true}"#;
const ACTUAL_LEGACY_AUDIT: [(i64, &str, &str, &str, &str); 4] = [
    (
        1,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"e7ea337a9619e616976969e4ed3e56415038852465dccf019a5de0eca29d1f8f","operation":"formal.admin.content.v1.f9e65bfa18da6cccfa157fb8caf97aa67cf67ff6cf96906026dc7bd9b6bfa160","role":"admin","timestamp":1792950136}"#,
        "",
        "3fbdc2bd80fb9fc82c745f6af50cf43190f336a94a0b65b54aaefc5e8e82e7f5",
    ),
    (
        2,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"surface-name-coverage","operation":"host_surface.register","role":"admin","timestamp":1792950136}"#,
        "3fbdc2bd80fb9fc82c745f6af50cf43190f336a94a0b65b54aaefc5e8e82e7f5",
        "d009a292c7be7ff26c7751d2bd4e86a595c0d5479f198c58033f744fc2e2beb9",
    ),
    (
        3,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"1afc3f9134c51ca8906394f4db5d15035ab7c77212ef30c3f1d85f34d21c2c7a","operation":"formal.host.content.v1.ff85845a29bd71ceaa8428eeaa85adc21748b5957d19b021cf84f115b6abd99a","role":"host","timestamp":1792950136}"#,
        "d009a292c7be7ff26c7751d2bd4e86a595c0d5479f198c58033f744fc2e2beb9",
        "dfa2831605b3e40469243a414e682c650c7b2e6f7e5aa87de8550fd5496ddf4a",
    ),
    (
        4,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"run-snapshot-new-name-coverage-run","operation":"release.prepare_run","role":"host","timestamp":1792950136}"#,
        "dfa2831605b3e40469243a414e682c650c7b2e6f7e5aa87de8550fd5496ddf4a",
        "b5cee4ddbf61a7510d40acd0f221d8535e777fb593c1de89f299a2b204cf73a5",
    ),
];

#[tokio::test]
async fn complete_legacy_run_snapshot_read_from_disk_keeps_original_projection() {
    let (dir, store) = database().await;
    store.close().await;
    let path = dir.path().join("name-coverage.sqlite3");
    let script = r#"
import json,sqlite3,sys
payload=json.loads(sys.argv[2])
con=sqlite3.connect(sys.argv[1])
for body in payload['bodies']:
    identity=json.loads(body)['id']
    con.execute("INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('name-coverage','artifact',?,'trusted-actor',?,1)",(identity,body))
con.executemany('INSERT INTO audit(seq,namespace,payload,previous_hash,digest) VALUES(?,?,?,?,?)',payload['audit'])
con.commit()
assert con.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
con.close()
"#;
    let payload =
        json!({"bodies":[ACTUAL_LEGACY_HOST,ACTUAL_LEGACY_RUN],"audit":ACTUAL_LEGACY_AUDIT});
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&path)
        .arg(serde_json::to_string(&payload).unwrap())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let store = Store::open(&path).await.unwrap();
    let snapshot = ReleaseStore::read_run_snapshot(&context(Role::Host), &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&snapshot).unwrap(), ACTUAL_LEGACY_RUN);
    assert_eq!(snapshot.host_surface_digest, REFERENCE_DIGEST);
    assert!(snapshot.projection.instructions.is_empty());
    assert_eq!(snapshot.projection.tools, vec!["reference-tool"]);
    assert_eq!(counts(&store).await, (2, 0, 4));
    let raw: Value = serde_json::from_str(ACTUAL_LEGACY_HOST).unwrap();
    assert_fixture_unchanged(&store, &raw).await;
    assert_eq!(
        hash(ACTUAL_LEGACY_HOST.as_bytes()),
        "b59766e4a6e200bb130882ebee4ac81f5cf3bd5c1ba9aeeed8904b48e729d9a6"
    );
    assert_eq!(
        hash(ACTUAL_LEGACY_RUN.as_bytes()),
        "6c351759d00107ad2f4f061ebe14114cdee47568fd4c074425cd7f399de33a8b"
    );
    let before = current_image(&store, dir.path()).await;
    let mut new_request = request();
    new_request.run_id = "new-use-of-frozen-v1".into();
    assert_invalid(
        ReleaseStore::prepare_run(&context(Role::Host), &store, new_request).await,
        "current prepare requires a source-bound host surface v2",
    );
    assert_eq!(current_image(&store, dir.path()).await, before);
    assert_fixture_unchanged(&store, &raw).await;
    store.close().await;
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
        // Preserve the exact old validation order without treating v1 as new input.
        let admin = context(Role::Admin);
        let mut session = store.session().await.unwrap();
        let record: HostSurfaceRecord = session.need(&admin, "artifact", SURFACE_ID).await.unwrap();
        session.commit().await.unwrap();
        assert_invalid(
            record
                .manifest
                .validate_against_extraction(&record.extracted),
            expected,
        );
        let result = ReleaseStore::prepare_run(&context(Role::Host), &store, request()).await;
        assert_invalid(result, expected);
        assert_eq!(counts(&store).await, (1, 0, 1));
        assert_fixture_unchanged(&store, &raw).await;
    }
}

// Current fixtures below are local reviewed software data. They prove source,
// extraction and persistence consistency; they provide no native-host, smoke,
// Formal, paid, attached or used authority. Every declared ToolOnly field is
// runtime_owned or unsupported, including the old instruction slot.
fn current_source(depth: usize, instruction: &str) -> ReviewedHostSourceEnvelopeV1 {
    let instruction = serde_json::to_string(instruction).unwrap();
    let object = format!(
        "{{\"decimal\":0.1,\"fraction\":1.5,\"inside\":null,\"label\":\"读写\",\"nested\":{}null{}}}",
        "[".repeat(depth),
        "]".repeat(depth),
    );
    let original_utf8 = [
        "\r\n{\"schema_version\":\"rsia.host_reviewed_declarations.v1\",\"host\":\"reference\",\"host_version\":\"0.1.0\",\"declarations\":[",
        "{\"name\":\"instruction\",\"value_type\":\"string\",\"enum_values\":[],\"default\":{\"kind\":\"value\",\"value\":",
        instruction.as_str(),
        "}},{\"name\":\"model\",\"value_type\":\"string\",\"enum_values\":[],\"default\":{\"kind\":\"none\"}},",
        "{\"name\":\"theme\",\"value_type\":\"object\",\"enum_values\":[],\"default\":{\"kind\":\"value\",\"value\":",
        object.as_str(),
        "}}]}\r\n",
    ].concat();
    ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8,
    }
}

fn current_registration() -> HostSurfaceRegistrationV2 {
    HostSurfaceRegistrationV2 {
        adapter_version: "0.2.0".into(),
        classifications: [
            (
                "instruction",
                SurfaceCoverage::Unsupported,
                "ToolOnly has no native instruction attachment",
            ),
            (
                "model",
                SurfaceCoverage::RuntimeOwned,
                "model identity remains host-owned",
            ),
            (
                "theme",
                SurfaceCoverage::Unsupported,
                "UI theme is outside the software projection",
            ),
        ]
        .into_iter()
        .map(|(name, coverage, reason)| HostSurfaceClassificationV2 {
            name: name.into(),
            coverage,
            mapped_field: None,
            consumer: None,
            reason: reason.into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        })
        .collect(),
        retired: Vec::new(),
    }
}

fn object_default(depth: usize) -> Value {
    let mut nested = Value::Null;
    for _ in 0..depth {
        nested = Value::Array(vec![nested]);
    }
    json!({"decimal":0.1,"fraction":1.5,"inside":null,"label":"读写","nested":nested})
}

fn assert_current_defaults(record: &HostSurfaceRecordV2, depth: usize) {
    assert_eq!(record.extracted, names(&["instruction", "model", "theme"]));
    assert_eq!(record.extracted_typed.len(), 3);
    assert_eq!(record.manifest.items.len(), 3);
    let expected = object_default(depth);
    for default in [
        &record.extracted_typed[2].default,
        &record.manifest.items[2].default,
    ] {
        let HostSurfaceDefaultV2::Value { value } = default else {
            panic!("the real typed object default was not preserved: {default:?}");
        };
        assert_eq!(value, &expected);
        assert_eq!(value["decimal"], json!(0.1));
        assert_eq!(value["fraction"], json!(1.5));
        assert_eq!(value["decimal"].as_f64(), Some(0.1));
        assert_eq!(value["fraction"].as_f64(), Some(1.5));
        assert_eq!(value["inside"], Value::Null);
        assert_eq!(value["label"], "读写");
    }
    assert!(record.manifest.items.iter().all(|item| {
        matches!(
            item.coverage,
            SurfaceCoverage::RuntimeOwned | SurfaceCoverage::Unsupported
        ) && item.mapped_field.is_none()
            && item.capability.is_none()
    }));
}

type ObjectDependencyImage = (String, String, Vec<(String, String)>);

#[derive(Debug, PartialEq, Eq)]
struct CurrentImage {
    artifacts: Vec<Value>,
    runs: Vec<Value>,
    edges: Vec<ObjectDependencyImage>,
    counts: (u64, u64, usize),
    facts: evo_storage::lifecycle::ControlPlaneFacts,
    files: BTreeMap<String, (bool, Vec<u8>)>,
}

// This fixture owns only the named database family: no second Store, SQL
// connection, cleanup or global serialization is introduced. Unexpected files,
// directories and symlinks fail the observation instead of being ignored.
async fn current_image(store: &Store, dir: &Path) -> CurrentImage {
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    let runs: Vec<Value> = session.list(&admin, "run").await.unwrap();
    let mut edges = Vec::new();
    for (kind, values) in [("artifact", &artifacts), ("run", &runs)] {
        for body in values {
            let id = body["id"].as_str().expect("fixture object id");
            edges.push((
                kind.into(),
                id.into(),
                session
                    .outgoing_edges_bounded(&admin, kind, id, 10_000)
                    .await
                    .unwrap(),
            ));
        }
    }
    session.commit().await.unwrap();
    let counts = counts(store).await;
    let facts =
        evo_storage::lifecycle::read_control_plane_facts(&dir.join("name-coverage.sqlite3"))
            .await
            .unwrap();
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        assert!(
            matches!(
                name.as_str(),
                "name-coverage.sqlite3" | "name-coverage.sqlite3-wal" | "name-coverage.sqlite3-shm"
            ),
            "unexpected fixture path: {name}"
        );
        let metadata = std::fs::symlink_metadata(entry.path()).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        files.insert(
            name,
            (
                metadata.permissions().readonly(),
                std::fs::read(entry.path()).unwrap(),
            ),
        );
    }
    assert!(files.contains_key("name-coverage.sqlite3"));
    CurrentImage {
        artifacts,
        runs,
        edges,
        counts,
        facts,
        files,
    }
}

fn assert_conflict<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Conflict(ref message)) if message == expected),
        "expected Conflict({expected:?}), got {result:?}"
    );
}

async fn current_fixture(depth: usize) -> (tempfile::TempDir, Store, HostSurfaceRecordV2) {
    let (dir, store) = database().await;
    let admin = context(Role::Admin);
    let source = current_source(depth, "baseline");
    let record = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        SURFACE_ID,
        source.clone(),
        current_registration(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(counts(&store).await, (1, 0, 2));
    assert_eq!(record.schema_version, HOST_SURFACE_RECORD_SCHEMA_V2);
    assert_eq!(
        record.source.original_utf8.as_bytes(),
        source.original_utf8.as_bytes()
    );
    assert!(record.source.original_utf8.starts_with("\r\n"));
    assert!(record.source.original_utf8.ends_with("\r\n"));
    assert_eq!(
        record.manifest.source_digest,
        hash(source.original_utf8.as_bytes())
    );
    assert_eq!(
        record.manifest.source_envelope_digest,
        fingerprint(&source).unwrap()
    );
    assert_eq!(
        record.manifest_digest,
        fingerprint(&record.manifest).unwrap()
    );
    assert_eq!(
        record.upgrade_diff.added,
        names(&["instruction", "model", "theme"])
    );
    assert_current_defaults(&record, depth);
    let wire = serde_json::to_string(&record).unwrap();
    store.close().await;
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let loaded: HostSurfaceRecordV2 = session
        .get(&admin, "artifact", SURFACE_ID)
        .await
        .unwrap()
        .expect("registered current surface must persist");
    let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(serde_json::to_string(&loaded).unwrap(), wire);
    assert_eq!(serde_json::to_string(&listed[0]).unwrap(), wire);
    assert_current_defaults(&loaded, depth);
    assert_current_defaults(&listed[0], depth);
    session.commit().await.unwrap();
    let host = context(Role::Host);
    let prepared = ReleaseStore::prepare_run(&host, &store, request())
        .await
        .unwrap();
    let current = prepared
        .current_v2()
        .expect("current producer must emit v2");
    assert_eq!(current.schema_version, RUN_APPLICATION_SCHEMA_V2);
    assert_eq!(current.host_surface_digest, record.manifest_digest);
    assert_eq!(
        fingerprint(&current.host_capabilities).unwrap(),
        fingerprint(&request().host_capabilities).unwrap()
    );
    assert_eq!(
        current.host_capabilities_digest,
        fingerprint(&current.host_capabilities).unwrap()
    );
    assert!(current.host_capabilities.available.is_empty());
    assert!(current.host_capabilities.granted.is_empty());
    assert!(current.projection.instructions.is_empty());
    assert_eq!(current.projection.tools, vec!["reference-tool"]);
    assert_eq!(current.capability_level, CapabilityLevel::ToolOnly);
    assert!(current.release_id.is_none() && current.bundle_digest.is_none());
    let prepared_wire = serde_json::to_value(&prepared).unwrap();
    assert_eq!(prepared_wire["schema_version"], RUN_APPLICATION_SCHEMA_V2);
    assert_eq!(
        prepared_wire["host_capabilities"],
        json!({"available":[],"granted":[]})
    );
    let mut session = store.session().await.unwrap();
    let persisted: Value = session
        .get(&admin, "artifact", &format!("run-snapshot-{RUN_ID}"))
        .await
        .unwrap()
        .expect("prepared current snapshot must persist");
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0], serde_json::to_value(&loaded).unwrap());
    assert_eq!(artifacts[1], prepared_wire);
    assert_eq!(persisted, prepared_wire);
    session.commit().await.unwrap();
    let loaded_run = ReleaseStore::read_run_snapshot(&host, &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&loaded_run).unwrap(), prepared_wire);
    let live = ReleaseStore::validate_run_snapshot_live(&host, &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&live).unwrap(), prepared_wire);
    assert_eq!(counts(&store).await, (2, 0, 4));
    (dir, store, loaded)
}

#[tokio::test]
async fn reviewed_current_shallow_and_deep200_survive_real_writer_reopen_get_list_prepare_live() {
    for depth in [0, 200] {
        let (dir, store, record) = current_fixture(depth).await;
        let prior = current_image(&store, dir.path()).await;
        let repeat = ReleaseStore::register_host_surface_from_source(
            &context(Role::Admin),
            &store,
            SURFACE_ID,
            record.source.clone(),
            current_registration(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(fingerprint(&repeat).unwrap(), fingerprint(&record).unwrap());
        let replay = ReleaseStore::prepare_run(&context(Role::Host), &store, request())
            .await
            .unwrap();
        assert_eq!(replay.host_surface_digest, record.manifest_digest);
        assert_eq!(current_image(&store, dir.path()).await, prior);
        assert_eq!(counts(&store).await, (2, 0, 4));
        store.close().await;
    }
}

#[tokio::test]
async fn current_same_id_change_and_run_frozen_input_conflicts_are_zero_write() {
    let (dir, store, record) = current_fixture(0).await;
    for changed in [
        current_source(0, "changed legitimate default"),
        current_source(1, "baseline"),
    ] {
        // Both originals are independently valid; rejection is immutable id
        // reuse, not malformed source/default or wrong-role preemption.
        extract_reviewed_host_source(&changed).unwrap();
        let prior = current_image(&store, dir.path()).await;
        assert_conflict(
            ReleaseStore::register_host_surface_from_source(
                &context(Role::Admin),
                &store,
                SURFACE_ID,
                changed,
                current_registration(),
                None,
            )
            .await,
            "host surface id reused with different content",
        );
        assert_eq!(current_image(&store, dir.path()).await, prior);
    }
    let mut changed = request();
    changed
        .host_capabilities
        .available
        .insert("software-read".into());
    let prior = current_image(&store, dir.path()).await;
    assert_conflict(
        ReleaseStore::prepare_run(&context(Role::Host), &store, changed).await,
        "run id reused with different frozen inputs",
    );
    assert_eq!(current_image(&store, dir.path()).await, prior);
    assert_eq!(counts(&store).await, (2, 0, 4));
    assert_eq!(
        record.manifest.source_digest,
        hash(record.source.original_utf8.as_bytes())
    );
    store.close().await;
}

#[tokio::test]
async fn current_role_and_namespace_guards_do_not_write_over_a_healthy_baseline() {
    let (dir, store, record) = current_fixture(0).await;
    for role in [Role::Agent, Role::Host, Role::Evaluator, Role::Worker] {
        let prior = current_image(&store, dir.path()).await;
        let result = ReleaseStore::register_host_surface_from_source(
            &context(role),
            &store,
            SURFACE_ID,
            record.source.clone(),
            current_registration(),
            None,
        )
        .await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{role:?}: {result:?}"
        );
        assert_eq!(current_image(&store, dir.path()).await, prior);
    }
    for role in [Role::Agent, Role::Admin, Role::Evaluator, Role::Worker] {
        let prior = current_image(&store, dir.path()).await;
        let result = ReleaseStore::prepare_run(&context(role), &store, request()).await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{role:?}: {result:?}"
        );
        assert_eq!(current_image(&store, dir.path()).await, prior);
    }
    for role in [Role::Agent, Role::Evaluator, Role::Worker] {
        let prior = current_image(&store, dir.path()).await;
        let result = ReleaseStore::validate_run_snapshot_live(&context(role), &store, RUN_ID).await;
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{role:?}: {result:?}"
        );
        assert_eq!(current_image(&store, dir.path()).await, prior);
    }
    let other = Context::new("other-name-coverage", "trusted-actor", Role::Host).unwrap();
    let prior = current_image(&store, dir.path()).await;
    let result = ReleaseStore::prepare_run(&other, &store, request()).await;
    assert!(matches!(result, Err(Error::NotFound)), "{result:?}");
    assert_eq!(current_image(&store, dir.path()).await, prior);
    assert_eq!(counts(&store).await, (2, 0, 4));
    store.close().await;
}

#[tokio::test]
async fn current_source_typed_identity_and_authentication_faults_hit_their_exact_guards() {
    // The object CHECK rejects a body id that differs from the object primary
    // key. This exercises the public writer boundary, not a reader alias or
    // SQLite rowid control; the healthy deep record remains persisted.
    let (dir, store, mut record) = current_fixture(200).await;
    let prior = current_image(&store, dir.path()).await;
    record.id = "alias-with-no-matching-row".into();
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    let result = session
        .put(&admin, "artifact", SURFACE_ID, admin.actor(), &record)
        .await;
    assert!(
        matches!(result, Err(Error::Internal)),
        "body id/object key mismatch: {result:?}"
    );
    session.commit().await.unwrap();
    assert_eq!(
        current_image(&store, dir.path()).await,
        prior,
        "rejected body id mismatch wrote"
    );
    ReleaseStore::validate_run_snapshot_live(&context(Role::Host), &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(
        current_image(&store, dir.path()).await,
        prior,
        "healthy live validation wrote"
    );
    store.close().await;

    for fault in [
        "source",
        "typed_default",
        "manifest_digest",
        "witness",
        "schema",
        "kind",
    ] {
        // Unknown schemas/kinds intentionally use the shallow baseline so an
        // old serde depth error cannot preempt their exact isolation guard.
        let depth = if matches!(fault, "schema" | "kind") {
            0
        } else {
            200
        };
        let (dir, store, mut record) = current_fixture(depth).await;
        let expected = match fault {
            "source" => {
                let changed = current_source(depth, "different valid source value");
                extract_reviewed_host_source(&changed).unwrap();
                assert_ne!(
                    hash(changed.original_utf8.as_bytes()),
                    record.manifest.source_digest
                );
                record.source = changed;
                "invalid reviewed host source/default"
            }
            "typed_default" => {
                record.extracted_typed[0].default = HostSurfaceDefaultV2::Value {
                    value: json!("wrong typed extraction"),
                };
                "invalid reviewed host source/default"
            }
            "manifest_digest" => {
                record.manifest_digest = hash(b"single deliberately wrong stored manifest digest");
                "current host surface identity/source differs"
            }
            "witness" => {
                // A locally valid classification change with the real digest
                // cannot reuse the previous Admin content authorization.
                record.manifest.adapter_version = "0.3.0-unattested".into();
                record.manifest_digest = fingerprint(&record.manifest).unwrap();
                evo_engine::release_store::validate_host_record_v2_source(&record).unwrap();
                "missing authenticated content authorization; quarantine"
            }
            "schema" => {
                record.schema_version = "rsia.host_surface_record.v3".into();
                "current prepare requires a source-bound host surface v2"
            }
            "kind" => "",
            _ => unreachable!(),
        };
        // Explicit fixture corruption after the genuine writer/reader/live
        // baseline. These raw puts neither mint nor impersonate authority.
        let admin = context(Role::Admin);
        let mut session = store.session().await.unwrap();
        if fault == "kind" {
            session
                .delete(&admin, "artifact", SURFACE_ID)
                .await
                .unwrap();
            session
                .put(&admin, "run", SURFACE_ID, admin.actor(), &record)
                .await
                .unwrap();
        } else {
            session
                .put(&admin, "artifact", SURFACE_ID, admin.actor(), &record)
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let prior = current_image(&store, dir.path()).await;
        let mut fresh = request();
        fresh.run_id = "new-current-run-after-single-fault".into();
        let result = ReleaseStore::prepare_run(&context(Role::Host), &store, fresh).await;
        if fault == "kind" {
            assert!(
                matches!(result, Err(Error::NotFound)),
                "{fault}: {result:?}"
            );
        } else if matches!(fault, "source" | "typed_default" | "schema") {
            assert_invalid(result, expected);
        } else {
            assert_conflict(result, expected);
        }
        assert_eq!(
            current_image(&store, dir.path()).await,
            prior,
            "{fault} prepare wrote"
        );
        let result =
            ReleaseStore::validate_run_snapshot_live(&context(Role::Host), &store, RUN_ID).await;
        if fault == "kind" {
            assert!(
                matches!(result, Err(Error::NotFound)),
                "{fault}: {result:?}"
            );
        } else if matches!(fault, "source" | "typed_default" | "schema") {
            assert_invalid(result, expected);
        } else {
            assert_conflict(result, expected);
        }
        assert_eq!(
            current_image(&store, dir.path()).await,
            prior,
            "{fault} live read wrote"
        );
        store.close().await;
    }
}

#[tokio::test]
async fn current_predecessor_is_a_real_same_host_record_and_foreign_or_self_inputs_refuse() {
    let (dir, store, root) = current_fixture(0).await;
    let admin = context(Role::Admin);
    let next_id = "surface-name-coverage-next";
    let next = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        next_id,
        root.source.clone(),
        current_registration(),
        Some(SURFACE_ID.into()),
    )
    .await
    .unwrap();
    assert_eq!(next.predecessor_surface_id.as_deref(), Some(SURFACE_ID));
    assert!(next.upgrade_diff.added.is_empty());
    assert!(next.upgrade_diff.default_changed.is_empty());
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .outgoing_edges_bounded(&admin, "artifact", next_id, 10_000)
            .await
            .unwrap(),
        vec![("artifact".into(), SURFACE_ID.into())]
    );
    session.commit().await.unwrap();
    let mut next_request = request();
    next_request.run_id = "next-current-run".into();
    next_request.host_surface_id = next_id.into();
    let prepared = ReleaseStore::prepare_run(&context(Role::Host), &store, next_request)
        .await
        .unwrap();
    assert_eq!(prepared.host_surface_digest, next.manifest_digest);
    let live =
        ReleaseStore::validate_run_snapshot_live(&context(Role::Host), &store, "next-current-run")
            .await
            .unwrap();
    assert_eq!(fingerprint(&live).unwrap(), fingerprint(&prepared).unwrap());
    let mut foreign = current_source(0, "baseline");
    foreign.original_utf8 =
        foreign
            .original_utf8
            .replacen("\"host\":\"reference\"", "\"host\":\"other-reference\"", 1);
    extract_reviewed_host_source(&foreign).unwrap();
    ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "foreign-surface",
        foreign,
        current_registration(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(counts(&store).await, (5, 0, 10));
    for (predecessor, expected) in [
        (next_id, "self predecessor"),
        (
            "foreign-surface",
            "host surface predecessor belongs to another host",
        ),
    ] {
        let prior = current_image(&store, dir.path()).await;
        assert_conflict(
            ReleaseStore::register_host_surface_from_source(
                &admin,
                &store,
                next_id,
                root.source.clone(),
                current_registration(),
                Some(predecessor.into()),
            )
            .await,
            expected,
        );
        assert_eq!(current_image(&store, dir.path()).await, prior);
        assert_eq!(counts(&store).await, (5, 0, 10));
    }
    store.close().await;
}

// Observe exact logical rows without deserializing a deliberately damaged body.
async fn raw_logical_image(path: &Path) -> Vec<(String, Vec<String>)> {
    let script = r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
quote=lambda value:'"'+value.replace('"','""')+'"'
result=[]
for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"):
    table=quote(name)
    columns=[row[1] for row in con.execute('PRAGMA table_info('+table+')')]
    expressions=','.join('quote('+quote(column)+')' for column in columns)
    rows=sorted(row[0] for row in con.execute('SELECT json_array('+expressions+') FROM '+table))
    result.append((name,rows))
con.close()
print(json.dumps(result,ensure_ascii=False,separators=(',',':')))
"#;
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
fn write_raw_host_body(path: &Path, body: &str) {
    let script = "import sqlite3,sys; con=sqlite3.connect(sys.argv[1]); con.execute(\"UPDATE objects SET body=? WHERE namespace='name-coverage' AND kind='artifact' AND id='surface-name-coverage'\",(sys.argv[2],)); con.commit(); con.close()";
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .arg(body)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn raw_host_registration_precision_fault_is_zero_write() {
    let (dir, store, _) = current_fixture(0).await;
    let healthy = current_source(0, "another precise source");
    let raw = serde_json::to_string(&healthy).unwrap();
    let same = evo_engine::hosts::register_reviewed_host_surface_json(
        &context(Role::Admin),
        &store,
        "healthy-raw-host",
        &raw,
        current_registration(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(same.source.original_utf8, healthy.original_utf8);
    let before = raw_logical_image(&dir.path().join("name-coverage.sqlite3")).await;
    let damaged = raw.replacen("0.1", "0.100000000000000001", 1);
    assert_ne!(damaged, raw);
    assert_invalid(
        evo_engine::hosts::register_reviewed_host_surface_json(
            &context(Role::Admin),
            &store,
            "precision-refused-host",
            &damaged,
            current_registration(),
            None,
        )
        .await,
        "invalid reviewed host source/default",
    );
    assert_eq!(
        raw_logical_image(&dir.path().join("name-coverage.sqlite3")).await,
        before
    );
    store.close().await;
}

async fn raw_host_reader_precision_control(reload: bool) {
    let (dir, store, record) = current_fixture(0).await;
    let path = dir.path().join("name-coverage.sqlite3");
    let healthy = ReleaseStore::read_run_snapshot(&context(Role::Host), &store, RUN_ID)
        .await
        .unwrap();
    assert!(healthy.current_v2().is_some());
    let original = serde_json::to_string(&record).unwrap();
    let damaged = original.replacen("\"decimal\":0.1", "\"decimal\":0.100000000000000001", 1);
    assert_ne!(damaged, original);
    store.close().await;
    write_raw_host_body(&path, &damaged);
    let store = Store::open(&path).await.unwrap();
    let before = raw_logical_image(&path).await;
    let refused = if reload {
        ReleaseStore::read_run_snapshot(&context(Role::Host), &store, RUN_ID).await
    } else {
        let mut fresh = request();
        fresh.run_id = "precision-new-run".into();
        ReleaseStore::prepare_run(&context(Role::Host), &store, fresh).await
    };
    assert_invalid(refused, "invalid reviewed host source/default");
    assert_eq!(raw_logical_image(&path).await, before);
    store.close().await;
}

#[tokio::test]
async fn raw_host_prepare_precision_fault_is_zero_write() {
    raw_host_reader_precision_control(false).await;
}

#[tokio::test]
async fn raw_host_reload_precision_fault_is_zero_write() {
    raw_host_reader_precision_control(true).await;
}
