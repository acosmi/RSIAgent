//! Local historical-wire models do not register real hosts or execution authority.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceManifest,
    HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1, RunProjection, SurfaceCoverage,
    SystemSnapshot,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::hosts::detect_surface_drift;
use evo_engine::release_store::{
    AppliedRequestMaterial, HOST_SURFACE_RECORD_SCHEMA, HostSurfaceRecord, PrepareRunRequest,
    ReleaseStore, RunApplicationSnapshot, TrustedHostExecutionEvidence,
};
use evo_storage::Store;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

const SURFACE_ID: &str = "legacy-consumer-surface";
const CURRENT_ID: &str = "source-bound-consumer-surface";
const RUN_ID: &str = "legacy-consumer-run";
const LEGACY_JSON: &str = r#"{"schema_version":"rsia.host_surface.v1","host":"reference","host_version":"0.1.0","adapter_version":"0.1.0","source_digest":"reference_host_surface_v1","items":[{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"reference-host","reason":"run-level instruction projection"},{"name":"model","coverage":"runtime_owned","mapped_field":null,"consumer":null,"reason":"model identity is host-declared"},{"name":"theme","coverage":"unsupported","mapped_field":null,"consumer":null,"reason":"UI theme is out of scope"}]}"#;
const MISMATCH: &str = "supported item consumer differs from field contract";
const SOURCE_REQUIRED: &str =
    "new v1 host surface registration lacks original source; use source-bound registration";

fn ctx(role: Role) -> Context {
    Context::new("consumers-v45", "trusted-fixture", role).unwrap()
}

fn legacy_record() -> HostSurfaceRecord {
    let manifest: HostSurfaceManifest = serde_json::from_str(LEGACY_JSON).unwrap();
    HostSurfaceRecord {
        id: SURFACE_ID.into(),
        schema_version: HOST_SURFACE_RECORD_SCHEMA.into(),
        manifest_digest: fingerprint(&manifest).unwrap(),
        manifest,
        extracted: ["instruction", "model", "theme"].map(String::from).to_vec(),
    }
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
    let store = Store::open(&dir.path().join("consumers.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

// The existing old-component content-witness format authenticates these local
// software models. It creates no Current source or registered principal set.
async fn put_historical<T: Serialize>(store: &Store, role: Role, id: &str, body: &T) {
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
    let mut session = store.session().await.unwrap();
    session
        .put(&c, "artifact", id, c.actor(), body)
        .await
        .unwrap();
    session.audit(&c, &operation, &entity).await.unwrap();
    session.commit().await.unwrap();
}

async fn historical_baseline(store: &Store) -> (HostSurfaceRecord, RunApplicationSnapshot) {
    let record = legacy_record();
    put_historical(store, Role::Admin, SURFACE_ID, &record).await;
    let r = request();
    let material = AppliedRequestMaterial {
        mandatory_context_digest: r.system_snapshot.mandatory_context_digest.clone(),
        task_input_digest: r.task_input_digest,
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
        host_surface_digest: record.manifest_digest.clone(),
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
    put_historical(store, Role::Host, &snapshot.id, &snapshot).await;
    ReleaseStore::read_run_snapshot(&ctx(Role::Host), store, RUN_ID)
        .await
        .unwrap();
    (record, snapshot)
}

async fn state(store: &Store) -> (u64, u64, usize, usize) {
    let admin = ctx(Role::Admin);
    let mut session = store.session().await.unwrap();
    let artifacts = session
        .namespace_object_count(&admin, "artifact")
        .await
        .unwrap();
    let runs = session.namespace_object_count(&admin, "run").await.unwrap();
    let mut edges = 0;
    for id in [
        SURFACE_ID,
        CURRENT_ID,
        "new-rawless",
        "run-snapshot-fresh-run",
    ] {
        edges += session
            .dependency_edges(&admin, "artifact", id)
            .await
            .unwrap()
            .len();
    }
    session.commit().await.unwrap();
    (
        artifacts,
        runs,
        store.verify_audit(&admin).await.unwrap(),
        edges,
    )
}

// Read SQL body strings independently so a typed reserialization cannot hide
// a writeback or wire-order change during legacy read/replay.
fn stored_wire(directory: &Path) -> Vec<u8> {
    let script = r#"
import json,sqlite3,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
rows=c.execute("SELECT kind,id,body FROM objects WHERE namespace='consumers-v45' ORDER BY kind,id").fetchall()
sys.stdout.write(json.dumps(rows,separators=(',',':')))
c.close()
"#;
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(directory.join("consumers.sqlite3"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
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
            "host":"reference","host_version":"0.2.0","declarations":declarations
        }))
        .unwrap(),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "0.2.0".into(),
        classifications: names
            .into_iter()
            .map(|name| HostSurfaceClassificationV2 {
                name: name.into(),
                coverage: if name == "model" {
                    SurfaceCoverage::RuntimeOwned
                } else {
                    SurfaceCoverage::Unsupported
                },
                mapped_field: None,
                consumer: None,
                reason:
                    "independent ToolOnly software declaration; no native instruction authority"
                        .into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            })
            .collect(),
        retired: vec![],
    };
    (source, registration)
}

fn assert_invalid<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == expected),
        "expected Invalid({expected:?}), got {result:?}"
    );
}

fn assert_conflict<T: std::fmt::Debug>(result: evo_core::Result<T>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Conflict(ref message)) if message == expected),
        "expected Conflict({expected:?}), got {result:?}"
    );
}

#[tokio::test]
async fn legacy_wrong_consumer_idempotency_and_run_reload_preserve_original_wire() {
    let (dir, store) = database().await;
    let (record, historical) = historical_baseline(&store).await;
    let original = stored_wire(dir.path());
    let before = state(&store).await;
    let idempotent = ReleaseStore::register_host_surface(
        &ctx(Role::Admin),
        &store,
        SURFACE_ID,
        record.manifest.clone(),
        record.extracted.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&idempotent).unwrap(),
        serde_json::to_vec(&record).unwrap()
    );
    store.close().await;
    let store = Store::open(&dir.path().join("consumers.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let typed: HostSurfaceRecord = session
        .need(&ctx(Role::Admin), "artifact", SURFACE_ID)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(typed.id, SURFACE_ID);
    assert_eq!(serde_json::to_string(&typed.manifest).unwrap(), LEGACY_JSON);
    assert_eq!(typed.manifest_digest, hash(LEGACY_JSON.as_bytes()));
    assert_eq!(fingerprint(&typed).unwrap(), fingerprint(&record).unwrap());
    for loaded in [
        ReleaseStore::read_run_snapshot(&ctx(Role::Host), &store, RUN_ID)
            .await
            .unwrap(),
        ReleaseStore::prepare_run(&ctx(Role::Host), &store, request())
            .await
            .unwrap(),
    ] {
        assert_eq!(loaded.schema_version, "rsia.run_application.v1");
        assert!(loaded.current_v2().is_none());
        assert_eq!(
            serde_json::to_vec(&loaded).unwrap(),
            serde_json::to_vec(&historical).unwrap()
        );
    }
    assert_eq!(state(&store).await, before);
    assert_eq!(stored_wire(dir.path()), original);
    store.close().await;
}

#[tokio::test]
async fn legacy_wrong_consumer_predecessor_survives_current_chain_without_projection_authority() {
    let (dir, store) = database().await;
    let record = legacy_record();
    put_historical(&store, Role::Admin, SURFACE_ID, &record).await;
    let (source, registration) = current_surface();
    let registered = ReleaseStore::register_host_surface_from_source(
        &ctx(Role::Admin),
        &store,
        CURRENT_ID,
        source.clone(),
        registration.clone(),
        Some(SURFACE_ID.into()),
    )
    .await
    .unwrap();
    assert_eq!(registered.source.original_utf8, source.original_utf8);
    assert_eq!(
        registered.manifest.source_digest,
        hash(source.original_utf8.as_bytes())
    );
    assert_eq!(
        registered.predecessor_surface_id.as_deref(),
        Some(SURFACE_ID)
    );
    assert_eq!(
        registered.upgrade_diff.legacy_untyped,
        ["instruction", "model", "theme"]
    );
    for (item, classification) in registered
        .manifest
        .items
        .iter()
        .zip(&registration.classifications)
    {
        assert_eq!(item.coverage, classification.coverage);
        assert_ne!(item.coverage, SurfaceCoverage::Supported);
        assert!(item.mapped_field.is_none());
        assert!(item.consumer.is_none());
        assert!(item.test_ref.is_none());
        assert!(item.capability.is_none());
    }
    let mut current_request = request();
    current_request.run_id = "source-bound-run".into();
    current_request.host_surface_id = CURRENT_ID.into();
    current_request.system_snapshot.host_version = "0.2.0".into();
    let prepared = ReleaseStore::prepare_run(&ctx(Role::Host), &store, current_request.clone())
        .await
        .unwrap();
    assert!(prepared.current_v2().is_some());
    assert!(prepared.projection.instructions.is_empty());
    assert_eq!(
        prepared.projection.tools,
        current_request.system_snapshot.tools
    );
    let original = stored_wire(dir.path());
    let before = state(&store).await;
    store.close().await;
    let store = Store::open(&dir.path().join("consumers.sqlite3"))
        .await
        .unwrap();
    let loaded = ReleaseStore::read_run_snapshot(&ctx(Role::Host), &store, "source-bound-run")
        .await
        .unwrap();
    let repeated = ReleaseStore::prepare_run(&ctx(Role::Host), &store, current_request)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&loaded).unwrap(),
        fingerprint(&prepared).unwrap()
    );
    assert_eq!(
        fingerprint(&repeated).unwrap(),
        fingerprint(&prepared).unwrap()
    );
    assert_eq!(stored_wire(dir.path()), original);
    assert_eq!(state(&store).await, before);
    store.close().await;
}

#[tokio::test]
async fn legacy_consumer_compatibility_keeps_identity_digest_and_witness_failures() {
    for fault in ["identity", "digest", "witness", "missing_name", "duplicate"] {
        let (dir, store) = database().await;
        let (mut record, _) = historical_baseline(&store).await;
        let expected = match fault {
            "identity" => {
                record.manifest.adapter_version = "different-content".into();
                "host surface id reused with different content"
            }
            "digest" => {
                record.manifest_digest = hash(b"wrong stored manifest digest");
                "host surface changed"
            }
            "witness" => {
                record.manifest.adapter_version = "unattested-content".into();
                record.manifest_digest = fingerprint(&record.manifest).unwrap();
                "missing authenticated content authorization; quarantine"
            }
            "missing_name" => {
                record.extracted.remove(0);
                "host surface field instruction is missing from extraction"
            }
            _ => {
                let mut duplicate = record.manifest.items[0].clone();
                duplicate.reason = "another historical meaning".into();
                record.manifest.items.push(duplicate);
                record.manifest_digest = fingerprint(&record.manifest).unwrap();
                "conflicting host surface items for name instruction"
            }
        };
        if fault == "witness" {
            let admin = ctx(Role::Admin);
            let mut session = store.session().await.unwrap();
            session
                .put(&admin, "artifact", SURFACE_ID, admin.actor(), &record)
                .await
                .unwrap();
            session.commit().await.unwrap();
        } else if fault != "identity" {
            put_historical(&store, Role::Admin, SURFACE_ID, &record).await;
        }
        let before = state(&store).await;
        let original = stored_wire(dir.path());
        if fault == "identity" {
            assert_conflict(
                ReleaseStore::register_host_surface(
                    &ctx(Role::Admin),
                    &store,
                    SURFACE_ID,
                    record.manifest,
                    record.extracted,
                )
                .await,
                expected,
            );
            let healthy = legacy_record();
            assert!(matches!(
                ReleaseStore::register_host_surface(
                    &ctx(Role::Host),
                    &store,
                    SURFACE_ID,
                    healthy.manifest,
                    healthy.extracted,
                )
                .await,
                Err(Error::Forbidden)
            ));
        } else {
            let loaded = ReleaseStore::read_run_snapshot(&ctx(Role::Host), &store, RUN_ID).await;
            let (source, registration) = current_surface();
            let successor = ReleaseStore::register_host_surface_from_source(
                &ctx(Role::Admin),
                &store,
                CURRENT_ID,
                source,
                registration,
                Some(SURFACE_ID.into()),
            )
            .await;
            if matches!(fault, "missing_name" | "duplicate") {
                assert_invalid(loaded, expected);
                assert_invalid(successor, expected);
            } else {
                assert_conflict(loaded, expected);
                assert_conflict(
                    successor,
                    if fault == "digest" {
                        "legacy predecessor identity differs"
                    } else {
                        expected
                    },
                );
            }
        }
        assert_eq!(state(&store).await, before, "{fault}");
        assert_eq!(stored_wire(dir.path()), original, "{fault}");
        store.close().await;
    }
}

#[tokio::test]
async fn strict_current_and_host_drift_reject_wrong_consumers_without_writes() {
    let (dir, store) = database().await;
    let (record, historical) = historical_baseline(&store).await;
    let before = state(&store).await;
    let original = stored_wire(dir.path());
    assert_invalid(
        detect_surface_drift(&record.manifest, &record.extracted),
        MISMATCH,
    );
    let mut fresh = request();
    fresh.run_id = "fresh-run".into();
    assert_invalid(
        ReleaseStore::prepare_run(&ctx(Role::Host), &store, fresh.clone()).await,
        MISMATCH,
    );
    assert_conflict(
        ReleaseStore::register_host_surface(
            &ctx(Role::Admin),
            &store,
            "new-rawless",
            record.manifest.clone(),
            record.extracted.clone(),
        )
        .await,
        SOURCE_REQUIRED,
    );
    assert_conflict(
        ReleaseStore::record_applied_request(
            &ctx(Role::Host),
            &store,
            RUN_ID,
            TrustedHostExecutionEvidence {
                actual_request_material: historical.request_material,
                environment_digest: historical.environment_digest,
                host_surface_digest: historical.host_surface_digest,
                host_capabilities_digest: historical.host_capabilities_digest,
                offered: Vec::new(),
                attached: Vec::new(),
                used: Vec::new(),
                execution_receipt_id: None,
                truncated: false,
            },
        )
        .await,
        "legacy run snapshot cannot authorize a new applied request",
    );
    assert_eq!(state(&store).await, before);
    assert_eq!(stored_wire(dir.path()), original);
    let mut healthy = record;
    healthy.manifest.items[0].consumer = Some("compiler".into());
    healthy.manifest_digest = fingerprint(&healthy.manifest).unwrap();
    detect_surface_drift(&healthy.manifest, &healthy.extracted).unwrap();
    put_historical(&store, Role::Admin, SURFACE_ID, &healthy).await;
    let before = state(&store).await;
    let original = stored_wire(dir.path());
    assert_invalid(
        ReleaseStore::prepare_run(&ctx(Role::Host), &store, fresh).await,
        "current prepare requires a source-bound host surface v2",
    );
    assert_eq!(state(&store).await, before);
    assert_eq!(stored_wire(dir.path()), original);
    store.close().await;
}
