//! E16.4 host authority tests. Structural fixtures never mint use or benefit.
use evo_core::contract::{
    AppliedReceipt, CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2,
    HostSurfaceManifest, HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
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

#[tokio::test]
async fn f21_relabelled_unverified_surfaces_cannot_enter_the_registration_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("registration.sqlite3"))
        .await
        .unwrap();
    let admin = context("admin", Role::Admin);
    let relabels: [(&str, &str, &str); 4] = [
        (
            "reference-host",
            "Unverified-no-fixed-version",
            "capital-unverified",
        ),
        ("claude-code-mcp-tool-only:v1", "1.0.0", "host-suffix"),
        ("Claude-Code", "1.0.0", "host-case"),
        (
            "vendor-claude-bridge",
            "2.0.0-fixed-smoke-tested",
            "host-family",
        ),
    ];
    for (host, version, id) in relabels {
        let mut manifest = unverified_claude_code_surface_candidate();
        manifest.host = host.into();
        manifest.host_version = version.into();
        let extracted = manifest
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect();
        assert!(
            matches!(
                ReleaseStore::register_host_surface(&admin, &store, id, manifest, extracted).await,
                Err(Error::Invalid(_))
            ),
            "relabel {host}/{version} must be refused"
        );
        let mut session = store.session().await.unwrap();
        assert!(
            session
                .get::<serde_json::Value>(&admin, "artifact", id)
                .await
                .unwrap()
                .is_none(),
            "relabel {host}/{version} must not persist"
        );
        session.commit().await.unwrap();
    }
}

async fn baseline_store() -> (tempfile::TempDir, Store, Context, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("host.sqlite3")).await.unwrap();
    let admin = context("admin", Role::Admin);
    let host = context("gateway", Role::Host);
    // These software declarations preserve the seven structural names without
    // promoting the unverified Claude candidate or granting native/tool use.
    let fields = [
        ("evo_prepare", SurfaceCoverage::RuntimeOwned),
        ("evo_feedback", SurfaceCoverage::RuntimeOwned),
        ("evo_propose", SurfaceCoverage::RuntimeOwned),
        ("evo_inspect", SurfaceCoverage::RuntimeOwned),
        ("shell_tool", SurfaceCoverage::Unsupported),
        ("web_search", SurfaceCoverage::Unsupported),
        ("model_selection", SurfaceCoverage::RuntimeOwned),
    ];
    let declarations: Vec<_> = fields
        .iter()
        .map(|(name, _)| {
            serde_json::json!({
                "name":name,"value_type":"opaque","enum_values":[],"default":{"kind":"unknown"}
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
            "host":"reference-host","host_version":"1.0.0-fixed-fixture",
            "declarations":declarations
        }))
        .unwrap(),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "0.2.0".into(),
        classifications: fields
            .into_iter()
            .map(|(name, coverage)| HostSurfaceClassificationV2 {
                name: name.into(),
                coverage,
                mapped_field: None,
                consumer: None,
                reason:
                    "ToolOnly software fixture; no verified native attachment or tool authority"
                        .into(),
                renamed_from: None,
                test_ref: None,
                capability: None,
            })
            .collect(),
        retired: vec![],
    };
    let registered = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "surface",
        source.clone(),
        registration,
        None,
    )
    .await
    .unwrap();
    assert_eq!(registered.source.original_utf8, source.original_utf8);
    assert_eq!(
        registered.manifest.source_digest,
        hash(source.original_utf8.as_bytes())
    );
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
            request_digest: snapshot.request_digest.clone(),
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
