use evo_core::contract::{CapabilityLevel, HostCapabilities, HostSurfaceManifest, SystemSnapshot};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::release_store::{
    HOST_SURFACE_RECORD_SCHEMA, HostSurfaceRecord, PrepareRunRequest, ReleaseStore,
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

async fn raw_surface(store: &Store) -> Value {
    let mut s = store.session().await.unwrap();
    let raw = s
        .raw_object(&ctx(Role::Admin), "artifact", SURFACE_ID)
        .await
        .unwrap();
    s.commit().await.unwrap();
    raw
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
    let mut m = prepare_surface();
    let mut conflict = m.items[0].clone();
    conflict.reason = "historical conflicting interpretation".into();
    m.items.push(conflict);
    let historical = record(m, HOST_SURFACE_RECORD_SCHEMA);
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
    assert_eq!(state(&store).await, before);
    assert_eq!(raw_surface(&store).await, raw);
    store.close().await;
}

#[tokio::test]
async fn unknown_or_v2_record_schema_cannot_be_typed_read_or_prepared() {
    for schema in ["", "unknown", "rsia.host_surface_record.v2"] {
        let (_dir, store) = database().await;
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
        assert_eq!(raw_surface(&store).await, raw);
        store.close().await;
    }
}

#[tokio::test]
async fn exact_clones_keep_registration_idempotency_prepare_and_live_reload() {
    let (_dir, store) = database().await;
    let mut m = prepare_surface();
    m.items.extend(m.items.clone());
    let first = ReleaseStore::register_host_surface(
        &ctx(Role::Admin),
        &store,
        SURFACE_ID,
        m.clone(),
        extracted(),
    )
    .await
    .unwrap();
    let before = state(&store).await;
    let second =
        ReleaseStore::register_host_surface(&ctx(Role::Admin), &store, SURFACE_ID, m, extracted())
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
        let mut registered = ReleaseStore::register_host_surface(
            &ctx(Role::Admin),
            &store,
            SURFACE_ID,
            prepare_surface(),
            extracted(),
        )
        .await
        .unwrap();
        let mut snapshot = ReleaseStore::prepare_run(&ctx(Role::Host), &store, request())
            .await
            .unwrap();
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
        assert_eq!(raw_surface(&store).await, raw);
        store.close().await;
    }
}
