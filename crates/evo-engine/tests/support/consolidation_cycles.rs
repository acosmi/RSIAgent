//! AG-139a shared fixture: one fixture-scope monitoring environment, two
//! trusted sources, a root budget and hand-written development reports closed
//! as cycles, with or without a before/after contrast. It uses only API that
//! predates AG-139a, so the parent-tree probes compile against it too.
//!
//! Each test crate that includes it also includes `trusted_compiler_inputs`.
#![allow(dead_code)]
use crate::trusted_compiler_inputs::{self, StoreOptimizationJournal};
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SkillSnapshot, SurfaceCoverage,
    SurfaceItem, SystemSnapshotV3,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::monitoring::{
    ClaimOutcome, CompleteDevelopmentCycleRequest, DevelopmentCycleRecord, MonitoringCoordinator,
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

pub const CYCLE_SCHEMA: &str = "rsia.monitoring.development_cycle.v2";
pub const SCOPE_SCHEMA: &str = "rsia.monitoring.consolidation_scope.v1";
pub const CLAIM_SCHEMA: &str = "rsia.monitoring.consolidation_claim.v1";
pub const RUN_SCHEMA: &str = "rsia.monitoring.consolidation_run.v1";
pub const TRIGGER_SCHEMA: &str = "rsia.management.consolidation_trigger.v1";

pub fn d(value: &str) -> String {
    hash(value.as_bytes())
}

pub fn context(actor: &str, role: Role) -> Context {
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
            "../fixtures/ag168-actual-mcp-descriptors.json"
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

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: std::path::PathBuf,
    pub store: Store,
    pub admin: Context,
    pub host: Context,
    pub worker: Context,
    pub environment: RecordedEnvironment,
    pub manifest_digest: String,
    pub grader_digest: String,
    pub strategy_digest: String,
    pub parent_skill_digest: String,
    pub parent_bundle_digest: String,
    pub budget: RootBudgetRecord,
}

impl Fixture {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("consolidation-cycles.sqlite3");
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
        let environment = MonitoringCoordinator::record_environment_from_run(
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
        Self {
            dir,
            database,
            store,
            admin,
            host,
            worker,
            environment,
            manifest_digest,
            grader_digest,
            strategy_digest,
            parent_skill_digest,
            parent_bundle_digest: d("parent-bundle"),
            budget,
        }
    }

    pub fn request(&self, fact_id: &str) -> CompleteDevelopmentCycleRequest {
        CompleteDevelopmentCycleRequest {
            skill_id: "skill-a".into(),
            profile_id: "profile".into(),
            environment_id: self.environment.environment.id.clone(),
            development_manifest_digest: self.manifest_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            generation_strategy_digest: self.strategy_digest.clone(),
            parent_skill_digest: self.parent_skill_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            billing_scope: self.budget.billing_scope.clone(),
            root_budget_id: self.budget.root_budget_id.clone(),
            report_fact_id: fact_id.into(),
            source_ids: vec!["source-2".into(), "source-1".into()],
            revoke_watermark: 1,
        }
    }

    /// The `consolidation.trigger` wire of the same cycle request.
    pub fn payload(&self, key: &str, fact_id: &str) -> Value {
        let request = self.request(fact_id);
        json!({
            "schema_version": TRIGGER_SCHEMA,
            "request_key": key,
            "skill_id": request.skill_id,
            "profile_id": request.profile_id,
            "environment_id": request.environment_id,
            "development_manifest_digest": request.development_manifest_digest,
            "grader_digest": request.grader_digest,
            "generation_strategy_digest": request.generation_strategy_digest,
            "parent_skill_digest": request.parent_skill_digest,
            "parent_bundle_digest": request.parent_bundle_digest,
            "billing_scope": request.billing_scope,
            "root_budget_id": request.root_budget_id,
            "report_fact_id": request.report_fact_id,
            "source_ids": request.source_ids,
            "revoke_watermark": request.revoke_watermark,
        })
    }

    /// Stages the development fact of cycle `cycle`: one task, a score change
    /// when `contrast`, equal scores otherwise. Returns the fact id.
    pub async fn stage(&self, cycle: u32, contrast: bool) -> String {
        let parent_execution = format!("parent-execution-{cycle}");
        let candidate_execution = format!("candidate-execution-{cycle}");
        let runner_execution = format!("runner-execution-{cycle}");
        let grader_receipt = d(&format!("grader-receipt-{cycle}"));
        let report = DevelopmentRunReport {
            request_id: format!("development-request-{cycle}"),
            manifest_digest: self.manifest_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
            environment_digest: self.environment.environment.environment_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            results: vec![PairedTaskResult {
                task_id: "task-a".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: if contrast { 600_000 } else { 500_000 },
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
        let mut session = self.store.session().await.unwrap();
        for (logical_kind, id) in [
            ("execution", parent_execution.as_str()),
            ("execution", candidate_execution.as_str()),
            ("execution", runner_execution.as_str()),
            ("grader", grader_receipt.as_str()),
        ] {
            session
                .put(
                    &self.worker,
                    "artifact",
                    id,
                    self.worker.actor(),
                    &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":cycle,"fixture":true}),
                )
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let mut dependencies = vec![
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
        ];
        for source in ["source-1", "source-2"] {
            dependencies.push(StageDependency {
                kind: "run".into(),
                id: source.into(),
            });
        }
        dependencies.push(StageDependency {
            kind: "revoke_watermark".into(),
            id: "1".into(),
        });
        let fact = StageFact {
            schema_version: "rsia.optimization.stage_fact.v1".into(),
            artifact_id: "placeholder".into(),
            namespace: self.worker.namespace().into(),
            episode_id: format!("development-episode-{cycle}"),
            step: cycle,
            attempt: 1,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id: report.request_id.clone(),
            input_digest: d(&format!("development-input-{cycle}")),
            output_digest: None,
            dependencies,
            payload: serde_json::to_value(&report).unwrap(),
        }
        .seal()
        .unwrap();
        let id = fact.artifact_id.clone();
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), self.worker.actor())
            .unwrap()
            .commit(fact)
            .await
            .unwrap();
        id
    }

    /// Stages cycle `cycle` and closes it through the library entry, claiming
    /// nothing.
    pub async fn close(&self, cycle: u32, contrast: bool) -> DevelopmentCycleRecord {
        let fact = self.stage(cycle, contrast).await;
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            self.request(&fact),
        )
        .await
        .unwrap()
    }

    pub async fn claim(&self, scope_id: &str) -> Result<ClaimOutcome, Error> {
        MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, scope_id).await
    }

    pub async fn artifacts(&self, schema: &str) -> Vec<Value> {
        let mut session = self.store.session().await.unwrap();
        let all: Vec<Value> = session.list(&self.admin, "artifact").await.unwrap();
        session.commit().await.unwrap();
        all.into_iter()
            .filter(|value| value["schema_version"] == schema)
            .collect()
    }

    /// Every artifact and job body, for zero-write comparisons.
    pub async fn rows(&self) -> Vec<String> {
        let mut session = self.store.session().await.unwrap();
        let jobs: Vec<Value> = session.list(&self.admin, "job").await.unwrap();
        let artifacts: Vec<Value> = session.list(&self.admin, "artifact").await.unwrap();
        session.commit().await.unwrap();
        jobs.iter()
            .chain(&artifacts)
            .map(|value| value.to_string())
            .collect()
    }

    pub async fn value(&self, id: &str) -> Value {
        let mut session = self.store.session().await.unwrap();
        let value = session.need(&self.admin, "artifact", id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    /// The stored body bytes of an artifact.
    pub async fn raw(&self, id: &str) -> String {
        let mut session = self.store.session().await.unwrap();
        let row = session
            .object_row(&self.admin, "artifact", id)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        row.body
    }

    pub async fn scope_id(&self) -> String {
        let scopes = self.artifacts(SCOPE_SCHEMA).await;
        assert_eq!(scopes.len(), 1, "exactly one consolidation scope");
        scopes[0]["id"].as_str().unwrap().to_owned()
    }

    /// The claim id the scope index records for `generation`.
    pub async fn claim_id(&self, generation: u64) -> String {
        let index = self.value(&self.scope_id().await).await;
        index["claim_ids"][generation.to_string()]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// `(spent, reserved)` of the fixture root.
    pub async fn root(&self) -> (i64, i64) {
        let mut session = self.store.session().await.unwrap();
        let root = session
            .root_budget(&self.admin, &self.budget.billing_scope)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        (root.spent_micros, root.reserved_micros)
    }

    /// `(operation, entity)` of every audit record, in order.
    pub fn audits(&self) -> Vec<(String, String)> {
        let output = std::process::Command::new("python3")
            .args([
                "-B",
                "-c",
                r#"
import json, sqlite3, sys
con = sqlite3.connect('file:' + sys.argv[1] + '?mode=ro', uri=True)
rows = [json.loads(p) for (p,) in con.execute("SELECT payload FROM audit ORDER BY seq")]
print(json.dumps([[row.get('operation'), row.get('entity')] for row in rows]))
con.close()
"#,
            ])
            .arg(&self.database)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Runs `script` against the fixture database with Python's sqlite3.
    pub fn sqlite(&self, script: &str) {
        let output = std::process::Command::new("python3")
            .args(["-B", "-c", script])
            .arg(&self.database)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "script failed: {}\n{}",
            script,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
