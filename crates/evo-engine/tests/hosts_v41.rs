//! E16.4 host authority tests. Structural fixtures never mint use or benefit.
use evo_core::contract::{
    AppliedReceipt, CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage,
    SystemSnapshot,
};
use evo_core::{Context, Error, Role, hash};
use evo_engine::hosts::{
    CLAUDE_CODE_TARGET, VerifiedHostStage, claude_code_support, detect_surface_drift,
    unverified_claude_code_surface_candidate, verify_host_receipt,
};
use evo_engine::release_store::{
    HOST_APPLICATION_SCHEMA, HostApplicationRecord, PrepareRunRequest, ReleaseStore,
};
use evo_storage::Store;
use std::fs;
use std::path::{Path, PathBuf};

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-host", actor, role).unwrap()
}

#[test]
fn claude_code_remains_blocked_without_fixed_version_handshake_and_smoke() {
    assert!(claude_code_support(None).is_err());
    assert!(claude_code_support(Some(&PathBuf::from("/nonexistent/claude"))).is_err());
    let executable_looking_file = tempfile::NamedTempFile::new().unwrap();
    let error = claude_code_support(Some(executable_looking_file.path())).unwrap_err();
    assert!(format!("{error}").contains("fixed version, handshake, and real smoke"));
}

#[test]
fn structural_surface_checks_do_not_change_blocked_support() {
    let manifest = unverified_claude_code_surface_candidate();
    let fields: Vec<String> = manifest
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    assert!(detect_surface_drift(&manifest, &fields).is_ok());
    assert!(manifest.host_version.starts_with("unverified-"));

    let mut drifted = fields.clone();
    drifted.push("agent_autonomy_mode".into());
    assert!(detect_surface_drift(&manifest, &drifted).is_err());

    let mut missing_consumer = manifest;
    missing_consumer.items[0].consumer = None;
    assert!(detect_surface_drift(&missing_consumer, &fields).is_err());
}

#[test]
fn checked_in_claude_fixture_is_only_an_untrusted_structural_fixture() {
    let fixture_path = Path::new("../../fixtures/hosts/claude_code_surface.v1.json");
    let content = if fixture_path.exists() {
        fs::read_to_string(fixture_path).unwrap()
    } else {
        fs::read_to_string("fixtures/hosts/claude_code_surface.v1.json").unwrap()
    };
    let manifest: HostSurfaceManifest = serde_json::from_str(&content).unwrap();
    assert_eq!(manifest.host, CLAUDE_CODE_TARGET);
    let extracted: Vec<String> = manifest
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    assert!(manifest.validate_against_extraction(&extracted).is_ok());
    assert!(claude_code_support(None).is_err());
}

#[tokio::test]
async fn unverified_claude_surface_cannot_enter_the_real_registration_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("registration.sqlite3"))
        .await
        .unwrap();
    let admin = context("admin", Role::Admin);
    let manifest = unverified_claude_code_surface_candidate();
    let extracted = manifest
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    assert!(matches!(
        ReleaseStore::register_host_surface(
            &admin,
            &store,
            "unverified-claude-surface",
            manifest,
            extracted,
        )
        .await,
        Err(Error::Invalid(_))
    ));
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .get::<serde_json::Value>(&admin, "artifact", "unverified-claude-surface")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();

    let mut relabelled = unverified_claude_code_surface_candidate();
    relabelled.host_version = "1.0.0".into();
    let extracted = relabelled
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    assert!(
        ReleaseStore::register_host_surface(
            &admin,
            &store,
            "relabelled-claude-surface",
            relabelled,
            extracted,
        )
        .await
        .is_err()
    );
}

async fn baseline_store() -> (tempfile::TempDir, Store, Context, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("host.sqlite3")).await.unwrap();
    let admin = context("admin", Role::Admin);
    let host = context("gateway", Role::Host);
    let mut manifest = unverified_claude_code_surface_candidate();
    manifest.host = "reference-host".into();
    manifest.host_version = "1.0.0-fixed-fixture".into();
    for item in &mut manifest.items {
        if item.coverage == SurfaceCoverage::Supported {
            item.consumer = Some("reference-fixture".into());
        }
    }
    let extracted = manifest
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    ReleaseStore::register_host_surface(&admin, &store, "surface", manifest, extracted)
        .await
        .unwrap();
    let run_id = "run-authority".to_string();
    ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: run_id.clone(),
            profile_id: "profile".into(),
            system_snapshot: SystemSnapshot {
                schema_version: "rsia.system_snapshot.v2".into(),
                profile_id: "profile".into(),
                host_id: "reference-host".into(),
                host_version: "1.0.0-fixed-fixture".into(),
                model_id: "disabled".into(),
                tools: vec!["evo_prepare".into()],
                mandatory_context_digest: hash(b"mandatory"),
            },
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            },
            task_input_digest: hash(b"task"),
            evolution_enabled: false,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    (dir, store, host, run_id)
}

#[tokio::test]
async fn caller_authored_used_and_benefit_cannot_mint_authority() {
    let (_dir, store, host, run_id) = baseline_store().await;
    let snapshot = ReleaseStore::read_run_snapshot(&host, &store, &run_id)
        .await
        .unwrap();
    let forged = HostApplicationRecord {
        id: format!("host-application-{run_id}"),
        schema_version: HOST_APPLICATION_SCHEMA.into(),
        run_id: run_id.clone(),
        release_id: None,
        actual_request_digest: snapshot.request_digest.clone(),
        execution_receipt_id: None,
        receipt: Some(AppliedReceipt {
            offered: vec!["invented".into()],
            attached: vec!["invented".into()],
            used: vec!["invented".into()],
            verified_benefit: vec!["invented".into()],
            bundle_digest: hash(b"invented-bundle"),
            request_digest: snapshot.request_digest,
            capability_level: CapabilityLevel::Attached,
            truncated: false,
            attested_by: "self".into(),
        }),
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&host, "receipt", &forged.id, host.actor(), &forged)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let result = verify_host_receipt(&host, &store, &run_id).await;
    assert!(matches!(result, Err(Error::Conflict(_))));
}

#[tokio::test]
async fn verifier_requires_a_real_persisted_application_and_trusted_role() {
    let (_dir, store, host, run_id) = baseline_store().await;
    assert!(matches!(
        verify_host_receipt(&host, &store, &run_id).await,
        Err(Error::NotFound)
    ));
    let agent = context("agent", Role::Agent);
    assert!(matches!(
        verify_host_receipt(&agent, &store, &run_id).await,
        Err(Error::Forbidden)
    ));
    let _ = VerifiedHostStage::Used;
}
