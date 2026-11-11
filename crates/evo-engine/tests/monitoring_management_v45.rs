//! AG-166d drift-only management controls for `monitoring.drift.record`
//! (v4.5 §6.1.2 L822, E13, §11; drift invalidation clauses).
//!
//! Real Store, real Admin context, real Host-produced ProgramFixture
//! environments and a real consolidation scope from a completed development
//! cycle. The strict wire carries only two existing environment ids, a stored
//! scope id and a bounded diagnostic reason; every actor/role/namespace/owner,
//! body clock, path, root, snapshot, source or authority carrier is refused
//! before any session, and the result carries only the committed identifiers.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SkillSnapshot, SurfaceCoverage,
    SurfaceItem, SystemSnapshotV3,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJobState, ManagementJobStatusV1, ManagementMonitoringResultV2,
    ManagementResultV2,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::monitoring::{
    ConsolidationScopeIndex, DeploymentObservation, EnvironmentDriftRecord,
    EnvironmentEvidenceScope, MonitoringCoordinator, QualityStatus, RecordEnvironmentRequest,
    RecordedEnvironment,
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
    worker: Context,
    previous: RecordedEnvironment,
    current: RecordedEnvironment,
    scope_id: String,
    budget: RootBudgetRecord,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("monitoring-management.sqlite3");
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
        assert_eq!(previous.observation.quality_status, QualityStatus::Unknown);
        assert_eq!(
            previous.environment.evidence_scope,
            EnvironmentEvidenceScope::ProgramFixture
        );
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
            cycle_request(
                &previous,
                &manifest_digest,
                &grader_digest,
                &strategy_digest,
                &parent_skill_digest,
                &parent_bundle_digest,
                &budget,
                fact,
            ),
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
        Self {
            _dir: dir,
            database,
            store,
            admin,
            _host: host,
            worker,
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

#[allow(clippy::too_many_arguments)]
fn cycle_request(
    previous: &RecordedEnvironment,
    manifest_digest: &str,
    grader_digest: &str,
    strategy_digest: &str,
    parent_skill_digest: &str,
    parent_bundle_digest: &str,
    budget: &RootBudgetRecord,
    fact: String,
) -> evo_engine::monitoring::CompleteDevelopmentCycleRequest {
    evo_engine::monitoring::CompleteDevelopmentCycleRequest {
        skill_id: "skill-a".into(),
        profile_id: "profile".into(),
        environment_id: previous.environment.id.clone(),
        development_manifest_digest: manifest_digest.into(),
        grader_digest: grader_digest.into(),
        generation_strategy_digest: strategy_digest.into(),
        parent_skill_digest: parent_skill_digest.into(),
        parent_bundle_digest: parent_bundle_digest.into(),
        billing_scope: budget.billing_scope.clone(),
        root_budget_id: budget.root_budget_id.clone(),
        report_fact_id: fact,
        source_ids: vec!["source-2".into(), "source-1".into()],
        revoke_watermark: 1,
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
    let parent_execution = "parent-execution-1".to_owned();
    let candidate_execution = "candidate-execution-1".to_owned();
    let runner_execution = "runner-execution-1".to_owned();
    let grader_receipt = d("grader-receipt-1");
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
            parent_execution_id: parent_execution.clone(),
            candidate_execution_id: candidate_execution.clone(),
            grader_receipt_digest: grader_receipt.clone(),
        }],
        execution_receipt_id: runner_execution.clone(),
        usage_record_ids: vec![],
        provenance: DevelopmentExecutionProvenance::Fixture,
    };
    let mut session = store.session().await.unwrap();
    for (logical_kind, id) in [
        ("execution", parent_execution.as_str()),
        ("execution", candidate_execution.as_str()),
        ("execution", runner_execution.as_str()),
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
            id: parent_execution,
        },
        StageDependency {
            kind: "execution".into(),
            id: candidate_execution,
        },
        StageDependency {
            kind: "grader".into(),
            id: grader_receipt,
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
    .expect("drift management job reaches a terminal state")
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

async fn artifact_wire(store: &Store, ctx: &Context) -> BTreeSet<(String, String)> {
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

/// The Admin role of the drift operation is judged before any payload is
/// parsed or a session opened. `monitoring.environment.record` stays unwired
/// in this drift-only range.
#[tokio::test]
async fn drift_record_is_admin_only_before_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drift-role.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = context("admin", Role::Admin);
    let evaluator = context("evaluator", Role::Evaluator);
    let host = context("host", Role::Host);
    let agent = context("agent", Role::Agent);
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let before = management_sql_snapshot(&path);
    let valid = drift_payload(
        "role-key",
        "env-previous",
        "env-current",
        "scope-1",
        "drift",
    );
    for caller in [&evaluator, &host, &agent] {
        for payload in [Value::Null, valid.clone(), json!({"schema_version": "x"})] {
            let error = dispatcher
                .submit(caller, "monitoring.drift.record", payload)
                .await
                .unwrap_err();
            assert!(matches!(error, Error::Forbidden), "{error:?}");
            assert_eq!(
                management_sql_snapshot(&path),
                before,
                "a role refusal writes nothing"
            );
        }
    }
    // environment.record keeps its fail-closed unwired status for Admin.
    let error = dispatcher
        .submit(
            &admin,
            "monitoring.environment.record",
            json!({
                "schema_version": "rsia.management.monitoring_environment_record.v1",
                "request_key": "environment-role",
                "run_id": "host-run-1",
                "development_control_id": "control-1",
            }),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Invalid(message) if message == "management_operation_not_wired"),
        "{error:?}"
    );
    assert_eq!(management_sql_snapshot(&path), before);
    store.close().await;
}

/// A real Admin job over the real Host-produced environments and the stored
/// scope writes the drift fact, the scope invalidation, both edges and the
/// audit record in its one committed transaction.
#[tokio::test]
async fn drift_reloads_both_environments_and_scope() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let status = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "reload-key",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "model deployment changed",
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");
    assert_eq!(status.step, "environment_drift_recorded");
    let result = drift_result(&status);
    let ManagementMonitoringResultV2::EnvironmentDriftRecorded {
        drift_id,
        previous_environment_id,
        current_environment_id,
        invalidated_scope_id,
    } = result;
    assert_eq!(previous_environment_id, fixture.previous_id());
    assert_eq!(current_environment_id, fixture.current_id());
    assert_eq!(invalidated_scope_id, &fixture.scope_id);
    let index = scope_index(&fixture.store, &fixture.admin, &fixture.scope_id).await;
    assert_eq!(index.invalidated_by.as_deref(), Some(drift_id.as_str()));
    let drift: EnvironmentDriftRecord = {
        let mut session = fixture.store.session().await.unwrap();
        let value = session
            .need(&fixture.admin, "artifact", drift_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        value
    };
    assert_eq!(drift.schema_version, "rsia.monitoring.environment_drift.v1");
    assert_eq!(drift.reason, "model deployment changed");
    assert_eq!(drift.previous_environment_id, fixture.previous_id());
    assert_eq!(drift.current_environment_id, fixture.current_id());
    assert_eq!(drift.invalidated_scope_id, fixture.scope_id);
    assert_eq!(
        drift.previous_identity_digest,
        fixture.previous.environment.identity_digest
    );
    assert_eq!(
        drift.current_identity_digest,
        fixture.current.environment.identity_digest
    );
    let edges = {
        let mut session = fixture.store.session().await.unwrap();
        let mut edges = session
            .dependency_edges(&fixture.admin, "artifact", drift_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        edges.sort();
        edges
    };
    let mut expected = vec![
        ("artifact".to_owned(), fixture.previous_id().to_owned()),
        ("artifact".to_owned(), fixture.current_id().to_owned()),
    ];
    expected.sort();
    assert_eq!(edges, expected);
    // The old scope can no longer be claimed; Active/approval/stop were not
    // touched by the drift.
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &fixture.scope_id
        )
        .await,
        Err(Error::Conflict(_))
    ));
    let audits = audit_facts(&fixture.database, "monitoring.environment.drift", drift_id);
    assert_eq!(audits, 1, "the drift audit record is committed once");
    // Job owner reads the bounded result; committed effects carry only ids.
    assert!(
        status
            .committed_side_effects
            .as_ref()
            .is_some_and(|effects| {
                effects.len() == 1
                    && serde_json::to_value(&effects[0]).unwrap()["kind"] == "environment_drift"
            })
    );
}

/// A same-identity pair and a changed revoke watermark are refused before the
/// drift's first write; neither leaves a drift fact or a scope invalidation.
#[tokio::test]
async fn drift_same_identity_or_stale_sources_refuse_before_write() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let same = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "same-identity",
            fixture.previous_id(),
            fixture.previous_id(),
            &fixture.scope_id,
            "no drift",
        ),
    )
    .await;
    assert_eq!(same.state, ManagementJobState::Failed, "{same:?}");
    assert_eq!(same.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by,
        None
    );

    assert_eq!(
        bump(&fixture.store, &fixture.admin, "stale-watermark").await,
        2
    );
    let stale = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "stale-sources",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "stale sources",
        ),
    )
    .await;
    assert_eq!(stale.state, ManagementJobState::Failed, "{stale:?}");
    assert_eq!(stale.error_code.as_deref(), Some("conflict"));
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id)
            .await
            .invalidated_by,
        None
    );
    let drift_facts = artifact_wire(&fixture.store, &fixture.admin)
        .await
        .into_iter()
        .filter(|(schema, _)| schema == "rsia.monitoring.environment_drift.v1")
        .count();
    assert_eq!(drift_facts, 0, "a refused drift writes no drift fact");
}

/// Drift is diagnostic: Active/approval/root stop and the Unknown quality of
/// the recorded environment are unchanged; only the drift and its private
/// input are added.
#[tokio::test]
async fn drift_does_not_change_active_approval_or_stop() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let root_before = {
        let mut session = fixture.store.session().await.unwrap();
        let root = session
            .root_budget(&fixture.admin, &fixture.budget.billing_scope)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        root
    };
    let artifacts_before = artifact_wire(&fixture.store, &fixture.admin).await;
    let status = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "active-unchanged",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "diagnostic only",
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded);
    let root_after = {
        let mut session = fixture.store.session().await.unwrap();
        let root = session
            .root_budget(&fixture.admin, &fixture.budget.billing_scope)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        root
    };
    assert_eq!(root_after, root_before, "root authorization/stop unchanged");
    let artifacts_after = artifact_wire(&fixture.store, &fixture.admin).await;
    let added: BTreeSet<_> = artifacts_after.difference(&artifacts_before).collect();
    let added_schemas: BTreeSet<&str> = added.iter().map(|(schema, _)| schema.as_str()).collect();
    assert_eq!(
        added_schemas,
        BTreeSet::from([
            "rsia.management_private_input.v1",
            "rsia.monitoring.environment_drift.v1"
        ]),
        "only the drift fact and its private input are added: {added:?}"
    );
    for (schema, _) in &artifacts_after {
        assert!(
            !schema.contains("release")
                && !schema.contains("approval")
                && !schema.contains("active"),
            "drift must not write a release/approval/Active fact: {schema}"
        );
    }
    let observation: DeploymentObservation = {
        let mut session = fixture.store.session().await.unwrap();
        let value = session
            .need(&fixture.admin, "artifact", &fixture.current.observation.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        value
    };
    assert_eq!(observation.quality_status, QualityStatus::Unknown);
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &fixture.scope_id
        )
        .await,
        Err(Error::Conflict(_))
    ));
}

/// The reason is bounded at the wire/semantic boundary and is a diagnostic
/// input only: it is stored on the drift fact and never returned in the job
/// result or its public status projection.
#[tokio::test]
async fn drift_reason_is_bounded_and_not_returned() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let before = management_sql_snapshot(&fixture.database);
    for reason in [String::new(), "x".repeat(513)] {
        let error = dispatcher
            .submit(
                &fixture.admin,
                "monitoring.drift.record",
                drift_payload(
                    "reason-bound",
                    fixture.previous_id(),
                    fixture.current_id(),
                    &fixture.scope_id,
                    &reason,
                ),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "drift reason must be 1..=512 bytes"),
            "{error:?}"
        );
    }
    assert_eq!(
        management_sql_snapshot(&fixture.database),
        before,
        "an out-of-range reason writes nothing"
    );
    let secret = format!("secret-reason-{}", "y".repeat(498));
    assert_eq!(secret.len(), 512);
    let status = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "reason-bounded",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            &secret,
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded);
    assert!(
        status
            .committed_side_effects
            .as_ref()
            .is_some_and(|effects| {
                effects.len() == 1
                    && serde_json::to_value(&effects[0]).unwrap()["kind"] == "environment_drift"
            }),
        "{status:?}"
    );
    let public = serde_json::to_string(&status).unwrap();
    assert!(
        !public.contains(&secret),
        "the diagnostic reason is not part of the result/status wire"
    );
    assert!(!public.contains("secret-reason-"));
}

/// The strict wire refuses every identity/authority/origin field and every
/// unknown member before any session, with zero writes.
#[tokio::test]
async fn drift_wire_rejects_identity_and_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drift-wire.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = context("admin", Role::Admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let before = management_sql_snapshot(&path);
    let valid = drift_payload(
        "wire-key",
        "env-previous",
        "env-current",
        "scope-1",
        "drift",
    );
    for (field, value) in [
        ("actor", json!("admin")),
        ("role", json!("admin")),
        ("namespace", json!("tenant")),
        ("owner", json!("admin")),
        ("created_at", json!(1)),
        ("now", json!(1)),
        ("path", json!("/tmp/private")),
        ("root", json!("/")),
        ("verified", json!(true)),
        ("verified_execution", json!(true)),
        ("snapshot", json!({"model_id": "m"})),
        ("source", json!("run-1")),
        ("previous_identity_digest", json!("0".repeat(64))),
        ("quality_status", json!("trusted")),
    ] {
        let mut payload = valid.clone();
        payload[field] = value;
        let error = dispatcher
            .submit(&admin, "monitoring.drift.record", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "invalid management payload"),
            "{field}: {error:?}"
        );
        assert_eq!(
            management_sql_snapshot(&path),
            before,
            "{field} must write nothing"
        );
    }
    // A different schema is refused at its exact version, not silently read.
    let mut wrong_schema = valid.clone();
    wrong_schema["schema_version"] = json!("rsia.management.monitoring_drift_record.v2");
    let error = dispatcher
        .submit(&admin, "monitoring.drift.record", wrong_schema)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Invalid(message) if message == "unsupported management payload schema"),
        "{error:?}"
    );
    // Missing members and non-object bodies are refused as invalid payloads.
    for payload in [
        json!({"schema_version": "rsia.management.monitoring_drift_record.v1", "request_key": "k"}),
        json!({"schema_version": "rsia.management.monitoring_drift_record.v1"}),
        Value::Null,
        json!([valid]),
    ] {
        assert!(matches!(
            dispatcher
                .submit(&admin, "monitoring.drift.record", payload)
                .await,
            Err(Error::Invalid(_))
        ));
        assert_eq!(management_sql_snapshot(&path), before);
    }
    store.close().await;
}

/// The result is readable by its exact owner (namespace/actor/role) only, and
/// its closed shape carries only the committed identifiers.
#[tokio::test]
async fn drift_result_is_owner_scoped_and_bounded() {
    let fixture = Fixture::new().await;
    let other_admin = Context::new("tenant", "other-admin", Role::Admin).unwrap();
    let evaluator = Context::new("tenant", "evaluator", Role::Evaluator).unwrap();
    let other_namespace = Context::new("other-tenant", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(
        fixture.store.clone(),
        vec![
            fixture.admin.clone(),
            other_admin.clone(),
            evaluator.clone(),
            other_namespace.clone(),
        ],
    )
    .unwrap();
    let status = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "owner-key",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "owner scoped",
        ),
    )
    .await;
    assert_eq!(status.state, ManagementJobState::Succeeded);
    let job_id = status.id.clone();
    assert!(matches!(
        dispatcher.status_view(&other_admin, &job_id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        dispatcher.status_view(&evaluator, &job_id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        dispatcher.status_view(&other_namespace, &job_id).await,
        Err(Error::NotFound)
    ));
    let owned = dispatcher
        .status_view(&fixture.admin, &job_id)
        .await
        .unwrap();
    let result = serde_json::to_value(owned.result.as_ref().unwrap()).unwrap();
    let keys: BTreeSet<&str> = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "result",
            "drift_id",
            "previous_environment_id",
            "current_environment_id",
            "invalidated_scope_id"
        ])
    );
    let public = serde_json::to_string(&owned).unwrap();
    for forbidden in ["reason", "snapshot", "source", "path", "credential"] {
        assert!(!public.contains(forbidden), "{forbidden} leaked: {public}");
    }
    assert_eq!(
        serde_json::to_value(&owned.committed_side_effects.as_ref().unwrap()[0]).unwrap()["kind"],
        "environment_drift"
    );
}

/// Neither the request body nor a scope-index body may substitute its own
/// namespace, and a shape-invalid scope index is refused before the write.
#[tokio::test]
async fn drift_namespace_and_scope_index_are_not_spoofable() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    // A body namespace cannot replace the authenticated context.
    let mut spoof = drift_payload(
        "namespace-wire",
        fixture.previous_id(),
        fixture.current_id(),
        &fixture.scope_id,
        "spoof",
    );
    spoof["namespace"] = json!("other-tenant");
    assert!(matches!(
        dispatcher
            .submit(&fixture.admin, "monitoring.drift.record", spoof)
            .await,
        Err(Error::Invalid(_))
    ));
    // A stored scope index with a mismatched cycle/report length is refused.
    let healthy = raw_artifact(&fixture.store, &fixture.admin, &fixture.scope_id).await;
    let mut tampered: Value = serde_json::from_str(&healthy).unwrap();
    tampered["cycle_ids"] = json!([]);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &fixture.scope_id,
            fixture.admin.actor(),
            &tampered,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let shape = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "scope-shape",
            fixture.previous_id(),
            fixture.current_id(),
            &fixture.scope_id,
            "tampered scope",
        ),
    )
    .await;
    assert_eq!(shape.state, ManagementJobState::Failed, "{shape:?}");
    assert_eq!(shape.error_code.as_deref(), Some("conflict"));
    // Restore the healthy index, then store a different-namespace scope under
    // this namespace: its own namespace must be re-checked, not trusted.
    let mut session = fixture.store.session().await.unwrap();
    let healthy: Value = serde_json::from_str(&healthy).unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &fixture.scope_id,
            fixture.admin.actor(),
            &healthy,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let reference: ConsolidationScopeIndex =
        scope_index(&fixture.store, &fixture.admin, &fixture.scope_id).await;
    let mut foreign_scope = reference.scope.clone();
    foreign_scope.namespace = "other-tenant".into();
    let foreign_scope_id = format!(
        "consolidation-scope-{}",
        &fingerprint(&foreign_scope).unwrap()[..32]
    );
    let foreign_index = ConsolidationScopeIndex {
        id: foreign_scope_id.clone(),
        schema_version: reference.schema_version.clone(),
        scope: foreign_scope,
        cycle_ids: vec![],
        report_fact_ids: vec![],
        claim_ids: Default::default(),
        invalidated_by: None,
    };
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &foreign_scope_id,
            fixture.admin.actor(),
            &foreign_index,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let namespace = done(
        &dispatcher,
        &fixture.admin,
        drift_payload(
            "scope-namespace",
            fixture.previous_id(),
            fixture.current_id(),
            &foreign_scope_id,
            "foreign namespace",
        ),
    )
    .await;
    assert_eq!(namespace.state, ManagementJobState::Failed, "{namespace:?}");
    assert_eq!(namespace.error_code.as_deref(), Some("forbidden"));
    assert_eq!(
        scope_index(&fixture.store, &fixture.admin, &foreign_scope_id)
            .await
            .invalidated_by,
        None
    );
    assert_eq!(
        artifact_wire(&fixture.store, &fixture.admin)
            .await
            .into_iter()
            .filter(|(schema, _)| schema == "rsia.monitoring.environment_drift.v1")
            .count(),
        0,
        "neither refusal writes a drift fact"
    );
}

fn audit_facts(path: &std::path::Path, operation: &str, entity: &str) -> usize {
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", r#"
import json, sqlite3, sys
con = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)
rows = [json.loads(p) for (p,) in con.execute("SELECT payload FROM audit")]
print(sum(1 for row in rows if row.get('operation') == sys.argv[2] and row.get('entity') == sys.argv[3]))
con.close()
"#])
        .arg(path)
        .arg(operation)
        .arg(entity)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
