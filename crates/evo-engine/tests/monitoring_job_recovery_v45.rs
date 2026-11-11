//! AG-166d drift-only recovery controls for `monitoring.drift.record`.
//!
//! The drift fact, the scope `invalidated_by`, its two environment edges, the
//! audit record and the terminal job state commit in one transaction; a crash
//! is reconciled from that same bound result, never re-run. Cancellation
//! before the effect leaves no business row, cancellation after it records the
//! arrival while keeping the drift, the scope and the audit. Stale leases and
//! changed revoke watermarks cannot complete a drift.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SkillSnapshot, SurfaceCoverage,
    SurfaceItem, SystemSnapshotV3,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash, now};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobStatusV1, ManagementJobV2,
    ManagementMonitoringResultV2, ManagementResultV2,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::monitoring::{
    ConsolidationScopeIndex, EnvironmentDriftRecord, MonitoringCoordinator, QualityStatus,
    RecordEnvironmentRequest, RecordedEnvironment,
};
use evo_engine::optimization::{
    DevelopmentExecutionProvenance, DevelopmentManifest, DevelopmentRunReport, DevelopmentTask,
    OptimizationJournal, OptimizationJournalStage, PairedTaskResult, StageDependency, StageFact,
    StageFactKind,
};
use evo_engine::release_store::{
    CompletePrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence,
};
use evo_storage::Store;
use evo_storage::budget::{RootBudgetAuthorization, RootBudgetRecord};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use trusted_compiler_inputs::StoreOptimizationJournal;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant", actor, role).unwrap()
}

fn surface() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "monitor-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: d("surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::RuntimeOwned,
            mapped_field: None,
            consumer: None,
            reason: "fixture projection".into(),
        }],
    }
}

fn system(model: &str) -> SystemSnapshotV3 {
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
    SystemSnapshotV3 {
        schema_version: "rsia.system_snapshot.v3".into(),
        profile_id: "profile".into(),
        host_id: "monitor-host".into(),
        host_version: "1.0.0".into(),
        model_id: model.into(),
        tools: source.names().unwrap(),
        tool_source: source,
        mandatory_context_digest: d("mandatory"),
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

struct Fixture {
    _dir: tempfile::TempDir,
    database: std::path::PathBuf,
    store: Store,
    admin: Context,
    _host: Context,
    _worker: Context,
    previous: RecordedEnvironment,
    current: RecordedEnvironment,
    scope_id: String,
    budget: RootBudgetRecord,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("monitoring-recovery.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let admin = context("admin", Role::Admin);
        let host = context("host", Role::Host);
        let worker = context("worker", Role::Worker);
        trusted_compiler_inputs::register_fixture_host_surface(
            &admin,
            &store,
            "surface",
            surface(),
            vec!["model".into()],
        )
        .await
        .unwrap();
        let snapshot = ReleaseStore::prepare_complete_run(
            &host,
            &store,
            CompletePrepareRunRequest {
                run_id: "host-run-1".into(),
                profile_id: "profile".into(),
                system_snapshot: system("model-v1"),
                host_surface_id: "surface".into(),
                host_capabilities: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                task_input_digest: d("task-input"),
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            },
        )
        .await
        .unwrap();
        ReleaseStore::record_applied_request(
            &host,
            &store,
            "host-run-1",
            TrustedHostExecutionEvidence {
                actual_request_material: snapshot.request_material.clone(),
                environment_digest: snapshot.environment_digest.clone(),
                host_surface_digest: snapshot.host_surface_digest.clone(),
                host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
                offered: vec![],
                attached: vec![],
                used: vec![],
                execution_receipt_id: None,
                truncated: false,
            },
        )
        .await
        .unwrap();
        for source in [
            authority("source-1", "first development source"),
            authority("source-2", "second development source"),
        ] {
            store_trace_authority(&store, &host, &source).await.unwrap();
        }
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .bump_watermark(&host, &d("initial-watermark"))
                .await
                .unwrap(),
            1
        );
        session.commit().await.unwrap();
        let manifest = DevelopmentManifest::build(
            "development-manifest",
            vec![DevelopmentTask {
                id: "task-a".into(),
                parent_family: "development-family".into(),
                input_digest: d("development-task-input"),
            }],
        )
        .unwrap();
        let manifest_digest = manifest.digest.clone();
        let grader_digest = d("development-grader");
        let strategy_digest = fingerprint(&Strategy::default()).unwrap();
        let parent_skill = SkillSnapshot {
            content: "parent rule".into(),
            applicability: "development only".into(),
            counterexample: "counterexample".into(),
            required_capabilities: vec![],
            dependencies: vec![],
        };
        let parent_skill_digest =
            evo_core::skill_edit::skill_snapshot_digest(&parent_skill).unwrap();
        let parent_bundle_digest = d("parent-bundle");
        let previous = MonitoringCoordinator::record_environment_from_run(
            &host,
            &store,
            RecordEnvironmentRequest {
                run_id: "host-run-1".into(),
                development_manifest_digest: manifest_digest.clone(),
                grader_digest: grader_digest.clone(),
                generation_strategy_digest: strategy_digest.clone(),
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap();
        let budget = store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "root-budget".into(),
                    billing_scope: "billing-scope".into(),
                    allowed_namespaces: vec!["tenant".into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "payer".into(),
                    authorization_receipt_digest: d("authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let fact = stage_report(
            &store,
            &worker,
            &previous,
            &manifest_digest,
            &grader_digest,
            &parent_bundle_digest,
        )
        .await;
        let cycle = MonitoringCoordinator::close_development_cycle(
            &worker,
            &store,
            evo_engine::monitoring::CompleteDevelopmentCycleRequest {
                skill_id: "skill-a".into(),
                profile_id: "profile".into(),
                environment_id: previous.environment.id.clone(),
                development_manifest_digest: manifest_digest.clone(),
                grader_digest: grader_digest.clone(),
                generation_strategy_digest: strategy_digest.clone(),
                parent_skill_digest,
                parent_bundle_digest,
                billing_scope: budget.billing_scope.clone(),
                root_budget_id: budget.root_budget_id.clone(),
                report_fact_id: fact,
                source_ids: vec!["source-2".into(), "source-1".into()],
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap();
        let scope_id = cycle.scope_id;
        let snapshot = ReleaseStore::prepare_complete_run(
            &host,
            &store,
            CompletePrepareRunRequest {
                run_id: "host-run-2".into(),
                profile_id: "profile".into(),
                system_snapshot: system("model-v2"),
                host_surface_id: "surface".into(),
                host_capabilities: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                task_input_digest: d("task-input-2"),
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            },
        )
        .await
        .unwrap();
        ReleaseStore::record_applied_request(
            &host,
            &store,
            "host-run-2",
            TrustedHostExecutionEvidence {
                actual_request_material: snapshot.request_material.clone(),
                environment_digest: snapshot.environment_digest.clone(),
                host_surface_digest: snapshot.host_surface_digest.clone(),
                host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
                offered: vec![],
                attached: vec![],
                used: vec![],
                execution_receipt_id: None,
                truncated: false,
            },
        )
        .await
        .unwrap();
        let current = MonitoringCoordinator::record_environment_from_run(
            &host,
            &store,
            RecordEnvironmentRequest {
                run_id: "host-run-2".into(),
                development_manifest_digest: manifest_digest.clone(),
                grader_digest: grader_digest.clone(),
                generation_strategy_digest: strategy_digest.clone(),
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap();
        assert_eq!(current.observation.quality_status, QualityStatus::Unknown);
        Self {
            _dir: dir,
            database,
            store,
            admin,
            _host: host,
            _worker: worker,
            previous,
            current,
            scope_id,
            budget,
        }
    }

    fn previous_id(&self) -> &str {
        &self.previous.environment.id
    }

    fn current_id(&self) -> &str {
        &self.current.environment.id
    }
}

async fn stage_report(
    store: &Store,
    worker: &Context,
    environment: &RecordedEnvironment,
    manifest_digest: &str,
    grader_digest: &str,
    parent_bundle_digest: &str,
) -> String {
    let report = DevelopmentRunReport {
        request_id: "development-request-1".into(),
        manifest_digest: manifest_digest.into(),
        parent_bundle_digest: parent_bundle_digest.into(),
        candidate_bundle_digest: d("candidate-bundle-1"),
        environment_digest: environment.environment.environment_digest.clone(),
        grader_digest: grader_digest.into(),
        results: vec![PairedTaskResult {
            task_id: "task-a".into(),
            parent_score_micros: 500_000,
            candidate_score_micros: 600_000,
            parent_passed: true,
            candidate_passed: true,
            parent_execution_id: "parent-execution-1".into(),
            candidate_execution_id: "candidate-execution-1".into(),
            grader_receipt_digest: d("grader-receipt-1"),
        }],
        execution_receipt_id: "runner-execution-1".into(),
        usage_record_ids: vec![],
        provenance: DevelopmentExecutionProvenance::Fixture,
    };
    let grader_receipt = d("grader-receipt-1");
    let mut session = store.session().await.unwrap();
    for (logical_kind, id) in [
        ("execution", "parent-execution-1"),
        ("execution", "candidate-execution-1"),
        ("execution", "runner-execution-1"),
        ("grader", grader_receipt.as_str()),
    ] {
        session
            .put(
                worker,
                "artifact",
                id,
                worker.actor(),
                &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":1,"fixture":true}),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let payload = serde_json::to_value(&report).unwrap();
    let dependencies = vec![
        StageDependency {
            kind: "execution".into(),
            id: "parent-execution-1".into(),
        },
        StageDependency {
            kind: "execution".into(),
            id: "candidate-execution-1".into(),
        },
        StageDependency {
            kind: "grader".into(),
            id: d("grader-receipt-1"),
        },
        StageDependency {
            kind: "run".into(),
            id: "source-1".into(),
        },
        StageDependency {
            kind: "run".into(),
            id: "source-2".into(),
        },
        StageDependency {
            kind: "revoke_watermark".into(),
            id: "1".into(),
        },
    ];
    let fact = StageFact {
        schema_version: "rsia.optimization.stage_fact.v1".into(),
        artifact_id: "placeholder".into(),
        namespace: worker.namespace().into(),
        episode_id: "development-episode-1".into(),
        step: 1,
        attempt: 1,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id: report.request_id,
        input_digest: d("development-input-1"),
        output_digest: None,
        dependencies,
        payload,
    }
    .seal()
    .unwrap();
    let id = fact.artifact_id.clone();
    StoreOptimizationJournal::new(store.clone(), worker.clone(), worker.actor())
        .unwrap()
        .commit(fact)
        .await
        .unwrap();
    id
}

fn drift_payload(
    key: &str,
    previous_id: &str,
    current_id: &str,
    scope_id: &str,
    reason: &str,
) -> Value {
    json!({
        "schema_version": "rsia.management.monitoring_drift_record.v1",
        "request_key": key,
        "previous_environment_id": previous_id,
        "current_environment_id": current_id,
        "scope_id": scope_id,
        "reason": reason,
    })
}

async fn bump(store: &Store, ctx: &Context, label: &str) -> i64 {
    let mut session = store.session().await.unwrap();
    let value = session
        .bump_watermark(ctx, &hash(label.as_bytes()))
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
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
    .expect("drift recovery job reaches a terminal state")
}

async fn done(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    payload: Value,
) -> ManagementJobStatusV1 {
    let queued = dispatcher
        .submit(ctx, "monitoring.drift.record", payload)
        .await
        .unwrap();
    terminal(dispatcher, ctx, queued.identity().id).await
}

fn drift_result(status: &ManagementJobStatusV1) -> &ManagementMonitoringResultV2 {
    match status.result.as_ref() {
        Some(ManagementResultV2::Monitoring(result)) => result,
        other => panic!("expected Monitoring result, got {other:?}"),
    }
}

async fn drift_facts(store: &Store, ctx: &Context) -> Vec<EnvironmentDriftRecord> {
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    artifacts
        .into_iter()
        .filter(|value| {
            value["schema_version"].as_str() == Some("rsia.monitoring.environment_drift.v1")
        })
        .map(|value| serde_json::from_value(value).unwrap())
        .collect()
}

async fn scope_index(store: &Store, ctx: &Context, scope_id: &str) -> ConsolidationScopeIndex {
    let mut session = store.session().await.unwrap();
    let index = session.need(ctx, "artifact", scope_id).await.unwrap();
    session.commit().await.unwrap();
    index
}

async fn raw_artifact(store: &Store, ctx: &Context, id: &str) -> String {
    let mut session = store.session().await.unwrap();
    let row = session
        .object_row(ctx, "artifact", id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    row.body
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

/// A queued job-row fixture for cancellation before any claim.
async fn accepted_job(store: &Store, ctx: &Context, key: &str) -> String {
    let owner = ctx.actor();
    let namespace = ctx.namespace();
    let digest = hash(format!("{namespace}\0{owner}\0monitoring.drift.record\0{key}").as_bytes());
    let id = format!("management-job-{}", &digest[..24]);
    let private_input_ref = format!("management-input-{}", &digest[..24]);
    let job = ManagementJobV2 {
        id: id.clone(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "monitoring.drift.record".into(),
        request_key: key.into(),
        payload_digest: hash(b"accepted-drift-fixture"),
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

/// The drift, its scope invalidation, edges, audit and the job terminal state
/// share one commit: an injected terminal-job refusal rolls the drift back.
#[tokio::test]
async fn drift_scope_invalidation_and_job_terminal_share_one_commit() {
    let fixture = Fixture::new().await;
    let scope_before = raw_artifact(&fixture.store, &fixture.admin, &fixture.scope_id).await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    sqlite(
        &fixture.database,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute(\"CREATE TRIGGER stop_drift_job_finish BEFORE INSERT ON objects WHEN NEW.id LIKE 'management-job-%' AND json_extract(NEW.body,'$.state')='succeeded' BEGIN SELECT RAISE(ABORT,'injected-job-stop'); END\")\nc.commit()\nprint('ok')",
    );
    let failed = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "atomic-drift",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "atomic drift",
        ),
    )
    .await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("internal_error"));
    assert!(drift_facts(&fixture.store, &fixture.admin).await.is_empty());
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by,
        None,
        "the scope invalidation rolled back with the job write"
    );
    assert_eq!(
        raw_artifact(&fixture.store, &fixture.admin, &fixture.scope_id).await,
        scope_before
    );
    sqlite(
        &fixture.database,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1], timeout=30)\nc.execute('DROP TRIGGER stop_drift_job_finish')\nc.commit()\nprint('ok')",
    );
    let healthy = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "atomic-drift-healthy",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "atomic drift healthy",
        ),
    )
    .await;
    assert_eq!(healthy.state, ManagementJobState::Succeeded, "{healthy:?}");
    assert_eq!(drift_facts(&fixture.store, &fixture.admin).await.len(), 1);
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by
            .as_deref(),
        Some(drift_id(drift_result(&healthy)))
    );
}

/// The same request key and reason replay the original immutable result; a
/// changed reason under the same key is an exact Conflict, and a different key
/// over the same drift identity cannot replace the original reason.
#[tokio::test]
async fn drift_same_key_reuses_original_fact() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let payload = drift_payload(
        "reuse-key",
        fixture.previous_id(),
        fixture.current_id(),
        &fixture.scope_id,
        "original reason",
    );
    let first = done(&dispatcher, &fixture.admin, payload.clone()).await;
    assert_eq!(first.state, ManagementJobState::Succeeded);
    let drift_id = drift_id(drift_result(&first)).to_owned();
    let audits = fixture.store.verify_audit(&fixture.admin).await.unwrap();
    let drift_rows = drift_facts(&fixture.store, &fixture.admin).await;
    assert_eq!(drift_rows.len(), 1);
    // Same key and same reason: the immutable terminal is returned without a
    // new drift, audit or scope write.
    let replay = dispatcher
        .submit(&fixture.admin, "monitoring.drift.record", payload.clone())
        .await
        .unwrap();
    assert_eq!(replay.identity().id, first.id);
    assert_eq!(replay.identity().state, ManagementJobState::Succeeded);
    assert_eq!(
        serde_json::to_value(replay.result()).unwrap(),
        serde_json::to_value(&first.result).unwrap()
    );
    assert_eq!(
        fixture.store.verify_audit(&fixture.admin).await.unwrap(),
        audits
    );
    assert_eq!(drift_facts(&fixture.store, &fixture.admin).await.len(), 1);
    // Same key, changed reason: the role-less idempotency row is content-bound
    // and refuses before any session.
    let mut changed = payload;
    changed["reason"] = json!("changed reason");
    let error = dispatcher
        .submit(&fixture.admin, "monitoring.drift.record", changed)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message) if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    assert_eq!(
        fixture.store.verify_audit(&fixture.admin).await.unwrap(),
        audits
    );
    // A different key over the same derived drift identity would change the
    // historical reason; the immutable drift fact refuses it.
    let replacement = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "replacement-key",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "replacement reason",
        ),
    )
    .await;
    assert_eq!(
        replacement.state,
        ManagementJobState::Failed,
        "{replacement:?}"
    );
    assert_eq!(replacement.error_code.as_deref(), Some("conflict"));
    let drift_rows = drift_facts(&fixture.store, &fixture.admin).await;
    assert_eq!(drift_rows.len(), 1);
    assert_eq!(drift_rows[0].reason, "original reason");
    assert_eq!(drift_rows[0].id, drift_id);
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by
            .as_deref(),
        Some(drift_id.as_str())
    );
}

/// Cancelling a never-claimed drift job is closure before effect: no drift,
/// no scope invalidation, no business row.
#[tokio::test]
async fn drift_cancel_before_effect_creates_no_business_rows() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let job_id = accepted_job(&fixture.store, &fixture.admin, "cancel-before").await;
    let cancelled = dispatcher.cancel(&fixture.admin, &job_id).await.unwrap();
    assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert!(drift_facts(&fixture.store, &fixture.admin).await.is_empty());
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by,
        None
    );
}

/// Cancelling a successful drift only records the arrival: the drift, the
/// scope invalidation, the result and the audit stay, with no refund or
/// invented zero-cost fact.
#[tokio::test]
async fn drift_cancel_after_effect_keeps_drift_and_scope() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let completed = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "cancel-after",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "cancel after",
        ),
    )
    .await;
    assert_eq!(completed.state, ManagementJobState::Succeeded);
    let drift_id = drift_id(drift_result(&completed)).to_owned();
    let before_scope = raw_artifact(&fixture.store, &fixture.admin, &fixture.scope_id).await;
    let before_drift = raw_artifact(&fixture.store, &fixture.admin, &drift_id).await;
    let before_root = {
        let mut session = fixture.store.session().await.unwrap();
        let root = session
            .root_budget(&fixture.admin, &fixture.budget.billing_scope)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        root
    };
    let audits = fixture.store.verify_audit(&fixture.admin).await.unwrap();
    let cancelled = dispatcher
        .cancel(&fixture.admin, &completed.id)
        .await
        .unwrap();
    assert_eq!(
        cancelled.identity().state,
        ManagementJobState::Succeeded,
        "a committed drift is an audit fact"
    );
    assert!(cancelled.cancel_requested());
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        serde_json::to_value(cancelled.result()).unwrap(),
        serde_json::to_value(&completed.result).unwrap()
    );
    assert_eq!(
        raw_artifact(&fixture.store, &fixture.admin, &drift_id).await,
        before_drift
    );
    assert_eq!(
        raw_artifact(&fixture.store, &fixture.admin, &fixture.scope_id).await,
        before_scope
    );
    let after_root = {
        let mut session = fixture.store.session().await.unwrap();
        let root = session
            .root_budget(&fixture.admin, &fixture.budget.billing_scope)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        root
    };
    assert_eq!(after_root, before_root, "no refund or fee invention");
    assert_eq!(
        fixture.store.verify_audit(&fixture.admin).await.unwrap(),
        audits + 1,
        "only the cancel audit is added"
    );
}

/// A stale generation/token can neither pass the physical job fence nor write,
/// and a changed namespace revoke watermark blocks completion with no drift.
#[tokio::test]
async fn drift_stale_lease_or_revoked_watermark_blocks_completion() {
    // A seeded running claim: stale generation/token/operation never match.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("drift-stale-fence.sqlite3"))
        .await
        .unwrap();
    let ctx = context("admin", Role::Admin);
    let job_id = "management-job-drift-fence";
    let lease = hash(b"current-drift-lease");
    let job = ManagementJobV2 {
        id: job_id.into(),
        schema_version: "rsia.management_job.v2".into(),
        operation: "monitoring.drift.record".into(),
        request_key: "drift-fence".into(),
        payload_digest: hash(b"payload"),
        owner_actor: ctx.actor().into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: "management-input-drift-fence".into(),
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
            operation: "monitoring.drift.record",
            generation: 0,
            lease_token: &lease,
        },
        evo_storage::ManagementJobFence {
            job_id,
            operation: "monitoring.drift.record",
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
        operation: "monitoring.drift.record",
        generation: 1,
        lease_token: &lease,
    };
    session
        .ensure_management_job_fence(&ctx, &current)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    // A watermark advance after the environments were recorded fails the real
    // completion before its first write.
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    assert_eq!(
        bump(&fixture.store, &fixture.admin, "revoked-watermark").await,
        2
    );
    let blocked = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "revoked-watermark",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "revoked watermark",
        ),
    )
    .await;
    assert_eq!(blocked.state, ManagementJobState::Failed, "{blocked:?}");
    assert_eq!(blocked.error_code.as_deref(), Some("conflict"));
    assert!(drift_facts(&fixture.store, &fixture.admin).await.is_empty());
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by,
        None
    );
}

fn drift_id(result: &ManagementMonitoringResultV2) -> &str {
    match result {
        ManagementMonitoringResultV2::EnvironmentDriftRecorded { drift_id, .. } => drift_id,
    }
}
