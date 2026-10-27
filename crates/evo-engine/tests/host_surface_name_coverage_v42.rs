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

async fn restore_actual_registration_prefix(
    directory: &Path,
    store: Store,
    body: &str,
    audit: &[(i64, &str, &str, &str, &str)],
) -> Store {
    store.close().await;
    let path = directory.join("name-coverage.sqlite3");
    let script = r#"
import json,sqlite3,sys
payload=json.loads(sys.argv[2])
con=sqlite3.connect(sys.argv[1])
body=payload['body']
con.execute("INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('name-coverage','artifact',?,'trusted-actor',?,1)",(json.loads(body)['id'],body))
con.executemany('INSERT INTO audit(seq,namespace,payload,previous_hash,digest) VALUES(?,?,?,?,?)',payload['audit'])
con.commit()
assert con.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
con.close()
"#;
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&path)
        .arg(serde_json::to_string(&json!({"body":body,"audit":audit})).unwrap())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Store::open(&path).await.unwrap()
}

#[tokio::test]
async fn complete_registration_is_idempotent_and_changed_content_conflicts() {
    let (dir, store) = database().await;
    // This is the actual successful registration prefix of the frozen parent
    // run, before its later prepare events; it creates no new v1 authority.
    let store = restore_actual_registration_prefix(
        dir.path(),
        store,
        ACTUAL_LEGACY_HOST,
        &ACTUAL_LEGACY_AUDIT[..2],
    )
    .await;
    assert_eq!(counts(&store).await, (1, 0, 2));
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
    let (dir, store) = database().await;
    let store = restore_actual_registration_prefix(
        dir.path(),
        store,
        ACTUAL_DUPLICATE_HOST,
        &ACTUAL_DUPLICATE_AUDIT,
    )
    .await;
    assert_eq!(counts(&store).await, (1, 0, 2));
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
    assert_eq!(
        expected_digest,
        "f3ef985779cf420b9b8aa9c6e77704933c67ce671efe670958d19e48b231eee3"
    );
    assert_eq!(
        serde_json::to_string(&record).unwrap(),
        ACTUAL_DUPLICATE_HOST
    );
    assert_eq!(counts(&store).await, (1, 0, 2));
    assert_fixture_unchanged(
        &store,
        &serde_json::from_str(ACTUAL_DUPLICATE_HOST).unwrap(),
    )
    .await;
    let before = counts(&store).await;
    let mut fresh = record.clone();
    fresh.id = "new-duplicate-v1".into();
    let refused = ReleaseStore::register_host_surface(
        &context(Role::Admin),
        &store,
        &fresh.id,
        fresh.manifest,
        fresh.extracted,
    )
    .await;
    assert!(
        matches!(refused, Err(Error::Conflict(ref m)) if m == "new v1 host surface registration lacks original source; use source-bound registration")
    );
    assert_eq!(counts(&store).await, before);
}

// Actual base-800 successful duplicate registration prefix. Record SHA-256:
// 99473dba6e1aa58309aa11b89dde2f69a307de4c3827625e62793c1a2be59a5b.
// Its four items, owner and two original audit events are retained byte for byte.
const ACTUAL_DUPLICATE_HOST: &str = r#"{"id":"surface-name-coverage","schema_version":"rsia.host_surface_record.v1","manifest":{"schema_version":"rsia.host_surface.v1","host":"reference","host_version":"0.1.0","adapter_version":"0.1.0","source_digest":"reference_host_surface_v1","items":[{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"compiler","reason":"run-level instruction projection"},{"name":"model","coverage":"runtime_owned","mapped_field":null,"consumer":null,"reason":"model identity is host-declared"},{"name":"theme","coverage":"unsupported","mapped_field":null,"consumer":null,"reason":"UI theme is out of scope"},{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"compiler","reason":"run-level instruction projection"}]},"manifest_digest":"f3ef985779cf420b9b8aa9c6e77704933c67ce671efe670958d19e48b231eee3","extracted":["instruction","model","theme","model"]}"#;
const ACTUAL_DUPLICATE_AUDIT: [(i64, &str, &str, &str, &str); 2] = [
    (
        1,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"be4bf1c6b626e1591b84b8d8df2410abf288f1b07dc48fac727acff670e58e79","operation":"formal.admin.content.v1.f9e65bfa18da6cccfa157fb8caf97aa67cf67ff6cf96906026dc7bd9b6bfa160","role":"admin","timestamp":1792966038}"#,
        "",
        "2942a91e56c22bfef270da2b29a203cc597a89c88f379f590ce7c4be84cd1e18",
    ),
    (
        2,
        "name-coverage",
        r#"{"actor":"trusted-actor","entity":"surface-name-coverage","operation":"host_surface.register","role":"admin","timestamp":1792966038}"#,
        "2942a91e56c22bfef270da2b29a203cc597a89c88f379f590ce7c4be84cd1e18",
        "c7069a2677c733a967747c74e712ebd2721193652d11052cfa91b01783a5f487",
    ),
];

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
    artifacts: Result<Vec<Value>, String>,
    runs: Result<Vec<Value>, String>,
    edges: Vec<ObjectDependencyImage>,
    raw_sql: Value,
    counts: (u64, u64, usize),
    facts: Result<evo_storage::lifecycle::ControlPlaneFacts, String>,
    files: BTreeMap<String, (bool, Vec<u8>)>,
}

// This fixture owns only the named database family. Its existing read-only SQL
// observer preserves every row even when a damaged Current body is refused by
// the typed reader. Unexpected files, directories and symlinks still fail.
async fn current_image(store: &Store, dir: &Path) -> CurrentImage {
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    let artifacts: Result<Vec<Value>, String> = session
        .list(&admin, "artifact")
        .await
        .map_err(|error| error.to_string());
    let runs: Result<Vec<Value>, String> = session
        .list(&admin, "run")
        .await
        .map_err(|error| error.to_string());
    let mut edges = Vec::new();
    for (kind, values) in [("artifact", &artifacts), ("run", &runs)] {
        let Ok(values) = values else {
            continue;
        };
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
            .map_err(|error| error.to_string());
    let raw_sql = raw_logical_image(&dir.join("name-coverage.sqlite3")).await;
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
        raw_sql,
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
                evo_engine::release_store::validate_host_record_v2_source(&record).unwrap();
                record.manifest_digest = hash(b"single deliberately wrong stored manifest digest");
                assert_conflict(
                    evo_engine::release_store::validate_host_record_v2_source(&record),
                    "current host surface identity/source differs",
                );
                "invalid reviewed host source/default"
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
        if matches!(fault, "source" | "typed_default" | "manifest_digest") {
            assert_eq!(
                prior.artifacts.as_ref().unwrap_err(),
                &Error::Invalid(expected.into()).to_string(),
                "{fault} typed observation must retain the exact refusal"
            );
        }
        let mut fresh = request();
        fresh.run_id = "new-current-run-after-single-fault".into();
        let result = ReleaseStore::prepare_run(&context(Role::Host), &store, fresh).await;
        if fault == "kind" {
            assert!(
                matches!(result, Err(Error::NotFound)),
                "{fault}: {result:?}"
            );
        } else if matches!(
            fault,
            "source" | "typed_default" | "manifest_digest" | "schema"
        ) {
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
        } else if matches!(
            fault,
            "source" | "typed_default" | "manifest_digest" | "schema"
        ) {
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
type RawDatabaseFamily = BTreeMap<String, (std::fs::Permissions, Vec<u8>)>;

// The fixture has no concurrent business writer. Capture the main database and
// every existing WAL/SHM member together; no SQLite client opens the source here.
fn capture_raw_database_family(path: &Path) -> RawDatabaseFamily {
    let mut files = BTreeMap::new();
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let member = std::path::PathBuf::from(name);
        let metadata = match std::fs::symlink_metadata(&member) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {
                continue;
            }
            Err(error) => panic!("cannot capture database family member {member:?}: {error}"),
        };
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        let bytes = std::fs::read(&member).unwrap();
        let after = std::fs::symlink_metadata(&member).unwrap();
        assert!(after.is_file() && !after.file_type().is_symlink());
        assert_eq!(metadata.permissions(), after.permissions());
        assert_eq!(metadata.len(), bytes.len() as u64);
        assert_eq!(after.len(), bytes.len() as u64);
        let name = member.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            files
                .insert(name, (metadata.permissions(), bytes))
                .is_none()
        );
    }
    files
}

async fn raw_logical_image(path: &Path) -> Value {
    let captured = capture_raw_database_family(path);
    let private = tempfile::tempdir().unwrap();
    for (name, (permissions, bytes)) in &captured {
        let copy = private.path().join(name);
        std::fs::write(&copy, bytes).unwrap();
        std::fs::set_permissions(&copy, permissions.clone()).unwrap();
    }
    assert_eq!(
        capture_raw_database_family(path),
        captured,
        "copying the private observation changed the source database family"
    );
    let copy_path = private.path().join(path.file_name().unwrap());
    let script = r#"
import json,sqlite3,struct,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
con.text_factory=bytes
quote=lambda value:'"'+value.replace('"','""')+'"'
def cell(storage,value):
    storage=storage.decode('ascii')
    if storage in ('text','blob'):
        assert isinstance(value,bytes)
        return [storage,value.hex()]
    if storage=='integer':
        assert isinstance(value,int)
        return [storage,str(value)]
    if storage=='real':
        assert isinstance(value,float)
        return [storage,struct.pack('>d',value).hex()]
    assert storage=='null' and value is None
    return [storage,None]
image={}
for (raw_name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"):
    name=raw_name.decode('utf-8')
    table=quote(name)
    columns=[row[1].decode('utf-8') for row in con.execute('PRAGMA table_xinfo('+table+')')]
    assert columns
    expressions=','.join('typeof('+quote(column)+'),'+quote(column) for column in columns)
    rows=[]
    for row in con.execute('SELECT '+expressions+' FROM '+table):
        rows.append([cell(row[2*i],row[2*i+1]) for i in range(len(columns))])
    image[name]={'columns':columns,'rows':sorted(rows,key=lambda row:json.dumps(row,separators=(',',':')))}
con.close()
print(json.dumps(image,sort_keys=True,separators=(',',':')))
"#;
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&copy_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        capture_raw_database_family(path),
        captured,
        "observing the private copy changed the source database family"
    );
    let image = serde_json::from_slice(&result.stdout).unwrap();
    private.close().unwrap();
    image
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
    let token = "\"decimal\":0.1";
    assert_eq!(healthy.original_utf8.matches(token).count(), 1);
    let mut damaged_source = healthy.clone();
    damaged_source.original_utf8 =
        healthy
            .original_utf8
            .replacen(token, "\"decimal\":0.100000000000000001", 1);
    let damaged = serde_json::to_string(&damaged_source).unwrap();
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

fn r9_declaration(name: &str, kind: &str, value: Value) -> Value {
    json!({
        "name":name,
        "value_type":kind,
        "enum_values":if kind == "enum" { vec!["a", "b"] } else { Vec::new() },
        "default":{"kind":"value","value":value},
    })
}

fn r9_source(declarations: Vec<Value>) -> ReviewedHostSourceEnvelopeV1 {
    ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: serde_json::to_string(&json!({
            "schema_version":"rsia.host_reviewed_declarations.v1",
            "host":"reference","host_version":"0.1.0","declarations":declarations,
        }))
        .unwrap(),
    }
}

fn r9_registration(source: &ReviewedHostSourceEnvelopeV1) -> HostSurfaceRegistrationV2 {
    HostSurfaceRegistrationV2 {
        adapter_version: "r8-9".into(),
        classifications: extract_reviewed_host_source(source)
            .unwrap()
            .declarations
            .into_iter()
            .map(|declaration| HostSurfaceClassificationV2 {
                name: declaration.name,
                coverage: SurfaceCoverage::Unsupported,
                mapped_field: None,
                consumer: None,
                reason: "unmapped reviewed software fixture".into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            })
            .collect(),
        retired: Vec::new(),
    }
}

// Data-only preview for independent final-wire counting. The real writer below
// must produce these bytes; this helper creates no witness or authority.
fn r9_candidate(
    source: &ReviewedHostSourceEnvelopeV1,
    registration: &HostSurfaceRegistrationV2,
) -> HostSurfaceRecordV2 {
    let (manifest, extracted_typed) =
        evo_core::contract::host_surface_v2_from_source(source, registration).unwrap();
    let upgrade_diff = evo_core::contract::host_surface_upgrade_diff_v2(&manifest, None).unwrap();
    HostSurfaceRecordV2 {
        id: SURFACE_ID.into(),
        schema_version: HOST_SURFACE_RECORD_SCHEMA_V2.into(),
        manifest_digest: fingerprint(&manifest).unwrap(),
        source: source.clone(),
        extracted: extracted_typed
            .iter()
            .map(|item| item.name.clone())
            .collect(),
        extracted_typed,
        manifest,
        predecessor_surface_id: None,
        upgrade_diff,
    }
}

// Count the actual serde wire independently of the bounded decoder. Strings
// (including embedded source UTF-8) are scalars; keys are not value nodes.
fn r9_raw_resources(raw: &str) -> (usize, usize, usize) {
    let bytes = raw.as_bytes();
    let (mut pos, mut nodes, mut depth, mut maximum) = (0, 0, 0_usize, 0);
    while pos < bytes.len() {
        match bytes[pos] {
            b'{' | b'[' => {
                nodes += 1;
                depth += 1;
                maximum = maximum.max(depth);
                pos += 1;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1).unwrap();
                pos += 1;
            }
            b'"' => {
                pos += 1;
                while bytes[pos] != b'"' {
                    if bytes[pos] == b'\\' {
                        pos += 1;
                    }
                    pos += 1;
                }
                pos += 1;
                let mut next = pos;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if bytes.get(next) != Some(&b':') {
                    nodes += 1;
                }
            }
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                nodes += 1;
                while pos < bytes.len()
                    && !matches!(bytes[pos], b',' | b'}' | b']')
                    && !bytes[pos].is_ascii_whitespace()
                {
                    pos += 1;
                }
            }
            b',' | b':' | b' ' | b'\r' | b'\n' | b'\t' => pos += 1,
            value => panic!("unexpected generated JSON byte {value}"),
        }
    }
    assert_eq!(depth, 0);
    (raw.len(), nodes, maximum)
}

async fn r9_healthy_chain(
    source: ReviewedHostSourceEnvelopeV1,
    registration: HostSurfaceRegistrationV2,
) -> (tempfile::TempDir, Store, HostSurfaceRecordV2) {
    let (dir, store) = database().await;
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"r8-9 actual backup namespace"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let expected = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    let record = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        SURFACE_ID,
        source.clone(),
        registration.clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_string(&record).unwrap(), expected);
    assert_eq!(record.source.original_utf8, source.original_utf8);
    store.close().await;
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let loaded: HostSurfaceRecordV2 = session.need(&admin, "artifact", SURFACE_ID).await.unwrap();
    let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(serde_json::to_string(&loaded).unwrap(), expected);
    assert_eq!(serde_json::to_string(&listed[0]).unwrap(), expected);
    session.commit().await.unwrap();
    let before = current_image(&store, dir.path()).await;
    let repeat = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        SURFACE_ID,
        source,
        registration,
        None,
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_string(&repeat).unwrap(), expected);
    assert_eq!(current_image(&store, dir.path()).await, before);
    let host = context(Role::Host);
    let prepared = ReleaseStore::prepare_run(&host, &store, request())
        .await
        .unwrap();
    assert_eq!(prepared.host_surface_digest, record.manifest_digest);
    let prepared = serde_json::to_value(&prepared).unwrap();
    let read = ReleaseStore::read_run_snapshot(&host, &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), prepared);
    let live = ReleaseStore::validate_run_snapshot_live(&host, &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&live).unwrap(), prepared);
    let before = current_image(&store, dir.path()).await;
    let replay = ReleaseStore::prepare_run(&host, &store, request())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&replay).unwrap(), prepared);
    assert_eq!(current_image(&store, dir.path()).await, before);
    (dir, store, record)
}

async fn r9_restore_chain(
    store: &Store,
    dir: &Path,
    record: &HostSurfaceRecordV2,
    number_wires: Value,
    typed_matrix: bool,
    resource_fault: Option<&str>,
) {
    let private = tempfile::tempdir().unwrap();
    let backup = private.path().join("actual-rust-backup");
    store.backup(&backup).await.unwrap();
    store.close().await;
    let anchor = dir.join("name-coverage.sqlite3");
    let before = capture_raw_database_family(&anchor);
    let payload = private.path().join("actual-rust-host-input.json");
    std::fs::write(
        &payload,
        serde_json::to_vec(&json!({
            "backup":backup,"anchor":anchor,"identity":record.id,
            "record_wire":serde_json::to_string(record).unwrap(),
            "number_wires":number_wires,"typed_matrix":typed_matrix,
            "raw_token_faults":if typed_matrix { r9_invalid_number_tokens() } else { Vec::new() },
            "resource_fault":resource_fault,
        }))
        .unwrap(),
    )
    .unwrap();
    let script = "import json,sys; from scripts.test_restore_backup import _rust_current_host_restore_matrix; print(json.dumps(_rust_current_host_restore_matrix(json.load(open(sys.argv[1],encoding='utf-8')))))";
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&payload)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Python restore bridge failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(capture_raw_database_family(&anchor), before);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let restored_path = Path::new(response["restored"].as_str().unwrap()).join("rsia.sqlite3");
    let restored = Store::open(&restored_path).await.unwrap();
    let admin = context(Role::Admin);
    let mut session = restored.session().await.unwrap();
    let loaded: HostSurfaceRecordV2 = session.need(&admin, "artifact", SURFACE_ID).await.unwrap();
    assert_eq!(
        serde_json::to_string(&loaded).unwrap(),
        serde_json::to_string(record).unwrap()
    );
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    assert!(artifacts.iter().any(|value| value["id"] == SURFACE_ID));
    session.commit().await.unwrap();
    let live = ReleaseStore::validate_run_snapshot_live(&context(Role::Host), &restored, RUN_ID)
        .await
        .unwrap();
    assert_eq!(live.host_surface_digest, record.manifest_digest);
    ReleaseStore::read_run_snapshot(&context(Role::Host), &restored, RUN_ID)
        .await
        .unwrap();
    restored.close().await;
    for destination in response["equivalents"].as_array().unwrap() {
        let restored = Store::open(&Path::new(destination.as_str().unwrap()).join("rsia.sqlite3"))
            .await
            .unwrap();
        let mut session = restored.session().await.unwrap();
        let loaded: HostSurfaceRecordV2 =
            session.need(&admin, "artifact", SURFACE_ID).await.unwrap();
        evo_engine::release_store::validate_host_record_v2_source(&loaded).unwrap();
        session.commit().await.unwrap();
        // Semantic content equality is not a replacement Admin witness for a
        // changed wire. Only the untouched actual producer above earns live use.
        restored.close().await;
    }
    assert_eq!(
        response["isolated"].as_u64().unwrap() > 0,
        typed_matrix || resource_fault.is_some()
    );
}

fn r9_legal_zero_tokens() -> Vec<&'static str> {
    vec![
        "0e1000000000000000000",
        "-0e1000000000000000000",
        "0e-1000000000000000000",
        "-0e-1000000000000000000",
        "0e9223372036854775807",
        "-0e9223372036854775807",
        "0e-9223372036854775808",
        "-0e-9223372036854775808",
        "0e+00000000000000000009223372036854775807",
        "-0e-00000000000000000009223372036854775808",
        "0.0e9223372036854775807",
        "-0.0e-9223372036854775807",
    ]
}

fn r9_invalid_number_tokens() -> Vec<&'static str> {
    vec![
        "0e9223372036854775808",
        "0e-9223372036854775809",
        "0e00000000000000000009223372036854775808",
        "0.0e-9223372036854775808",
        "-0.0e-9223372036854775808",
        "10e9223372036854775807",
        "1e-1000000000000000000",
        "1e1000000000000000000",
        "0.100000000000000001",
        "00e1",
        "0e+",
        "0.e1",
    ]
}

#[tokio::test]
async fn current_host_number_object_and_typed_defaults_survive_cross_language_restore() {
    let zero_tokens = r9_legal_zero_tokens();
    let zeros = zero_tokens
        .iter()
        .enumerate()
        .map(|(index, _)| (format!("zero_{index}"), json!(0.0)))
        .collect();
    let declarations = vec![
        r9_declaration("enabled", "bool", json!(true)),
        r9_declaration("disabled", "bool", json!(false)),
        r9_declaration("count", "integer", json!(1)),
        r9_declaration("mode", "enum", json!("b")),
        r9_declaration(
            "reserved",
            "object",
            json!({"kind":"value","value":null,"default":{"kind":"value","value":null}}),
        ),
        r9_declaration(
            "numbers",
            "object",
            json!({
                "fixed_high":1e15,"exponent_high":1e16,"fixed_low":1e-5,"exponent_low":1e-6,
                "large_int":1e21,"negative_zero":-0.0,"fraction":0.1,"one":1.0,"zero":-0.0,
                "flags":[true,false,{"on":true}],
            }),
        ),
        r9_declaration("boundary", "string", json!("x".repeat(4094))),
        r9_declaration("compact_numbers", "object", json!({"n":vec![1e16;817]})),
        r9_declaration("raw_zeros", "object", Value::Object(zeros)),
    ];
    let mut source = r9_source(declarations);
    source.original_utf8 = source
        .original_utf8
        .replace(
            "\"large_int\":1e+21",
            "\"large_int\":1000000000000000000000",
        )
        .replace("\"negative_zero\":-0.0", "\"negative_zero\":-0");
    assert!(source.original_utf8.contains("1000000000000000000000"));
    assert!(source.original_utf8.contains("\"negative_zero\":-0,"));
    for (index, token) in zero_tokens.iter().enumerate() {
        let old = format!("\"zero_{index}\":0.0");
        let original = format!("\"zero_{index}\":{token}");
        assert!(source.original_utf8.contains(&old));
        source.original_utf8 = source.original_utf8.replacen(&old, &original, 1);
        assert!(source.original_utf8.contains(&original));
    }
    let registration = r9_registration(&source);
    let (dir, store, record) = r9_healthy_chain(source, registration.clone()).await;
    for (index, token) in r9_invalid_number_tokens().iter().enumerate() {
        let mut damaged = record.source.clone();
        let old = format!("\"zero_0\":{}", zero_tokens[0]);
        let replacement = format!("\"zero_0\":{token}");
        damaged.original_utf8 = damaged.original_utf8.replacen(&old, &replacement, 1);
        assert_ne!(damaged.original_utf8, record.source.original_utf8);
        let before = current_image(&store, dir.path()).await;
        let result = ReleaseStore::register_host_surface_from_source(
            &context(Role::Admin),
            &store,
            &format!("raw-token-fault-{index}"),
            damaged,
            registration.clone(),
            None,
        )
        .await;
        assert_invalid(result, "invalid reviewed host source/default");
        assert_eq!(current_image(&store, dir.path()).await, before);
    }
    let seeds = [
        0.0_f64,
        1.0,
        0.1,
        2.5,
        1e15,
        1e16,
        1e-5,
        1e-6,
        1e21,
        1e23,
        1.0000000000000002,
        1.2345678901234567,
        0.8455124082255701,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::from_bits(1),
    ];
    let mut bits = BTreeSet::new();
    for seed in seeds {
        for sign in [0, 1_u64 << 63] {
            let value = seed.to_bits() ^ sign;
            for candidate in [value.wrapping_sub(1), value, value.wrapping_add(1)] {
                if f64::from_bits(candidate).is_finite() {
                    bits.insert(candidate);
                }
            }
        }
    }
    let number_wires = Value::Array(
        bits.into_iter()
            .map(|bits| {
                json!({
                    "bits":format!("{bits:016x}"),
                    "wire":serde_json::to_string(&f64::from_bits(bits)).unwrap(),
                })
            })
            .collect(),
    );
    // This one marker exports only this actual software producer's material.
    // The controller captures stdout; no QA/local path or authority is embedded.
    println!(
        "AG135B_R8_9_ACTUAL_PRODUCER_V1 {}",
        serde_json::to_string(&json!({
            "record_wire":serde_json::to_string(&record).unwrap(),
            "source_wire":serde_json::to_string(&record.source).unwrap(),
            "manifest_wire":serde_json::to_string(&record.manifest).unwrap(),
            "number_wires":number_wires,
            "raw_zero_tokens":zero_tokens,
            "raw_token_faults":r9_invalid_number_tokens(),
        }))
        .unwrap()
    );
    r9_restore_chain(&store, dir.path(), &record, number_wires, true, None).await;
    r10_text_matrix().await;
    r11_number_admission_matrix().await;
}

fn r9_byte_case(target: usize) -> (ReviewedHostSourceEnvelopeV1, HostSurfaceRegistrationV2) {
    let make = |size| {
        r9_source(
            (0..64)
                .map(|index| {
                    r9_declaration(
                        &format!("field-{index:02}"),
                        "string",
                        json!("x".repeat(size)),
                    )
                })
                .collect(),
        )
    };
    let (mut low, mut high) = (0_usize, 3000_usize);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let source = make(middle);
        let registration = r9_registration(&source);
        let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
        if raw.len() <= target {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let source = make(low);
    let mut registration = r9_registration(&source);
    let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    registration.classifications[0]
        .reason
        .push_str(&"x".repeat(target - raw.len()));
    assert!(registration.classifications[0].reason.len() <= 512);
    let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    assert_eq!(raw.len(), target);
    (source, registration)
}

fn r9_node_case(target: usize) -> (ReviewedHostSourceEnvelopeV1, HostSurfaceRegistrationV2) {
    // Changing the declaration parity permits adjacent complete-record node
    // counts even though each default is repeated twice in the final record.
    let count = if target & 1 == 0 { 16 } else { 17 };
    let make = |extra: usize| {
        r9_source(
            (0..count)
                .map(|index| {
                    let size = extra / count + usize::from(index < extra % count);
                    r9_declaration(
                        &format!("field-{index:02}"),
                        "object",
                        json!({"values":vec![0;size]}),
                    )
                })
                .collect(),
        )
    };
    let source = make(0);
    let registration = r9_registration(&source);
    let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    let baseline = r9_raw_resources(&raw).1;
    assert_eq!((target - baseline) % 2, 0);
    let source = make((target - baseline) / 2);
    let registration = r9_registration(&source);
    let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    assert_eq!(r9_raw_resources(&raw).1, target);
    (source, registration)
}

fn r9_depth_case(target: usize) -> (ReviewedHostSourceEnvelopeV1, HostSurfaceRegistrationV2) {
    let mut nested = Value::Null;
    for _ in 0..target - 6 {
        nested = Value::Array(vec![nested]);
    }
    let source = r9_source(vec![r9_declaration(
        "nested",
        "object",
        json!({"nested":nested}),
    )]);
    let registration = r9_registration(&source);
    let raw = serde_json::to_string(&r9_candidate(&source, &registration)).unwrap();
    assert_eq!(r9_raw_resources(&raw).2, target);
    (source, registration)
}

fn r9_install_first_dml_sentinel(path: &Path) {
    let script = "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(\"CREATE TRIGGER r8_9_first_dml BEFORE INSERT ON objects BEGIN SELECT RAISE(FAIL,'AG135B_R8_9_FIRST_DML'); END\"); c.commit(); c.close()";
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn current_host_final_record_resource_amplification_is_zero_write() {
    for dimension in ["UTF-8 bytes", "nodes", "depth"] {
        let make = |outside| match dimension {
            "UTF-8 bytes" => r9_byte_case(262_144 + usize::from(outside)),
            "nodes" => r9_node_case(32_768 + usize::from(outside)),
            "depth" => r9_depth_case(256 + usize::from(outside)),
            _ => unreachable!(),
        };
        let (source, registration) = make(false);
        let legal = r9_candidate(&source, &registration);
        let raw = serde_json::to_string(&legal).unwrap();
        let inside = r9_raw_resources(&raw);
        assert!(inside.0 <= 262_144 && inside.1 <= 32_768 && inside.2 <= 256);
        evo_core::contract::decode_host_surface_record_v2_json(&raw).unwrap();
        let (dir, store, record) = r9_healthy_chain(source, registration).await;
        assert_eq!(serde_json::to_string(&record).unwrap(), raw);
        let (source, registration) = make(true);
        let source_resources = r9_raw_resources(&source.original_utf8);
        assert!(
            source_resources.0 <= 262_144
                && source_resources.1 <= 32_768
                && source_resources.2 <= 256
        );
        // This full producer extraction proves every default remains valid;
        // only the final repeated record exceeds its separate resource cap.
        extract_reviewed_host_source(&source).unwrap();
        let illegal = r9_candidate(&source, &registration);
        let damaged = serde_json::to_string(&illegal).unwrap();
        let outside = r9_raw_resources(&damaged);
        match dimension {
            "UTF-8 bytes" => {
                assert!(outside.0 == 262_145 && outside.1 <= 32_768 && outside.2 <= 256);
            }
            "nodes" => assert!(outside.0 <= 262_144 && outside.1 == 32_769 && outside.2 <= 256),
            "depth" => assert!(outside.0 <= 262_144 && outside.1 <= 32_768 && outside.2 == 257),
            _ => unreachable!(),
        }
        let expected = format!("reviewed host JSON resource limit: {dimension}");
        assert_invalid(
            evo_core::contract::decode_host_surface_record_v2_json(&damaged),
            &expected,
        );
        r9_restore_chain(
            &store,
            dir.path(),
            &record,
            json!([]),
            false,
            Some(&damaged),
        )
        .await;

        let (refused_dir, refused_store) = database().await;
        let admin = context(Role::Admin);
        let control_source = r9_source(vec![r9_declaration("healthy", "bool", json!(true))]);
        let control_registration = r9_registration(&control_source);
        ReleaseStore::register_host_surface_from_source(
            &admin,
            &refused_store,
            "resource-healthy-control",
            control_source,
            control_registration,
            None,
        )
        .await
        .unwrap();
        r9_install_first_dml_sentinel(&refused_dir.path().join("name-coverage.sqlite3"));
        let before = current_image(&refused_store, refused_dir.path()).await;
        let result = ReleaseStore::register_host_surface_from_source(
            &admin,
            &refused_store,
            SURFACE_ID,
            source,
            registration,
            None,
        )
        .await;
        assert_invalid(result, &expected);
        assert_eq!(
            current_image(&refused_store, refused_dir.path()).await,
            before
        );
        refused_store.close().await;
    }
}

const R10_WHITE_SPACE: &str = "\u{0009}\u{000a}\u{000b}\u{000c}\u{000d}\u{0020}\u{0085}\u{00a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
const R10_PREDECESSOR: &str = "r10-text-predecessor";

#[derive(Clone, Copy, Debug)]
enum R10TextField {
    HostVersion,
    AdapterVersion,
    Reason,
    RetiredReason,
    TestRef,
}

impl R10TextField {
    fn name(self) -> &'static str {
        match self {
            Self::HostVersion => "host_version",
            Self::AdapterVersion => "adapter_version",
            Self::Reason => "reason",
            Self::RetiredReason => "retired_reason",
            Self::TestRef => "test_ref",
        }
    }
    fn limit(self) -> usize {
        match self {
            Self::HostVersion | Self::AdapterVersion => 128,
            Self::Reason | Self::RetiredReason => 512,
            Self::TestRef => panic!("test_ref has no independent byte cap"),
        }
    }
    fn expected(self) -> &'static str {
        match self {
            Self::HostVersion => "host_version: expected nonempty text <= 128 bytes",
            Self::AdapterVersion => "adapter_version: expected nonempty text <= 128 bytes",
            Self::Reason => "reason: expected nonempty text <= 512 bytes",
            Self::RetiredReason => "retired reason: expected nonempty text <= 512 bytes",
            Self::TestRef => "invalid reviewed host source/default",
        }
    }
}

#[derive(Clone)]
struct R10TextInputs {
    source: ReviewedHostSourceEnvelopeV1,
    registration: HostSurfaceRegistrationV2,
}

struct R10TextFault {
    name: String,
    field: R10TextField,
    value: Option<String>,
}

fn r10_replace_source_version(source: &mut ReviewedHostSourceEnvelopeV1, value: &str) {
    // All variants start from the genuine simple text source below. Use the
    // real JSON string serializer, including NUL/C0 escapes, for this one token.
    let old = "\"host_version\":\"0.1.0\"";
    let replacement = format!("\"host_version\":{}", serde_json::to_string(value).unwrap());
    assert!(source.original_utf8.contains(old));
    source.original_utf8 = source.original_utf8.replacen(old, &replacement, 1);
}

fn r10_text_inputs() -> R10TextInputs {
    let source = r9_source(vec![r9_declaration(
        "new-setting",
        "string",
        json!("reviewed software content"),
    )]);
    let mut registration = r9_registration(&source);
    let item = &mut registration.classifications[0];
    item.coverage = SurfaceCoverage::Supported;
    item.mapped_field = Some("skill.content".into());
    item.consumer = Some("compiler".into());
    item.capability = Some("r10-text-content".into());
    item.test_ref = Some("tests/r10/text-contract".into());
    registration.retired = vec![evo_core::contract::RetiredHostSurfaceV2 {
        name: "old-setting".into(),
        reason: "retired reviewed software field".into(),
    }];
    R10TextInputs {
        source,
        registration,
    }
}

fn r10_set_input(inputs: &mut R10TextInputs, field: R10TextField, value: Option<&str>) {
    match field {
        R10TextField::HostVersion => {
            r10_replace_source_version(&mut inputs.source, value.unwrap());
        }
        R10TextField::AdapterVersion => {
            inputs.registration.adapter_version = value.unwrap().into();
        }
        R10TextField::Reason => {
            inputs.registration.classifications[0].reason = value.unwrap().into();
        }
        R10TextField::RetiredReason => {
            inputs.registration.retired[0].reason = value.unwrap().into();
        }
        R10TextField::TestRef => {
            inputs.registration.classifications[0].test_ref = value.map(str::to_owned);
        }
    }
}

fn r10_positive_text_inputs() -> Vec<(String, R10TextInputs)> {
    let base = r10_text_inputs();
    let fields = [
        R10TextField::HostVersion,
        R10TextField::AdapterVersion,
        R10TextField::Reason,
        R10TextField::RetiredReason,
    ];
    let mut cases = vec![("base".into(), base.clone())];
    for field in fields {
        let limit = field.limit();
        for (label, value) in [
            ("ascii-bound", "x".repeat(limit)),
            (
                "utf8-bound",
                "汉".repeat(limit / 3) + &"a".repeat(limit % 3),
            ),
        ] {
            assert_eq!(value.len(), limit);
            let mut inputs = base.clone();
            r10_set_input(&mut inputs, field, Some(&value));
            cases.push((format!("{}-{label}", field.name()), inputs));
        }
    }
    for separator in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'] {
        for (label, value) in [
            ("single", separator.to_string()),
            (
                "padded",
                format!("{R10_WHITE_SPACE}{separator}{R10_WHITE_SPACE}"),
            ),
        ] {
            assert!(value.len() <= 128);
            let mut inputs = base.clone();
            for field in fields {
                r10_set_input(&mut inputs, field, Some(&value));
            }
            cases.push((format!("c0-{:04x}-{label}", u32::from(separator)), inputs));
        }
    }
    let mut references = vec!["t\0".into(), "\0".into(), "r".repeat(513), "汉".repeat(171)];
    for separator in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'] {
        references.push(separator.to_string());
        references.push(format!("{R10_WHITE_SPACE}{separator}{R10_WHITE_SPACE}"));
    }
    for (index, reference) in references.into_iter().enumerate() {
        let mut inputs = base.clone();
        r10_set_input(&mut inputs, R10TextField::TestRef, Some(&reference));
        cases.push((format!("reference-{index:02}"), inputs));
    }
    assert_eq!(cases.len(), 29);
    cases
}

fn r10_text_faults() -> Vec<R10TextFault> {
    let mut cases = Vec::new();
    for field in [
        R10TextField::HostVersion,
        R10TextField::AdapterVersion,
        R10TextField::Reason,
        R10TextField::RetiredReason,
    ] {
        let mut values = vec![
            ("ascii-over".into(), "x".repeat(field.limit() + 1)),
            ("utf8-over".into(), "汉".repeat(field.limit() / 3 + 1)),
            ("empty".into(), String::new()),
            ("nul-only".into(), "\0".into()),
            ("nul-first".into(), "\0x".into()),
            ("nul-middle".into(), "x\0y".into()),
            ("nul-last".into(), "x\0".into()),
            ("all-white".into(), R10_WHITE_SPACE.into()),
        ];
        for character in R10_WHITE_SPACE.chars() {
            values.push((
                format!("white-{:04x}", u32::from(character)),
                character.to_string(),
            ));
        }
        assert_eq!(values.len(), 33);
        for (label, value) in values {
            cases.push(R10TextFault {
                name: format!("{}-{label}", field.name()),
                field,
                value: Some(value),
            });
        }
    }
    let mut references = vec![None, Some(String::new()), Some(R10_WHITE_SPACE.into())];
    references.extend(R10_WHITE_SPACE.chars().map(|value| Some(value.to_string())));
    assert_eq!(references.len(), 28);
    for (index, value) in references.into_iter().enumerate() {
        cases.push(R10TextFault {
            name: format!("test-ref-empty-{index:02}"),
            field: R10TextField::TestRef,
            value,
        });
    }
    assert_eq!(cases.len(), 160);
    cases
}

async fn r10_open_text_store() -> (tempfile::TempDir, Store, HostSurfaceRecordV2) {
    let (dir, store) = database().await;
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"r10 actual text fixture namespace"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let source = r9_source(vec![r9_declaration("old-setting", "string", json!("old"))]);
    let registration = r9_registration(&source);
    let previous = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        R10_PREDECESSOR,
        source,
        registration,
        None,
    )
    .await
    .unwrap();
    (dir, store, previous)
}

fn r10_bad_text_record(base: &HostSurfaceRecordV2, fault: &R10TextFault) -> HostSurfaceRecordV2 {
    let mut record = base.clone();
    match fault.field {
        R10TextField::HostVersion => {
            let value = fault.value.as_deref().unwrap();
            r10_replace_source_version(&mut record.source, value);
            record.manifest.host_version = value.into();
            record.manifest.source_digest = hash(record.source.original_utf8.as_bytes());
            record.manifest.source_envelope_digest = fingerprint(&record.source).unwrap();
        }
        R10TextField::AdapterVersion => {
            record.manifest.adapter_version = fault.value.clone().unwrap();
        }
        R10TextField::Reason => {
            record.manifest.items[0].reason = fault.value.clone().unwrap();
        }
        R10TextField::RetiredReason => {
            record.manifest.retired[0].reason = fault.value.clone().unwrap();
        }
        R10TextField::TestRef => {
            record.manifest.items[0].test_ref = fault.value.clone();
        }
    }
    record.manifest_digest = fingerprint(&record.manifest).unwrap();
    assert_eq!(
        record.manifest_digest,
        fingerprint(&record.manifest).unwrap()
    );
    assert_eq!(
        record.manifest.source_envelope_digest,
        fingerprint(&record.source).unwrap()
    );
    assert_eq!(
        record.manifest.source_digest,
        hash(record.source.original_utf8.as_bytes())
    );
    assert_eq!(record.extracted, base.extracted);
    assert_eq!(
        fingerprint(&record.extracted_typed).unwrap(),
        fingerprint(&base.extracted_typed).unwrap()
    );
    assert_eq!(record.predecessor_surface_id, base.predecessor_surface_id);
    assert_eq!(record.upgrade_diff, base.upgrade_diff);
    let raw = serde_json::to_string(&record).unwrap();
    evo_core::contract::decode_host_surface_record_v2_json(&raw).unwrap();
    assert_invalid(
        evo_engine::release_store::validate_host_record_v2_source(&record),
        fault.field.expected(),
    );
    record
}

async fn r10_writer_text_faults(inputs: &R10TextInputs, faults: &[R10TextFault]) {
    let (dir, store, previous) = r10_open_text_store().await;
    let admin = context(Role::Admin);
    ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "r10-writer-healthy",
        inputs.source.clone(),
        inputs.registration.clone(),
        Some(previous.id),
    )
    .await
    .unwrap();
    r9_install_first_dml_sentinel(&dir.path().join("name-coverage.sqlite3"));
    let before = current_image(&store, dir.path()).await;
    for (index, fault) in faults.iter().enumerate() {
        // Mutate healthy inputs only. Invalid host_version never goes through
        // r9_registration/r9_candidate, whose healthy-only extraction unwraps.
        let mut damaged = inputs.clone();
        r10_set_input(&mut damaged, fault.field, fault.value.as_deref());
        let result = ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            &format!("r10-rejected-{index}"),
            damaged.source,
            damaged.registration,
            Some(R10_PREDECESSOR.into()),
        )
        .await;
        assert_invalid(result, fault.field.expected());
        assert_eq!(
            current_image(&store, dir.path()).await,
            before,
            "{} writer changed the full database image",
            fault.name
        );
    }
    store.close().await;
}

async fn r10_restore_text_content(
    store: &Store,
    dir: &Path,
    records: &[HostSurfaceRecordV2],
    faults: Vec<Value>,
) {
    let private = tempfile::tempdir().unwrap();
    let backup = private.path().join("text-content-backup");
    store.backup(&backup).await.unwrap();
    store.close().await;
    let anchor = dir.join("name-coverage.sqlite3");
    let before = capture_raw_database_family(&anchor);
    let payload = private.path().join("text-content-input.json");
    let rows = records
        .iter()
        .map(|record| json!({"id":record.id,"wire":serde_json::to_string(record).unwrap()}))
        .collect::<Vec<_>>();
    let fault_count = faults.len();
    std::fs::write(
        &payload,
        serde_json::to_vec(&json!({
            "backup":backup,"anchor":anchor,"records":rows,"faults":faults,
        }))
        .unwrap(),
    )
    .unwrap();
    let script = "import json,sys; from scripts.test_restore_backup import _rust_current_host_text_restore_matrix; print(json.dumps(_rust_current_host_text_restore_matrix(json.load(open(sys.argv[1],encoding='utf-8')))))";
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&payload)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "text content bridge failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(capture_raw_database_family(&anchor), before);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["accepted"].as_u64(), Some(records.len() as u64));
    assert_eq!(response["isolated"].as_u64(), Some(fault_count as u64));
    assert_eq!(fault_count, 162);
    assert_eq!(response["text_isolated"].as_u64(), Some(160));
    assert_eq!(response["shape_isolated"].as_u64(), Some(2));
    let path = Path::new(response["restored"].as_str().unwrap()).join("rsia.sqlite3");
    let restored = Store::open(&path).await.unwrap();
    let admin = context(Role::Admin);
    let mut session = restored.session().await.unwrap();
    let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(listed.len(), records.len());
    for record in records {
        let loaded: HostSurfaceRecordV2 =
            session.need(&admin, "artifact", &record.id).await.unwrap();
        assert_eq!(
            serde_json::to_string(&loaded).unwrap(),
            serde_json::to_string(record).unwrap()
        );
        evo_engine::release_store::validate_host_record_v2_source(&loaded).unwrap();
    }
    session.commit().await.unwrap();
    // This independent text fixture has no run snapshot. Its supported test_ref
    // values prove content acceptance only, never real behavior/live authority.
    restored.close().await;
}

async fn r10_text_matrix() {
    assert_eq!(R10_WHITE_SPACE.chars().count(), 25);
    for character in R10_WHITE_SPACE.chars() {
        assert!(character.is_whitespace());
    }
    for character in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'] {
        assert!(!character.is_whitespace());
    }
    let (dir, store, previous) = r10_open_text_store().await;
    let admin = context(Role::Admin);
    let mut records = vec![previous];
    let mut replays = Vec::new();
    let mut baseline = None;
    for (name, inputs) in r10_positive_text_inputs() {
        let id = format!("r10-text-{name}");
        let record = ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            &id,
            inputs.source.clone(),
            inputs.registration.clone(),
            Some(R10_PREDECESSOR.into()),
        )
        .await
        .unwrap();
        assert_eq!(
            record.predecessor_surface_id.as_deref(),
            Some(R10_PREDECESSOR)
        );
        assert_eq!(record.upgrade_diff.added, names(&["new-setting"]));
        assert_eq!(record.upgrade_diff.removed, names(&["old-setting"]));
        if name == "base" {
            baseline = Some(record.clone());
        }
        replays.push((inputs, id));
        records.push(record);
    }
    assert_eq!(records.len(), 30); // 29 successors and their actual predecessor.
    store.close().await;
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(listed.len(), records.len());
    for record in &records {
        let loaded: HostSurfaceRecordV2 =
            session.need(&admin, "artifact", &record.id).await.unwrap();
        assert_eq!(
            serde_json::to_string(&loaded).unwrap(),
            serde_json::to_string(record).unwrap()
        );
        evo_engine::release_store::validate_host_record_v2_source(&loaded).unwrap();
    }
    session.commit().await.unwrap();
    let before = current_image(&store, dir.path()).await;
    for (inputs, id) in replays {
        let repeat = ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            &id,
            inputs.source,
            inputs.registration,
            Some(R10_PREDECESSOR.into()),
        )
        .await
        .unwrap();
        let original = records.iter().find(|record| record.id == id).unwrap();
        assert_eq!(
            serde_json::to_string(&repeat).unwrap(),
            serde_json::to_string(original).unwrap()
        );
    }
    assert_eq!(current_image(&store, dir.path()).await, before);
    let baseline = baseline.unwrap();
    let faults = r10_text_faults();
    let mut bad_wires = Vec::new();
    for fault in &faults {
        let damaged = r10_bad_text_record(&baseline, fault);
        bad_wires.push(json!({
            "name":fault.name,"field":fault.field.name(),"id":damaged.id,
            "wire":serde_json::to_string(&damaged).unwrap(),
        }));
    }
    assert_eq!(bad_wires.len(), 160);
    let previous = &records[0];
    assert_eq!(previous.extracted, names(&["old-setting"]));
    assert!(previous.predecessor_surface_id.is_none());
    assert!(previous.manifest.retired.is_empty());
    assert_eq!(
        serde_json::to_value(&previous.extracted_typed[0].default).unwrap(),
        json!({"kind":"value","value":"old"})
    );
    let original = serde_json::to_string(previous).unwrap();
    assert_eq!(original.matches("\"retired\":[]").count(), 1);
    let healthy = evo_core::contract::decode_host_surface_record_v2_json(&original).unwrap();
    serde_json::from_value::<HostSurfaceRecordV2>(healthy.clone()).unwrap();
    for (shape, replacement, expected) in [
        (
            "object",
            "\"retired\":{}",
            "invalid type: map, expected a sequence",
        ),
        (
            "string",
            "\"retired\":\"\"",
            "invalid type: string \"\", expected a sequence",
        ),
    ] {
        let wire = original.replacen("\"retired\":[]", replacement, 1);
        assert_eq!(wire.len(), original.len());
        let mut decoded = evo_core::contract::decode_host_surface_record_v2_json(&wire).unwrap();
        let error = serde_json::from_value::<HostSurfaceRecordV2>(decoded.clone()).unwrap_err();
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert_eq!(error.to_string(), expected);
        let error = serde_json::from_value::<evo_core::contract::HostSurfaceManifestV2>(
            decoded["manifest"].clone(),
        )
        .unwrap_err();
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert_eq!(error.to_string(), expected);
        assert_eq!(decoded["manifest_digest"], healthy["manifest_digest"]);
        for field in ["source_digest", "source_envelope_digest"] {
            assert_eq!(decoded["manifest"][field], healthy["manifest"][field]);
        }
        decoded["manifest"]["retired"] = json!([]);
        assert_eq!(decoded, healthy);
        bad_wires.push(json!({
            "name":format!("retired-empty-{shape}"),"field":"retired_shape","shape":shape,
            "id":previous.id,"wire":wire,
        }));
    }
    assert_eq!(bad_wires.len(), 162);
    {
        let (root_dir, root_store) = database().await;
        let source = previous.source.clone();
        let registration = r9_registration(&source);
        let root = ReleaseStore::register_host_surface_from_source(
            &admin,
            &root_store,
            SURFACE_ID,
            source.clone(),
            registration.clone(),
            None,
        )
        .await
        .unwrap();
        assert!(root.predecessor_surface_id.is_none());
        assert!(root.manifest.retired.is_empty());
        assert!(root.manifest.items[0].renamed_from.is_none());
        let root_wire = serde_json::to_string(&root).unwrap();
        let path = root_dir.path().join("name-coverage.sqlite3");
        r9_install_first_dml_sentinel(&path);
        let before = current_image(&root_store, root_dir.path()).await;
        let mut semantic_wires = Vec::new();
        for retired in [true, false] {
            let mut changed_registration = registration.clone();
            let mut damaged = root.clone();
            if retired {
                changed_registration.retired = vec![evo_core::contract::RetiredHostSurfaceV2 {
                    name: "ghost".into(),
                    reason: "retired".into(),
                }];
                damaged.manifest.retired = changed_registration.retired.clone();
            } else {
                changed_registration.classifications[0].renamed_from = Some("ghost".into());
                damaged.manifest.items[0].renamed_from = Some("ghost".into());
            }
            damaged.manifest_digest = fingerprint(&damaged.manifest).unwrap();
            let wire = serde_json::to_string(&damaged).unwrap();
            evo_core::contract::decode_host_surface_record_v2_json(&wire).unwrap();
            let mut repaired = damaged.clone();
            if retired {
                repaired.manifest.retired.clear();
            } else {
                repaired.manifest.items[0].renamed_from = None;
            }
            repaired.manifest_digest = root.manifest_digest.clone();
            assert_eq!(serde_json::to_string(&repaired).unwrap(), root_wire);
            assert_invalid(
                ReleaseStore::register_host_surface_from_source(
                    &admin,
                    &root_store,
                    SURFACE_ID,
                    source.clone(),
                    changed_registration,
                    None,
                )
                .await,
                "invalid reviewed host source/default",
            );
            assert_eq!(current_image(&root_store, root_dir.path()).await, before);
            semantic_wires.push(wire);
        }
        assert_eq!(semantic_wires.len(), 2);
        root_store.close().await;
        for wire in semantic_wires {
            write_raw_host_body(&path, &wire);
            let cold = capture_raw_database_family(&path);
            let cold_logical = raw_logical_image(&path).await;
            let reader = Store::open(&path).await.unwrap();
            let warm = current_image(&reader, root_dir.path()).await;
            assert_eq!(warm.raw_sql, cold_logical);
            let mut session = reader.session().await.unwrap();
            assert_invalid(
                session
                    .need::<HostSurfaceRecordV2>(&admin, "artifact", SURFACE_ID)
                    .await,
                "invalid reviewed host source/default",
            );
            assert_invalid(
                session
                    .list::<HostSurfaceRecordV2>(&admin, "artifact")
                    .await,
                "invalid reviewed host source/default",
            );
            session.commit().await.unwrap();
            assert_eq!(current_image(&reader, root_dir.path()).await, warm);
            reader.close().await;
            assert_eq!(capture_raw_database_family(&path), cold);
            assert_eq!(raw_logical_image(&path).await, cold_logical);
        }
    }
    r10_writer_text_faults(&r10_text_inputs(), &faults).await;
    r10_restore_text_content(&store, dir.path(), &records, bad_wires).await;
}

async fn r11_number_admission_matrix() {
    let mut raw_cases = Vec::new();
    let mut raw_accepted = 0;
    let mut raw_refused = 0;
    for (token, accepted) in [
        ("1", true),
        ("1.0", true),
        ("8000000000000004", true),
        ("18446744073709551615", true),
        ("-9223372036854775808", true),
        ("-0", true),
        ("-0.0", true),
        ("1000000000000000000000", true),
        ("0e2147483648", true),
        ("-0e-2147483649", true),
        ("1.000000000000000000000000000000000000", true),
        ("5e-324", true),
        ("8000000000000004.0", false),
        ("51.248178375505404", false),
        ("18446744073709551616", false),
        ("-9223372036854775809", false),
        ("1e2147483648", false),
        ("1e-2147483649", false),
        ("1.000000000000000000000000000000000001", false),
        ("1e-400", false),
        ("1e309", false),
    ] {
        let raw = format!("{{\"schema_version\":\"rsia.host_surface_record.v2\",\"n\":{token}}}");
        let result = evo_core::contract::decode_host_surface_record_v2_json(&raw);
        if accepted {
            let decoded = result.unwrap();
            raw_accepted += 1;
            raw_cases.push(json!({
                "token":token,"accepted":true,
                "number_kind":if decoded["n"].is_f64() { "float" } else { "integer" },
                "number_wire":serde_json::to_string(&decoded["n"]).unwrap(),
            }));
        } else {
            assert_invalid(result, "invalid reviewed host source/default");
            raw_refused += 1;
            raw_cases.push(json!({"token":token,"accepted":false}));
        }
    }
    assert_eq!(raw_accepted, 12);
    assert_eq!(raw_refused, 9);
    let observed: serde_json::Number = serde_json::from_str("8000000000000004.0").unwrap();
    assert_eq!(
        serde_json::to_string(&observed).unwrap(),
        "8000000000000003.0"
    );

    let (dir, store) = database().await;
    let admin = context(Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"r11 actual number admission namespace"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut records = Vec::new();
    let mut replays = Vec::new();
    for (id, value) in [
        (
            "r11-number-integer",
            json!({"n":8000000000000004_i64,"safe":1_i64}),
        ),
        ("r11-number-float", json!({"n":1.0_f64})),
    ] {
        let source = r9_source(vec![r9_declaration("numbers", "object", value)]);
        let registration = r9_registration(&source);
        let record = ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            id,
            source.clone(),
            registration.clone(),
            None,
        )
        .await
        .unwrap();
        replays.push((id, source, registration));
        records.push(record);
    }
    assert_eq!(records.len(), 2);
    store.close().await;
    let store = Store::open(&dir.path().join("name-coverage.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
    assert_eq!(listed.len(), 2);
    for record in &records {
        let loaded: HostSurfaceRecordV2 =
            session.need(&admin, "artifact", &record.id).await.unwrap();
        assert_eq!(
            serde_json::to_string(&loaded).unwrap(),
            serde_json::to_string(record).unwrap()
        );
        evo_engine::release_store::validate_host_record_v2_source(&loaded).unwrap();
    }
    session.commit().await.unwrap();
    let before = current_image(&store, dir.path()).await;
    for (id, source, registration) in replays {
        let repeat = ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            id,
            source,
            registration,
            None,
        )
        .await
        .unwrap();
        let original = records.iter().find(|record| record.id == id).unwrap();
        assert_eq!(
            serde_json::to_string(&repeat).unwrap(),
            serde_json::to_string(original).unwrap()
        );
    }
    assert_eq!(current_image(&store, dir.path()).await, before);

    let record = &records[0];
    let original = serde_json::to_string(record).unwrap();
    let typed = serde_json::to_string(&record.extracted_typed).unwrap();
    let target = format!("\"extracted_typed\":{typed}");
    assert_eq!(original.matches(&target).count(), 1);
    let mut variants = Vec::new();
    let mut equivalent = None;
    for (field, old, new, accepted) in [
        ("safe", "1", "1.0", true),
        ("n", "8000000000000004", "8000000000000004.0", false),
    ] {
        let token = format!("\"{field}\":{old}");
        assert_eq!(typed.matches(&token).count(), 1);
        let changed = typed.replacen(&token, &format!("\"{field}\":{new}"), 1);
        let replacement = format!("\"extracted_typed\":{changed}");
        let wire = original.replacen(&target, &replacement, 1);
        assert_ne!(wire, original);
        assert_eq!(wire.replacen(&replacement, &target, 1), original);
        for (field, value) in [
            ("source", serde_json::to_string(&record.source).unwrap()),
            ("manifest", serde_json::to_string(&record.manifest).unwrap()),
            (
                "manifest_digest",
                serde_json::to_string(&record.manifest_digest).unwrap(),
            ),
        ] {
            assert!(wire.contains(&format!("\"{field}\":{value}")));
        }
        let result = evo_core::contract::decode_host_surface_record_v2_json(&wire);
        if accepted {
            let decoded: HostSurfaceRecordV2 = serde_json::from_value(result.unwrap()).unwrap();
            evo_engine::release_store::validate_host_record_v2_source(&decoded).unwrap();
            equivalent = Some(decoded);
        } else {
            assert_invalid(result, "invalid reviewed host source/default");
        }
        variants.push(json!({
            "id":record.id,"field":field,"old_token":old,"new_token":new,
            "accepted":accepted,"wire":wire,
        }));
    }
    let equivalent = equivalent.unwrap();
    assert_eq!(variants.len(), 2);
    let private = tempfile::tempdir().unwrap();
    let backup = private.path().join("number-admission-backup");
    store.backup(&backup).await.unwrap();
    store.close().await;
    let anchor = dir.path().join("name-coverage.sqlite3");
    let before = capture_raw_database_family(&anchor);
    let rows = records
        .iter()
        .map(|record| json!({"id":record.id,"wire":serde_json::to_string(record).unwrap()}))
        .collect::<Vec<_>>();
    let payload = private.path().join("number-admission-input.json");
    std::fs::write(
        &payload,
        serde_json::to_vec(&json!({
            "backup":backup,"anchor":anchor,"records":rows,
            "variants":variants,"raw_cases":raw_cases,"typed_wire":typed,
        }))
        .unwrap(),
    )
    .unwrap();
    let script = "import json,sys; from scripts.test_restore_backup import _rust_current_host_number_admission_matrix; print(json.dumps(_rust_current_host_number_admission_matrix(json.load(open(sys.argv[1],encoding='utf-8')))))";
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&payload)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "number admission bridge failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(capture_raw_database_family(&anchor), before);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    for (field, count) in [
        ("accepted", 2),
        ("equivalent_count", 1),
        ("isolated", 1),
        ("raw_accepted", 12),
        ("raw_refused", 9),
    ] {
        assert_eq!(response[field].as_u64(), Some(count));
    }
    for (field, changed) in [("restored", false), ("equivalent", true)] {
        let path = Path::new(response[field].as_str().unwrap()).join("rsia.sqlite3");
        let restored = Store::open(&path).await.unwrap();
        let mut session = restored.session().await.unwrap();
        let listed: Vec<HostSurfaceRecordV2> = session.list(&admin, "artifact").await.unwrap();
        assert_eq!(listed.len(), 2);
        for record in &records {
            let loaded: HostSurfaceRecordV2 =
                session.need(&admin, "artifact", &record.id).await.unwrap();
            let expected = if changed && record.id == equivalent.id {
                &equivalent
            } else {
                record
            };
            assert_eq!(
                serde_json::to_string(&loaded).unwrap(),
                serde_json::to_string(expected).unwrap()
            );
            evo_engine::release_store::validate_host_record_v2_source(&loaded).unwrap();
        }
        session.commit().await.unwrap();
        restored.close().await;
    }
}
