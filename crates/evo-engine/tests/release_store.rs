use evo_core::contract::{
    CapabilityLevel, CompileParts, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile,
    SkillPatch, SkillSnapshot, SurfaceCoverage, SurfaceItem, SystemSnapshot, compile_bundle,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    AppliedRequestMaterial, PersistentRelease, PersistentReleaseState, PrepareRunRequest,
    ReleaseCandidateRecord, ReleaseStore, RunApplicationSnapshot, StageBundleRequest,
    TrustedHostExecutionEvidence, TypedSourceRef,
};
use evo_storage::Store;
use std::collections::BTreeSet;

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn bundle(profile_id: &str, content: &str) -> evo_core::contract::ResolvedBundle {
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
    compile_bundle(CompileParts {
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
    })
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

fn surface() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "reference-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: d("surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("runner".into()),
            reason: "reference fixture mapping".into(),
        }],
    }
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
    let staged = ReleaseStore::stage_bundle(
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
        ReleaseStore::stage_bundle(
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
        ReleaseStore::stage_bundle(
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
        ReleaseStore::stage_bundle(
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
    ReleaseStore::stage_bundle(
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
    ReleaseStore::register_host_surface(
        &admin,
        &store,
        "surface-1",
        surface(),
        vec!["model".into()],
    )
    .await
    .unwrap();
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
    ReleaseStore::register_host_surface(
        &admin,
        &store,
        "surface-1",
        surface(),
        vec!["model".into()],
    )
    .await
    .unwrap();
    let run = ReleaseStore::prepare_run(
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
            actual_request_material: run.request_material,
            environment_digest: run.environment_digest,
            host_surface_digest: run.host_surface_digest,
            host_capabilities_digest: run.host_capabilities_digest,
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
    let candidate = ReleaseStore::stage_bundle(
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

#[test]
fn release_envelopes_reject_unknown_json_and_bundle_digest_tampering() {
    let mut record = ReleaseCandidateRecord {
        id: "candidate-1".into(),
        schema_version: "rsia.release_candidate.v1".into(),
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
    let mut value = serde_json::to_value(&record).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("verdict".into(), serde_json::json!("improved"));
    assert!(serde_json::from_value::<ReleaseCandidateRecord>(value).is_err());
    record.bundle.digest = d("tampered");
    assert!(evo_engine::releases::validate_resolved_bundle_identity(&record.bundle).is_err());
}
