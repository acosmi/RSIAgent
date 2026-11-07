//! AG-175 management-plane trace export controls: the real `trace.export`
//! dispatcher jobs over an actually authorized captured source. Every positive
//! control runs through `ManagementDispatcher::submit`/`status` and the real
//! publication chain; no record, review or delivery fact is hand-written.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceRegistrationV2,
    ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
};
use evo_core::{Context, Error, Feedback, Outcome, Prepare, Role};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementResultV2, ManagementTraceExportResultV2,
    VersionedManagementJob,
};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_engine::trace_export::{prepare_native_candidate, prepare_native_members};
use evo_storage::{Store, TRACE_EXPORT_TREE_UNAVAILABLE};
use serde_json::{Value, json};
use std::time::Duration;

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-a", actor, role).unwrap()
}

async fn setup() -> (tempfile::TempDir, HostService, Context) {
    let source = evo_core::contract::ToolSourceV1 {
        schema_version: evo_core::contract::TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    };
    let config = CompleteHostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: evo_core::contract::SystemSnapshotV3 {
            schema_version: "rsia.system_snapshot.v3".into(),
            profile_id: "profile-a".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: source.names().unwrap(),
            tool_source: source,
            mandatory_context_digest: evo_core::hash(b"mandatory"),
        },
        host_surface_id: "reference-surface".into(),
        host_capabilities: HostCapabilities {
            available: ["fs_read".into()].into(),
            granted: ["fs_read".into()].into(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("service.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let admin = context("admin", Role::Admin);
    let source = ReviewedHostSourceEnvelopeV1 {
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
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "1.0.0".into(),
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
    };
    ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "reference-surface",
        source.clone(),
        registration,
        None,
    )
    .await
    .unwrap();
    let service = HostService::new(store, host.clone()).unwrap();
    let agent = context("agent-a", Role::Agent);
    let mut sources = Vec::new();
    for key in ["management-a", "management-b"] {
        let prepared = service
            .prepare_complete(
                &agent,
                Prepare {
                    request_key: format!("prepare-{key}"),
                    goal: format!("private goal of {key}"),
                    capabilities: vec!["fs_read".into()],
                },
                &config,
            )
            .await
            .unwrap();
        service
            .feedback(
                &agent,
                Feedback {
                    request_key: format!("feedback-{key}"),
                    run_id: prepared.run.id.clone(),
                    outcome: Outcome::Failure,
                    details: format!("private feedback of {key}"),
                    failure_class: evo_core::FailureClass::Reasoning,
                },
            )
            .await
            .unwrap();
        sources.push(
            service
                .record_service_facts(&host, &prepared.run.id)
                .await
                .unwrap()
                .record
                .id,
        );
    }
    (dir, service, host)
}

fn prepare_payload(request_key: &str, export_id: &str, source_ids: &[String]) -> Value {
    json!({
        "schema_version": "rsia.management.trace_export.prepare.v1",
        "request_key": request_key,
        "export_id": export_id,
        "source_ids": source_ids,
    })
}

fn publish_payload(request_key: &str, export_id: &str, candidate_digest: &str) -> Value {
    json!({
        "schema_version": "rsia.management.trace_export.publish.v1",
        "request_key": request_key,
        "export_id": export_id,
        "candidate_digest": candidate_digest,
    })
}

fn is_terminal(job: &VersionedManagementJob) -> bool {
    matches!(
        job.identity().state,
        ManagementJobState::Succeeded
            | ManagementJobState::Failed
            | ManagementJobState::Cancelled
            | ManagementJobState::Blocked
    )
}

async fn run_to_terminal(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    job: VersionedManagementJob,
) -> VersionedManagementJob {
    let id = job.identity().id.to_owned();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = dispatcher.status(ctx, &id).await.unwrap();
            if is_terminal(&current) {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("trace export job must reach a terminal state")
}

fn trace_result(job: &VersionedManagementJob) -> ManagementTraceExportResultV2 {
    match job.result() {
        Some(ManagementResultV2::Trace(result)) => result,
        other => panic!("expected a trace export result, got {other:?}"),
    }
}

/// One healthy prepare and its publish, through the real dispatcher jobs.
async fn healthy_export(
    dispatcher: &ManagementDispatcher,
    admin: &Context,
    export_id: &str,
    source_ids: &[String],
) -> (String, VersionedManagementJob) {
    let prepared = dispatcher
        .submit(
            admin,
            "trace.export",
            prepare_payload(&format!("prepare-{export_id}"), export_id, source_ids),
        )
        .await
        .unwrap();
    let prepared = run_to_terminal(dispatcher, admin, prepared).await;
    assert_eq!(prepared.identity().state, ManagementJobState::Succeeded);
    assert_eq!(prepared.step(), "trace_export_prepared");
    let ManagementTraceExportResultV2::TraceExportPrepared {
        candidate_digest, ..
    } = trace_result(&prepared)
    else {
        panic!("expected the prepared result");
    };
    let published = dispatcher
        .submit(
            admin,
            "trace.export",
            publish_payload(
                &format!("publish-{export_id}"),
                export_id,
                &candidate_digest,
            ),
        )
        .await
        .unwrap();
    let published = run_to_terminal(dispatcher, admin, published).await;
    assert_eq!(published.identity().state, ManagementJobState::Succeeded);
    (candidate_digest, published)
}

async fn revoke_run(
    service: &HostService,
    admin: &Context,
    id: &str,
) -> evo_storage::lifecycle::CleanupStatus {
    evo_storage::lifecycle::LifecycleStore::begin_revoke(
        admin,
        service.store(),
        evo_storage::lifecycle::TypedObjectRef {
            kind: "run".into(),
            id: id.into(),
        },
        "revoked between prepare and publish",
        10,
    )
    .await
    .unwrap()
}

/// The frozen members of a candidate as the record stores them.
fn frozen(files: &[evo_storage::LocalExportFile]) -> Vec<evo_storage::TraceExportMember> {
    files
        .iter()
        .map(|file| evo_storage::TraceExportMember {
            path: file.path.clone(),
            digest_sha256: evo_core::hash(&file.bytes),
            size_bytes: file.bytes.len() as u64,
        })
        .collect()
}

async fn record_body(service: &HostService, admin: &Context, record_id: &str) -> Option<String> {
    let mut session = service.store().session().await.unwrap();
    let row = session
        .object_row(admin, "artifact", record_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    row.map(|row| row.body)
}

/// The Admin role gate runs before the payload is parsed and before the
/// captured source is observed: a trusted Evaluator with even a valid healthy
/// payload and a present source is refused with zero writes.
#[tokio::test]
async fn trace_export_admin_role_precedes_payload_and_source_observation() {
    let (dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let evaluator = context("evaluator", Role::Evaluator);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    assert_eq!(sources.len(), 2, "real captured sources exist");
    let dispatcher = ManagementDispatcher::new(
        service.store().clone(),
        vec![admin.clone(), evaluator.clone()],
    )
    .unwrap();
    let before = management_sql_snapshot(&dir.path().join("service.sqlite3"));
    // A trusted non-Admin caller: Forbidden whatever the payload holds, and
    // the healthy source stays byte-identical.
    for payload in [
        prepare_payload("evaluator-key", "evaluator-export", &sources),
        Value::Null,
        json!(["not a business DTO"]),
    ] {
        assert!(
            matches!(
                dispatcher
                    .submit(&evaluator, "trace.export", payload)
                    .await
                    .unwrap_err(),
                Error::Forbidden
            ),
            "the Admin role gate precedes payload parsing"
        );
    }
    // An untrusted identity of any role never reaches the operation at all.
    for caller in [
        context("agent-a", Role::Agent),
        context("gateway-host", Role::Host),
        Context::new("other-namespace", "admin", Role::Admin).unwrap(),
    ] {
        assert!(matches!(
            dispatcher
                .submit(&caller, "trace.export", Value::Null)
                .await
                .unwrap_err(),
            Error::Forbidden
        ));
    }
    let mut evaluator_session = service.store().session().await.unwrap();
    assert!(
        matches!(
            prepare_native_candidate(&evaluator, &mut evaluator_session, &sources[0]).await,
            Err(Error::Forbidden)
        ),
        "the in-session loader keeps the same Admin-first order"
    );
    evaluator_session.commit().await.unwrap();
    assert_eq!(
        management_sql_snapshot(&dir.path().join("service.sqlite3")),
        before,
        "every refused call leaves the whole database unchanged"
    );
    let mut session = service.store().session().await.unwrap();
    for source in &sources {
        assert!(
            session
                .object_row(&admin, "run", source)
                .await
                .unwrap()
                .is_some(),
            "the captured source is still in place"
        );
    }
    session.commit().await.unwrap();
    let (_digest, delivered) =
        healthy_export(&dispatcher, &admin, "role-order", &[sources[0].clone()]).await;
    assert_eq!(delivered.step(), "trace_export_delivered");
}

/// One logical export owns exactly one artifact: the same job replays the same
/// record, and a second prepare job on the same export is refused with zero
/// record changes.
#[tokio::test]
async fn trace_export_same_job_replays_one_artifact() {
    let (_dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    let dispatcher =
        ManagementDispatcher::new(service.store().clone(), vec![admin.clone()]).unwrap();
    let payload = prepare_payload("one-prepare-key", "one-artifact", &sources);
    let first = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(&admin, "trace.export", payload.clone())
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first.identity().state, ManagementJobState::Succeeded);
    let ManagementTraceExportResultV2::TraceExportPrepared {
        record_id,
        candidate_digest,
        ..
    } = trace_result(&first)
    else {
        panic!("expected the prepared result");
    };
    let body = record_body(&service, &admin, &record_id).await.unwrap();
    let audit = service.store().verify_audit(&admin).await.unwrap();
    // The same request replays the same job, its result and its one record,
    // with no new write of any kind.
    let replay = dispatcher
        .submit(&admin, "trace.export", payload)
        .await
        .unwrap();
    assert_eq!(replay.identity().id, first.identity().id);
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(
        record_body(&service, &admin, &record_id).await.unwrap(),
        body
    );
    assert_eq!(service.store().verify_audit(&admin).await.unwrap(), audit);
    // A second prepare job on the same export cannot write a second artifact:
    // the record keeps its first prepare job and its frozen content.
    let second = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                prepare_payload("other-prepare-key", "one-artifact", &sources),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(second.identity().state, ManagementJobState::Failed);
    assert_eq!(second.error_code(), Some("conflict"));
    assert_eq!(
        record_body(&service, &admin, &record_id).await.unwrap(),
        body,
        "the one artifact is unchanged"
    );
    let mut session = service.store().session().await.unwrap();
    let trace_rows: Vec<_> = session
        .object_rows_page(&admin, "artifact", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.body.contains("rsia.trace_export.v1"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    assert_eq!(
        trace_rows,
        vec![record_id.clone()],
        "exactly one physical trace export artifact exists"
    );
    // The frozen export still publishes normally afterwards.
    let published = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                publish_payload("one-publish-key", "one-artifact", &candidate_digest),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(published.identity().state, ManagementJobState::Succeeded);
}

/// The same request key with a changed run scope is a conflict with zero I/O:
/// no job, private input, record byte, audit row or export directory changes.
#[tokio::test]
async fn trace_export_same_key_changed_run_scope_conflicts_without_io() {
    let (dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    let dispatcher =
        ManagementDispatcher::new(service.store().clone(), vec![admin.clone()]).unwrap();
    let accepted = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                prepare_payload("scope-key", "scope-export", &[sources[0].clone()]),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(accepted.identity().state, ManagementJobState::Succeeded);
    let before = management_sql_snapshot(&dir.path().join("service.sqlite3"));
    for changed in [
        prepare_payload("scope-key", "scope-export", &[sources[1].clone()]),
        prepare_payload("scope-key", "scope-export", &sources),
        prepare_payload("scope-key", "other-export", &[sources[0].clone()]),
    ] {
        match dispatcher
            .submit(&admin, "trace.export", changed)
            .await
            .unwrap_err()
        {
            Error::Conflict(message) => {
                assert_eq!(message, "idempotency key reused with different content")
            }
            other => panic!("changed scope must conflict, got {other:?}"),
        }
    }
    // An empty scope is not a request at all: the boundary refuses it before
    // the key is looked up, with the same zero I/O.
    assert!(matches!(
        dispatcher
            .submit(
                &admin,
                "trace.export",
                prepare_payload("scope-key", "scope-export", &[]),
            )
            .await,
        Err(Error::Invalid(message)) if message == "trace export source count is invalid"
    ));
    assert!(
        !dir.path().join("trace-exports").exists(),
        "no download surface appeared"
    );
    assert_eq!(
        management_sql_snapshot(&dir.path().join("service.sqlite3")),
        before,
        "the conflict performs zero I/O"
    );
}

/// The job's visible progress and the record's effect share one fence: the
/// served result, the terminal step, the published tree and the receipt digest
/// all name the same delivery, on every status read.
#[tokio::test]
async fn trace_export_progress_and_effect_use_same_fence() {
    let (_dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    let dispatcher =
        ManagementDispatcher::new(service.store().clone(), vec![admin.clone()]).unwrap();
    let prepared = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                prepare_payload("fence-prepare", "fence-export", &sources),
            )
            .await
            .unwrap(),
    )
    .await;
    let ManagementTraceExportResultV2::TraceExportPrepared {
        candidate_digest, ..
    } = trace_result(&prepared)
    else {
        panic!("expected the prepared result");
    };
    assert!(
        !service
            .store()
            .trace_export_directory(&admin, "fence-export")
            .unwrap()
            .exists(),
        "no effect before the publish decision"
    );
    let published = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                publish_payload("fence-publish", "fence-export", &candidate_digest),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(published.step(), "trace_export_delivered");
    let ManagementTraceExportResultV2::TraceExportDelivered {
        candidate_digest: delivered_digest,
        receipt_digest,
        ..
    } = trace_result(&published)
    else {
        panic!("expected the delivered result");
    };
    assert_eq!(delivered_digest, candidate_digest);
    // The effect behind that progress is the exact frozen tree and receipt;
    // re-deriving the members read-only and re-verifying the published
    // directory proves the fence names one delivery, on every read.
    for _ in 0..2 {
        assert_eq!(
            serde_json::to_value(
                dispatcher
                    .status(&admin, published.identity().id)
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&published).unwrap()
        );
    }
    let mut session = service.store().session().await.unwrap();
    let members = prepare_native_members(&admin, &mut session, &sources)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(members.candidate_digest, candidate_digest);
    let receipt = service
        .store()
        .verify_trace_export_tree(
            &admin,
            "fence-export",
            &frozen(&members.files),
            &members.candidate_digest,
        )
        .await
        .unwrap();
    assert_eq!(receipt.completion_receipt_digest, receipt_digest);
    assert_eq!(receipt.package_tree_digest, candidate_digest);
    // The watermark half of the same fence is the export's own source
    // closure, not the namespace watermark. Two more exports are prepared,
    // then an unrelated opaque trace of the same namespace is revoked through
    // the real revoke API: the namespace watermark moves, yet the delivered
    // status and a later publication are unaffected.
    let mut prepared = Vec::new();
    for (key, export, source) in [
        ("fence-prepare-2", "fence-export-2", &sources[0]),
        ("fence-prepare-3", "fence-export-3", &sources[1]),
    ] {
        let job = run_to_terminal(
            &dispatcher,
            &admin,
            dispatcher
                .submit(
                    &admin,
                    "trace.export",
                    prepare_payload(key, export, std::slice::from_ref(source)),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(job.identity().state, ManagementJobState::Succeeded);
        let ManagementTraceExportResultV2::TraceExportPrepared {
            candidate_digest,
            record_id,
            ..
        } = trace_result(&job)
        else {
            panic!("expected the prepared result");
        };
        prepared.push((candidate_digest, record_id));
    }
    let mut opaque = {
        let host = context("gateway-host", Role::Host);
        let mut session = service.store().session().await.unwrap();
        let authority = evo_engine::evidence::load_stored_source(&mut session, &host, &sources[0])
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut opaque = authority.clone();
        opaque.record.id = "fence-unrelated-upstream".into();
        opaque.trace.run_id = opaque.record.id.clone();
        opaque
    };
    opaque.record.body = b"unrelated opaque history for the watermark move".to_vec();
    opaque.trace.source_digest = evo_core::hash(&opaque.record.body);
    opaque.trace.excerpt = String::from_utf8(opaque.record.body.clone()).unwrap();
    opaque.excerpt_end = opaque.record.body.len();
    service
        .record_trace(&context("gateway-host", Role::Host), &opaque)
        .await
        .unwrap();
    let cleanup = revoke_run(&service, &admin, &opaque.record.id).await;
    assert_eq!(cleanup.state, evo_storage::lifecycle::CleanupState::Pending);
    assert_eq!(
        serde_json::to_value(
            dispatcher
                .status(&admin, published.identity().id)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&published).unwrap(),
        "an unrelated revocation leaves the delivered view intact"
    );
    let sibling_published = run_to_terminal(
        &dispatcher,
        &admin,
        dispatcher
            .submit(
                &admin,
                "trace.export",
                publish_payload("fence-publish-2", "fence-export-2", &prepared[0].0),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        sibling_published.identity().state,
        ManagementJobState::Succeeded
    );
    assert_eq!(sibling_published.step(), "trace_export_delivered");
    // A revocation inside the third export's own closure, between its prepare
    // and its publish, refuses the publication before any write: no job, no
    // tree, and the record keeps its honest prepared phase.
    let before = record_body(&service, &admin, &prepared[1].1).await.unwrap();
    revoke_run(&service, &admin, &sources[1]).await;
    match dispatcher
        .submit(
            &admin,
            "trace.export",
            publish_payload("fence-publish-3", "fence-export-3", &prepared[1].0),
        )
        .await
    {
        Err(Error::Conflict(message)) => {
            assert!(message.contains("whose source was revoked"), "{message}")
        }
        other => panic!("a revoked closure must refuse the publication, got {other:?}"),
    }
    assert!(
        !service
            .store()
            .trace_export_directory(&admin, "fence-export-3")
            .unwrap()
            .exists(),
        "the closure revocation blocked every filesystem effect"
    );
    assert_eq!(
        record_body(&service, &admin, &prepared[1].1).await.unwrap(),
        before
    );
    // The first delivery is untouched by both revocations.
    assert_eq!(
        service
            .store()
            .verify_trace_export_tree(
                &admin,
                "fence-export",
                &frozen(&members.files),
                &members.candidate_digest,
            )
            .await
            .unwrap()
            .completion_receipt_digest,
        receipt_digest
    );
}

/// A cancellation that lands before the first write leaves no record and no
/// download surface, and the store still serves a later healthy export.
#[tokio::test]
async fn trace_export_prepublication_cancel_keeps_no_download() {
    let (dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    // Accept the job on a runtime whose spawned worker never runs, so the
    // cancellation provably lands before the first write (AG-155 pattern).
    let payload = prepare_payload("cancel-key", "cancel-export", &[sources[0].clone()]);
    let accepted = {
        let store = service.store().clone();
        let ctx = admin.clone();
        let operation = "trace.export".to_owned();
        tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let accepted = runtime.block_on(async {
                ManagementDispatcher::new(store, vec![ctx.clone()])
                    .unwrap()
                    .submit(&ctx, &operation, payload)
                    .await
                    .unwrap()
            });
            drop(runtime);
            accepted
        })
        .await
        .unwrap()
    };
    assert_eq!(accepted.identity().state, ManagementJobState::Queued);
    assert_eq!(accepted.generation(), 0);
    let dispatcher =
        ManagementDispatcher::new(service.store().clone(), vec![admin.clone()]).unwrap();
    let stopped = dispatcher
        .cancel(&admin, accepted.identity().id)
        .await
        .unwrap();
    assert_eq!(stopped.identity().state, ManagementJobState::Cancelled);
    assert_eq!(stopped.step(), "cancelled_before_claim");
    // No record, no export tree, no staging residue: zero download surface.
    let mut session = service.store().session().await.unwrap();
    let trace_rows = session
        .object_rows_page(&admin, "artifact", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.body.contains("rsia.trace_export.v1"))
        .count();
    session.commit().await.unwrap();
    assert_eq!(trace_rows, 0, "no trace export record was written");
    assert!(!dir.path().join("exports").exists());
    assert!(
        !dir.path().join(".export-staging").exists()
            || std::fs::read_dir(dir.path().join(".export-staging"))
                .unwrap()
                .next()
                .is_none()
    );
    // The cancelled job replays as itself and is never executed later.
    let replay = dispatcher
        .submit(
            &admin,
            "trace.export",
            prepare_payload("cancel-key", "cancel-export", &[sources[0].clone()]),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(
            dispatcher
                .status(&admin, accepted.identity().id)
                .await
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(replay.identity().state, ManagementJobState::Cancelled);
    // A later healthy export on the same store still works end to end.
    let (_digest, delivered) =
        healthy_export(&dispatcher, &admin, "after-cancel", &[sources[0].clone()]).await;
    assert_eq!(delivered.step(), "trace_export_delivered");
}

/// After a real publication, a restart on the same store recovers the exact
/// delivery receipt: startup reconciliation leaves the terminal job and its
/// result untouched, every status read re-proves the published tree, and stale
/// private staging residue is released by the open itself.
#[tokio::test]
async fn trace_export_restart_after_publish_recovers_exact_receipt() {
    let (dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    let dispatcher =
        ManagementDispatcher::new(service.store().clone(), vec![admin.clone()]).unwrap();
    let (_digest, published) =
        healthy_export(&dispatcher, &admin, "restart-export", &sources).await;
    let ManagementTraceExportResultV2::TraceExportDelivered { receipt_digest, .. } =
        trace_result(&published)
    else {
        panic!("expected the delivered result");
    };
    let job_id = published.identity().id.to_owned();
    let staging = dir
        .path()
        .join(".export-staging")
        .join(evo_core::hash(admin.namespace().as_bytes()))
        .join("lost-attempt");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("member.json"), b"stale private staging").unwrap();
    // The restart: release the first process's handles, then a fresh store
    // open and dispatcher, then recovery.
    drop(dispatcher);
    drop(service);
    let reopened = Store::open(&dir.path().join("service.sqlite3"))
        .await
        .unwrap();
    let restarted = ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
    assert!(
        !staging.exists(),
        "startup released the stale private staging residue"
    );
    let recovered = restarted.status(&admin, &job_id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&recovered).unwrap(),
        serde_json::to_value(&published).unwrap(),
        "the exact receipt and terminal state survive the restart"
    );
    let members = {
        let mut session = reopened.session().await.unwrap();
        let members = prepare_native_members(&admin, &mut session, &sources)
            .await
            .unwrap();
        session.commit().await.unwrap();
        members
    };
    let receipt = reopened
        .verify_trace_export_tree(
            &admin,
            "restart-export",
            &frozen(&members.files),
            &members.candidate_digest,
        )
        .await
        .unwrap();
    assert_eq!(receipt.completion_receipt_digest, receipt_digest);
    // Removing the published tree turns every later status read into the
    // honest unavailable conflict; the receipt is never re-earned or resent.
    std::fs::remove_dir_all(
        reopened
            .trace_export_directory(&admin, "restart-export")
            .unwrap(),
    )
    .unwrap();
    match restarted.status(&admin, &job_id).await {
        Err(Error::Conflict(reason)) => assert_eq!(reason, TRACE_EXPORT_TREE_UNAVAILABLE),
        other => panic!("a missing tree must be unavailable, got {other:?}"),
    }
    assert!(
        !reopened
            .trace_export_directory(&admin, "restart-export")
            .unwrap()
            .exists(),
        "the read-only proof never republishes"
    );
}

/// A stale owner cannot complete or delete a published export: the record's
/// one publish job is final for every caller, foreign owners see nothing, and
/// neither the record row nor the tree can be removed.
#[tokio::test]
async fn trace_export_stale_owner_cannot_complete_or_delete_published_tree() {
    let (_dir, service, _host) = setup().await;
    let admin = context("admin", Role::Admin);
    let admin_two = context("admin-two", Role::Admin);
    let other_namespace = Context::new("other-namespace", "admin", Role::Admin).unwrap();
    let mut session = service.store().session().await.unwrap();
    let sources: Vec<String> = session
        .object_rows_page(&admin, "run", None)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.id.starts_with("service-facts-"))
        .map(|row| row.id.clone())
        .collect();
    session.commit().await.unwrap();
    let dispatcher = ManagementDispatcher::new(
        service.store().clone(),
        vec![admin.clone(), admin_two.clone(), other_namespace.clone()],
    )
    .unwrap();
    let (digest, published) = healthy_export(&dispatcher, &admin, "owned-export", &sources).await;
    let ManagementTraceExportResultV2::TraceExportDelivered { record_id, .. } =
        trace_result(&published)
    else {
        panic!("expected the delivered result");
    };
    let body = record_body(&service, &admin, &record_id).await.unwrap();
    // A second owner's publish job cannot complete the delivered export again,
    // and in a foreign namespace the export does not exist at all.
    for (owner, key, code) in [
        (&admin_two, "stale-owner-publish", "conflict"),
        (&other_namespace, "foreign-owner-publish", "not_found"),
    ] {
        let refused = run_to_terminal(
            &dispatcher,
            owner,
            dispatcher
                .submit(
                    owner,
                    "trace.export",
                    publish_payload(key, "owned-export", &digest),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(refused.identity().state, ManagementJobState::Failed);
        assert_eq!(refused.error_code(), Some(code));
    }
    assert_eq!(
        record_body(&service, &admin, &record_id).await.unwrap(),
        body,
        "no stale owner changed the record"
    );
    // The foreign owner cannot even observe the first owner's terminal job.
    assert!(matches!(
        dispatcher.status(&admin_two, published.identity().id).await,
        Err(Error::NotFound)
    ));
    // Neither owner can delete the published history through the real gate.
    for owner in [&admin, &admin_two] {
        let mut session = service.store().session().await.unwrap();
        match session.delete(owner, "artifact", &record_id).await {
            Err(Error::Conflict(message)) => {
                assert_eq!(message, "trace export history must be retained")
            }
            other => panic!("published history must be retained, got {other:?}"),
        }
        session.commit().await.unwrap();
    }
    // The tree itself stays the one delivery: it is still exactly verifiable
    // and no owner action republishes or replaces it.
    let mut session = service.store().session().await.unwrap();
    let members = prepare_native_members(&admin, &mut session, &sources)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let receipt = service
        .store()
        .verify_trace_export_tree(
            &admin,
            "owned-export",
            &frozen(&members.files),
            &members.candidate_digest,
        )
        .await
        .unwrap();
    let ManagementTraceExportResultV2::TraceExportDelivered { receipt_digest, .. } =
        trace_result(&published)
    else {
        unreachable!("already matched above");
    };
    assert_eq!(receipt.completion_receipt_digest, receipt_digest);
    assert_eq!(
        record_body(&service, &admin, &record_id).await.unwrap(),
        body
    );
}

// A read-only SQLite snapshot retains every table, row identity and cell type.
// Python's sqlite3 is already part of the repository's required test runtime.
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
