#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{
    CapabilityLevel, CompileParts, HostCapabilities, HostSurfaceClassificationV2,
    HostSurfaceManifest, HostSurfaceRegistrationV2, ImproverPatch, Profile,
    ReviewedHostSourceEnvelopeV1, SkillPatch, SkillSnapshot, SurfaceCoverage, SurfaceItem,
    SystemSnapshot, SystemSnapshotV3, ToolSourceV1,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    AppliedRequestMaterial, CompletePrepareRunRequest,
    CurrentReleaseCandidateRecord as ReleaseCandidateRecord,
    CurrentStageBundleRequest as StageBundleRequest, HostSurfaceRecord, HostSurfaceRecordV2,
    PersistentRelease, PersistentReleaseState, PrepareRunRequest, ReleaseStore,
    RunApplicationSnapshot, TrustedHostExecutionEvidence, TypedSourceRef,
};
use evo_storage::Store;
use std::collections::BTreeSet;

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn bundle(profile_id: &str, content: &str) -> evo_core::contract::ResolvedBundleV3 {
    let profile = Profile {
        id: profile_id.into(),
        evolution_enabled: true,
        parent_digest: d("parent"),
        baseline_digest: d("baseline"),
    };
    let parent = SkillSnapshot {
        content: content.into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    trusted_compiler_inputs::bundle_content_current(
        CompileParts {
            profile: &profile,
            parent: &parent,
            baseline: &SkillSnapshot::empty(),
            parent_strategy: &Strategy::default(),
            baseline_strategy: &Strategy::default(),
            skill_patch: &SkillPatch::default(),
            improver_patch: &ImproverPatch::default(),
            caps: &HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: &BTreeSet::new(),
        },
        "n",
        &system("p1").digest().unwrap(),
    )
    .unwrap()
}

fn system(profile_id: &str) -> SystemSnapshot {
    SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: profile_id.into(),
        host_id: "reference-host".into(),
        host_version: "1.0.0".into(),
        model_id: "model-v1".into(),
        tools: vec!["tool-a".into()],
        mandatory_context_digest: d("mandatory"),
    }
}

fn complete_system(profile_id: &str) -> SystemSnapshotV3 {
    let legacy = system(profile_id);
    let source = ToolSourceV1 {
        schema_version: evo_core::contract::TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    };
    SystemSnapshotV3 {
        schema_version: "rsia.system_snapshot.v3".into(),
        profile_id: legacy.profile_id,
        host_id: legacy.host_id,
        host_version: legacy.host_version,
        model_id: legacy.model_id,
        tools: source.names().unwrap(),
        mandatory_context_digest: legacy.mandatory_context_digest,
        tool_source: source,
    }
}

// Reviewed software declarations are data, not native-host or supported-use authority.
// The model field remains runtime-owned for these ToolOnly baseline controls.
fn surface() -> ReviewedHostSourceEnvelopeV1 {
    ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: concat!(
            "{\"schema_version\":\"rsia.host_reviewed_declarations.v1\",",
            "\"host\":\"reference-host\",\"host_version\":\"1.0.0\",\"declarations\":[",
            "{\"name\":\"model\",\"value_type\":\"string\",\"enum_values\":[],",
            "\"default\":{\"kind\":\"none\"}}]}"
        )
        .into(),
    }
}

fn surface_registration() -> HostSurfaceRegistrationV2 {
    HostSurfaceRegistrationV2 {
        adapter_version: "adapter-v1".into(),
        classifications: vec![HostSurfaceClassificationV2 {
            name: "model".into(),
            coverage: SurfaceCoverage::RuntimeOwned,
            mapped_field: None,
            consumer: None,
            reason: "model identity remains host-owned in this ToolOnly fixture".into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        }],
        retired: vec![],
    }
}

async fn register_surface(admin: &Context, store: &Store, id: &str) -> HostSurfaceRecordV2 {
    let source = surface();
    let record = ReleaseStore::register_host_surface_from_source(
        admin,
        store,
        id,
        source.clone(),
        surface_registration(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(record.source.original_utf8, source.original_utf8);
    assert_eq!(
        record.manifest.source_digest,
        hash(source.original_utf8.as_bytes())
    );
    assert_eq!(
        record.manifest_digest,
        evo_core::fingerprint(&record.manifest).unwrap()
    );
    record
}

fn authority(id: &str, body: &str) -> StoredTraceAuthority {
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

async fn setup_sources(store: &Store, namespace: &str) -> (Context, Context, Vec<TypedSourceRef>) {
    let host = ctx(namespace, "host", Role::Host);
    trusted_compiler_inputs::install_bundle_current_sources(store, &bundle("p1", "setup"))
        .await
        .unwrap();
    let proposer = ctx(namespace, "proposer", Role::Worker);
    let first = authority("run-1", "first trusted trace");
    let second = authority("run-2", "second trusted trace");
    store_trace_authority(store, &host, &first).await.unwrap();
    store_trace_authority(store, &host, &second).await.unwrap();
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&host, &d("initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(watermark, 1);
    (
        host,
        proposer,
        vec![
            TypedSourceRef {
                kind: "run".into(),
                id: first.record.id,
                content_digest: first.trace.source_digest,
            },
            TypedSourceRef {
                kind: "run".into(),
                id: second.record.id,
                content_digest: second.trace.source_digest,
            },
        ],
    )
}

async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("e06.sqlite3")).await.unwrap();
    (dir, store)
}

#[tokio::test]
async fn candidate_is_immutable_and_second_source_revoke_blocks_reuse() {
    let (_dir, store) = database().await;
    let (host, proposer, sources) = setup_sources(&store, "n").await;
    let environment = system("p1").digest().unwrap();
    let staged = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "candidate-1".into(),
            bundle: bundle("p1", "rule one"),
            environment_digest: environment.clone(),
            proposer_actor: proposer.actor().into(),
            sources: sources.clone(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(staged.sources.len(), 2);
    assert!(
        ReleaseStore::stage_current_bundle(
            &proposer,
            &store,
            StageBundleRequest {
                candidate_id: "candidate-1".into(),
                bundle: staged.bundle.clone(),
                environment_digest: d("changed-environment"),
                proposer_actor: proposer.actor().into(),
                sources: sources.clone(),
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("candidate id reused")
    );
    assert!(
        ReleaseStore::stage_current_bundle(
            &proposer,
            &store,
            StageBundleRequest {
                candidate_id: "candidate-1".into(),
                bundle: bundle("p1", "changed after evaluation"),
                environment_digest: environment,
                proposer_actor: proposer.actor().into(),
                sources: sources.clone(),
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("candidate id reused")
    );

    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "tombstone",
            "run-2",
            host.actor(),
            &serde_json::json!({"id":"run-2"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&host, &d("source-2-revoked"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        ReleaseStore::stage_current_bundle(
            &proposer,
            &store,
            StageBundleRequest {
                candidate_id: "candidate-1".into(),
                bundle: staged.bundle,
                environment_digest: staged.environment_digest,
                proposer_actor: proposer.actor().into(),
                sources,
                revoke_watermark: 2,
            },
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn self_approval_cross_namespace_and_unverified_report_are_rejected() {
    let (_dir, store) = database().await;
    let (_host, proposer, sources) = setup_sources(&store, "n").await;
    ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "candidate-1".into(),
            bundle: bundle("p1", "rule"),
            environment_digest: system("p1").digest().unwrap(),
            proposer_actor: proposer.actor().into(),
            sources,
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let self_admin = ctx("n", "proposer", Role::Admin);
    assert!(matches!(
        ReleaseStore::approve_verified(&self_admin, &store, "candidate-1", "report-missing").await,
        Err(Error::Forbidden)
    ));
    let other_admin = ctx("other", "approver", Role::Admin);
    assert!(
        ReleaseStore::approve_verified(&other_admin, &store, "candidate-1", "report-missing")
            .await
            .is_err()
    );
    let approver = ctx("n", "approver", Role::Admin);
    assert!(
        ReleaseStore::approve_verified(&approver, &store, "candidate-1", "report-missing")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn baseline_runs_are_frozen_and_profile_switch_has_no_managed_residue() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let host = ctx("n", "host", Role::Host);
    register_surface(&admin, &store, "surface-1").await;
    let first = ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: "run-a".into(),
            profile_id: "p1".into(),
            system_snapshot: system("p1"),
            host_surface_id: "surface-1".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d("task-a"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert!(first.release_id.is_none());
    assert!(first.projection.instructions.is_empty());
    assert_eq!(first.projection.tools, vec!["tool-a"]);
    let stored = ReleaseStore::read_run_snapshot(&host, &store, "run-a")
        .await
        .unwrap();
    assert_eq!(stored.request_digest, first.request_digest);

    let changed_same_run = ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: "run-a".into(),
            profile_id: "p1".into(),
            system_snapshot: system("p1"),
            host_surface_id: "surface-1".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d("different-task"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await;
    assert!(changed_same_run.is_err());

    let second = ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: "run-b".into(),
            profile_id: "p2".into(),
            system_snapshot: system("p2"),
            host_surface_id: "surface-1".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d("task-b"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert!(second.request_material.instructions.is_empty());
    assert_eq!(second.request_material.tools, vec!["tool-a"]);
}

#[tokio::test]
async fn raw_host_claims_cannot_turn_zero_skill_or_tool_only_into_used() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let host = ctx("n", "host", Role::Host);
    register_surface(&admin, &store, "surface-1").await;
    let run = ReleaseStore::prepare_complete_run(
        &host,
        &store,
        CompletePrepareRunRequest {
            run_id: "run-a".into(),
            profile_id: "p1".into(),
            system_snapshot: complete_system("p1"),
            host_surface_id: "surface-1".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d("task-a"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert!(
        ReleaseStore::record_applied_request(
            &host,
            &store,
            "run-a",
            TrustedHostExecutionEvidence {
                actual_request_material: run.request_material.clone(),
                environment_digest: run.environment_digest.clone(),
                host_surface_digest: run.host_surface_digest.clone(),
                host_capabilities_digest: run.host_capabilities_digest.clone(),
                offered: vec!["skill-a".into()],
                attached: vec!["skill-a".into()],
                used: vec!["skill-a".into()],
                execution_receipt_id: Some("claimed-only".into()),
                truncated: false,
            },
        )
        .await
        .is_err()
    );
    let recorded = ReleaseStore::record_applied_request(
        &host,
        &store,
        "run-a",
        TrustedHostExecutionEvidence {
            actual_request_material: run.request_material.clone(),
            environment_digest: run.environment_digest.clone(),
            host_surface_digest: run.host_surface_digest.clone(),
            host_capabilities_digest: run.host_capabilities_digest.clone(),
            offered: vec![],
            attached: vec![],
            used: vec![],
            execution_receipt_id: None,
            truncated: false,
        },
    )
    .await
    .unwrap();
    assert!(recorded.receipt.is_none());
}

#[tokio::test]
async fn arbitrary_rollback_target_is_rejected_without_active_pointer() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    assert!(
        ReleaseStore::rollback(&admin, &store, "p1", "made-up-digest", 0, &d("environment"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cached_active_run_requires_live_report_source_and_snapshot_records() {
    let (_dir, store) = database().await;
    let (host, proposer, sources) = setup_sources(&store, "n").await;
    let bundle = bundle("p1", "live rule");
    let environment = system("p1").digest().unwrap();
    let candidate = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "candidate-live".into(),
            bundle: bundle.clone(),
            environment_digest: environment.clone(),
            proposer_actor: proposer.actor().into(),
            sources,
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let release = PersistentRelease {
        id: "release-live".into(),
        schema_version: "rsia.persistent_release.v1".into(),
        candidate_id: candidate.id.clone(),
        bundle_digest: candidate.bundle_digest.clone(),
        environment_digest: environment.clone(),
        profile_id: "p1".into(),
        parent_digest: candidate.parent_digest.clone(),
        baseline_bundle_digest: d("old-bundle"),
        report_id: "report-fixture".into(),
        report_digest: d("report"),
        proposer_actor: proposer.actor().into(),
        evaluator_actor: "evaluator".into(),
        approved_by: "approver".into(),
        expected_parent_release_id: None,
        expected_pointer_epoch: 0,
        state: PersistentReleaseState::Active,
    };
    let material = AppliedRequestMaterial {
        mandatory_context_digest: d("mandatory"),
        task_input_digest: d("task"),
        instructions: vec!["live rule".into()],
        tools: vec!["tool-a".into()],
    };
    let snapshot = RunApplicationSnapshot {
        id: "run-snapshot-live-run".into(),
        schema_version: "rsia.run_application.v1".into(),
        run_id: "live-run".into(),
        profile_id: "p1".into(),
        pointer_epoch: 1,
        release_id: Some(release.id.clone()),
        bundle_digest: Some(bundle.digest.clone()),
        environment_digest: environment,
        system_snapshot: system("p1"),
        host_surface_id: "surface".into(),
        host_surface_digest: d("surface"),
        host_capabilities_digest: d("caps"),
        projection: evo_core::contract::RunProjection {
            bundle_digest: bundle.digest,
            instructions: material.instructions.clone(),
            tools: material.tools.clone(),
        },
        request_digest: evo_core::fingerprint(&material).unwrap(),
        request_material: material,
        capability_level: CapabilityLevel::Attached,
        evolution_enabled: true,
    };
    let admin = ctx("n", "approver", Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "release", &release.id, admin.actor(), &release)
        .await
        .unwrap();
    session
        .put(&host, "artifact", &snapshot.id, host.actor(), &snapshot)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        ReleaseStore::validate_run_snapshot_live(&host, &store, "live-run")
            .await
            .is_err()
    );
    assert!(
        ReleaseStore::read_run_snapshot(&host, &store, "live-run")
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "run-2",
            admin.actor(),
            &serde_json::json!({"id":"run-2"}),
        )
        .await
        .unwrap();
    session.bump_watermark(&admin, &d("revoked")).await.unwrap();
    session.commit().await.unwrap();
    assert!(
        ReleaseStore::validate_run_snapshot_live(&host, &store, "live-run")
            .await
            .is_err()
    );
    assert!(
        ReleaseStore::read_run_snapshot(&host, &store, "live-run")
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    session
        .delete(&admin, "artifact", "run-snapshot-live-run")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        ReleaseStore::validate_run_snapshot_live(&host, &store, "live-run")
            .await
            .is_err()
    );
    assert!(
        ReleaseStore::read_run_snapshot(&host, &store, "live-run")
            .await
            .is_err()
    );
}

// Full logical SQL image, including all source, dependency, audit and budget
// tables. This observer deliberately does not use the validator under test.
fn release_sql_image(directory: &std::path::Path) -> serde_json::Value {
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
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(directory.join("e06.sqlite3"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn release_envelopes_reject_unknown_json_and_bundle_digest_tampering() {
    let mut record = ReleaseCandidateRecord {
        id: "candidate-1".into(),
        schema_version: "rsia.release_candidate.v2".into(),
        bundle: bundle("p1", "rule"),
        bundle_digest: String::new(),
        environment_digest: d("environment"),
        profile_id: "p1".into(),
        parent_digest: d("parent"),
        proposer_actor: "proposer".into(),
        sources: vec![],
        revoke_watermark: 1,
    };
    record.bundle_digest = record.bundle.digest.clone();
    assert_eq!(
        evo_core::contract::resolved_bundle_digest_v3(&record.bundle).unwrap(),
        record.bundle.digest
    );
    let mut value = serde_json::to_value(&record).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("verdict".into(), serde_json::json!("improved"));
    assert!(serde_json::from_value::<ReleaseCandidateRecord>(value).is_err());
    record.bundle.digest = d("tampered");
    assert_ne!(
        evo_core::contract::resolved_bundle_digest_v3(&record.bundle).unwrap(),
        record.bundle.digest
    );
    // Retain the original V2 algorithm's real rejection independently of the
    // Current consumer. A legacy DTO alone grants no new execution authority.
    let mut legacy = trusted_compiler_inputs::software_candidate("n", &d("environment"), "rule");
    evo_engine::releases::validate_resolved_bundle_identity(&legacy).unwrap();
    legacy.digest = d("tampered");
    assert!(matches!(
        evo_engine::releases::validate_resolved_bundle_identity(&legacy),
        Err(Error::Conflict(ref m)) if m == "resolved bundle digest does not match its complete content"
    ));

    let (directory, store) = database().await;
    let (_, proposer, sources) = setup_sources(&store, "n").await;
    let request = StageBundleRequest {
        candidate_id: "healthy-envelope-candidate".into(),
        bundle: bundle("p1", "rule"),
        environment_digest: system("p1").digest().unwrap(),
        proposer_actor: proposer.actor().into(),
        sources,
        revoke_watermark: 1,
    };
    let staged = ReleaseStore::stage_current_bundle(&proposer, &store, request.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&staged.bundle).unwrap(),
        serde_json::to_vec(&request.bundle).unwrap()
    );
    let mut bad = request;
    bad.candidate_id = "tampered-envelope-candidate".into();
    bad.bundle.digest = d("tampered");
    let before = release_sql_image(directory.path());
    let refused = ReleaseStore::stage_current_bundle(&proposer, &store, bad).await;
    assert!(
        matches!(refused, Err(Error::Conflict(ref m)) if m == "Current resolved bundle identity differs"),
        "{refused:?}"
    );
    assert_eq!(release_sql_image(directory.path()), before);
    store.close().await;
}

#[tokio::test]
async fn restored_host_snapshot_rejects_resealed_public_object_without_host_witness() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let host = ctx("n", "host", Role::Host);
    let worker = ctx("n", "worker", Role::Worker);
    register_surface(&admin, &store, "surface").await;
    let snapshot = ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: "baseline-run".into(),
            profile_id: "p1".into(),
            system_snapshot: system("p1"),
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            },
            task_input_digest: d("task"),
            evolution_enabled: false,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert!(snapshot.release_id.is_none());
    ReleaseStore::read_run_snapshot(&host, &store, "baseline-run")
        .await
        .unwrap();
    let mut forged = snapshot.current_v2().unwrap().clone();
    forged.request_material.task_input_digest = d("other-task");
    forged.request_digest = evo_core::fingerprint(&forged.request_material).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&worker, "artifact", &forged.id, "host", &forged)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        ReleaseStore::read_run_snapshot(&host, &store, "baseline-run")
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    session
        .put(&worker, "artifact", &snapshot.id, "worker", &snapshot)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let restored = ReleaseStore::read_run_snapshot(&host, &store, "baseline-run")
        .await
        .unwrap();
    assert_eq!(
        evo_core::fingerprint(&restored).unwrap(),
        evo_core::fingerprint(&snapshot).unwrap()
    );
    store.close().await;
}

#[tokio::test]
async fn typed_host_surface_requires_actual_admin_witness_and_correct_kind() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let worker = ctx("n", "worker", Role::Worker);
    let record = register_surface(&admin, &store, "surface").await;
    let mut session = store.session().await.unwrap();
    evo_engine::formal_authority::read_typed_closure(
        &mut session,
        &worker,
        &[("artifact".into(), "surface".into())],
    )
    .await
    .unwrap();
    session
        .put(&worker, "pointer", &record.id, "admin", &record)
        .await
        .unwrap();
    assert!(
        evo_engine::formal_authority::read_typed_closure(
            &mut session,
            &worker,
            &[("pointer".into(), "surface".into())]
        )
        .await
        .is_err()
    );
    let mut forged = record.clone();
    forged.source.original_utf8 = forged.source.original_utf8.replace(
        "\"kind\":\"none\"",
        "\"kind\":\"value\",\"value\":\"replaced-model\"",
    );
    let (manifest, extracted_typed) =
        evo_core::contract::host_surface_v2_from_source(&forged.source, &surface_registration())
            .unwrap();
    forged.manifest = manifest;
    forged.extracted_typed = extracted_typed;
    forged.manifest_digest = evo_core::fingerprint(&forged.manifest).unwrap();
    evo_engine::release_store::validate_host_record_v2_source(&forged).unwrap();
    session
        .put(&worker, "artifact", &record.id, "admin", &forged)
        .await
        .unwrap();
    assert!(
        evo_engine::formal_authority::read_typed_closure(
            &mut session,
            &worker,
            &[("artifact".into(), "surface".into())]
        )
        .await
        .is_err()
    );
    session
        .put(&worker, "artifact", &record.id, "worker", &record)
        .await
        .unwrap();
    evo_engine::formal_authority::read_typed_closure(
        &mut session,
        &worker,
        &[("artifact".into(), "surface".into())],
    )
    .await
    .unwrap();
    session.commit().await.unwrap();
    store.close().await;
}

async fn audit_f01_stage_refuses_strategy_change(change_instruction: bool) {
    let (_dir, store) = database().await;
    let (_, proposer, sources) = setup_sources(&store, "n").await;
    let environment = system("p1").digest().unwrap();
    ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: d("audit-f01-healthy"),
            bundle: bundle("p1", "healthy skill"),
            environment_digest: environment.clone(),
            proposer_actor: proposer.actor().into(),
            sources: sources.clone(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let mut strategy = Strategy::default();
    if change_instruction {
        strategy
            .instruction
            .push_str(" unauthorized persistent instruction");
    } else {
        strategy.max_candidates = 3;
    }
    let profile = Profile {
        id: "p1".into(),
        evolution_enabled: true,
        parent_digest: d("parent"),
        baseline_digest: d("baseline"),
    };
    let invalid = trusted_compiler_inputs::bundle_content_current(
        CompileParts {
            profile: &profile,
            parent: &SkillSnapshot::empty(),
            baseline: &SkillSnapshot::empty(),
            parent_strategy: &strategy,
            baseline_strategy: &Strategy::default(),
            skill_patch: &SkillPatch::default(),
            improver_patch: &ImproverPatch::default(),
            caps: &HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            },
            revoked: &Default::default(),
        },
        "n",
        &environment,
    )
    .unwrap();
    let before = {
        let mut s = store.session().await.unwrap();
        s.namespace_object_count(&proposer, "artifact")
            .await
            .unwrap()
    };
    let id = d("audit-f01-refused");
    let result = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: id.clone(),
            bundle: invalid,
            environment_digest: environment,
            proposer_actor: proposer.actor().into(),
            sources,
            revoke_watermark: 1,
        },
    )
    .await;
    eprintln!(
        "AUDIT_F01 healthy Stage passed; changed_instruction={change_instruction}, target={result:?}"
    );
    assert!(matches!(result, Err(Error::Forbidden)));
    let mut s = store.session().await.unwrap();
    assert_eq!(
        s.namespace_object_count(&proposer, "artifact")
            .await
            .unwrap(),
        before
    );
    assert!(
        s.get::<serde_json::Value>(&proposer, "artifact", &id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.dependency_edges(&proposer, "artifact", &id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !s.has_role_audit_witness(&proposer, proposer.role(), "release.candidate.stage", &id)
            .await
            .unwrap()
    );
    drop(s);
    store.close().await;
}

#[tokio::test]
async fn audit_f01_direct_stage_rejects_unauthorized_instruction() {
    audit_f01_stage_refuses_strategy_change(true).await;
}

#[tokio::test]
async fn audit_f01_direct_stage_rejects_unauthorized_candidate_limit() {
    audit_f01_stage_refuses_strategy_change(false).await;
}

// These are explicit historical v1 records, not projections of Current wire
// and not authority for any real host or a new Current-source write.
fn source_gate_legacy_surface() -> HostSurfaceRecord {
    let item = SurfaceItem {
        name: "model".into(),
        coverage: SurfaceCoverage::RuntimeOwned,
        mapped_field: None,
        consumer: None,
        reason: "historical host-owned model field".into(),
    };
    let manifest = HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "reference-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: hash(b"historical model field source bytes"),
        // Exact clones retain the frozen v1 name-contract behavior.
        items: vec![item.clone(), item],
    };
    HostSurfaceRecord {
        id: "surface-legacy".into(),
        schema_version: "rsia.host_surface_record.v1".into(),
        manifest_digest: evo_core::fingerprint(&manifest).unwrap(),
        manifest,
        extracted: vec!["model".into(), "model".into()],
    }
}

async fn put_source_gate_history<T: serde::Serialize>(
    store: &Store,
    owner: &Context,
    id: &str,
    body: &T,
) {
    let prefix = match owner.role() {
        Role::Admin => "formal.admin.content.v1",
        Role::Host => "formal.host.content.v1",
        _ => unreachable!(),
    };
    let operation = format!(
        "{prefix}.{}",
        evo_core::fingerprint(&(owner.namespace(), "artifact", id)).unwrap()
    );
    let entity = evo_core::fingerprint(&(
        owner.namespace(),
        "artifact",
        id,
        evo_engine::formal_authority::content_digest(body).unwrap(),
    ))
    .unwrap();
    let mut s = store.session().await.unwrap();
    s.put(owner, "artifact", id, owner.actor(), body)
        .await
        .unwrap();
    s.audit(owner, &operation, &entity).await.unwrap();
    s.commit().await.unwrap();
}

async fn source_gate_state(store: &Store, admin: &Context) -> (u64, u64, u64, usize, usize) {
    let mut s = store.session().await.unwrap();
    let artifacts = s.namespace_object_count(admin, "artifact").await.unwrap();
    let receipts = s.namespace_object_count(admin, "receipt").await.unwrap();
    let runs = s.namespace_object_count(admin, "run").await.unwrap();
    let mut edges = 0;
    for (kind, id) in [
        ("artifact", "surface-legacy"),
        ("artifact", "run-snapshot-legacy-baseline"),
        ("receipt", "host-application-legacy-baseline"),
    ] {
        edges += s.dependency_edges(admin, kind, id).await.unwrap().len();
    }
    s.commit().await.unwrap();
    (
        artifacts,
        receipts,
        runs,
        store.verify_audit(admin).await.unwrap(),
        edges,
    )
}

#[tokio::test]
async fn rawless_v1_registration_keeps_history_but_cannot_create_source_authority() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let historical = source_gate_legacy_surface();
    historical
        .manifest
        .validate_against_extraction(&historical.extracted)
        .unwrap();
    let before = source_gate_state(&store, &admin).await;
    let rejected = ReleaseStore::register_host_surface(
        &admin,
        &store,
        &historical.id,
        historical.manifest.clone(),
        historical.extracted.clone(),
    )
    .await;
    assert!(
        matches!(rejected, Err(Error::Conflict(ref message))
        if message == "new v1 host surface registration lacks original source; use source-bound registration"),
        "{rejected:?}"
    );
    assert_eq!(source_gate_state(&store, &admin).await, before);

    // A matched new source-bound registration stays healthy.
    register_surface(&admin, &store, "surface-current").await;
    put_source_gate_history(&store, &admin, &historical.id, &historical).await;
    let wire = serde_json::to_vec(&historical).unwrap();
    let before = source_gate_state(&store, &admin).await;
    let repeated = ReleaseStore::register_host_surface(
        &admin,
        &store,
        &historical.id,
        historical.manifest.clone(),
        historical.extracted.clone(),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_vec(&repeated).unwrap(), wire);
    assert_eq!(source_gate_state(&store, &admin).await, before);

    let mut changed = historical.manifest.clone();
    changed.source_digest = hash(b"different historical source bytes");
    let reused = ReleaseStore::register_host_surface(
        &admin,
        &store,
        &historical.id,
        changed,
        historical.extracted.clone(),
    )
    .await;
    assert!(
        matches!(reused, Err(Error::Conflict(ref message))
        if message == "host surface id reused with different content"),
        "{reused:?}"
    );
    assert_eq!(source_gate_state(&store, &admin).await, before);
    let mut s = store.session().await.unwrap();
    let original: HostSurfaceRecord = s.need(&admin, "artifact", &historical.id).await.unwrap();
    s.commit().await.unwrap();
    assert_eq!(serde_json::to_vec(&original).unwrap(), wire);
    store.close().await;
}

#[tokio::test]
async fn legacy_baseline_reload_and_replay_cannot_authorize_new_applied_request() {
    let (_dir, store) = database().await;
    let admin = ctx("n", "admin", Role::Admin);
    let host = ctx("n", "host", Role::Host);
    let surface = source_gate_legacy_surface();
    put_source_gate_history(&store, &admin, &surface.id, &surface).await;
    let system = system("p1");
    let capabilities = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let material = AppliedRequestMaterial {
        mandatory_context_digest: system.mandatory_context_digest.clone(),
        task_input_digest: d("legacy-task"),
        instructions: Vec::new(),
        tools: system.tools.clone(),
    };
    let historical = RunApplicationSnapshot {
        id: "run-snapshot-legacy-baseline".into(),
        schema_version: "rsia.run_application.v1".into(),
        run_id: "legacy-baseline".into(),
        profile_id: "p1".into(),
        pointer_epoch: 0,
        release_id: None,
        bundle_digest: None,
        environment_digest: system.digest().unwrap(),
        system_snapshot: system.clone(),
        host_surface_id: surface.id.clone(),
        host_surface_digest: surface.manifest_digest.clone(),
        host_capabilities_digest: evo_core::fingerprint(&capabilities).unwrap(),
        projection: evo_core::contract::RunProjection {
            bundle_digest: String::new(),
            instructions: material.instructions.clone(),
            tools: material.tools.clone(),
        },
        request_digest: evo_core::fingerprint(&material).unwrap(),
        request_material: material,
        capability_level: CapabilityLevel::ToolOnly,
        evolution_enabled: true,
    };
    put_source_gate_history(&store, &host, &historical.id, &historical).await;
    let wire = serde_json::to_vec(&historical).unwrap();
    let before = source_gate_state(&store, &admin).await;
    let reloaded = ReleaseStore::read_run_snapshot(&host, &store, &historical.run_id)
        .await
        .unwrap();
    assert!(reloaded.current_v2().is_none());
    assert_eq!(serde_json::to_vec(&reloaded).unwrap(), wire);
    let repeated = ReleaseStore::prepare_run(
        &host,
        &store,
        PrepareRunRequest {
            run_id: historical.run_id.clone(),
            profile_id: historical.profile_id.clone(),
            system_snapshot: system.clone(),
            host_surface_id: surface.id.clone(),
            host_capabilities: capabilities.clone(),
            task_input_digest: historical.request_material.task_input_digest.clone(),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_vec(&repeated).unwrap(), wire);
    assert_eq!(source_gate_state(&store, &admin).await, before);
    let rejected = ReleaseStore::record_applied_request(
        &host,
        &store,
        &historical.run_id,
        TrustedHostExecutionEvidence {
            actual_request_material: historical.request_material.clone(),
            environment_digest: historical.environment_digest.clone(),
            host_surface_digest: historical.host_surface_digest.clone(),
            host_capabilities_digest: historical.host_capabilities_digest.clone(),
            offered: Vec::new(),
            attached: Vec::new(),
            used: Vec::new(),
            execution_receipt_id: None,
            truncated: false,
        },
    )
    .await;
    assert!(
        matches!(rejected, Err(Error::Conflict(ref message))
        if message == "legacy run snapshot cannot authorize a new applied request"),
        "{rejected:?}"
    );
    assert_eq!(source_gate_state(&store, &admin).await, before);
    let reloaded = ReleaseStore::read_run_snapshot(&host, &store, &historical.run_id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_vec(&reloaded).unwrap(), wire);

    register_surface(&admin, &store, "surface-current").await;
    let current = ReleaseStore::prepare_complete_run(
        &host,
        &store,
        CompletePrepareRunRequest {
            run_id: "current-baseline".into(),
            profile_id: "p1".into(),
            system_snapshot: complete_system("p1"),
            host_surface_id: "surface-current".into(),
            host_capabilities: capabilities,
            task_input_digest: d("current-task"),
            evolution_enabled: true,
            capability_level: CapabilityLevel::ToolOnly,
        },
    )
    .await
    .unwrap();
    assert!(current.current_v3().is_some());
    let applied = ReleaseStore::record_applied_request(
        &host,
        &store,
        &current.run_id,
        TrustedHostExecutionEvidence {
            actual_request_material: current.request_material.clone(),
            environment_digest: current.environment_digest.clone(),
            host_surface_digest: current.host_surface_digest.clone(),
            host_capabilities_digest: current.host_capabilities_digest.clone(),
            offered: Vec::new(),
            attached: Vec::new(),
            used: Vec::new(),
            execution_receipt_id: None,
            truncated: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(applied.run_id, current.run_id);
    assert!(applied.receipt.is_none());
    store.close().await;
}

#[tokio::test]
async fn current_diagnostic_stage_and_unapproved_rollback_bind_exact_history() {
    let (_directory, store) = database().await;
    let (_host, proposer, sources) = setup_sources(&store, "n").await;
    let request = StageBundleRequest {
        candidate_id: "current-diagnostic-history".into(),
        bundle: bundle("p1", "complete Current diagnostic candidate"),
        environment_digest: system("p1").digest().unwrap(),
        proposer_actor: proposer.actor().into(),
        sources,
        revoke_watermark: 1,
    };
    let staged = ReleaseStore::stage_current_bundle(&proposer, &store, request.clone())
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let loaded: evo_engine::release_store::ReadCandidate = session
        .need(&proposer, "artifact", &staged.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded).unwrap(),
        serde_json::to_vec(&staged).unwrap()
    );
    let original = session
        .list::<serde_json::Value>(&proposer, "artifact")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let audit = store
        .verify_audit(&ctx("n", "admin", Role::Admin))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(
            &ReleaseStore::stage_current_bundle(&proposer, &store, request)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_vec(&staged).unwrap()
    );
    let approver = ctx("n", "approver", Role::Admin);
    assert!(matches!(
        ReleaseStore::approve_verified(
            &approver,
            &store,
            &staged.id,
            "missing-authenticated-report"
        )
        .await,
        Err(Error::NotFound)
    ));
    assert!(
        matches!(ReleaseStore::rollback(&approver, &store, "p1", "unapproved-history", 0, &staged.environment_digest).await,
        Err(Error::Conflict(reason)) if reason == "profile has no active release")
    );
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .list::<serde_json::Value>(&proposer, "artifact")
            .await
            .unwrap(),
        original
    );
    assert!(
        session
            .list::<serde_json::Value>(&proposer, "release")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        session
            .list::<serde_json::Value>(&proposer, "pointer")
            .await
            .unwrap()
            .is_empty()
    );
    session.commit().await.unwrap();
    assert_eq!(
        store
            .verify_audit(&ctx("n", "admin", Role::Admin))
            .await
            .unwrap(),
        audit
    );
    store.close().await;
}
