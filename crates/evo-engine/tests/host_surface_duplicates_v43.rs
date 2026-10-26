use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceManifest,
    HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1, RunProjection, SurfaceCoverage,
    SystemSnapshot,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::release_store::{
    AppliedRequestMaterial, HOST_SURFACE_RECORD_SCHEMA, HostSurfaceRecord, PrepareRunRequest,
    ReleaseStore, RunApplicationSnapshot,
};
use evo_storage::Store;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

const REFERENCE_JSON: &str = include_str!("../../../fixtures/reference-host/host_surface.v1.json");
const SURFACE_ID: &str = "surface-duplicates";
const RUN_ID: &str = "duplicates-run";

fn ctx(role: Role) -> Context {
    Context::new("duplicates-v43", "trusted-fixture", role).unwrap()
}

fn surface() -> HostSurfaceManifest {
    serde_json::from_str(REFERENCE_JSON).unwrap()
}

// Tool-only prepare fixtures have no native instruction slot. The original
// supported v1 golden remains used for registration/name-contract tests.
fn prepare_surface() -> HostSurfaceManifest {
    let mut manifest = surface();
    for item in &mut manifest.items {
        if item.coverage == evo_core::contract::SurfaceCoverage::Supported {
            item.coverage = evo_core::contract::SurfaceCoverage::RuntimeOwned;
            item.mapped_field = None;
            item.consumer = None;
            item.reason = "Tool-only fixture has no native instruction slot".into();
        }
    }
    manifest
}

fn extracted() -> Vec<String> {
    ["theme", "instruction", "model", "model"]
        .map(String::from)
        .to_vec()
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
    let store = Store::open(&dir.path().join("surface.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn state(store: &Store) -> (u64, u64, usize, usize) {
    let admin = ctx(Role::Admin);
    let mut s = store.session().await.unwrap();
    let artifacts = s.namespace_object_count(&admin, "artifact").await.unwrap();
    let runs = s.namespace_object_count(&admin, "run").await.unwrap();
    let mut edges = s
        .dependency_edges(&admin, "artifact", SURFACE_ID)
        .await
        .unwrap()
        .len();
    edges += s
        .dependency_edges(&admin, "artifact", &format!("run-snapshot-{RUN_ID}"))
        .await
        .unwrap()
        .len();
    s.commit().await.unwrap();
    (
        artifacts,
        runs,
        store.verify_audit(&admin).await.unwrap(),
        edges,
    )
}

// Observe original SQL bytes independently of the production typed reader:
// malformed Current labels must still be observable after that reader refuses.
async fn raw_surface(directory: &std::path::Path) -> Value {
    let script = r#"
import sqlite3,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
row=c.execute("SELECT body FROM objects WHERE namespace='duplicates-v43' AND kind='artifact' AND id=?",(sys.argv[2],)).fetchone()
assert row is not None
sys.stdout.write(row[0])
c.close()
"#;
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(directory.join("surface.sqlite3"))
        .arg(SURFACE_ID)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

// Model an authenticated old component record through the existing local
// storage/content-witness format, without claiming any registered real host.
async fn put_historical_fixture<T: Serialize>(store: &Store, role: Role, id: &str, body: &T) {
    let c = ctx(role);
    let prefix = match role {
        Role::Admin => "formal.admin.content.v1",
        Role::Host => "formal.host.content.v1",
        _ => unreachable!(),
    };
    let operation = format!(
        "{prefix}.{}",
        fingerprint(&(c.namespace(), "artifact", id)).unwrap()
    );
    let entity = fingerprint(&(
        c.namespace(),
        "artifact",
        id,
        evo_engine::formal_authority::content_digest(body).unwrap(),
    ))
    .unwrap();
    let mut s = store.session().await.unwrap();
    s.put(&c, "artifact", id, c.actor(), body).await.unwrap();
    s.audit(&c, &operation, &entity).await.unwrap();
    s.commit().await.unwrap();
}

fn record(manifest: HostSurfaceManifest, schema: &str) -> HostSurfaceRecord {
    HostSurfaceRecord {
        id: SURFACE_ID.into(),
        schema_version: schema.into(),
        manifest_digest: fingerprint(&manifest).unwrap(),
        manifest,
        extracted: extracted(),
    }
}

// Complete historical v1 wire is constructed as v1, never projected from a
// current snapshot. Local content witnesses model old component persistence,
// not a new registered real-host or Current-source authorization.
async fn historical_baseline(store: &Store) -> (HostSurfaceRecord, RunApplicationSnapshot) {
    let registered = record(prepare_surface(), HOST_SURFACE_RECORD_SCHEMA);
    put_historical_fixture(store, Role::Admin, SURFACE_ID, &registered).await;
    let r = request();
    let material = AppliedRequestMaterial {
        mandatory_context_digest: r.system_snapshot.mandatory_context_digest.clone(),
        task_input_digest: r.task_input_digest.clone(),
        instructions: Vec::new(),
        tools: r.system_snapshot.tools.clone(),
    };
    let snapshot = RunApplicationSnapshot {
        id: format!("run-snapshot-{}", r.run_id),
        schema_version: "rsia.run_application.v1".into(),
        run_id: r.run_id,
        profile_id: r.profile_id,
        pointer_epoch: 0,
        release_id: None,
        bundle_digest: None,
        environment_digest: r.system_snapshot.digest().unwrap(),
        system_snapshot: r.system_snapshot,
        host_surface_id: r.host_surface_id,
        host_surface_digest: registered.manifest_digest.clone(),
        host_capabilities_digest: fingerprint(&r.host_capabilities).unwrap(),
        projection: RunProjection {
            bundle_digest: String::new(),
            instructions: material.instructions.clone(),
            tools: material.tools.clone(),
        },
        request_digest: fingerprint(&material).unwrap(),
        request_material: material,
        capability_level: r.capability_level,
        evolution_enabled: r.evolution_enabled,
    };
    put_historical_fixture(store, Role::Host, &snapshot.id, &snapshot).await;
    let loaded = ReleaseStore::read_run_snapshot(&ctx(Role::Host), store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded).unwrap(),
        serde_json::to_vec(&snapshot).unwrap()
    );
    (registered, snapshot)
}

fn current_surface() -> (ReviewedHostSourceEnvelopeV1, HostSurfaceRegistrationV2) {
    let names = ["instruction", "model", "theme"];
    let declarations: Vec<_> = names
        .iter()
        .map(|name| {
            serde_json::json!({
                "name":name,"value_type":"string","enum_values":[],"default":{"kind":"none"}
            })
        })
        .collect();
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: serde_json::to_string(&serde_json::json!({
            "schema_version":"rsia.host_reviewed_declarations.v1",
            "host":"reference","host_version":"0.1.0","declarations":declarations
        }))
        .unwrap(),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "0.1.0".into(),
        classifications: names
            .into_iter()
            .map(|name| HostSurfaceClassificationV2 {
                name: name.into(),
                coverage: if name == "theme" {
                    SurfaceCoverage::Unsupported
                } else {
                    SurfaceCoverage::RuntimeOwned
                },
                mapped_field: None,
                consumer: None,
                reason: "ToolOnly software fixture; no native instruction attachment".into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            })
            .collect(),
        retired: vec![],
    };
    (source, registration)
}

// Distinguish fresh Current admission from the old snapshot reader/replay
// diagnostics below; a legacy record cannot create a new run or any state.
async fn assert_fresh_legacy_refused(store: &Store) {
    let before = state(store).await;
    let mut fresh = request();
    fresh.run_id = "fresh-legacy-run".into();
    assert!(matches!(
        ReleaseStore::prepare_run(&ctx(Role::Host), store, fresh).await,
        Err(Error::Invalid(message))
            if message == "conflicting host surface items for name instruction"
    ));
    assert_eq!(state(store).await, before);
}

fn assert_conflict<T: std::fmt::Debug>(result: evo_core::Result<T>) {
    assert!(
        matches!(result, Err(Error::Invalid(ref message))
            if message == "conflicting host surface items for name instruction"),
        "{result:?}"
    );
}

#[tokio::test]
async fn every_conflicting_registration_refuses_before_objects_audit_and_edges() {
    for change in 0..4 {
        let (_dir, store) = database().await;
        let mut m = surface();
        let mut item = m.items[0].clone();
        match change {
            0 => item.reason = "another reason".into(),
            1 => item.consumer = Some("another-consumer".into()),
            2 => item.mapped_field = Some("host.model".into()),
            _ => {
                item.coverage = evo_core::contract::SurfaceCoverage::RuntimeOwned;
                item.mapped_field = None;
                item.consumer = None;
            }
        }
        m.items.push(item);
        assert_conflict(
            ReleaseStore::register_host_surface(
                &ctx(Role::Admin),
                &store,
                SURFACE_ID,
                m,
                extracted(),
            )
            .await,
        );
        assert_eq!(state(&store).await, (0, 0, 0, 0));
        store.close().await;
    }
}

#[tokio::test]
async fn disk_legacy_conflict_is_readable_but_cannot_prepare_a_new_run() {
    let (dir, store) = database().await;
    let (_, mut snapshot) = historical_baseline(&store).await;
    let mut m = prepare_surface();
    let mut conflict = m.items[0].clone();
    conflict.reason = "historical conflicting interpretation".into();
    m.items.push(conflict);
    let historical = record(m, HOST_SURFACE_RECORD_SCHEMA);
    snapshot.host_surface_digest = historical.manifest_digest.clone();
    put_historical_fixture(&store, Role::Host, &snapshot.id, &snapshot).await;
    put_historical_fixture(&store, Role::Admin, SURFACE_ID, &historical).await;
    let raw = serde_json::to_value(&historical).unwrap();
    let before = state(&store).await;
    store.close().await;
    let store = Store::open(&dir.path().join("surface.sqlite3"))
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    let read: HostSurfaceRecord = s
        .need(&ctx(Role::Admin), "artifact", SURFACE_ID)
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), raw);
    assert_conflict(ReleaseStore::prepare_run(&ctx(Role::Host), &store, request()).await);
    assert_fresh_legacy_refused(&store).await;
    assert_eq!(state(&store).await, before);
    assert_eq!(raw_surface(dir.path()).await, raw);
    store.close().await;
}

#[tokio::test]
async fn unknown_or_v2_record_schema_cannot_be_typed_read_or_prepared() {
    // Each bad wire is an old v1 body relabelled, not a complete source-backed v2.
    for schema in ["", "unknown", "rsia.host_surface_record.v2"] {
        let (_dir, store) = database().await;
        historical_baseline(&store).await;
        let bad = record(prepare_surface(), schema);
        put_historical_fixture(&store, Role::Admin, SURFACE_ID, &bad).await;
        let raw = serde_json::to_value(&bad).unwrap();
        let before = state(&store).await;
        assert!(serde_json::from_value::<HostSurfaceRecord>(raw.clone()).is_err());
        let mut s = store.session().await.unwrap();
        assert!(
            s.get::<HostSurfaceRecord>(&ctx(Role::Admin), "artifact", SURFACE_ID)
                .await
                .is_err()
        );
        s.commit().await.unwrap();
        assert!(matches!(
            ReleaseStore::prepare_run(&ctx(Role::Host), &store, request()).await,
            Err(Error::Internal)
        ));
        assert_eq!(state(&store).await, before);
        assert_eq!(raw_surface(_dir.path()).await, raw);
        store.close().await;
    }
}

#[tokio::test]
async fn exact_clones_keep_registration_idempotency_prepare_and_live_reload() {
    let (_dir, store) = database().await;
    let mut m = prepare_surface();
    m.items.extend(m.items.clone());
    // Preserve duplicate v1 items and old UTF-8 golden as historical mechanics.
    // Current strict declarations are unique; the two contracts are not equivalent.
    m.validate_against_extraction(&extracted()).unwrap();
    let mut historical = record(m, HOST_SURFACE_RECORD_SCHEMA);
    historical.id = format!("{SURFACE_ID}-historical-clones");
    put_historical_fixture(&store, Role::Admin, &historical.id, &historical).await;
    let mut s = store.session().await.unwrap();
    let loaded: HostSurfaceRecord = s
        .need(&ctx(Role::Admin), "artifact", &historical.id)
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded).unwrap(),
        serde_json::to_vec(&historical).unwrap()
    );
    let (source, registration) = current_surface();
    let first = ReleaseStore::register_host_surface_from_source(
        &ctx(Role::Admin),
        &store,
        SURFACE_ID,
        source.clone(),
        registration.clone(),
        None,
    )
    .await
    .unwrap();
    let before = state(&store).await;
    let second = ReleaseStore::register_host_surface_from_source(
        &ctx(Role::Admin),
        &store,
        SURFACE_ID,
        source,
        registration,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    assert_eq!(state(&store).await, before);
    let snapshot = ReleaseStore::prepare_run(&ctx(Role::Host), &store, request())
        .await
        .unwrap();
    let frozen_state = state(&store).await;
    let reload = ReleaseStore::read_run_snapshot(&ctx(Role::Host), &store, RUN_ID)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&snapshot).unwrap(),
        fingerprint(&reload).unwrap()
    );
    let repeat = ReleaseStore::prepare_run(&ctx(Role::Host), &store, request())
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&snapshot).unwrap(),
        fingerprint(&repeat).unwrap()
    );
    assert_eq!(state(&store).await, frozen_state);
    store.close().await;
}

#[tokio::test]
async fn existing_prepare_and_reload_revalidate_authenticated_legacy_surface() {
    for invalid_schema in [false, true] {
        let (_dir, store) = database().await;
        let (mut registered, mut snapshot) = historical_baseline(&store).await;
        if invalid_schema {
            registered.schema_version = "rsia.host_surface_record.v2".into();
        } else {
            let mut item = registered.manifest.items[0].clone();
            item.reason = "authenticated historical conflict".into();
            registered.manifest.items.push(item);
            registered.manifest_digest = fingerprint(&registered.manifest).unwrap();
            snapshot.host_surface_digest = registered.manifest_digest.clone();
            put_historical_fixture(&store, Role::Host, &snapshot.id, &snapshot).await;
        }
        put_historical_fixture(&store, Role::Admin, SURFACE_ID, &registered).await;
        let raw = serde_json::to_value(&registered).unwrap();
        let before = state(&store).await;
        let reloaded = ReleaseStore::read_run_snapshot(&ctx(Role::Host), &store, RUN_ID).await;
        let repeated = ReleaseStore::prepare_run(&ctx(Role::Host), &store, request()).await;
        if invalid_schema {
            assert!(matches!(reloaded, Err(Error::Internal)), "{reloaded:?}");
            assert!(matches!(repeated, Err(Error::Internal)), "{repeated:?}");
        } else {
            assert_conflict(reloaded);
            assert_conflict(repeated);
        }
        assert_eq!(state(&store).await, before);
        assert_eq!(raw_surface(_dir.path()).await, raw);
        store.close().await;
    }
}
