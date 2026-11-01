//! Public zero-baseline gates and source history. Nonempty Current/Active is not seeded here.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceRegistrationV2,
    ReviewedHostSourceEnvelopeV1, SurfaceCoverage, ToolSourceV1, host_surface_upgrade_diff_v2,
    host_surface_v2_from_source,
};
use evo_core::{Context, Error, Prepare, Role, fingerprint, hash};
use evo_engine::release_store::{
    CompletePrepareRunRequest, HostSurfaceRecordV2, ReleaseStore, TrustedHostExecutionEvidence,
    validate_host_record_v2_source,
};
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_storage::Store;
use serde_json::{Value, json};
use std::path::Path;

fn context(actor: &str, role: Role) -> Context {
    Context::new("tool-only-contract", actor, role).unwrap()
}

fn source(name: &str) -> ReviewedHostSourceEnvelopeV1 {
    ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: format!(
            r#"{{"schema_version":"rsia.host_reviewed_declarations.v1","host":"software-tool-only-host","host_version":"1","declarations":[{{"name":"{name}","value_type":"string","enum_values":[],"default":{{"kind":"none"}}}}]}}"#
        ),
    }
}

fn registration(name: &str, supported: bool) -> HostSurfaceRegistrationV2 {
    HostSurfaceRegistrationV2 {
        adapter_version: "1".into(),
        classifications: vec![HostSurfaceClassificationV2 {
            name: name.into(),
            coverage: if supported {
                SurfaceCoverage::Supported
            } else {
                SurfaceCoverage::RuntimeOwned
            },
            mapped_field: supported.then(|| "host.model".into()),
            consumer: supported.then(|| "runner".into()),
            reason: "software fixture, no real host support credit".into(),
            renamed_from: None,
            test_ref: supported.then(|| "software model projection fixture".into()),
            capability: supported.then(|| "fs_read".into()),
        }],
        retired: vec![],
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    service: HostService,
    host: Context,
    agent: Context,
    config: CompleteHostPrepareConfig,
}
impl Fixture {
    async fn new(supported: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("public-tool-only.sqlite3");
        let store = Store::open(&path).await.unwrap();
        ReleaseStore::register_host_surface_from_source(
            &context("admin", Role::Admin),
            &store,
            "surface",
            source("model"),
            registration("model", supported),
            None,
        )
        .await
        .unwrap();
        let host = context("gateway-host", Role::Host);
        let tool_source = ToolSourceV1 {
            schema_version: "rsia.tool_source.v1".into(),
            domain: "mcp_protocol".into(),
            producer_id: "rmcp.ToolRouter.list_all".into(),
            producer_version: "rmcp-3.3.0".into(),
            definitions: serde_json::from_str(include_str!(
                "fixtures/ag168-actual-mcp-descriptors.json"
            ))
            .unwrap(),
        };
        let config = CompleteHostPrepareConfig {
            profile_id: "profile".into(),
            system_snapshot: evo_core::contract::SystemSnapshotV3 {
                schema_version: "rsia.system_snapshot.v3".into(),
                profile_id: "profile".into(),
                host_id: "software-tool-only-host".into(),
                host_version: "1".into(),
                model_id: "disabled".into(),
                tools: tool_source.names().unwrap(),
                tool_source,
                mandatory_context_digest: hash(b"public mandatory context"),
            },
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: ["fs_read".into()].into(),
                granted: ["fs_read".into()].into(),
            },
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        };
        Self {
            _dir: dir,
            path,
            service: HostService::new(store, host.clone()).unwrap(),
            host,
            agent: context("agent", Role::Agent),
            config,
        }
    }
    fn prepare(&self, key: &str) -> Prepare {
        Prepare {
            request_key: key.into(),
            goal: "read actual project configuration".into(),
            capabilities: vec![],
        }
    }
    fn request(&self, run_id: &str) -> CompletePrepareRunRequest {
        CompletePrepareRunRequest {
            run_id: run_id.into(),
            profile_id: self.config.profile_id.clone(),
            system_snapshot: self.config.system_snapshot.clone(),
            host_surface_id: self.config.host_surface_id.clone(),
            host_capabilities: self.config.host_capabilities.clone(),
            task_input_digest: hash(b"public task"),
            evolution_enabled: self.config.evolution_enabled,
            capability_level: self.config.capability_level,
        }
    }
    async fn evidence(&self, run_id: &str) -> TrustedHostExecutionEvidence {
        let snapshot = self
            .service
            .read_run_snapshot(&self.host, run_id)
            .await
            .unwrap();
        TrustedHostExecutionEvidence {
            actual_request_material: snapshot.request_material.clone(),
            environment_digest: snapshot.environment_digest.clone(),
            host_surface_digest: snapshot.host_surface_digest.clone(),
            host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
            offered: vec![],
            attached: vec![],
            used: vec![],
            truncated: false,
            execution_receipt_id: None,
        }
    }
}

// Exact logical contents of every table, read only. This also observes audit,
// dependency and idempotency writes, rather than checking one expected key.
fn database_rows(path: &Path) -> Value {
    let result = std::process::Command::new("python3")
        .args([
            "-B",
            "-c",
            r#"
import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
print(json.dumps(list(con.iterdump()),sort_keys=True,separators=(',',':')))
con.close()
"#,
        ])
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

#[tokio::test]
async fn public_prepare_rejects_attached_without_skill_content_before_writes() {
    let fixture = Fixture::new(false).await;
    let healthy = fixture
        .service
        .prepare_complete(&fixture.agent, fixture.prepare("healthy"), &fixture.config)
        .await
        .unwrap();
    assert!(healthy.skills.is_empty());
    let before = database_rows(&fixture.path);
    let mut request = fixture.request("attached-failure");
    request.capability_level = CapabilityLevel::Attached;
    assert!(
        matches!(ReleaseStore::prepare_complete_run(&fixture.host, fixture.service.store(), request.clone()).await,
        Err(Error::Conflict(reason)) if reason == "host_capability_attached_requires_skill_content")
    );
    assert_eq!(database_rows(&fixture.path), before);
    assert!(matches!(
        ReleaseStore::prepare_complete_run(&fixture.agent, fixture.service.store(), request).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(database_rows(&fixture.path), before);
}

#[tokio::test]
async fn public_prepare_rejects_tool_only_supported_before_writes() {
    let fixture = Fixture::new(true).await;
    let before = database_rows(&fixture.path);
    assert!(
        matches!(ReleaseStore::prepare_complete_run(&fixture.host, fixture.service.store(), fixture.request("supported-failure")).await,
        Err(Error::Conflict(reason)) if reason == "host_capability_tool_only_requires_zero_supported")
    );
    assert_eq!(database_rows(&fixture.path), before);
    assert!(
        matches!(fixture.service.prepare_complete(&fixture.agent, fixture.prepare("supported-service-failure"), &fixture.config).await,
        Err(Error::Conflict(reason)) if reason == "host_capability_tool_only_requires_zero_supported")
    );
    assert_eq!(database_rows(&fixture.path), before);
}

#[tokio::test]
async fn registration_excludes_rsia_protocol_fields_and_keeps_history_readable() {
    let fixture = Fixture::new(false).await;
    let admin = context("admin", Role::Admin);
    for name in evo_core::contract::MODEL_TOOLS {
        let before = database_rows(&fixture.path);
        assert!(
            matches!(ReleaseStore::register_host_surface_from_source(&admin, fixture.service.store(), "new-protocol-surface", source(name), registration(name, false), None).await,
            Err(Error::Invalid(reason)) if reason == "host_surface_rsia_tool_is_not_configuration")
        );
        assert_eq!(database_rows(&fixture.path), before);
    }
    // An actual historical wire plus an actual Admin body witness is read-only
    // lineage, never a new registration or current-use capability proof.
    let historical_source = source("evo_prepare");
    let historical_registration = registration("evo_prepare", false);
    let (manifest, extracted_typed) =
        host_surface_v2_from_source(&historical_source, &historical_registration).unwrap();
    let upgrade_diff = host_surface_upgrade_diff_v2(&manifest, None).unwrap();
    let historical = HostSurfaceRecordV2 {
        id: "historical-protocol-surface".into(),
        schema_version: "rsia.host_surface_record.v2".into(),
        manifest_digest: fingerprint(&manifest).unwrap(),
        manifest,
        source: historical_source.clone(),
        extracted: vec!["evo_prepare".into()],
        extracted_typed,
        predecessor_surface_id: None,
        upgrade_diff,
    };
    validate_host_record_v2_source(&historical).unwrap();
    let mut session = fixture.service.store().session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &historical.id,
            admin.actor(),
            &historical,
        )
        .await
        .unwrap();
    let operation = format!(
        "formal.admin.content.v1.{}",
        fingerprint(&(admin.namespace(), "artifact", &historical.id)).unwrap()
    );
    let entity = fingerprint(&(
        admin.namespace(),
        "artifact",
        &historical.id,
        evo_core::canonical_fingerprint(&historical).unwrap(),
    ))
    .unwrap();
    session.audit(&admin, &operation, &entity).await.unwrap();
    session.commit().await.unwrap();
    let before = database_rows(&fixture.path);
    let replay = ReleaseStore::register_host_surface_from_source(
        &admin,
        fixture.service.store(),
        &historical.id,
        historical_source,
        historical_registration,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&replay).unwrap(),
        serde_json::to_vec(&historical).unwrap()
    );
    assert_eq!(database_rows(&fixture.path), before);
    assert_eq!(
        evo_engine::hosts::unverified_claude_code_surface_candidate()
            .items
            .len(),
        7
    );
}

#[tokio::test]
async fn normal_tool_only_prepare_and_cached_replay_keep_original_conflicts() {
    let fixture = Fixture::new(false).await;
    let request = fixture.prepare("normal");
    let first = fixture
        .service
        .prepare_complete(&fixture.agent, request.clone(), &fixture.config)
        .await
        .unwrap();
    let before = database_rows(&fixture.path);
    let replay = fixture
        .service
        .prepare_complete(&fixture.agent, request.clone(), &fixture.config)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert_eq!(database_rows(&fixture.path), before);
    let mut changed = request.clone();
    changed.goal.push_str(" changed");
    assert!(matches!(
        fixture
            .service
            .prepare_complete(&fixture.agent, changed, &fixture.config)
            .await,
        Err(Error::Conflict(_))
    ));
    let mut changed_config = fixture.config.clone();
    changed_config.capability_level = CapabilityLevel::Attached;
    assert!(matches!(
        fixture
            .service
            .prepare_complete(&fixture.agent, request, &changed_config)
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(database_rows(&fixture.path), before);
    let snapshot = fixture
        .service
        .read_run_snapshot(&fixture.host, &first.run.id)
        .await
        .unwrap();
    let mut changed_run = fixture.request(&first.run.id);
    changed_run.task_input_digest = snapshot.request_material.task_input_digest.clone();
    changed_run.capability_level = CapabilityLevel::Attached;
    assert!(
        matches!(ReleaseStore::prepare_complete_run(&fixture.host, fixture.service.store(), changed_run).await,
        Err(Error::Conflict(reason)) if reason == "run id reused with different frozen inputs")
    );
    assert_eq!(database_rows(&fixture.path), before);
    let feedback = fixture
        .service
        .feedback(
            &fixture.agent,
            evo_core::Feedback {
                request_key: "normal-feedback".into(),
                run_id: first.run.id.clone(),
                outcome: evo_core::Outcome::Failure,
                details: "host_capability_tool_only is only self-reported here".into(),
                failure_class: Default::default(),
            },
        )
        .await
        .unwrap();
    let proposal = evo_core::Proposal {
        request_key: "normal-proposal".into(),
        run_id: first.run.id.clone(),
        kind: evo_core::ArtifactKind::Skill,
        parent_snapshot: first.run.snapshot_id.clone(),
        hypothesis: "ordinary reasoning improvement".into(),
        applicability: "ordinary software scope".into(),
        counterexample: "unrelated setting".into(),
        content: "read precedence first".into(),
        evidence_refs: vec![feedback.id],
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let candidate = fixture
        .service
        .propose(&fixture.agent, proposal.clone())
        .await
        .unwrap();
    assert_eq!(candidate.state, evo_core::CandidateState::Proposed);
    let replay = fixture
        .service
        .propose(&fixture.agent, proposal)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(candidate).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
}

#[tokio::test]
async fn zero_skill_application_never_creates_environment_diagnostic() {
    let fixture = Fixture::new(false).await;
    let prepared = fixture
        .service
        .prepare_complete(
            &fixture.agent,
            fixture.prepare("zero-application"),
            &fixture.config,
        )
        .await
        .unwrap();
    let evidence = fixture.evidence(&prepared.run.id).await;
    let first = fixture
        .service
        .record_application(&fixture.host, &prepared.run.id, evidence.clone())
        .await
        .unwrap();
    assert!(first.receipt.is_none());
    let before = database_rows(&fixture.path);
    let replay = fixture
        .service
        .record_application(&fixture.host, &prepared.run.id, evidence)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    assert_eq!(database_rows(&fixture.path), before);
    let mut session = fixture.service.store().session().await.unwrap();
    let artifacts: Vec<Value> = session.list(&fixture.host, "artifact").await.unwrap();
    assert!(
        artifacts
            .iter()
            .all(|value| value["schema_version"] != "rsia.monitoring.environment.v2")
    );
    assert_eq!(
        session
            .namespace_object_count(&fixture.host, "candidate")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        session
            .namespace_object_count(&fixture.host, "capability_request")
            .await
            .unwrap(),
        0
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn forged_zero_skill_offering_never_creates_diagnostic() {
    let fixture = Fixture::new(false).await;
    let prepared = fixture
        .service
        .prepare_complete(
            &fixture.agent,
            fixture.prepare("forged-application"),
            &fixture.config,
        )
        .await
        .unwrap();
    let mut evidence = fixture.evidence(&prepared.run.id).await;
    evidence.offered = vec!["self-reported-skill".into()];
    let before = database_rows(&fixture.path);
    assert!(
        matches!(fixture.service.record_application(&fixture.host, &prepared.run.id, evidence.clone()).await,
        Err(Error::Invalid(reason)) if reason == "zero-skill baseline cannot claim application")
    );
    assert!(
        matches!(ReleaseStore::record_applied_request(&fixture.host, fixture.service.store(), &prepared.run.id, evidence).await,
        Err(Error::Invalid(reason)) if reason == "zero-skill baseline cannot claim application")
    );
    assert_eq!(database_rows(&fixture.path), before);
    let inspected = fixture
        .service
        .inspect(
            &fixture.agent,
            evo_core::Inspect {
                kind: evo_core::EntityKind::Run,
                id: prepared.run.id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        inspected,
        json!({"kind":"run","id":prepared.run.id,"owner":prepared.run.owner,"snapshot_id":prepared.run.snapshot_id,"status":prepared.run.status,"source":prepared.run.source,"created_at":prepared.run.created_at})
    );
}
