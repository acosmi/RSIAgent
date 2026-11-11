//! AG-166b management controls for asset/local-export (v4.5 §6.1.2 L827,
//! §11.2–11.3.1, E16.1/E16.2).
//!
//! Real Store, real Admin contexts, real registered blobs and the real
//! `ManagementDispatcher` job lifecycle. The strict wire carries only a
//! transient input-blob digest, staged/candidate/export ids and a request key;
//! every physical root, destination, role, owner, namespace, Verified carrier,
//! permission boolean and body clock is refused before any session. Results
//! carry only identifiers, digests, bounded counters and a closed state.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementAssetExportResultV2, ManagementAssetExportState, ManagementDispatcher,
    ManagementJobState, ManagementJobStatusV1, ManagementResultV2,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::packages::{
    AssetPackageManifest, CandidateMaterialBinding, CandidateStrategyAuthority,
    E16_STAGED_ASSET_SCHEMA, E16Envelope, E16SourceRef, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER,
    PackageEntry, PackageKind, PersistentPackageStore, RedactionReport, RedactionStatus,
    StagePackageRequest, StagedAssetPayload, StagedAssetState,
};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn admin() -> Context {
    Context::new("tenant", "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("asset-export-management.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn bump(store: &Store, ctx: &Context, label: &str) {
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(ctx, &hash(label.as_bytes()))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn put_blob(store: &Store, ctx: &Context, bytes: &[u8]) -> String {
    store
        .put_blob_bounded(ctx, bytes, 64 * 1024 * 1024)
        .await
        .unwrap()
}

fn package_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "skill.md".into(),
            b"# Test Skill\nInstructions here\n".to_vec(),
        ),
        ("config.json".into(), b"{\"param\": 123}\n".to_vec()),
    ])
}

fn package_manifest(asset_id: &str, files: &BTreeMap<String, Vec<u8>>) -> AssetPackageManifest {
    AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: asset_id.into(),
        name: "Managed asset".into(),
        version: "1.0.0".into(),
        description: "Managed stage fixture".into(),
        license: "Apache-2.0".into(),
        entries: files
            .iter()
            .map(|(path, bytes)| PackageEntry {
                path: path.clone(),
                digest_sha256: hash(bytes),
                size_bytes: bytes.len() as u64,
                compressed_bytes: bytes.len() as u64,
            })
            .collect(),
        dependencies: vec![],
        redaction_report: RedactionReport {
            scanned_at: 1,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: PRIVACY_DISCLAIMER.into(),
            findings: vec![],
        },
        foreign_metadata: None,
    }
}

fn stage_input_bytes(
    asset_id: &str,
    files: &BTreeMap<String, Vec<u8>>,
    sources: &[E16SourceRef],
    candidate_material: Option<&CandidateMaterialBinding>,
) -> Vec<u8> {
    let text_files = files
        .iter()
        .map(|(path, bytes)| (path.clone(), String::from_utf8(bytes.clone()).unwrap()))
        .collect::<BTreeMap<_, _>>();
    serde_json::to_vec(&json!({
        "schema_version": "rsia.management.asset_stage_input.v1",
        "manifest": package_manifest(asset_id, files),
        "files": text_files,
        "source_refs": sources,
        "baseline_digest": hash(b"baseline"),
        "local_digest": hash(b"local"),
        "upstream_digest": null,
        "environment_digest": hash(b"environment"),
        "compiler_version": "compiler-v1",
        "candidate_material": candidate_material,
    }))
    .unwrap()
}

async fn source(store: &Store, ctx: &Context, id: &str) -> E16SourceRef {
    let body = json!({"schema_version":"fixture.source.v1","id":id});
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, ctx.actor(), &body)
        .await
        .unwrap();
    session.commit().await.unwrap();
    E16SourceRef {
        kind: "artifact".into(),
        id: id.into(),
        digest: fingerprint(&body).unwrap(),
    }
}

fn stage_payload(key: &str, digest: &str) -> Value {
    json!({
        "schema_version": "rsia.management.asset_stage.v1",
        "request_key": key,
        "input_blob_digest": digest,
    })
}

fn abort_payload(key: &str, staged: &str) -> Value {
    json!({
        "schema_version": "rsia.management.asset_abort.v1",
        "request_key": key,
        "staged_asset_id": staged,
    })
}

fn handoff_payload(key: &str, staged: &str, candidate: &str) -> Value {
    json!({
        "schema_version": "rsia.management.asset_handoff.v1",
        "request_key": key,
        "staged_asset_id": staged,
        "candidate_id": candidate,
    })
}

fn prepare_payload(key: &str, staged: &str) -> Value {
    json!({
        "schema_version": "rsia.management.export_prepare.v1",
        "request_key": key,
        "staged_asset_id": staged,
    })
}

fn complete_payload(key: &str, export_id: &str) -> Value {
    json!({
        "schema_version": "rsia.management.export_complete.v1",
        "request_key": key,
        "export_id": export_id,
    })
}

fn export_abort_payload(key: &str, export_id: &str) -> Value {
    json!({
        "schema_version": "rsia.management.export_abort.v1",
        "request_key": key,
        "export_id": export_id,
    })
}

async fn terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
) -> ManagementJobStatusV1 {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let status = dispatcher.status_view(ctx, job_id).await.unwrap();
            if matches!(
                status.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("asset/export management job reaches a terminal state")
}

async fn done(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    operation: &str,
    payload: Value,
) -> ManagementJobStatusV1 {
    let queued = dispatcher.submit(ctx, operation, payload).await.unwrap();
    terminal(dispatcher, ctx, queued.identity().id).await
}

fn asset_result(status: &ManagementJobStatusV1) -> &ManagementAssetExportResultV2 {
    match status.result.as_ref() {
        Some(ManagementResultV2::AssetExport(result)) => result,
        other => panic!("expected AssetExport result, got {other:?}"),
    }
}

async fn staged_value(store: &Store, ctx: &Context, id: &str) -> E16Envelope<StagedAssetPayload> {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn direct_stage(
    store: &Store,
    ctx: &Context,
    key: &str,
    asset_id: &str,
    files: BTreeMap<String, Vec<u8>>,
    sources: Vec<E16SourceRef>,
) -> E16Envelope<StagedAssetPayload> {
    PersistentPackageStore::stage_package(
        ctx,
        store,
        StagePackageRequest {
            request_key: key.into(),
            manifest: package_manifest(asset_id, &files),
            files,
            source_refs: sources,
            baseline_digest: hash(b"baseline"),
            local_digest: hash(b"local"),
            upstream_digest: None,
            environment_digest: hash(b"environment"),
            compiler_version: "compiler-v1".into(),
            candidate_material: None,
        },
    )
    .await
    .unwrap()
}

async fn management_stage(
    dispatcher: &ManagementDispatcher,
    store: &Store,
    ctx: &Context,
    key: &str,
    asset_id: &str,
    files: BTreeMap<String, Vec<u8>>,
    sources: &[E16SourceRef],
) -> ManagementJobStatusV1 {
    let input = stage_input_bytes(asset_id, &files, sources, None);
    let digest = put_blob(store, ctx, &input).await;
    done(dispatcher, ctx, "asset.stage", stage_payload(key, &digest)).await
}

async fn direct_prepare(
    store: &Store,
    ctx: &Context,
    key: &str,
    staged_id: &str,
) -> E16Envelope<evo_engine::packages::ExportAttemptPayload> {
    PersistentPackageStore::prepare_export(ctx, store, key, staged_id, "local_namespace", None)
        .await
        .unwrap()
}

// The real Admin role is judged before any payload or source observation.
#[tokio::test]
async fn six_asset_export_handlers_check_role_before_payload() {
    let (dir, store) = store().await;
    let admin = admin();
    let evaluator = Context::new("tenant", "evaluator", Role::Evaluator).unwrap();
    bump(&store, &admin, "role-watermark").await;
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let path = dir.path().join("asset-export-management.sqlite3");
    let before = management_sql_snapshot(&path);
    let digest = hash(b"never-read");
    for (operation, payload) in [
        ("asset.stage", stage_payload("role-stage", &digest)),
        (
            "asset.abort",
            abort_payload("role-abort", "e16-staged-role"),
        ),
        (
            "asset.handoff",
            handoff_payload("role-handoff", "e16-staged-role", "candidate-role"),
        ),
        (
            "export.prepare",
            prepare_payload("role-prepare", "e16-staged-role"),
        ),
        (
            "export.complete",
            complete_payload("role-complete", "e16-export-attempt-role"),
        ),
        (
            "export.abort",
            export_abort_payload("role-export-abort", "e16-export-attempt-role"),
        ),
    ] {
        for payload in [Value::Null, payload] {
            let error = dispatcher
                .submit(&evaluator, operation, payload)
                .await
                .unwrap_err();
            assert!(matches!(error, Error::Forbidden), "{operation}: {error:?}");
        }
    }
    assert_eq!(management_sql_snapshot(&path), before);
    store.close().await;
}

// A stored projection blob becomes the real Staged asset: the server-derived
// manifest/digest/bytes land under the authenticated namespace, the result is
// bounded, and the same key replays the same job and effect.
#[tokio::test]
async fn asset_stage_uses_stored_projection_and_fixed_blob_root() {
    let (_dir, store) = store().await;
    let admin = admin();
    let source = source(&store, &admin, "stage-source").await;
    bump(&store, &admin, "stage-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let files = package_files();
    let input = stage_input_bytes("managed_asset", &files, std::slice::from_ref(&source), None);
    let input_digest = put_blob(&store, &admin, &input).await;
    let payload = stage_payload("stage-real", &input_digest);
    let status = done(&dispatcher, &admin, "asset.stage", payload.clone()).await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "asset_staged");
    let ManagementAssetExportResultV2::AssetStaged {
        staged_asset_id,
        manifest_digest,
        content_blob_digest,
        content_bytes,
        state,
    } = asset_result(&status)
    else {
        panic!("expected AssetStaged");
    };
    assert_eq!(*state, ManagementAssetExportState::Staged);
    let wire = staged_value(&store, &admin, staged_asset_id).await;
    assert_eq!(wire.schema_version, E16_STAGED_ASSET_SCHEMA);
    assert_eq!(wire.namespace, "tenant");
    assert_eq!(wire.owner_actor, "admin");
    assert_eq!(wire.payload.state, StagedAssetState::Staged);
    assert_eq!(&wire.payload.manifest_digest, manifest_digest);
    assert_eq!(&wire.payload.content_blob_digest, content_blob_digest);
    assert_eq!(wire.payload.content_bytes, *content_bytes);
    assert_eq!(
        wire.payload.manifest_digest,
        fingerprint(&package_manifest("managed_asset", &files)).unwrap()
    );
    let blob = store
        .read_blob(&admin, content_blob_digest, 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(hash(&blob), *content_blob_digest);
    assert_eq!(blob.len() as u64, *content_bytes);

    // The persisted private wire has no physical root or destination.
    let mut session = store.session().await.unwrap();
    let private: Value = session
        .need(&admin, "artifact", &status.private_input_ref)
        .await
        .unwrap();
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    let wire = private["payload"].to_string();
    for forbidden in ["roots", "\"path\"", "destination", "namespace", "\"owner\""] {
        assert!(!wire.contains(forbidden), "wire holds {forbidden}: {wire}");
    }
    assert!(wire.contains(&input_digest));
    assert_eq!(
        artifacts
            .iter()
            .filter(|value| value["schema_version"] == E16_STAGED_ASSET_SCHEMA)
            .count(),
        1
    );
    assert!(artifacts.iter().all(
        |value| value["schema_version"] != "rsia.e16.staged_asset.v1"
            || value["payload"]["content_blob_digest"] == *content_blob_digest
    ));
    assert!(
        artifacts
            .iter()
            .all(|value| value["schema_version"] != "rsia.e16.export_attempt.v2")
    );

    // Same key, same input: the same job and the same single artifact.
    let replay = done(&dispatcher, &admin, "asset.stage", payload).await;
    assert_eq!(replay.id, status.id);
    assert_eq!(
        serde_json::to_value(&replay.result).unwrap(),
        serde_json::to_value(&status.result).unwrap()
    );
    let mut session = store.session().await.unwrap();
    let after: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(after.len(), artifacts.len());
    store.close().await;
}

// Every client-selected path/root/destination/role/owner/namespace/Verified
// carrier/permission boolean is an unknown wire field; an absolute member path
// in the controlled input blob is refused by the package kernel.
#[tokio::test]
async fn asset_stage_rejects_client_paths_and_authority_claims() {
    let (dir, store) = store().await;
    let admin = admin();
    let source = source(&store, &admin, "reject-source").await;
    bump(&store, &admin, "reject-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("asset-export-management.sqlite3");
    let before = management_sql_snapshot(&path);
    let digest = hash(b"unpublished-input");
    let base = stage_payload("reject-stage", &digest);
    for (field, value) in [
        ("root", json!("/tmp/root")),
        ("physical_root", json!("/tmp/root")),
        ("path", json!("/tmp/x")),
        ("destination", json!("/tmp/out")),
        ("destination_scope", json!("local_namespace")),
        ("role", json!("admin")),
        ("owner", json!("admin")),
        ("namespace", json!("other")),
        ("verified", json!(true)),
        ("verified_carrier", json!("trusted")),
        ("permission", json!(true)),
        ("outbound_authorized", json!(true)),
        ("body_clock", json!(1)),
        (
            "budget_ref",
            json!({"root_budget_id": "root", "billing_scope": "scope"}),
        ),
        ("candidate_material", json!({})),
    ] {
        let mut payload = base.clone();
        payload[field] = value;
        let error = dispatcher
            .submit(&admin, "asset.stage", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "invalid management payload"),
            "{field}: {error:?}"
        );
    }
    for (operation, mut payload) in [
        ("asset.abort", abort_payload("reject-abort", "e16-staged-x")),
        (
            "export.prepare",
            prepare_payload("reject-prepare", "e16-staged-x"),
        ),
        (
            "export.complete",
            complete_payload("reject-complete", "e16-export-x"),
        ),
    ] {
        payload["root"] = json!("/tmp/root");
        assert!(matches!(
            dispatcher
                .submit(&admin, operation, payload)
                .await
                .unwrap_err(),
            Error::Invalid(_)
        ));
    }
    assert_eq!(management_sql_snapshot(&path), before);

    // A member path outside the package namespace is refused even inside the
    // controlled blob; the job fails honestly and writes no staged asset.
    let mut files = package_files();
    files.insert("/etc/passwd".into(), b"root\n".to_vec());
    let input = stage_input_bytes("absolute_asset", &files, &[source], None);
    let digest = put_blob(&store, &admin, &input).await;
    let failed = done(
        &dispatcher,
        &admin,
        "asset.stage",
        stage_payload("reject-absolute-member", &digest),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("invalid_input"));
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert!(
        artifacts
            .iter()
            .all(|value| value["schema_version"] != E16_STAGED_ASSET_SCHEMA),
        "no staged asset for an absolute member"
    );
    store.close().await;
}

// Abort is a closure: replay is idempotent, shared blobs stay readable and the
// record keeps occupying the instance staging capacity.
#[tokio::test]
async fn asset_abort_replays_state_and_preserves_shared_blobs() {
    let (_dir, store) = store().await;
    let admin = admin();
    let source = source(&store, &admin, "abort-source").await;
    bump(&store, &admin, "abort-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let files = package_files();
    let stage_status = management_stage(
        &dispatcher,
        &store,
        &admin,
        "abort-stage",
        "abort_asset",
        files,
        &[source],
    )
    .await;
    assert_eq!(stage_status.state, ManagementJobState::Succeeded);
    let (staged_id, content_digest) = match asset_result(&stage_status) {
        ManagementAssetExportResultV2::AssetStaged {
            staged_asset_id,
            content_blob_digest,
            ..
        } => (staged_asset_id.clone(), content_blob_digest.clone()),
        other => panic!("{other:?}"),
    };

    let first = done(
        &dispatcher,
        &admin,
        "asset.abort",
        abort_payload("abort-one", &staged_id),
    )
    .await;
    assert_eq!(first.state, ManagementJobState::Succeeded, "{first:?}");
    assert_eq!(first.step, "asset_aborted");
    assert!(matches!(
        asset_result(&first),
        ManagementAssetExportResultV2::AssetAborted {
            staged_asset_id,
            state: ManagementAssetExportState::Aborted,
        } if staged_asset_id == &staged_id
    ));
    assert_eq!(
        staged_value(&store, &admin, &staged_id).await.payload.state,
        StagedAssetState::Aborted
    );
    let replay = done(
        &dispatcher,
        &admin,
        "asset.abort",
        abort_payload("abort-one", &staged_id),
    )
    .await;
    assert_eq!(replay.id, first.id);
    assert_eq!(
        serde_json::to_value(&replay.result).unwrap(),
        serde_json::to_value(&first.result).unwrap()
    );
    let second = done(
        &dispatcher,
        &admin,
        "asset.abort",
        abort_payload("abort-two", &staged_id),
    )
    .await;
    assert_eq!(second.state, ManagementJobState::Succeeded);
    assert_eq!(
        staged_value(&store, &admin, &staged_id).await.payload.state,
        StagedAssetState::Aborted
    );
    assert_eq!(
        store
            .read_blob(&admin, &content_digest, 64 * 1024 * 1024)
            .await
            .unwrap()
            .len(),
        staged_value(&store, &admin, &staged_id)
            .await
            .payload
            .content_bytes as usize
    );
    let mut session = store.session().await.unwrap();
    let usage = session.capacity_usage_v41(0).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        usage.staged_packages, 1,
        "terminal staged records still occupy"
    );
    assert_eq!(
        staged_value(&store, &admin, &staged_id)
            .await
            .payload
            .quarantine_reason,
        None
    );
    store.close().await;
}

fn trusted_authority(id: &str, body: &str) -> StoredTraceAuthority {
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

fn candidate_bundle(content: &str) -> evo_core::contract::ResolvedBundleV3 {
    let profile = Profile {
        id: "package-profile".into(),
        evolution_enabled: true,
        parent_digest: hash(b"candidate-parent"),
        baseline_digest: hash(b"candidate-baseline"),
    };
    let parent = SkillSnapshot {
        content: content.into(),
        applicability: "package applicability".into(),
        counterexample: "package counterexample".into(),
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
                available: Default::default(),
                granted: Default::default(),
            },
            revoked: &Default::default(),
        },
        "tenant",
        &hash(b"environment"),
    )
    .unwrap()
}

fn candidate_material_package(
    bundle: &evo_core::contract::ResolvedBundleV3,
) -> (
    AssetPackageManifest,
    BTreeMap<String, Vec<u8>>,
    CandidateMaterialBinding,
) {
    let skill_path = "material/skill_snapshot.json";
    let strategy_path = "material/strategy.json";
    let skill = serde_json::to_vec(&bundle.skill).unwrap();
    let strategy = serde_json::to_vec(&bundle.improver).unwrap();
    let files = BTreeMap::from([
        (skill_path.into(), skill.clone()),
        (strategy_path.into(), strategy.clone()),
    ]);
    (
        package_manifest_with_kind("candidate_material", &files, PackageKind::Skill),
        files,
        CandidateMaterialBinding {
            skill_snapshot_entry_path: skill_path.into(),
            strategy_entry_path: strategy_path.into(),
            strategy_authority: CandidateStrategyAuthority::BuiltinDefaultV1,
        },
    )
}

fn package_manifest_with_kind(
    asset_id: &str,
    files: &BTreeMap<String, Vec<u8>>,
    kind: PackageKind,
) -> AssetPackageManifest {
    let mut manifest = package_manifest(asset_id, files);
    manifest.kind = kind;
    manifest
}

async fn handoff_setup(
    content: &str,
) -> (
    tempfile::TempDir,
    Store,
    Context,
    E16Envelope<StagedAssetPayload>,
    evo_engine::release_store::CurrentReleaseCandidateRecord,
) {
    let (dir, store) = store().await;
    let admin = admin();
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    let proposer = Context::new("tenant", "proposer", Role::Worker).unwrap();
    let authority = trusted_authority("handoff-source", "actual trusted handoff source");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let source = E16SourceRef {
        kind: "run".into(),
        id: authority.record.id.clone(),
        digest: fingerprint(&serde_json::to_value(&authority).unwrap()).unwrap(),
    };
    bump(&store, &admin, "handoff-watermark").await;
    let bundle = candidate_bundle(content);
    trusted_compiler_inputs::install_bundle_current_sources(&store, &bundle)
        .await
        .unwrap();
    let (manifest, files, binding) = candidate_material_package(&bundle);
    let request = StagePackageRequest {
        request_key: "handoff-material".into(),
        manifest,
        files,
        source_refs: vec![source],
        baseline_digest: hash(b"baseline"),
        local_digest: hash(b"local"),
        upstream_digest: None,
        environment_digest: hash(b"environment"),
        compiler_version: "compiler-v1".into(),
        candidate_material: Some(binding),
    };
    let staged = PersistentPackageStore::stage_package(&admin, &store, request)
        .await
        .unwrap();
    let candidate = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "handoff-candidate".into(),
            environment_digest: hash(b"environment"),
            proposer_actor: proposer.actor().into(),
            bundle,
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: authority.record.id.clone(),
                content_digest: authority.trace.source_digest.clone(),
            }],
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    (dir, store, admin, staged, candidate)
}

// Handoff links an existing Current candidate without approving it or moving
// the Active pointer.
#[tokio::test]
async fn asset_handoff_uses_existing_current_candidate_without_release() {
    let (_dir, store, admin, staged, candidate) = handoff_setup("handoff material").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let status = done(
        &dispatcher,
        &admin,
        "asset.handoff",
        handoff_payload("handoff-real", &staged.id, &candidate.id),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "asset_handed_off");
    assert!(matches!(
        asset_result(&status),
        ManagementAssetExportResultV2::AssetHandedOff { staged_asset_id, candidate_id }
            if staged_asset_id == &staged.id && candidate_id == &candidate.id
    ));
    let handed = staged_value(&store, &admin, &staged.id).await;
    assert_eq!(
        handed.payload.candidate_ref.as_deref(),
        Some(candidate.id.as_str())
    );
    assert_eq!(handed.payload.state, StagedAssetState::Staged);
    let mut session = store.session().await.unwrap();
    assert!(
        session
            .list::<Value>(&admin, "release")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        session
            .list::<Value>(&admin, "pointer")
            .await
            .unwrap()
            .is_empty()
    );
    let candidate_wire: Value = session
        .need(&admin, "artifact", &candidate.id)
        .await
        .unwrap();
    assert_eq!(candidate_wire, serde_json::to_value(&candidate).unwrap());
    let edges = session
        .dependency_edges(&admin, "artifact", &candidate.id)
        .await
        .unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| edge == &("artifact".into(), staged.id.clone()))
    );
    session.commit().await.unwrap();
    assert_eq!(
        serde_json::to_value(&status.committed_side_effects).unwrap(),
        json!([{
            "kind": "asset_handed_off",
            "staged_asset_id": staged.id,
            "candidate_id": candidate.id,
        }])
    );
    store.close().await;
}

// A legacy candidate and a candidate whose effective content differs from the
// staged material are both refused with zero ref/edge writes.
#[tokio::test]
async fn asset_handoff_rejects_legacy_or_material_source_mismatch() {
    let (_dir, store, admin, staged, _candidate) = handoff_setup("matching material").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    // A real Current candidate with different effective content.
    let proposer = Context::new("tenant", "proposer", Role::Worker).unwrap();
    let authority = trusted_authority("handoff-source", "actual trusted handoff source");
    let other = candidate_bundle("different effective content");
    trusted_compiler_inputs::install_bundle_current_sources(&store, &other)
        .await
        .unwrap();
    let other_candidate = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "mismatched-candidate".into(),
            environment_digest: hash(b"environment"),
            proposer_actor: proposer.actor().into(),
            bundle: other,
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: authority.record.id.clone(),
                content_digest: authority.trace.source_digest.clone(),
            }],
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let mismatch = done(
        &dispatcher,
        &admin,
        "asset.handoff",
        handoff_payload("handoff-mismatch", &staged.id, &other_candidate.id),
    )
    .await;
    assert_eq!(mismatch.state, ManagementJobState::Failed, "{mismatch:?}");
    assert_eq!(mismatch.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        staged_value(&store, &admin, &staged.id)
            .await
            .payload
            .candidate_ref,
        None
    );
    let mut session = store.session().await.unwrap();
    let edges = session
        .dependency_edges(&admin, "artifact", &other_candidate.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        edges
            .iter()
            .all(|(kind, id)| !(kind == "artifact" && id == &staged.id)),
        "a refused handoff writes no candidate edge"
    );

    // A structurally valid legacy candidate is never Current input.
    let legacy_bundle = trusted_compiler_inputs::software_candidate(
        "tenant",
        &hash(b"environment"),
        "legacy candidate material",
    );
    let legacy = evo_engine::release_store::ReleaseCandidateRecord {
        id: "legacy-candidate".into(),
        schema_version: "rsia.release_candidate.v1".into(),
        bundle_digest: legacy_bundle.digest.clone(),
        environment_digest: hash(b"environment"),
        profile_id: legacy_bundle.profile_id.clone(),
        parent_digest: legacy_bundle.parent_digest.clone(),
        proposer_actor: "proposer".into(),
        sources: vec![],
        revoke_watermark: 1,
        bundle: legacy_bundle,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", &legacy.id, admin.actor(), &legacy)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let rejected = done(
        &dispatcher,
        &admin,
        "asset.handoff",
        handoff_payload("handoff-legacy", &staged.id, &legacy.id),
    )
    .await;
    assert_eq!(rejected.state, ManagementJobState::Failed, "{rejected:?}");
    assert_eq!(
        staged_value(&store, &admin, &staged.id)
            .await
            .payload
            .candidate_ref,
        None
    );
    store.close().await;
}

// Prepare freezes the fixed server-side local destination; the wire cannot
// choose one, and a reused key with a different staged asset conflicts before
// any second intent.
#[tokio::test]
async fn export_prepare_uses_fixed_local_namespace() {
    let (_dir, store) = store().await;
    let admin = admin();
    let source = source(&store, &admin, "prepare-source").await;
    bump(&store, &admin, "prepare-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let first = direct_stage(
        &store,
        &admin,
        "prepare-staged-one",
        "prepare_asset_one",
        package_files(),
        vec![source.clone()],
    )
    .await;
    let second = direct_stage(
        &store,
        &admin,
        "prepare-staged-two",
        "prepare_asset_two",
        package_files(),
        vec![source],
    )
    .await;

    let payload = prepare_payload("prepare-real", &first.id);
    let status = done(&dispatcher, &admin, "export.prepare", payload.clone()).await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "export_prepared");
    let ManagementAssetExportResultV2::ExportPrepared {
        export_id,
        staged_asset_id,
        package_tree_digest,
        delivered_bytes,
    } = asset_result(&status)
    else {
        panic!("expected ExportPrepared");
    };
    assert_eq!(staged_asset_id, &first.id);
    let mut session = store.session().await.unwrap();
    let attempt: Value = session.need(&admin, "artifact", export_id).await.unwrap();
    let attempts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    let private: Value = session
        .need(&admin, "artifact", &status.private_input_ref)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(attempt["schema_version"], "rsia.e16.export_attempt.v2");
    assert_eq!(attempt["payload"]["state"], "prepared");
    assert_eq!(attempt["payload"]["destination_scope"], "local_namespace");
    assert_eq!(attempt["payload"]["delivery_kind"], "local_directory_v1");
    assert_eq!(attempt["payload"]["budget_ref"], Value::Null);
    assert_eq!(
        attempt["payload"]["package_tree_digest"],
        package_tree_digest.as_str()
    );
    assert_eq!(
        attempt["payload"]["delivered_bytes"].as_u64(),
        Some(*delivered_bytes)
    );
    let private_wire = private["payload"].to_string();
    assert!(!private_wire.contains("destination"));
    assert!(!private_wire.contains("budget_ref"));
    assert_eq!(
        attempts
            .iter()
            .filter(|value| value["schema_version"] == "rsia.e16.export_attempt.v2")
            .count(),
        1
    );

    // Same key and staged asset replays the same job with zero new intent.
    let replay = done(&dispatcher, &admin, "export.prepare", payload).await;
    assert_eq!(replay.id, status.id);
    assert_eq!(
        serde_json::to_value(&replay.result).unwrap(),
        serde_json::to_value(&status.result).unwrap()
    );
    // Same key, different staged asset: refused at the idempotency key.
    let error = dispatcher
        .submit(
            &admin,
            "export.prepare",
            prepare_payload("prepare-real", &second.id),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    let mut session = store.session().await.unwrap();
    let attempts: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        attempts
            .iter()
            .filter(|value| value["schema_version"] == "rsia.e16.export_attempt.v2")
            .count(),
        1
    );
    // A caller-selected destination is an unknown wire field.
    let mut forged = prepare_payload("prepare-forged", &first.id);
    forged["destination_scope"] = json!("external");
    assert!(matches!(
        dispatcher
            .submit(&admin, "export.prepare", forged)
            .await
            .unwrap_err(),
        Error::Invalid(_)
    ));
    store.close().await;
}

// Completion publishes the real local tree, its audit and receipt, and the
// existing direct completion sees exactly the same delivery fact.
#[tokio::test]
async fn export_complete_publishes_real_tree_and_receipt() {
    let (_dir, store) = store().await;
    let admin = admin();
    let source = source(&store, &admin, "complete-source").await;
    bump(&store, &admin, "complete-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let files = package_files();
    let staged = direct_stage(
        &store,
        &admin,
        "complete-staged",
        "complete_asset",
        files.clone(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &admin, "complete-attempt", &staged.id).await;

    let status = done(
        &dispatcher,
        &admin,
        "export.complete",
        complete_payload("complete-real", &attempt.id),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "export_completed");
    let ManagementAssetExportResultV2::ExportCompleted {
        export_id,
        staged_asset_id,
        package_tree_digest,
        delivered_bytes,
        receipt_digest,
        delivery_audit_id,
    } = asset_result(&status)
    else {
        panic!("expected ExportCompleted");
    };
    assert_eq!(export_id, &attempt.id);
    assert_eq!(staged_asset_id, &staged.id);
    assert_eq!(package_tree_digest, &attempt.payload.package_tree_digest);
    assert_eq!(*delivered_bytes, attempt.payload.delivered_bytes);

    let dir = store.local_export_directory(&admin, export_id).unwrap();
    assert!(tokio::fs::try_exists(&dir).await.unwrap());
    let manifest_bytes = tokio::fs::read(dir.join("manifest.json")).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&manifest_bytes).unwrap(),
        serde_json::to_value(package_manifest("complete_asset", &files)).unwrap()
    );
    for (path, bytes) in &files {
        assert_eq!(&tokio::fs::read(dir.join(path)).await.unwrap(), bytes);
    }
    let mut session = store.session().await.unwrap();
    let completed: Value = session.need(&admin, "artifact", export_id).await.unwrap();
    let audit: Value = session
        .need(&admin, "artifact", delivery_audit_id)
        .await
        .unwrap();
    let audits: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(completed["payload"]["state"], "completed");
    assert_eq!(
        completed["payload"]["completion_receipt_digest"],
        receipt_digest.as_str()
    );
    assert_eq!(audit["schema_version"], "rsia.e16.delivery_audit.v2");
    assert_eq!(audit["payload"]["export_attempt_id"], export_id.as_str());
    assert_eq!(
        audits
            .iter()
            .filter(|value| value["schema_version"] == "rsia.e16.delivery_audit.v2")
            .count(),
        1
    );

    // Healthy direct completion control: it reads the same stored delivery.
    let (same_attempt, same_audit, same_receipt) =
        PersistentPackageStore::complete_export(&admin, &store, export_id)
            .await
            .unwrap();
    assert_eq!(serde_json::to_value(&same_attempt).unwrap(), completed);
    assert_eq!(serde_json::to_value(&same_audit).unwrap(), audit);
    assert_eq!(same_receipt.completion_receipt_digest, *receipt_digest);
    assert_eq!(
        serde_json::to_value(&status.committed_side_effects).unwrap(),
        json!([{
            "kind": "export_completed",
            "export_id": export_id,
            "receipt_digest": receipt_digest,
            "delivery_audit_id": delivery_audit_id,
        }])
    );
    store.close().await;
}

// A published delivery cannot be erased by abort.
#[tokio::test]
async fn export_abort_cannot_erase_published_delivery() {
    let (_dir, store) = store().await;
    let admin = admin();
    let other = Context::new("tenant", "admin-two", Role::Admin).unwrap();
    let source = source(&store, &admin, "erase-source").await;
    bump(&store, &admin, "erase-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let staged = direct_stage(
        &store,
        &admin,
        "erase-staged",
        "erase_asset",
        package_files(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &admin, "erase-attempt", &staged.id).await;
    let completed = done(
        &dispatcher,
        &admin,
        "export.complete",
        complete_payload("erase-complete", &attempt.id),
    )
    .await;
    assert_eq!(completed.state, ManagementJobState::Succeeded);
    let dir = store.local_export_directory(&admin, &attempt.id).unwrap();
    let before = export_tree_image(&dir);
    let mut session = store.session().await.unwrap();
    let before_attempt: Value = session.need(&admin, "artifact", &attempt.id).await.unwrap();
    let before_all: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    let before_audits: Vec<Value> = before_all
        .into_iter()
        .filter(|value| value["schema_version"] == "rsia.e16.delivery_audit.v2")
        .collect();
    session.commit().await.unwrap();

    let rejected = done(
        &dispatcher,
        &admin,
        "export.abort",
        export_abort_payload("erase-abort", &attempt.id),
    )
    .await;
    assert_eq!(rejected.state, ManagementJobState::Failed, "{rejected:?}");
    assert_eq!(rejected.error_code.as_deref(), Some("conflict"));
    assert_eq!(export_tree_image(&dir), before);
    let mut session = store.session().await.unwrap();
    let after_attempt: Value = session.need(&admin, "artifact", &attempt.id).await.unwrap();
    let after_all: Vec<Value> = session.list(&admin, "artifact").await.unwrap();
    let after_audits: Vec<Value> = after_all
        .into_iter()
        .filter(|value| value["schema_version"] == "rsia.e16.delivery_audit.v2")
        .collect();
    session.commit().await.unwrap();
    assert_eq!(before_attempt, after_attempt);
    assert_eq!(before_audits, after_audits);
    assert_eq!(after_audits.len(), 1);

    // Another owner cannot abort the delivery either.
    assert!(matches!(
        PersistentPackageStore::abort_export(&other, &store, &attempt.id).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(export_tree_image(&dir), before);
    store.close().await;
}

// Results are owner-scoped and bounded: another Admin of the same namespace
// cannot read the job, and the result wire carries no path, content or secret.
#[tokio::test]
async fn six_asset_export_results_are_owner_scoped_and_bounded() {
    let (_dir, store) = store().await;
    let admin = admin();
    let other = Context::new("tenant", "admin-two", Role::Admin).unwrap();
    let source = source(&store, &admin, "bounded-source").await;
    bump(&store, &admin, "bounded-watermark").await;
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), other.clone()]).unwrap();
    let stage_status = management_stage(
        &dispatcher,
        &store,
        &admin,
        "bounded-stage",
        "bounded_asset",
        package_files(),
        &[source],
    )
    .await;
    let staged_id = match asset_result(&stage_status) {
        ManagementAssetExportResultV2::AssetStaged {
            staged_asset_id, ..
        } => staged_asset_id.clone(),
        other => panic!("{other:?}"),
    };
    let prepare_status = done(
        &dispatcher,
        &admin,
        "export.prepare",
        prepare_payload("bounded-prepare", &staged_id),
    )
    .await;
    let export_id = match asset_result(&prepare_status) {
        ManagementAssetExportResultV2::ExportPrepared { export_id, .. } => export_id.clone(),
        other => panic!("{other:?}"),
    };
    let complete_status = done(
        &dispatcher,
        &admin,
        "export.complete",
        complete_payload("bounded-complete", &export_id),
    )
    .await;
    assert_eq!(complete_status.state, ManagementJobState::Succeeded);

    for status in [&stage_status, &prepare_status, &complete_status] {
        assert!(matches!(
            dispatcher.status(&other, &status.id).await,
            Err(Error::NotFound)
        ));
        let wire = serde_json::to_string(&status.result).unwrap();
        for forbidden in [
            "/",
            "blobs",
            "exports/",
            "skill.md",
            "Instructions here",
            "Tenant",
            "secret",
            "refund",
        ] {
            assert!(
                !wire.contains(forbidden),
                "result holds {forbidden}: {wire}"
            );
        }
        assert!(wire.contains("64") || wire.len() > 20);
    }
    let stage_wire = serde_json::to_string(&stage_status.result).unwrap();
    assert!(stage_wire.contains(&staged_id));
    let prepare_wire = serde_json::to_string(&prepare_status.result).unwrap();
    assert!(prepare_wire.contains(&export_id));
    store.close().await;
}

fn export_tree_image(dir: &std::path::Path) -> BTreeMap<String, String> {
    let mut image = BTreeMap::new();
    fn walk(root: &std::path::Path, path: &std::path::Path, image: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let entry_path = entry.path();
            if entry_path.is_dir() {
                walk(root, &entry_path, image);
            } else {
                image.insert(
                    entry_path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hash(&std::fs::read(&entry_path).unwrap()),
                );
            }
        }
    }
    walk(dir, dir, &mut image);
    image
}

// A read-only SQLite snapshot retains every table, row identity and cell type.
fn management_sql_snapshot(path: &std::path::Path) -> Vec<u8> {
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", r#"
import json, pathlib, sqlite3, struct, sys
con = sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri() + '?mode=ro', uri=True)
con.execute('BEGIN')
def cell(v):
    if v is None: return ['null']
    if isinstance(v, int): return ['integer', str(v)]
    if isinstance(v, float): return ['real', struct.pack('>d', v).hex()]
    if isinstance(v, bytes): return ['blob', v.hex()]
    return ['text', v.encode('utf-8').hex()]
facts = []
for name, ddl in con.execute("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    table = '"' + name.replace('"', '""') + '"'
    columns = '*' if 'WITHOUT ROWID' in (ddl or '').upper() else 'rowid,*'
    rows = [[cell(v) for v in row] for row in con.execute('SELECT ' + columns + ' FROM ' + table)]
    rows.sort(key=lambda row: json.dumps(row, separators=(',', ':')))
    facts.append([name, ddl, rows])
print(json.dumps(facts, separators=(',', ':')))
con.close()
"#])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.is_empty());
    result.stdout
}
