//! AG-166b recovery controls for the six asset/local-export operations.
//!
//! Each handler commits its business rows, terminal job state and result in one
//! transaction; a crash is reconciled, never re-run. The one exceptional state
//! is `export.complete`: a tree renamed into place before its SQL commit keeps
//! the actual delivery, and only an exact attempt/source/watermark/tree/bytes/
//! dependency/actual-directory agreement may add the missing SQL completion.
//! These tests use real triggers and real stores, not timing guesses.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash, now};
use evo_engine::dispatch::{
    ManagementAssetExportResultV2, ManagementDispatcher, ManagementJobState, ManagementJobStatusV1,
    ManagementJobV2, ManagementResultV2,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::packages::{
    AssetPackageManifest, CandidateMaterialBinding, CandidateStrategyAuthority, E16Envelope,
    E16SourceRef, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER, PackageEntry, PackageKind,
    PersistentPackageStore, RedactionReport, RedactionStatus, StagePackageRequest,
    StagedAssetPayload,
};
use evo_engine::release_store::{
    CurrentStageBundleRequest as StageBundleRequest, ReleaseStore, TypedSourceRef,
};
use evo_storage::{Store, decode_management_job};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn admin() -> Context {
    Context::new("tenant", "admin", Role::Admin).unwrap()
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("asset-export-recovery.sqlite3"))
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
        name: "Recovery asset".into(),
        version: "1.0.0".into(),
        description: "Recovery fixture".into(),
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

async fn terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
) -> ManagementJobStatusV1 {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
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
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("asset/export recovery job reaches a terminal state")
}

async fn wait_state(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job_id: &str,
    expected: ManagementJobState,
) -> ManagementJobStatusV1 {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let status = dispatcher.status_view(ctx, job_id).await.unwrap();
            if status.state == expected {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected job state")
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

async fn staged_value(store: &Store, ctx: &Context, id: &str) -> E16Envelope<StagedAssetPayload> {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn artifact_value(store: &Store, ctx: &Context, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn artifact_schemas(store: &Store, ctx: &Context) -> Vec<(String, String)> {
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    artifacts
        .into_iter()
        .map(|value| {
            (
                value["schema_version"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                value["id"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn sqlite(path: &std::path::Path, script: &str) {
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "script failed: {}\n{}",
        script,
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn blob_file_count(dir: &std::path::Path) -> usize {
    let path = dir.join("blobs").join(hash(b"tenant"));
    let mut count = 0;
    let mut entries = tokio::fs::read_dir(&path).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry.file_type().await.unwrap().is_file() {
            count += 1;
        }
    }
    count
}

// The staged phase and the terminal job state share one commit: a refusal
// leaves no staged asset and no successful job.
#[tokio::test]
async fn asset_prepared_phase_and_job_progress_commit_together() {
    let (dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "atomic-stage-source").await;
    bump(&store, &ctx, "atomic-stage-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let path = dir.path().join("asset-export-recovery.sqlite3");
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_stage BEFORE INSERT ON objects WHEN json_extract(NEW.body,'$.schema_version')='rsia.e16.staged_asset.v1' BEGIN SELECT RAISE(ABORT,'injected-stage-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let input = stage_input_bytes("atomic_asset", &package_files(), &[source], None);
    let digest = put_blob(&store, &ctx, &input).await;
    let failed = done(
        &dispatcher,
        &ctx,
        "asset.stage",
        stage_payload("atomic-stage", &digest),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert!(
        artifact_schemas(&store, &ctx)
            .await
            .iter()
            .all(|(schema, _)| schema != "rsia.e16.staged_asset.v1"),
        "no half-committed staged phase"
    );
    assert_eq!(
        store
            .read_blob(&ctx, &digest, 64 * 1024 * 1024)
            .await
            .unwrap(),
        input
    );
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_stage')\nc.commit()\nprint('ok')",
    );
    let healthy = done(
        &dispatcher,
        &ctx,
        "asset.stage",
        stage_payload("atomic-stage-healthy", &digest),
    )
    .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    store.close().await;
}

// A failure after the blob was published leaves only a content-addressed blob
// and no staged asset; a new key derives the same content again.
#[tokio::test]
async fn asset_blob_publish_then_finish_crash_recovers_exactly() {
    let (dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "blob-stage-source").await;
    bump(&store, &ctx, "blob-stage-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let path = dir.path().join("asset-export-recovery.sqlite3");
    let input = stage_input_bytes("blob_asset", &package_files(), &[source], None);
    let digest = put_blob(&store, &ctx, &input).await;
    assert_eq!(blob_file_count(dir.path()).await, 1);
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_final BEFORE INSERT ON objects WHEN json_extract(NEW.body,'$.schema_version')='rsia.e16.staged_asset.v1' AND json_extract(NEW.body,'$.payload.state')='staged' BEGIN SELECT RAISE(ABORT,'injected-final-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = done(
        &dispatcher,
        &ctx,
        "asset.stage",
        stage_payload("blob-stage", &digest),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    let schemas = artifact_schemas(&store, &ctx).await;
    assert!(
        schemas
            .iter()
            .all(|(schema, _)| schema != "rsia.e16.staged_asset.v1"),
        "no staged row after the rolled-back finalize"
    );
    assert_eq!(
        blob_file_count(dir.path()).await,
        2,
        "the projection blob stays as an orphan content-addressed file"
    );
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_final')\nc.commit()\nprint('ok')",
    );
    let healthy = done(
        &dispatcher,
        &ctx,
        "asset.stage",
        stage_payload("blob-stage-retry", &digest),
    )
    .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    let result = match healthy.result.as_ref() {
        Some(ManagementResultV2::AssetExport(value)) => value,
        other => panic!("{other:?}"),
    };
    let ManagementAssetExportResultV2::AssetStaged {
        content_blob_digest,
        ..
    } = result
    else {
        panic!("expected AssetStaged");
    };
    assert_eq!(content_blob_digest.len(), 64);
    assert_eq!(
        blob_file_count(dir.path()).await,
        2,
        "the retry reuses the same projection digest"
    );
    assert_eq!(
        hash(
            &store
                .read_blob(&ctx, content_blob_digest, 64 * 1024 * 1024)
                .await
                .unwrap()
        ),
        *content_blob_digest
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
        package_manifest("candidate_material", &files),
        files,
        CandidateMaterialBinding {
            skill_snapshot_entry_path: skill_path.into(),
            strategy_entry_path: strategy_path.into(),
            strategy_authority: CandidateStrategyAuthority::BuiltinDefaultV1,
        },
    )
}

// The candidate link/edge/audit and the terminal job state share one commit:
// a failure leaves no candidate_ref and no edge.
#[tokio::test]
async fn asset_handoff_link_and_job_completion_share_one_commit() {
    let (dir, store) = store().await;
    let admin = admin();
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    let proposer = Context::new("tenant", "proposer", Role::Worker).unwrap();
    let authority = trusted_authority("handoff-recovery-source", "handoff recovery source");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let source = E16SourceRef {
        kind: "run".into(),
        id: authority.record.id.clone(),
        digest: fingerprint(&serde_json::to_value(&authority).unwrap()).unwrap(),
    };
    bump(&store, &admin, "handoff-recovery-watermark").await;
    let bundle = candidate_bundle("handoff recovery material");
    trusted_compiler_inputs::install_bundle_current_sources(&store, &bundle)
        .await
        .unwrap();
    let (manifest, files, binding) = candidate_material_package(&bundle);
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        StagePackageRequest {
            request_key: "handoff-recovery-material".into(),
            manifest,
            files,
            source_refs: vec![source],
            baseline_digest: hash(b"baseline"),
            local_digest: hash(b"local"),
            upstream_digest: None,
            environment_digest: hash(b"environment"),
            compiler_version: "compiler-v1".into(),
            candidate_material: Some(binding),
        },
    )
    .await
    .unwrap();
    let candidate = ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "handoff-recovery-candidate".into(),
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
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("asset-export-recovery.sqlite3");
    sqlite(
        &path,
        &format!(
            "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_handoff BEFORE INSERT ON objects WHEN NEW.id='{}' AND json_extract(NEW.body,'$.payload.candidate_ref') IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected-handoff-stop'); END\")\nc.commit()\nprint('ok')",
            staged.id
        ),
    );
    let failed = done(
        &dispatcher,
        &admin,
        "asset.handoff",
        handoff_payload("handoff-recovery", &staged.id, &candidate.id),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    let current = staged_value(&store, &admin, &staged.id).await;
    assert_eq!(current.payload.candidate_ref, None);
    let mut session = store.session().await.unwrap();
    let edges = session
        .dependency_edges(&admin, "artifact", &candidate.id)
        .await
        .unwrap();
    let candidate_wire: Value = session
        .need(&admin, "artifact", &candidate.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        edges
            .iter()
            .all(|(kind, id)| !(kind == "artifact" && id == &staged.id)),
        "no candidate edge after the rolled-back handoff"
    );
    assert_eq!(candidate_wire, serde_json::to_value(&candidate).unwrap());
    store.close().await;
}

// One request key freezes one intent; a different staged asset under the same
// key conflicts before any second attempt row.
#[tokio::test]
async fn export_prepare_job_replays_one_intent() {
    let (_dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "intent-source").await;
    bump(&store, &ctx, "intent-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let first = direct_stage(
        &store,
        &ctx,
        "intent-staged-one",
        "intent_asset_one",
        package_files(),
        vec![source.clone()],
    )
    .await;
    let second = direct_stage(
        &store,
        &ctx,
        "intent-staged-two",
        "intent_asset_two",
        package_files(),
        vec![source],
    )
    .await;
    let payload = prepare_payload("intent-key", &first.id);
    let first_status = done(&dispatcher, &ctx, "export.prepare", payload.clone()).await;
    assert_eq!(first_status.state, ManagementJobState::Succeeded);
    let export_id = match first_status.result.as_ref() {
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::ExportPrepared {
            export_id,
            ..
        })) => export_id.clone(),
        other => panic!("{other:?}"),
    };
    let replay = done(&dispatcher, &ctx, "export.prepare", payload).await;
    assert_eq!(replay.id, first_status.id);
    assert_eq!(
        serde_json::to_value(&replay.result).unwrap(),
        serde_json::to_value(&first_status.result).unwrap()
    );
    let error = dispatcher
        .submit(
            &ctx,
            "export.prepare",
            prepare_payload("intent-key", &second.id),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    let schemas = artifact_schemas(&store, &ctx).await;
    assert_eq!(
        schemas
            .iter()
            .filter(|(schema, _)| schema == "rsia.e16.export_attempt.v2")
            .count(),
        1
    );
    assert_eq!(schemas.iter().filter(|(_, id)| id == &export_id).count(), 1);
    store.close().await;
}

// A tree renamed into its formal directory before the SQL commit is recovered
// exactly: only the missing SQL completion is added, never a second tree.
#[tokio::test]
async fn export_publish_then_job_crash_recovers_actual_receipt() {
    let (dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "publish-crash-source").await;
    bump(&store, &ctx, "publish-crash-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let staged = direct_stage(
        &store,
        &ctx,
        "publish-crash-staged",
        "publish_crash_asset",
        package_files(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &ctx, "publish-crash-attempt", &staged.id).await;
    let path = dir.path().join("asset-export-recovery.sqlite3");
    let mut marker_session = store.session().await.unwrap();
    marker_session
        .put(
            &ctx,
            "artifact",
            "crash-marker",
            ctx.actor(),
            &json!({"schema_version":"fixture.marker.v1","id":"crash-marker"}),
        )
        .await
        .unwrap();
    marker_session.commit().await.unwrap();
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_complete_job BEFORE INSERT ON objects WHEN NEW.kind='job' AND json_extract(NEW.body,'$.operation')='export.complete' AND json_extract(NEW.body,'$.state')='succeeded' AND EXISTS(SELECT 1 FROM objects WHERE namespace=NEW.namespace AND kind='artifact' AND id='crash-marker') BEGIN SELECT RAISE(ABORT,'injected-complete-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let queued = dispatcher
        .submit(
            &ctx,
            "export.complete",
            complete_payload("publish-crash", &attempt.id),
        )
        .await
        .unwrap();
    let job_id = queued.identity().id.to_owned();
    let export_dir = store.local_export_directory(&ctx, &attempt.id).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !tokio::fs::try_exists(&export_dir).await.unwrap() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the tree is renamed before the failing SQL commit");
    assert_eq!(
        artifact_value(&store, &ctx, &attempt.id).await["payload"]["state"],
        "prepared"
    );
    let before = tree_image(&export_dir);
    assert!(
        artifact_schemas(&store, &ctx)
            .await
            .iter()
            .all(|(schema, _)| schema != "rsia.e16.delivery_audit.v2"),
        "no delivery audit before the commit"
    );
    // Let the failed worker and its first reconciliation attempt observe the
    // still-active marker; removing it through the same store lets the next
    // retry complete.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut marker_session = store.session().await.unwrap();
    marker_session
        .delete(&ctx, "artifact", "crash-marker")
        .await
        .unwrap();
    marker_session.commit().await.unwrap();
    let status = terminal(&dispatcher, &ctx, &job_id).await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "export_completed");
    let completed = artifact_value(&store, &ctx, &attempt.id).await;
    assert_eq!(completed["payload"]["state"], "completed");
    let audit_id = completed["payload"]["delivery_audit_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let audit = artifact_value(&store, &ctx, &audit_id).await;
    assert_eq!(audit["schema_version"], "rsia.e16.delivery_audit.v2");
    assert_eq!(audit["payload"]["export_attempt_id"], attempt.id.as_str());
    assert_eq!(
        tree_image(&export_dir),
        before,
        "recovery never rewrites the tree"
    );
    let result = match status.result.as_ref() {
        Some(ManagementResultV2::AssetExport(value)) => value,
        other => panic!("{other:?}"),
    };
    let ManagementAssetExportResultV2::ExportCompleted {
        receipt_digest,
        delivery_audit_id,
        ..
    } = result
    else {
        panic!("expected ExportCompleted");
    };
    assert_eq!(delivery_audit_id, &audit_id);
    assert_eq!(
        completed["payload"]["completion_receipt_digest"],
        receipt_digest.as_str()
    );
    assert_eq!(
        status.generation, 1,
        "the same claim completes, no re-claim"
    );
    store.close().await;
}

// A published tree whose identities do not match is never converted into a
// success or a false failure: it stays Draining with the attempt Prepared.
#[tokio::test]
async fn export_published_tree_mismatch_stays_draining() {
    let (dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "mismatch-source").await;
    bump(&store, &ctx, "mismatch-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let staged = direct_stage(
        &store,
        &ctx,
        "mismatch-staged",
        "mismatch_asset",
        package_files(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &ctx, "mismatch-attempt", &staged.id).await;
    let path = dir.path().join("asset-export-recovery.sqlite3");
    let mut marker_session = store.session().await.unwrap();
    marker_session
        .put(
            &ctx,
            "artifact",
            "crash-marker",
            ctx.actor(),
            &json!({"schema_version":"fixture.marker.v1","id":"crash-marker"}),
        )
        .await
        .unwrap();
    marker_session.commit().await.unwrap();
    sqlite(
        &path,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_complete_job BEFORE INSERT ON objects WHEN NEW.kind='job' AND json_extract(NEW.body,'$.operation')='export.complete' AND json_extract(NEW.body,'$.state')='succeeded' AND EXISTS(SELECT 1 FROM objects WHERE namespace=NEW.namespace AND kind='artifact' AND id='crash-marker') BEGIN SELECT RAISE(ABORT,'injected-complete-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let queued = dispatcher
        .submit(
            &ctx,
            "export.complete",
            complete_payload("mismatch", &attempt.id),
        )
        .await
        .unwrap();
    let job_id = queued.identity().id.to_owned();
    let export_dir = store.local_export_directory(&ctx, &attempt.id).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !tokio::fs::try_exists(&export_dir).await.unwrap() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("tree published");
    tokio::fs::remove_file(export_dir.join("skill.md"))
        .await
        .unwrap();
    let tampered = tree_image(&export_dir);
    // The marker stays: the job write keeps failing, so every reconciliation
    // attempt re-inspects the tampered tree and settles on Draining.
    let draining = wait_state(&dispatcher, &ctx, &job_id, ManagementJobState::Draining).await;
    assert_eq!(draining.state, ManagementJobState::Draining);
    assert_eq!(
        artifact_value(&store, &ctx, &attempt.id).await["payload"]["state"],
        "prepared"
    );
    assert!(draining.result.is_none());
    assert!(
        artifact_schemas(&store, &ctx)
            .await
            .iter()
            .all(|(schema, _)| schema != "rsia.e16.delivery_audit.v2")
    );
    assert_eq!(tree_image(&export_dir), tampered, "never rewritten");
    store.close().await;
}

// A stale generation/lease/owner can neither publish nor delete; the narrow
// physical fence refuses before the first filesystem effect.
#[tokio::test]
async fn stale_asset_export_worker_cannot_publish_or_delete() {
    let (dir, store) = store().await;
    let ctx = admin();
    let other = Context::new("tenant", "admin-two", Role::Admin).unwrap();
    let source = source(&store, &ctx, "stale-fence-source").await;
    bump(&store, &ctx, "stale-fence-watermark").await;
    let staged = direct_stage(
        &store,
        &ctx,
        "stale-fence-staged",
        "stale_fence_asset",
        package_files(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &ctx, "stale-fence-attempt", &staged.id).await;
    let export_dir = store.local_export_directory(&ctx, &attempt.id).unwrap();
    assert!(matches!(
        PersistentPackageStore::complete_export(&other, &store, &attempt.id).await,
        Err(Error::Forbidden)
    ));
    assert!(!tokio::fs::try_exists(&export_dir).await.unwrap());
    PersistentPackageStore::complete_export(&ctx, &store, &attempt.id)
        .await
        .unwrap();
    assert!(matches!(
        PersistentPackageStore::abort_export(&other, &store, &attempt.id).await,
        Err(Error::Forbidden)
    ));
    store.close().await;

    // A seeded running claim: stale generation or lease never matches.
    let store = Store::open(&dir.path().join("stale-fence.sqlite3"))
        .await
        .unwrap();
    let ctx = admin();
    let job_id = "management-job-export-fence";
    let lease = hash(b"current-lease");
    let job = ManagementJobV2 {
        id: job_id.into(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "export.complete".into(),
        request_key: "export-fence".into(),
        payload_digest: hash(b"payload"),
        owner_actor: ctx.actor().into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: "management-input-export-fence".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: Some(lease.clone()),
        lease_until: now().unwrap() + 60,
        generation: 1,
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&ctx, "job", job_id, ctx.actor(), &job)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut session = store.registered_blob_write_session().await.unwrap();
    for stale in [
        evo_storage::ManagementJobFence {
            job_id,
            operation: "export.complete",
            generation: 0,
            lease_token: &lease,
        },
        evo_storage::ManagementJobFence {
            job_id,
            operation: "export.complete",
            generation: 1,
            lease_token: "stale-lease",
        },
        evo_storage::ManagementJobFence {
            job_id,
            operation: "asset.stage",
            generation: 1,
            lease_token: &lease,
        },
    ] {
        assert!(matches!(
            session.ensure_management_job_fence(&ctx, &stale).await,
            Err(Error::Conflict(_))
        ));
    }
    let current = evo_storage::ManagementJobFence {
        job_id,
        operation: "export.complete",
        generation: 1,
        lease_token: &lease,
    };
    session
        .ensure_management_job_fence(&ctx, &current)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
}

// A cancellation before any claim is closure before effect.
#[tokio::test]
async fn asset_export_cancel_before_effect_is_zero_write() {
    let (_dir, store) = store().await;
    let ctx = admin();
    bump(&store, &ctx, "cancel-before-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let job_id = accepted_job(&store, &ctx, "cancel-before", "export.complete").await;
    let cancelled = dispatcher.cancel(&ctx, &job_id).await.unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Cancelled,
        "{cancelled:?}"
    );
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert!(
        artifact_schemas(&store, &ctx)
            .await
            .iter()
            .all(|(schema, _)| schema != "rsia.e16.export_attempt.v2"
                && schema != "rsia.e16.delivery_audit.v2")
    );
    store.close().await;
}

// Cancelling after a completed delivery only records the arrival: the tree,
// audit and result stay, with no refund or zero-cost invention.
#[tokio::test]
async fn asset_export_cancel_after_effect_keeps_audit_and_spend() {
    let (_dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "cancel-after-source").await;
    bump(&store, &ctx, "cancel-after-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let staged = direct_stage(
        &store,
        &ctx,
        "cancel-after-staged",
        "cancel_after_asset",
        package_files(),
        vec![source],
    )
    .await;
    let attempt = direct_prepare(&store, &ctx, "cancel-after-attempt", &staged.id).await;
    let completed = done(
        &dispatcher,
        &ctx,
        "export.complete",
        complete_payload("cancel-after-complete", &attempt.id),
    )
    .await;
    assert_eq!(completed.state, ManagementJobState::Succeeded);
    let export_dir = store.local_export_directory(&ctx, &attempt.id).unwrap();
    let before_tree = tree_image(&export_dir);
    let before_attempt = artifact_value(&store, &ctx, &attempt.id).await;
    let before_audits = artifact_schemas(&store, &ctx).await;
    let cancelled = dispatcher.cancel(&ctx, &completed.id).await.unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Succeeded,
        "a completed delivery is an audit fact"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(completed.result.as_ref()).unwrap()
    );
    assert_eq!(tree_image(&export_dir), before_tree);
    assert_eq!(
        artifact_value(&store, &ctx, &attempt.id).await,
        before_attempt
    );
    assert_eq!(artifact_schemas(&store, &ctx).await, before_audits);
    store.close().await;
}

// A revoked/advanced source closure blocks a stale completion and never
// removes an already delivered tree or audit.
#[tokio::test]
async fn asset_export_revoked_closure_blocks_resume() {
    let (_dir, store) = store().await;
    let ctx = admin();
    let source = source(&store, &ctx, "revoked-export-source").await;
    bump(&store, &ctx, "revoked-export-watermark").await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let delivered_staged = direct_stage(
        &store,
        &ctx,
        "revoked-delivered-staged",
        "revoked_delivered_asset",
        package_files(),
        vec![source.clone()],
    )
    .await;
    let delivered_attempt = direct_prepare(
        &store,
        &ctx,
        "revoked-delivered-attempt",
        &delivered_staged.id,
    )
    .await;
    let completed = done(
        &dispatcher,
        &ctx,
        "export.complete",
        complete_payload("revoked-delivered", &delivered_attempt.id),
    )
    .await;
    assert_eq!(completed.state, ManagementJobState::Succeeded);
    let delivered_dir = store
        .local_export_directory(&ctx, &delivered_attempt.id)
        .unwrap();
    let delivered_tree = tree_image(&delivered_dir);
    let delivered_audit =
        artifact_value(&store, &ctx, &delivered_attempt.id).await["payload"]["delivery_audit_id"]
            .as_str()
            .unwrap()
            .to_owned();

    // A second export is prepared before the revocation; completing it after
    // the watermark advanced must fail with no tree and no new audit.
    let pending_staged = direct_stage(
        &store,
        &ctx,
        "revoked-pending-staged",
        "revoked_pending_asset",
        package_files(),
        vec![source],
    )
    .await;
    let pending_attempt =
        direct_prepare(&store, &ctx, "revoked-pending-attempt", &pending_staged.id).await;
    bump(&store, &ctx, "revoked-after-prepare").await;
    let failed = done(
        &dispatcher,
        &ctx,
        "export.complete",
        complete_payload("revoked-pending", &pending_attempt.id),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        artifact_value(&store, &ctx, &pending_attempt.id).await["payload"]["state"],
        "prepared"
    );
    assert!(
        !tokio::fs::try_exists(
            store
                .local_export_directory(&ctx, &pending_attempt.id)
                .unwrap()
        )
        .await
        .unwrap()
    );
    // The already delivered tree and audit survive the revocation.
    assert_eq!(tree_image(&delivered_dir), delivered_tree);
    let audit = artifact_value(&store, &ctx, &delivered_audit).await;
    assert_eq!(audit["schema_version"], "rsia.e16.delivery_audit.v2");
    assert_eq!(audit["payload"]["revoked"], false);
    store.close().await;
}

/// A queued job-row fixture for cancellation before any claim.
async fn accepted_job(store: &Store, ctx: &Context, key: &str, operation: &str) -> String {
    let owner = ctx.actor();
    let namespace = ctx.namespace();
    let job_digest = hash(format!("{namespace}\0{owner}\0{operation}\0{key}").as_bytes());
    let id = format!("management-job-{}", &job_digest[..24]);
    let input_digest = hash(format!("{namespace}\0{owner}\0{operation}\0{key}").as_bytes());
    let private_input_ref = format!("management-input-{}", &input_digest[..24]);
    let job = ManagementJobV2 {
        id: id.clone(),
        schema_version: "rsia.management_job.v2".into(),
        operation: operation.into(),
        request_key: key.into(),
        payload_digest: hash(b"accepted-fixture"),
        owner_actor: owner.into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref,
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: Vec::new(),
        created_at: now().unwrap(),
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    let mut session = store.session().await.unwrap();
    session.put(ctx, "job", &id, owner, &job).await.unwrap();
    session.commit().await.unwrap();
    id
}

fn tree_image(dir: &std::path::Path) -> BTreeMap<String, String> {
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

// The persisted result wire is bound to its physical operation.
#[test]
fn asset_export_results_are_bound_to_their_physical_operation() {
    let staged = json!({
        "result": "asset_staged",
        "staged_asset_id": "e16-staged-binding",
        "manifest_digest": "a".repeat(64),
        "content_blob_digest": "b".repeat(64),
        "content_bytes": 10,
        "state": "staged",
    });
    let job = |operation: &str, result: Value| {
        json!({
            "id": "management-job-binding",
            "schema_version": "rsia.management_job.v2",
            "operation": operation,
            "request_key": "binding-key",
            "payload_digest": "b".repeat(64),
            "owner_actor": "admin",
            "owner_role": "admin",
            "state": "succeeded",
            "step": "finished",
            "private_input_ref": "management-input-binding",
            "result": result,
            "error_code": null,
            "cancel_requested": false,
            "lease_token": null,
            "lease_until": 0,
            "generation": 1,
            "diagnostics": [],
            "created_at": 1
        })
    };
    let encoded = |value: &Value| serde_json::to_string(value).unwrap();
    decode_management_job(&encoded(&job("asset.stage", staged.clone()))).unwrap();
    for operation in ["asset.abort", "export.prepare", "meta.start"] {
        let error = decode_management_job(&encoded(&job(operation, staged.clone()))).unwrap_err();
        assert!(
            matches!(error, Error::Conflict(ref reason) if reason == "management_job_invalid_shape"),
            "{operation}: {error:?}"
        );
    }
    let completed = json!({
        "result": "export_completed",
        "export_id": "e16-export-binding",
        "staged_asset_id": "e16-staged-binding",
        "package_tree_digest": "c".repeat(64),
        "delivered_bytes": 1,
        "receipt_digest": "d".repeat(64),
        "delivery_audit_id": "e16-delivery-binding",
    });
    decode_management_job(&encoded(&job("export.complete", completed.clone()))).unwrap();
    for operation in ["export.abort", "asset.stage"] {
        assert!(
            decode_management_job(&encoded(&job(operation, completed.clone()))).is_err(),
            "{operation}"
        );
    }
}
