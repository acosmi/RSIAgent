// Static pending draft: the old empty-result assertion is intentionally still pending actual parent proof.
//! AG081: existing public API fixtures for report-derived cycle identity and
//! actual parent/manifest binding before any consolidation port or persistent write.
//! Helper fixtures are adapted from monitoring_consolidation_v42 and
//! monitoring_trusted_cycle_v42; no existing tests are included or rerun here.
//! Host environment remains ProgramFixture. The registered pure-function control
//! proves typed development receipts only, never real-provider benefit or approval.

use async_trait::async_trait;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile, SkillSnapshot,
    SurfaceCoverage, SurfaceItem, SystemSnapshot,
};
use evo_core::evidence::{EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::ConsolidationClass;
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::broker::BudgetPortBinding;
use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
    registered_runner_digest, typed_receipt_closure,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::monitoring::{
    CONSOLIDATION_CLAIM_SCHEMA, ClaimOutcome, CompleteDevelopmentCycleRequest, ConsolidationClaim,
    ConsolidationClaimState, ConsolidationDevRunner, ConsolidationModelPort,
    ConsolidationRunOutcome, ConsolidationRunRecord, ConsolidationTrigger, CycleTriggerOutcome,
    DEVELOPMENT_CYCLE_SCHEMA, DevelopmentCycleRecord, EnvironmentEvidenceScope,
    MonitoringCoordinator, RecordEnvironmentRequest, RecordedEnvironment,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepRequest, PairedTaskResult,
    StageDependency, StageFact, StageFactKind, StoreOptimizationJournal,
};
use evo_engine::release_store::{PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, RootBudgetRecord,
    UsageCharge,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const TENANT: &str = "tenant";
const SCOPE: &str = "billing-scope";
const ROOT: &str = "root-budget";
const CONTROL_ID: &str = "dev-control";
const REDACTED: &str = "rsia.redacted.v1";
const GRANT_PLACEHOLDER: &str = "optgrant-fixture";
const STEP_PLACEHOLDER: &str = "optstage-step-fixture";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn context(actor: &str, role: Role) -> Context {
    Context::new(TENANT, actor, role).unwrap()
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
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("runner".into()),
            reason: "fixture projection".into(),
        }],
    }
}

fn system(model: &str) -> SystemSnapshot {
    SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: "profile".into(),
        host_id: "monitor-host".into(),
        host_version: "1.0.0".into(),
        model_id: model.into(),
        tools: vec!["tool-a".into()],
        mandatory_context_digest: d("mandatory"),
    }
}

/// A trusted Host development source. `source-1` is a diagnosed failure and
/// `source-2` a success, so the optimizer has a failure and a success batch.
fn authority(id: &str, body: &str) -> StoredTraceAuthority {
    let failing = id == "source-1";
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
            outcome: if failing {
                TraceOutcome::TaskFailure
            } else {
                TraceOutcome::Success
            },
            diagnosis: failing.then(|| SkillFailureDiagnosis {
                kind: SkillFailureKind::SkillDefect,
                skill_id: "skill-a".into(),
                bundle_digest: d("parent-bundle"),
                request_digest: d("task-request"),
                rule_id: Some("rule-a".into()),
                support: vec![EvidenceRef {
                    id: id.into(),
                    digest: hash(body.as_bytes()),
                }],
                counterexamples: vec![],
                reason: "fixture defect".into(),
            }),
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    }
}

fn tasks() -> Vec<DevelopmentTaskSpecV1> {
    vec![
        DevelopmentTaskSpecV1::registered(
            "task-a",
            "family-a",
            RegisteredTargetInputV1::ClampI64 {
                value: -2,
                min: -1,
                max: 1,
            },
        )
        .unwrap(),
        DevelopmentTaskSpecV1::registered(
            "task-b",
            "family-b",
            RegisteredTargetInputV1::ClampI64 {
                value: 5,
                min: 0,
                max: 3,
            },
        )
        .unwrap(),
    ]
}

/// The Admin-registered control of the registered runner, bound to the monitoring
/// fixture's billing root and trusted sources.
fn control(
    id: &str,
    billing_scope: &str,
    root_budget_id: &str,
    environment_digest: &str,
) -> DevelopmentControlV1 {
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = grader_spec();
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: id.into(),
        namespace: TENANT.into(),
        billing_scope: billing_scope.into(),
        root_budget_id: root_budget_id.into(),
        executor_actor: "dev-executor".into(),
        grader_actor: "dev-grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "dev-manifest".into(),
        manifest_digest: String::new(),
        tasks: tasks(),
        environment_digest: environment_digest.into(),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        source_ids: vec!["source-1".into(), "source-2".into()],
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

fn build_request(
    control: &DevelopmentControlV1,
    request_id: &str,
    episode_id: &str,
    step: u32,
) -> DevelopmentRunRequest {
    DevelopmentRunRequest {
        request_id: request_id.into(),
        namespace: TENANT.into(),
        purpose: Purpose::Development,
        episode_id: episode_id.into(),
        step,
        attempt: 1,
        manifest: control.manifest().unwrap(),
        parent_bundle_digest: d("parent-bundle"),
        candidate_bundle_digest: d(&format!("candidate-bundle-{request_id}")),
        environment_digest: control.environment_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        rules_digest: control.rules_digest.clone(),
        tools_digest: control.tools_digest.clone(),
        revoke_watermark: 1,
        idempotency_key: format!("{request_id}-idem"),
    }
}

/// What a journaled Development fact carries besides its own receipts: the trusted
/// sources, the watermark and the two journal artifacts (the model-excerpt grant
/// and the step record). `Fixture::new` stores placeholders for the latter two.
fn journal_extras() -> Vec<StageDependency> {
    vec![
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
        StageDependency {
            kind: "artifact".into(),
            id: GRANT_PLACEHOLDER.into(),
        },
        StageDependency {
            kind: "artifact".into(),
            id: STEP_PLACEHOLDER.into(),
        },
    ]
}

fn request_fact(request: &DevelopmentRunRequest) -> StageFact {
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: TENANT.into(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentRequestPrepared,
        request_id: request.request_id.clone(),
        input_digest: request.manifest.digest.clone(),
        output_digest: None,
        dependencies: journal_extras(),
        payload: serde_json::to_value(request).unwrap(),
    }
    .seal()
    .unwrap()
}

/// The observation as the E03 step journals it. `typed_closure` adds the typed
/// receipt edges a non-fixture report must declare; a hand-written fact has none.
fn observed_fact(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
    typed_closure: bool,
) -> StageFact {
    let mut dependencies: Vec<StageDependency> = report
        .results
        .iter()
        .flat_map(|result| {
            [
                StageDependency {
                    kind: "execution".into(),
                    id: result.parent_execution_id.clone(),
                },
                StageDependency {
                    kind: "execution".into(),
                    id: result.candidate_execution_id.clone(),
                },
                StageDependency {
                    kind: "grader".into(),
                    id: result.grader_receipt_digest.clone(),
                },
            ]
        })
        .collect();
    if typed_closure {
        dependencies.extend(typed_receipt_closure(report).unwrap());
    }
    dependencies.extend(journal_extras());
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: TENANT.into(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id: request.request_id.clone(),
        input_digest: request.manifest.digest.clone(),
        output_digest: None,
        dependencies,
        payload: serde_json::to_value(report).unwrap(),
    }
    .seal()
    .unwrap()
}
/// One paired task of a hand-written (fixture) development report.
#[derive(Clone)]
struct ResultSpec {
    task_id: &'static str,
    parent_score: u32,
    candidate_score: u32,
    parent_passed: bool,
    candidate_passed: bool,
}

/// Both pass, score 500k -> 600k: a contrast made of a score change only.
fn score_only_contrast() -> Vec<ResultSpec> {
    vec![ResultSpec {
        task_id: "task-a",
        parent_score: 500_000,
        candidate_score: 600_000,
        parent_passed: true,
        candidate_passed: true,
    }]
}

/// A real registered-runner cycle: staged request and observation facts and the
/// report behind them.
struct Staged {
    request: DevelopmentRunRequest,
    report: DevelopmentRunReport,
    request_fact_id: String,
    observed_fact_id: String,
}
struct Fixture {
    _dir: tempfile::TempDir,
    database: std::path::PathBuf,
    store: Store,
    admin: Context,
    host: Context,
    worker: Context,
    executor: Context,
    grader: Context,
    environment: RecordedEnvironment,
    authorities: Vec<StoredTraceAuthority>,
    control: DevelopmentControlV1,
    manifest: DevelopmentManifest,
    parent_skill: SkillSnapshot,
    manifest_digest: String,
    grader_digest: String,
    strategy_digest: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
    budget: RootBudgetRecord,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("evidence-binding.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let admin = context("admin", Role::Admin);
        let host = context("host", Role::Host);
        let worker = context("worker", Role::Worker);
        let executor = context("dev-executor", Role::Worker);
        let grader = context("dev-grader", Role::Evaluator);
        ReleaseStore::register_host_surface(
            &admin,
            &store,
            "surface",
            surface(),
            vec!["model".into()],
        )
        .await
        .unwrap();
        prepare_host_run(&host, &store, "host-run-1", "model-v1", "task-input").await;
        let authorities = vec![
            authority("source-1", "first development source"),
            authority("source-2", "second development source"),
        ];
        for source in &authorities {
            store_trace_authority(&store, &host, source).await.unwrap();
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

        // The registered control's manifest and grader digests freeze the monitoring
        // environment; the environment digest the Host run yields is then the one the
        // control registers.
        let mut control = control(CONTROL_ID, SCOPE, ROOT, &d("environment-placeholder"));
        let manifest = control.manifest().unwrap();
        let manifest_digest = manifest.digest.clone();
        let grader_digest = control.grader_digest.clone();
        let strategy_digest = fingerprint(&Strategy::default()).unwrap();
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
        assert_eq!(
            environment.environment.evidence_scope,
            EnvironmentEvidenceScope::ProgramFixture
        );
        control.environment_digest = environment.environment.environment_digest.clone();
        control.validate().unwrap();
        let budget = store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: ROOT.into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![TENANT.into()],
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
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        for id in [GRANT_PLACEHOLDER, STEP_PLACEHOLDER] {
            put_artifact(
                &store,
                &worker,
                id,
                &json!({"id": id, "schema_version": "rsia.journal.fixture.v1"}),
            )
            .await;
        }
        let parent_skill = SkillSnapshot {
            content: "parent rule".into(),
            applicability: "development only".into(),
            counterexample: "counterexample".into(),
            required_capabilities: vec![],
            dependencies: vec![],
        };
        Self {
            _dir: dir,
            database,
            store,
            admin,
            host,
            worker,
            executor,
            grader,
            environment,
            authorities,
            control,
            manifest,
            manifest_digest,
            grader_digest,
            strategy_digest,
            parent_skill_digest: skill_snapshot_digest(&parent_skill).unwrap(),
            parent_skill,
            parent_bundle_digest: d("parent-bundle"),
            budget,
        }
    }

    fn runner_for(&self, control_id: &str) -> RegisteredDevelopmentRunner {
        RegisteredDevelopmentRunner::with_clock(
            self.store.clone(),
            self.executor.clone(),
            self.grader.clone(),
            control_id,
            600,
            Arc::new(|| 100),
        )
        .unwrap()
    }

    fn runner(&self) -> RegisteredDevelopmentRunner {
        self.runner_for(CONTROL_ID)
    }

    fn journal(&self) -> StoreOptimizationJournal {
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker").unwrap()
    }

    fn cycle_request(&self, fact_id: String) -> CompleteDevelopmentCycleRequest {
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
            report_fact_id: fact_id,
            source_ids: vec!["source-2".into(), "source-1".into()],
            revoke_watermark: 1,
        }
    }

    /// Runs the real registered runner for cycle `n` and journals the request and
    /// observation facts the way the E03 step and its recovery journal would.
    async fn stage_trusted(&self, n: u32) -> Staged {
        let request = build_request(
            &self.control,
            &format!("dev-request-{n}"),
            &format!("cycle-episode-{n}"),
            n,
        );
        let report = self.runner().run(request.clone()).await.unwrap();
        assert_eq!(
            report.provenance,
            DevelopmentExecutionProvenance::RegisteredPureFunction
        );
        let journal = self.journal();
        let request_fact = request_fact(&request);
        let observed = observed_fact(&request, &report, true);
        journal.commit(request_fact.clone()).await.unwrap();
        journal.commit(observed.clone()).await.unwrap();
        Staged {
            request,
            report,
            request_fact_id: request_fact.artifact_id,
            observed_fact_id: observed.artifact_id,
        }
    }

    async fn close_and_trigger(&self, fact_id: &str) -> Result<CycleTriggerOutcome, Error> {
        MonitoringCoordinator::close_development_cycle_and_trigger(
            &self.worker,
            &self.store,
            self.cycle_request(fact_id.into()),
        )
        .await
    }

    async fn close(&self, fact_id: &str) -> Result<DevelopmentCycleRecord, Error> {
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            self.cycle_request(fact_id.into()),
        )
        .await
    }
    /// Stages a hand-written fixture report for `cycle` (fixture provenance, opaque
    /// receipts stored under their own ids) and closes it as a cycle.
    async fn close_fixture_cycle(
        &self,
        cycle: u32,
        results: &[ResultSpec],
    ) -> Result<DevelopmentCycleRecord, Error> {
        let fact = self.stage_fixture_report(cycle, results).await;
        self.close(&fact).await
    }

    /// Closes two fixture cycles with a contrast and claims the first generation:
    /// the only kind of claim that can be dispatched.
    async fn claimed(&self) -> ConsolidationClaim {
        for cycle in 1..=2 {
            self.close_fixture_cycle(cycle, &score_only_contrast())
                .await
                .unwrap();
        }
        let scope_id = self.scope_id().await;
        let ClaimOutcome::Claimed(claim) =
            MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, &scope_id)
                .await
                .unwrap()
        else {
            panic!("the first consolidation generation was not claimed")
        };
        claim
    }

    async fn scope_id(&self) -> String {
        let scopes = artifacts_with_schema(
            &self.store,
            &self.worker,
            "rsia.monitoring.consolidation_scope.v1",
        )
        .await;
        assert_eq!(scopes.len(), 1, "exactly one consolidation scope");
        scopes[0]["id"].as_str().unwrap().to_string()
    }

    /// Stages a DevelopmentObserved fact whose report pairs `results` for one
    /// cycle, with fixture receipts stored under the ids the fact depends on.
    async fn stage_fixture_report(&self, cycle: u32, results: &[ResultSpec]) -> String {
        let runner_execution = format!("runner-execution-{cycle}");
        let mut paired = Vec::new();
        let mut dependencies = Vec::new();
        let mut receipts = vec![("execution", runner_execution.clone())];
        for (index, spec) in results.iter().enumerate() {
            let parent_execution = format!("parent-execution-{cycle}-{index}");
            let candidate_execution = format!("candidate-execution-{cycle}-{index}");
            let grader_receipt = d(&format!("grader-receipt-{cycle}-{index}"));
            receipts.push(("execution", parent_execution.clone()));
            receipts.push(("execution", candidate_execution.clone()));
            receipts.push(("grader", grader_receipt.clone()));
            dependencies.push(StageDependency {
                kind: "execution".into(),
                id: parent_execution.clone(),
            });
            dependencies.push(StageDependency {
                kind: "execution".into(),
                id: candidate_execution.clone(),
            });
            dependencies.push(StageDependency {
                kind: "grader".into(),
                id: grader_receipt.clone(),
            });
            paired.push(PairedTaskResult {
                task_id: spec.task_id.into(),
                parent_score_micros: spec.parent_score,
                candidate_score_micros: spec.candidate_score,
                parent_passed: spec.parent_passed,
                candidate_passed: spec.candidate_passed,
                parent_execution_id: parent_execution,
                candidate_execution_id: candidate_execution,
                grader_receipt_digest: grader_receipt,
            });
        }
        let report = DevelopmentRunReport {
            request_id: format!("development-request-{cycle}"),
            manifest_digest: self.manifest_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
            environment_digest: self.environment.environment.environment_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            results: paired,
            execution_receipt_id: runner_execution,
            usage_record_ids: vec![],
            provenance: DevelopmentExecutionProvenance::Fixture,
        };
        let mut session = self.store.session().await.unwrap();
        for (logical_kind, id) in &receipts {
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
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: "placeholder".into(),
            namespace: TENANT.into(),
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
        self.journal().commit(fact).await.unwrap();
        id
    }
}

async fn prepare_host_run(host: &Context, store: &Store, run_id: &str, model: &str, task: &str) {
    let snapshot = ReleaseStore::prepare_run(
        host,
        store,
        PrepareRunRequest {
            run_id: run_id.into(),
            profile_id: "profile".into(),
            system_snapshot: system(model),
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d(task),
            evolution_enabled: true,
            capability_level: CapabilityLevel::Attached,
        },
    )
    .await
    .unwrap();
    ReleaseStore::record_applied_request(
        host,
        store,
        run_id,
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
}

/// Model fixture from `tests/optimization.rs::EditingFixtureModelPort`: one
/// bounded insertion per reflection batch. `tag` makes the inserted text, and
/// therefore the candidate bundle, differ between instances; the receipt names
/// the claim's root budget so the root-binding wrapper accepts it.
struct EditingModelPort {
    tag: &'static str,
    calls: AtomicUsize,
    bindings: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl EditingModelPort {
    fn new(tag: &'static str) -> Self {
        Self {
            tag,
            calls: AtomicUsize::new(0),
            bindings: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ModelPort for EditingModelPort {
    async fn dispatch(&self, request: ModelRequest) -> evo_core::Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let source = request.source_closure[0].clone();
        let parent: SkillSnapshot = serde_json::from_str(
            &request
                .input
                .iter()
                .find(|part| part.label == "parent-skill")
                .ok_or_else(|| Error::Invalid("fixture parent input missing".into()))?
                .content,
        )
        .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
        let strategy: Strategy = serde_json::from_str(
            &request
                .input
                .iter()
                .find(|part| part.label == "generation-strategy")
                .ok_or_else(|| Error::Invalid("fixture strategy input missing".into()))?
                .content,
        )
        .map_err(|_| Error::Invalid("fixture strategy input invalid".into()))?;
        // A consolidation step makes every model call under `Consolidate`, so the
        // reflect stage is read from the batch the call carries: its evidence part
        // is labelled with the batch id.
        let stage = match request.stage {
            ModelStage::Consolidate => request
                .input
                .iter()
                .find_map(|part| match part.label.as_str() {
                    "reflection-failure" => Some(ModelStage::ReflectFailure),
                    "reflection-success" => Some(ModelStage::ReflectSuccess),
                    _ => None,
                })
                .ok_or_else(|| Error::Invalid("unexpected fixture batch".into()))?,
            other => other,
        };
        let (id, batch_id, field, start) = match stage {
            ModelStage::ReflectFailure => (
                "fix-rule",
                "reflection-failure",
                SkillTextField::Content,
                parent.content.len(),
            ),
            ModelStage::ReflectSuccess => (
                "preserve-rule",
                "reflection-success",
                SkillTextField::Applicability,
                parent.applicability.len(),
            ),
            _ => return Err(Error::Invalid("unexpected fixture stage".into())),
        };
        let suggestion = EditSuggestion {
            id: id.into(),
            hypothesis: "bounded fixture hypothesis".into(),
            batch_ids: vec![batch_id.into()],
            support: vec![source.clone()],
            counterexamples: if stage == ModelStage::ReflectSuccess {
                vec![source.clone()]
            } else {
                vec![]
            },
            dependencies: vec![source],
            edit: SkillTextEdit {
                field,
                start,
                end: start,
                expected_text_digest: hash(b""),
                exact_anchor: None,
                operation: TextEditOperation::Insert {
                    text: format!(" [{} {}]", strategy.instruction, self.tag),
                },
            },
        };
        let output = serde_json::to_string(&vec![suggestion]).unwrap();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: format!("fixture-response-{id}"),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: format!("fixture-call-{id}"),
                dispatch_id: format!("fixture-dispatch-{id}"),
                root_budget_id: ROOT.into(),
                provider_request_id: format!("fixture-provider-{id}"),
                usage_record_id: format!("fixture-usage-{id}"),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

#[async_trait]
impl ConsolidationModelPort for EditingModelPort {
    async fn trusted_budget_binding(
        &self,
        _namespace: &str,
    ) -> evo_core::Result<BudgetPortBinding> {
        self.bindings.fetch_add(1, Ordering::SeqCst);
        Ok(BudgetPortBinding {
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        })
    }
}

/// Development runner that spends through the real root ledger under the claim's
/// dispatch group, as the root-binding wrapper requires, and reports either a
/// strict improvement or a regression over the frozen manifest.
struct LedgerDevRunner {
    store: Store,
    worker: Context,
    improves: bool,
    calls: AtomicUsize,
    bindings: AtomicUsize,
    requests: Mutex<Vec<DevelopmentRunRequest>>,
}

impl LedgerDevRunner {
    fn new(fixture: &Fixture, improves: bool) -> Self {
        Self {
            store: fixture.store.clone(),
            worker: fixture.worker.clone(),
            improves,
            calls: AtomicUsize::new(0),
            bindings: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl DevRunner for LedgerDevRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let call_id = format!("dev-call-{}", request.request_id);
        let input = d(&request.request_id);
        let lease = format!("lease-{}", request.request_id);
        let call = self
            .store
            .reserve_budget_call(
                &self.worker,
                &BudgetCallReservation {
                    billing_scope: SCOPE.into(),
                    call_id: call_id.clone(),
                    dispatch_group_id: request.episode_id.clone(),
                    stage: BudgetStage::DevelopmentExecution,
                    actual_input_digest: input.clone(),
                    request_artifact: None,
                    max_cost_micros: 10,
                    lease_token: lease.clone(),
                    lease_until: 10_000,
                    now: 100,
                },
            )
            .await?;
        let fence = BudgetCallFence {
            billing_scope: SCOPE.into(),
            call_id,
            actual_input_digest: input,
            lease_token: lease,
            lease_epoch: call.lease_epoch,
            now: 101,
        };
        let dispatched = self
            .store
            .begin_budget_dispatch(&self.worker, &fence)
            .await?;
        let usage = format!("usage-{}", request.request_id);
        self.store
            .finalize_budget_call(
                &self.worker,
                &fence,
                &UsageCharge {
                    amount_micros: 10,
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    provider_request_id: "fixture-runner".into(),
                    usage_record_id: usage.clone(),
                    output_digest: d("development-output"),
                },
            )
            .await?;
        // The root admits one open execution at a time: close this one so the
        // next claim's run can dispatch.
        self.store
            .close_budget_call_execution(
                &self.worker,
                SCOPE,
                &fence.call_id,
                dispatched.call.dispatch_id.as_deref().unwrap(),
                "fixture_execution_closed",
                102,
            )
            .await?;
        Ok(DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: request.manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results: request
                .manifest
                .tasks
                .iter()
                .map(|task| PairedTaskResult {
                    task_id: task.id.clone(),
                    parent_score_micros: 500_000,
                    candidate_score_micros: if self.improves { 700_000 } else { 400_000 },
                    parent_passed: true,
                    candidate_passed: true,
                    parent_execution_id: format!("parent-{}", task.id),
                    candidate_execution_id: format!("candidate-{}", task.id),
                    grader_receipt_digest: d(&format!("grade-{}", task.id)),
                })
                .collect(),
            execution_receipt_id: "development-runner-execution".into(),
            usage_record_ids: vec![usage],
            provenance: DevelopmentExecutionProvenance::Fixture,
        })
    }
}

#[async_trait]
impl ConsolidationDevRunner for LedgerDevRunner {
    async fn trusted_budget_binding(
        &self,
        _namespace: &str,
    ) -> evo_core::Result<BudgetPortBinding> {
        self.bindings.fetch_add(1, Ordering::SeqCst);
        Ok(BudgetPortBinding {
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        })
    }
}

/// Everything an `OptimizationStepRequest` borrows, frozen once per fixture.
struct StepMaterial {
    selection: SourceSelection,
    evidence: EvidenceSet,
    bindings: Vec<TrustedSourceBinding>,
    allowed: Vec<EvidenceRef>,
    edit_context: TrustedEditContext,
    edit_template: SkillEditBatch,
    profile: Profile,
    baseline: SkillSnapshot,
    strategy: Strategy,
    patch: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
}

impl StepMaterial {
    async fn new(fixture: &Fixture) -> Self {
        let selection = SourceSelection {
            roots: vec![],
            run_ids: vec!["source-1".into(), "source-2".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        };
        store_source_selection(&fixture.store, &fixture.host, &selection)
            .await
            .unwrap();
        let records = fixture
            .authorities
            .iter()
            .map(|authority| authority.record.clone())
            .collect::<Vec<_>>();
        let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
        let bindings = fixture
            .authorities
            .iter()
            .map(|authority| TrustedSourceBinding {
                source_id: authority.record.id.clone(),
                source_digest: authority.trace.source_digest.clone(),
                parent_family: authority.record.parent_family.clone(),
            })
            .collect::<Vec<_>>();
        let allowed = fixture
            .authorities
            .iter()
            .map(|authority| EvidenceRef {
                id: authority.record.id.clone(),
                digest: authority.trace.source_digest.clone(),
            })
            .collect::<Vec<_>>();
        let approved_parent = d("approved-parent");
        let safe_baseline = d("safe-baseline");
        let edit_context = TrustedEditContext::new(
            TENANT,
            "profile",
            "skill-a",
            "v1",
            approved_parent.clone(),
            safe_baseline.clone(),
            &fixture.parent_skill,
            allowed.clone(),
        )
        .unwrap();
        let edit_template = SkillEditBatch {
            schema_version: SKILL_EDIT_SCHEMA.into(),
            compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
            namespace: TENANT.into(),
            profile_id: "profile".into(),
            skill_id: "skill-a".into(),
            skill_version: "v1".into(),
            input_digest: skill_snapshot_digest(&fixture.parent_skill).unwrap(),
            approved_parent_digest: approved_parent.clone(),
            safe_baseline_digest: safe_baseline.clone(),
            evidence: EvidenceClosure {
                support: vec![allowed[0].clone()],
                counterexamples: vec![allowed[1].clone()],
                dependencies: allowed.clone(),
            },
            edits: vec![],
        };
        Self {
            selection,
            evidence,
            bindings,
            allowed,
            edit_context,
            edit_template,
            profile: Profile {
                id: "profile".into(),
                evolution_enabled: true,
                parent_digest: approved_parent,
                baseline_digest: safe_baseline,
            },
            baseline: fixture.parent_skill.clone(),
            strategy: Strategy::default(),
            patch: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
        }
    }

    /// The claim's optimization request. `with_traces` false leaves the step
    /// without an eligible reflection batch, which ends as `NoChange`.
    fn request<'a>(
        &'a self,
        fixture: &'a Fixture,
        claim_id: &str,
        sequence: u32,
        with_traces: bool,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.selection,
            source_bindings: &self.bindings,
            traces: if with_traces {
                fixture
                    .authorities
                    .iter()
                    .map(|authority| authority.trace.clone())
                    .collect()
            } else {
                vec![]
            },
            model_context: ModelRequestContext {
                request_id: format!("consolidation-request-{sequence}"),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                stage: ModelStage::Consolidate,
                episode_id: claim_id.into(),
                step: sequence,
                attempt: 1,
                parent_skill_digest: fixture.parent_skill_digest.clone(),
                bundle_digest: fixture.parent_bundle_digest.clone(),
                source_closure: self.allowed.clone(),
                model_digest: d("fixture-model"),
                tools_digest: d("tools"),
                rules_digest: d("rules"),
                sampling_digest: d("sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &fixture.parent_skill,
            edit_context: &self.edit_context,
            edit_batch_template: self.edit_template.clone(),
            protected_ranges: &[],
            bundle_context: BundleCompileContext {
                profile: &self.profile,
                baseline: &self.baseline,
                parent_strategy: &self.strategy,
                baseline_strategy: &self.strategy,
                improver_patch: &self.patch,
                caps: &self.caps,
                revoked: &self.revoked,
            },
            development_request: DevelopmentRunRequest {
                request_id: format!("consolidation-development-{sequence}"),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                episode_id: claim_id.into(),
                step: sequence,
                attempt: 1,
                manifest: fixture.manifest.clone(),
                parent_bundle_digest: fixture.parent_bundle_digest.clone(),
                candidate_bundle_digest: d(&format!("candidate-{sequence}")),
                environment_digest: fixture.environment.environment.environment_digest.clone(),
                grader_digest: fixture.grader_digest.clone(),
                rules_digest: d("rules"),
                tools_digest: d("tools"),
                revoke_watermark: 1,
                idempotency_key: format!("consolidation-idempotency-{sequence}"),
            },
            allow_rank_call: false,
        }
    }
}

async fn value(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let result = session.need(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    result
}

async fn artifacts_with_schema(store: &Store, ctx: &Context, schema: &str) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let artifacts: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    artifacts
        .into_iter()
        .filter(|body| body["schema_version"] == schema)
        .collect()
}

async fn put_artifact(store: &Store, ctx: &Context, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, ctx.actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// All persistent SQLite tables/rows, not just selected counters. This read-only
/// test oracle includes audit, object revisions, edges and the complete ledger.
/// The comparison is logical; normal SQLite coordination files are not business writes.
fn logical_snapshot(database: &Path) -> String {
    let script = r#"
import json, sqlite3, sys, urllib.parse
con=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1],safe='/')+'?mode=ro',uri=True)
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def encode(v):
    if isinstance(v,bytes): return {'blob_hex':v.hex()}
    return v
schema=con.execute('SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name').fetchall()
tables={}
for _,name,_,_ in schema:
    if con.execute('SELECT type FROM sqlite_schema WHERE name=?',(name,)).fetchone()[0]!='table': continue
    escaped=name.replace('"','""')
    rows=[[encode(x) for x in row] for row in con.execute('SELECT * FROM "'+escaped+'"')]
    rows.sort(key=lambda row:json.dumps(row,sort_keys=True,separators=(',',':')))
    tables[name]=rows
con.rollback()
con.close()
print(json.dumps({'schema':schema,'tables':tables},sort_keys=True,separators=(',',':')))
"#;
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(database)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "snapshot oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hash(&output.stdout)
}

fn port_counts(model: &EditingModelPort, runner: &LedgerDevRunner) -> (usize, usize, usize, usize) {
    (
        model.bindings.load(Ordering::SeqCst),
        model.calls.load(Ordering::SeqCst),
        runner.bindings.load(Ordering::SeqCst),
        runner.calls.load(Ordering::SeqCst),
    )
}

/// The actual parent generator exports new files only; it never overwrites logs
/// or a previous golden. Full-suite runs leave this opt-in variable unset.
fn export_golden(name: &str, material: &Value) {
    let bytes = serde_json::to_vec(material).unwrap();
    if let Some(directory) = std::env::var_os("AG081_GOLDEN_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.json"));
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        println!(
            "AG081_GOLDEN {name} bytes={} sha256={}",
            bytes.len(),
            hash(&bytes)
        );
    }
}

async fn check_cycle_mutation(score_axis: bool) {
    let fixture = Fixture::new().await;
    let first = fixture
        .close_fixture_cycle(1, &score_only_contrast())
        .await
        .unwrap();
    let second = fixture
        .close_fixture_cycle(2, &score_only_contrast())
        .await
        .unwrap();
    assert_eq!(first.scope_id, second.scope_id);
    let original_fact = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &second.report_fact_id,
    )
    .await;
    let mut tampered = second.clone();
    if score_axis {
        tampered.pairs[0].candidate_score_micros = 700_000;
    } else {
        tampered.pairs[0].parent_passed = false;
        tampered.pairs[0].class = ConsolidationClass::Improved;
    }
    // The class, contrast and valid ranges remain self-consistent. The original
    // report/fact/digest/receipts are not rewritten.
    assert!(tampered.has_contrast);
    put_artifact(
        &fixture.store,
        &fixture.worker,
        &second.id,
        &serde_json::to_value(&tampered).unwrap(),
    )
    .await;
    let before = logical_snapshot(&fixture.database);
    let repeated = fixture.close(&second.report_fact_id).await;
    let after_repeat = logical_snapshot(&fixture.database);
    let claim = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &second.scope_id,
    )
    .await;
    let after_claim = logical_snapshot(&fixture.database);
    println!(
        "AG081_CYCLE_OBSERVATION score_axis={score_axis} repeat_rejected={} claim_rejected={} repeat_unchanged={} claim_unchanged={}",
        repeated.is_err(),
        claim.is_err(),
        before == after_repeat,
        before == after_claim
    );
    assert_eq!(
        value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &second.report_fact_id
        )
        .await,
        original_fact
    );
    assert!(
        repeated.is_err(),
        "repeat close accepted report-inconsistent but self-consistent pairs"
    );
    assert!(
        claim.is_err(),
        "first claim accepted report-inconsistent pairs"
    );
    assert_eq!(before, after_repeat);
    assert_eq!(before, after_claim);
    assert!(
        artifacts_with_schema(&fixture.store, &fixture.worker, CONSOLIDATION_CLAIM_SCHEMA)
            .await
            .is_empty()
    );
    put_artifact(
        &fixture.store,
        &fixture.worker,
        &second.id,
        &serde_json::to_value(&second).unwrap(),
    )
    .await;
    assert_eq!(fixture.close(&second.report_fact_id).await.unwrap(), second);
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &second.scope_id
        )
        .await
        .unwrap(),
        ClaimOutcome::Claimed(_)
    ));
}

#[tokio::test]
async fn self_consistent_pass_fail_pairs_must_still_equal_the_original_report() {
    check_cycle_mutation(false).await;
}

#[tokio::test]
async fn self_consistent_score_pairs_must_still_equal_the_original_report() {
    check_cycle_mutation(true).await;
}

async fn check_execution_mutation(axis: &str) {
    let fixture = Fixture::new().await;
    let claim = fixture.claimed().await;
    let material = StepMaterial::new(&fixture).await;
    let model = EditingModelPort::new("first");
    let runner = LedgerDevRunner::new(&fixture, true);
    let journal = fixture.journal();
    let root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert!(!root.stopped && root.total_limit_micros > root.spent_micros + root.reserved_micros);
    let mut parent = fixture.parent_skill.clone();
    parent.content.push_str(" AG081_SECRET_PARENT");
    let mut request = material.request(&fixture, &claim.id, 1, true);
    match axis {
        "skill" => request.parent_skill = &parent,
        "manifest-input" => {
            request.development_request.manifest.tasks[0].input_digest = d("changed-input")
        }
        "manifest-order" => request.development_request.manifest.tasks.reverse(),
        _ => panic!("unknown mutation axis"),
    }
    let before = logical_snapshot(&fixture.database);
    let result = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        request,
    )
    .await;
    let after = logical_snapshot(&fixture.database);
    println!(
        "AG081_EXECUTION_OBSERVATION axis={axis} rejected={} ports={:?} unchanged={}",
        result.is_err(),
        port_counts(&model, &runner),
        before == after
    );
    assert!(
        result.is_err(),
        "bad actual content reached a persisted terminal result"
    );
    assert!(!format!("{:?}", result.unwrap_err()).contains("AG081_SECRET_PARENT"));
    assert_eq!(
        port_counts(&model, &runner),
        (0, 0, 0, 0),
        "binding/dispatch/run ports were called before rejecting bad actual content"
    );
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(runner.requests.lock().unwrap().is_empty());
    assert_eq!(before, after, "bad actual content wrote persistent state");
    let stored: ConsolidationClaim =
        serde_json::from_value(value(&fixture.store, &fixture.worker, "artifact", &claim.id).await)
            .unwrap();
    assert_eq!(stored.state, ConsolidationClaimState::Claimed);
    assert_eq!(stored.execution_input_digest, None);
    // The unchanged real request uses the same claim and functioning ports.
    let good = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        material.request(&fixture, &claim.id, 1, true),
    )
    .await
    .unwrap();
    assert_eq!(good.outcome, ConsolidationRunOutcome::Candidate);
    assert!(model.calls.load(Ordering::SeqCst) > 0);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn actual_parent_skill_is_rejected_before_any_port_or_write() {
    check_execution_mutation("skill").await;
}

#[tokio::test]
async fn actual_manifest_task_input_is_rejected_before_any_port_or_write() {
    check_execution_mutation("manifest-input").await;
}

#[tokio::test]
async fn actual_manifest_task_order_is_rejected_before_any_port_or_write() {
    check_execution_mutation("manifest-order").await;
}

#[tokio::test]
async fn valid_fixture_paths_export_actual_parent_golden() {
    let fixture = Fixture::new().await;
    let first = fixture
        .close_fixture_cycle(1, &score_only_contrast())
        .await
        .unwrap();
    let second = fixture
        .close_fixture_cycle(2, &score_only_contrast())
        .await
        .unwrap();
    assert_eq!(first.schema_version, DEVELOPMENT_CYCLE_SCHEMA);
    assert_eq!(fixture.close(&second.report_fact_id).await.unwrap(), second);
    let ClaimOutcome::Claimed(claim) = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &second.scope_id,
    )
    .await
    .unwrap() else {
        panic!("claim expected")
    };
    let ClaimOutcome::AlreadyClaimed(repeat) = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &second.scope_id,
    )
    .await
    .unwrap() else {
        panic!("repeated claim expected")
    };
    assert_eq!(
        serde_json::to_vec(&claim).unwrap(),
        serde_json::to_vec(&repeat).unwrap()
    );
    let material = StepMaterial::new(&fixture).await;
    let model = EditingModelPort::new("first");
    let runner = LedgerDevRunner::new(&fixture, true);
    let journal = fixture.journal();
    let run = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        material.request(&fixture, &claim.id, 1, true),
    )
    .await
    .unwrap();
    assert_eq!(run.outcome, ConsolidationRunOutcome::Candidate);
    let before_reentry = logical_snapshot(&fixture.database);
    let repeated_run = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        None,
        None,
        None,
        material.request(&fixture, &claim.id, 1, true),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&run).unwrap(),
        serde_json::to_vec(&repeated_run).unwrap()
    );
    assert_eq!(before_reentry, logical_snapshot(&fixture.database));
    let proposal = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        run.proposal_id.as_ref().unwrap(),
    )
    .await;
    let staged = MonitoringCoordinator::stage_consolidation_candidate(
        &fixture.worker,
        &fixture.store,
        run.proposal_id.as_ref().unwrap(),
        "candidate-golden",
    )
    .await
    .unwrap();
    let again = MonitoringCoordinator::stage_consolidation_candidate(
        &fixture.worker,
        &fixture.store,
        run.proposal_id.as_ref().unwrap(),
        "candidate-golden",
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&staged.candidate).unwrap(),
        serde_json::to_vec(&again.candidate).unwrap()
    );
    assert_eq!(staged.provenance, EnvironmentEvidenceScope::ProgramFixture);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    let requests = model.requests.lock().unwrap().clone();
    assert!(requests.iter().all(|r| r.stage == ModelStage::Consolidate));
    assert!(
        requests
            .iter()
            .all(|r| !r.input_digest.is_empty() && !r.cache_key_digest.is_empty())
    );
    export_golden(
        "fixture",
        &json!({"cycles":[first,second],"claim":claim,"run":run,"proposal":proposal,"model_requests":requests,"runner_requests":runner.requests.lock().unwrap().clone(),"candidate":staged.candidate,"provenance":staged.provenance}),
    );
}

#[tokio::test]
async fn real_registered_pure_function_paths_export_actual_parent_golden() {
    let fixture = Fixture::new().await;
    let first = fixture.stage_trusted(1).await;
    let second = fixture.stage_trusted(2).await;
    let one = fixture
        .close_and_trigger(&first.observed_fact_id)
        .await
        .unwrap();
    assert!(matches!(one.trigger, ConsolidationTrigger::NotEligible));
    let two = fixture
        .close_and_trigger(&second.observed_fact_id)
        .await
        .unwrap();
    let ConsolidationTrigger::Claimed {
        ref claim_id,
        state,
    } = two.trigger
    else {
        panic!("second trusted cycle did not trigger a claim")
    };
    assert_eq!(state, ConsolidationClaimState::NoContrast);
    assert_eq!(
        two.cycle.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    assert_eq!(
        fixture.close(&second.observed_fact_id).await.unwrap(),
        two.cycle
    );
    let claim = value(&fixture.store, &fixture.worker, "artifact", claim_id).await;
    let run =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, claim_id)
            .await
            .unwrap();
    assert_eq!(run.outcome, ConsolidationRunOutcome::NoChange);
    assert_eq!(
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, claim_id)
            .await
            .unwrap(),
        run
    );
    export_golden(
        "pure-function",
        &json!({"first_request":first.request,"first_report":first.report,"first_request_fact":first.request_fact_id,"cycles":[one.cycle,two.cycle],"second_request":second.request,"second_report":second.report,"second_request_fact":second.request_fact_id,"claim":claim,"run":run,"control":fixture.control}),
    );
}

#[test]
fn original_run_without_proposal_id_exports_actual_parent_golden() {
    let original = json!({
        "id":"consolidation-run-old", "schema_version":"rsia.monitoring.consolidation_run.v1",
        "claim_id":"claim-old", "input_digest":null, "outcome":"no_change",
        "reason":"no_change", "budget_call_ids":[]
    });
    let decoded: ConsolidationRunRecord = serde_json::from_value(original.clone()).unwrap();
    assert!(decoded.proposal_id.is_none());
    export_golden("old-run", &json!({"original":original,"decoded":decoded}));
}

// Static post-baseline draft. Append only after the actual old API baseline/golden closes.

async fn paired_fixture() -> (Fixture, DevelopmentCycleRecord) {
    let fixture = Fixture::new().await;
    fixture
        .close_fixture_cycle(1, &score_only_contrast())
        .await
        .unwrap();
    let second = fixture
        .close_fixture_cycle(2, &score_only_contrast())
        .await
        .unwrap();
    (fixture, second)
}

async fn refuse_first_claim_without_write(
    fixture: &Fixture,
    cycle: &DevelopmentCycleRecord,
) -> Error {
    let before = logical_snapshot(&fixture.database);
    let result = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &cycle.scope_id,
    )
    .await;
    let after = logical_snapshot(&fixture.database);
    assert!(
        result.is_err(),
        "invalid report-derived binding was consumed"
    );
    assert_eq!(
        before, after,
        "refusal changed persistent tables/revisions/audit/fees"
    );
    assert!(
        artifacts_with_schema(&fixture.store, &fixture.worker, CONSOLIDATION_CLAIM_SCHEMA)
            .await
            .is_empty()
    );
    let error = result.unwrap_err();
    assert!(!format!("{error:?}").contains("AG081_SECRET_REPORT"));
    error
}

#[tokio::test]
async fn report_derived_projections_cannot_be_replaced_by_cycle_declarations() {
    for axis in [
        "request-id",
        "bundle",
        "receipt",
        "usage",
        "provenance",
        "report-digest",
    ] {
        let (fixture, second) = paired_fixture().await;
        let mut altered = second.clone();
        match axis {
            "request-id" => altered.request_id = "other-request".into(),
            "bundle" => altered.candidate_bundle_digest = d("other-candidate"),
            "receipt" => altered.execution_receipt_id = "other-execution".into(),
            "usage" => altered.usage_record_ids = vec!["other-usage".into()],
            "provenance" => {
                altered.provenance = DevelopmentExecutionProvenance::RegisteredPureFunction
            }
            "report-digest" => altered.report_digest = d("well-formed-but-not-the-report"),
            _ => unreachable!(),
        }
        put_artifact(
            &fixture.store,
            &fixture.worker,
            &second.id,
            &serde_json::to_value(altered).unwrap(),
        )
        .await;
        assert!(
            matches!(
                refuse_first_claim_without_write(&fixture, &second).await,
                Error::Conflict(_)
            ),
            "axis={axis}"
        );
        println!("AG081_PROJECTION_REFUSED {axis}");
    }
}

#[tokio::test]
async fn cycle_identity_is_bound_to_both_the_lookup_key_and_report_scope() {
    for axis in ["stored-id", "scope-id", "report-fact-id"] {
        let (fixture, second) = paired_fixture().await;
        let mut altered = second.clone();
        match axis {
            "stored-id" => altered.id = "different-cycle-id".into(),
            "scope-id" => altered.scope_id = "different-scope-id".into(),
            "report-fact-id" => altered.report_fact_id = "different-report-fact".into(),
            _ => unreachable!(),
        }
        if axis == "stored-id" {
            // Existing storage negative control: its body/key CHECK rejects this
            // write before any AG081 consumer identity gate can be reached.
            let original_cycle_bytes = serde_json::to_vec(
                &value(&fixture.store, &fixture.worker, "artifact", &second.id).await,
            )
            .unwrap();
            let before = logical_snapshot(&fixture.database);
            let mut session = fixture.store.session().await.unwrap();
            let result = session
                .put(
                    &fixture.worker,
                    "artifact",
                    &second.id,
                    fixture.worker.actor(),
                    &serde_json::to_value(&altered).unwrap(),
                )
                .await;
            // Session owns a transaction. Drop rolls it back; this subsequent
            // public read/commit reacquires the Store's single connection before
            // the SQLite oracle runs, so the rejected session has fully closed.
            drop(session);
            let restored_cycle_bytes = serde_json::to_vec(
                &value(&fixture.store, &fixture.worker, "artifact", &second.id).await,
            )
            .unwrap();
            let after = logical_snapshot(&fixture.database);
            println!(
                "AG081_EXISTING_STORAGE_ID_MISMATCH error={result:?} unchanged={} cycle_bytes_unchanged={}",
                before == after,
                original_cycle_bytes == restored_cycle_bytes
            );
            assert!(
                matches!(&result, Err(Error::Internal)),
                "body/key mismatch must hit the existing storage CHECK: {result:?}"
            );
            assert_eq!(
                before, after,
                "rejected storage write changed persistent state"
            );
            assert_eq!(original_cycle_bytes, restored_cycle_bytes);
            continue;
        }
        put_artifact(
            &fixture.store,
            &fixture.worker,
            &second.id,
            &serde_json::to_value(altered).unwrap(),
        )
        .await;
        assert!(
            matches!(
                refuse_first_claim_without_write(&fixture, &second).await,
                Error::Conflict(_)
            ),
            "axis={axis}"
        );
        println!("AG081_CYCLE_IDENTITY_REFUSED {axis}");
    }
}

#[tokio::test]
async fn original_report_fact_must_remain_present_typed_and_key_bound() {
    for axis in [
        "changed-report",
        "missing-report",
        "kind",
        "stage",
        "namespace",
        "artifact-id",
        "payload",
        "schema",
    ] {
        let (fixture, second) = paired_fixture().await;
        let original = value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &second.report_fact_id,
        )
        .await;
        let mut fact: StageFact = serde_json::from_value(original).unwrap();
        if axis == "missing-report" {
            let mut session = fixture.store.session().await.unwrap();
            session
                .delete(&fixture.admin, "artifact", &second.report_fact_id)
                .await
                .unwrap();
            session.commit().await.unwrap();
        } else {
            match axis {
                "changed-report" => {
                    fact.payload["results"][0]["candidate_score_micros"] = json!(700_000)
                }
                "kind" => fact.kind = StageFactKind::DevelopmentRequestPrepared,
                "stage" => fact.stage = OptimizationJournalStage::ReflectSuccess,
                "namespace" => fact.namespace = "foreign-namespace".into(),
                "artifact-id" => fact.episode_id = "another-canonical-fact".into(),
                "payload" => fact.payload = json!({"AG081_SECRET_REPORT":"not-a-report"}),
                "schema" => fact.schema_version = "rsia.unknown-stage-fact.v1".into(),
                _ => unreachable!(),
            }
            // A canonical, valid StageFact whenever that axis permits it. Wrong
            // body/key relations use ordinary Session::put, never raw SQLite damage.
            if axis != "schema" {
                fact = fact.seal().unwrap();
                fact.validate().unwrap();
            }
            put_artifact(
                &fixture.store,
                &fixture.worker,
                &second.report_fact_id,
                &serde_json::to_value(fact).unwrap(),
            )
            .await;
        }
        let error = refuse_first_claim_without_write(&fixture, &second).await;
        match axis {
            "missing-report" => assert!(matches!(error, Error::NotFound)),
            "changed-report" | "artifact-id" => assert!(matches!(error, Error::Conflict(_))),
            _ => assert!(matches!(error, Error::Invalid(_))),
        }
        println!("AG081_REPORT_FACT_REFUSED {axis}");
    }
}

#[tokio::test]
async fn original_cycle_schema_and_self_consistency_errors_keep_priority() {
    for axis in [
        "legacy",
        "redacted",
        "unsupported",
        "class",
        "contrast",
        "score-range",
    ] {
        let (fixture, second) = paired_fixture().await;
        let mut altered = serde_json::to_value(&second).unwrap();
        let expected = match axis {
            "legacy" => {
                altered["schema_version"] = json!("rsia.monitoring.development_cycle.v1");
                (false, "pre-v2 development cycle records are not supported")
            }
            "redacted" => {
                altered = json!({"schema_version":REDACTED});
                (
                    true,
                    "development cycle was redacted after source revocation",
                )
            }
            "unsupported" => {
                altered["schema_version"] = json!("rsia.unsupported-cycle.v9");
                (false, "unsupported development cycle schema")
            }
            "class" => {
                altered["pairs"][0]["class"] = json!("improved");
                (
                    true,
                    "stored pair class differs from its pass/fail outcomes",
                )
            }
            "contrast" => {
                altered["has_contrast"] = json!(false);
                (true, "stored cycle contrast differs from its pairs")
            }
            "score-range" => {
                altered["pairs"][0]["candidate_score_micros"] = json!(1_000_001);
                (false, "development score micros must be within 0..=1000000")
            }
            _ => unreachable!(),
        };
        put_artifact(&fixture.store, &fixture.worker, &second.id, &altered).await;
        let mut session = fixture.store.session().await.unwrap();
        session
            .delete(&fixture.admin, "artifact", &second.report_fact_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        match refuse_first_claim_without_write(&fixture, &second).await {
            Error::Conflict(message) if expected.0 => assert_eq!(message, expected.1),
            Error::Invalid(message) if !expected.0 => assert_eq!(message, expected.1),
            error => panic!("priority changed for {axis}: {error:?}"),
        }
        println!("AG081_ORIGINAL_CYCLE_PRIORITY {axis}");
    }
}

#[tokio::test]
async fn real_receipt_cycles_preserve_report_pair_order_and_canonical_usage() {
    for axis in ["pair-order", "usage-order"] {
        let fixture = Fixture::new().await;
        let first = fixture.stage_trusted(1).await;
        let second = fixture.stage_trusted(2).await;
        fixture.close(&first.observed_fact_id).await.unwrap();
        let cycle = fixture.close(&second.observed_fact_id).await.unwrap();
        assert_eq!(
            cycle.provenance,
            DevelopmentExecutionProvenance::RegisteredPureFunction
        );
        let mut altered = cycle.clone();
        match axis {
            "pair-order" => {
                assert_eq!(altered.pairs.len(), 2);
                altered.pairs.swap(0, 1);
            }
            "usage-order" => {
                assert!(altered.usage_record_ids.len() >= 2);
                assert!(
                    altered
                        .usage_record_ids
                        .windows(2)
                        .all(|ids| ids[0] < ids[1])
                );
                altered.usage_record_ids.reverse();
            }
            _ => unreachable!(),
        }
        put_artifact(
            &fixture.store,
            &fixture.worker,
            &cycle.id,
            &serde_json::to_value(&altered).unwrap(),
        )
        .await;
        assert!(matches!(
            refuse_first_claim_without_write(&fixture, &cycle).await,
            Error::Conflict(_)
        ));
        put_artifact(
            &fixture.store,
            &fixture.worker,
            &cycle.id,
            &serde_json::to_value(&cycle).unwrap(),
        )
        .await;
        assert_eq!(
            fixture.close(&second.observed_fact_id).await.unwrap(),
            cycle
        );
        let ClaimOutcome::Claimed(claim) = MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &cycle.scope_id,
        )
        .await
        .unwrap() else {
            panic!("restored real report was not claimable")
        };
        assert_eq!(claim.state, ConsolidationClaimState::NoContrast);
        println!("AG081_REAL_REPORT_ORDER_REFUSED_AND_RESTORED {axis}");
    }
}

#[tokio::test]
async fn original_execution_state_stage_and_scope_errors_keep_priority() {
    for axis in ["claim-state", "stage", "parent-label", "manifest-label"] {
        let fixture = Fixture::new().await;
        let mut claim = fixture.claimed().await;
        let material = StepMaterial::new(&fixture).await;
        let model = EditingModelPort::new("first");
        let runner = LedgerDevRunner::new(&fixture, true);
        let journal = fixture.journal();
        let mut parent = fixture.parent_skill.clone();
        parent.content.push_str(" AG081_SECRET_PARENT");
        let mut request = material.request(&fixture, &claim.id, 1, true);
        request.parent_skill = &parent;
        match axis {
            "claim-state" => {
                claim.state = ConsolidationClaimState::CompletedRejected;
                put_artifact(
                    &fixture.store,
                    &fixture.worker,
                    &claim.id,
                    &serde_json::to_value(&claim).unwrap(),
                )
                .await;
            }
            "stage" => request.model_context.stage = ModelStage::ReflectFailure,
            "parent-label" => request.model_context.parent_skill_digest = d("wrong-parent-label"),
            "manifest-label" => {
                request.development_request.manifest.digest = d("wrong-manifest-label")
            }
            _ => unreachable!(),
        }
        let before = logical_snapshot(&fixture.database);
        let result = MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &claim.id,
            Some(&model),
            Some(&runner),
            Some(&journal),
            request,
        )
        .await;
        match (axis, result.unwrap_err()) {
            ("claim-state", Error::Conflict(message)) => {
                assert_eq!(message, "claim is not dispatchable")
            }
            ("stage", Error::Forbidden) => {}
            (_, Error::Conflict(message)) => {
                assert_eq!(message, "optimization request differs from persisted claim")
            }
            (_, error) => panic!("execution priority changed: {error:?}"),
        }
        assert_eq!(port_counts(&model, &runner), (0, 0, 0, 0));
        assert_eq!(before, logical_snapshot(&fixture.database));
        println!("AG081_ORIGINAL_EXECUTION_PRIORITY {axis}");
    }
}

#[tokio::test]
async fn actual_snapshot_other_text_fields_are_also_bound_before_ports() {
    for axis in ["applicability", "counterexample"] {
        let fixture = Fixture::new().await;
        let claim = fixture.claimed().await;
        let material = StepMaterial::new(&fixture).await;
        let model = EditingModelPort::new("first");
        let runner = LedgerDevRunner::new(&fixture, true);
        let journal = fixture.journal();
        let mut parent = fixture.parent_skill.clone();
        match axis {
            "applicability" => parent.applicability.push_str(" AG081_SECRET_PARENT"),
            "counterexample" => parent.counterexample.push_str(" AG081_SECRET_PARENT"),
            _ => unreachable!(),
        }
        let mut request = material.request(&fixture, &claim.id, 1, true);
        request.parent_skill = &parent;
        let before = logical_snapshot(&fixture.database);
        let error = MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &claim.id,
            Some(&model),
            Some(&runner),
            Some(&journal),
            request,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, Error::Conflict(ref message) if message == "actual parent skill differs from persisted claim")
        );
        assert!(!format!("{error:?}").contains("AG081_SECRET_PARENT"));
        assert_eq!(port_counts(&model, &runner), (0, 0, 0, 0));
        assert_eq!(before, logical_snapshot(&fixture.database));
        println!("AG081_ACTUAL_SKILL_FIELD_REFUSED {axis}");
    }
}

#[tokio::test]
async fn missing_ports_are_only_an_auxiliary_control_for_actual_content_binding() {
    for axis in ["skill", "manifest"] {
        let fixture = Fixture::new().await;
        let claim = fixture.claimed().await;
        let material = StepMaterial::new(&fixture).await;
        let mut parent = fixture.parent_skill.clone();
        parent.content.push_str(" AG081_SECRET_PARENT");
        let mut request = material.request(&fixture, &claim.id, 1, true);
        if axis == "skill" {
            request.parent_skill = &parent;
        } else {
            request.development_request.manifest.tasks[0].input_digest = d("different-input");
        }
        let before = logical_snapshot(&fixture.database);
        let error = MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &claim.id,
            None,
            None,
            None,
            request,
        )
        .await
        .unwrap_err();
        match error {
            Error::Conflict(message) if axis == "skill" => {
                assert_eq!(message, "actual parent skill differs from persisted claim")
            }
            Error::Conflict(message) => assert_eq!(message, "development manifest digest mismatch"),
            error => panic!("missing-port error won over actual binding: {error:?}"),
        }
        assert_eq!(before, logical_snapshot(&fixture.database));
    }
}

#[tokio::test]
async fn missing_results_and_environment_mismatch_remain_explicit_rejections() {
    let fixture = Fixture::new().await;
    let empty_fact = fixture.stage_fixture_report(1, &[]).await;
    let before = logical_snapshot(&fixture.database);
    assert!(
        matches!(fixture.close(&empty_fact).await, Err(Error::Invalid(ref message)) if message == "development report lacks execution or grader receipts")
    );
    assert_eq!(before, logical_snapshot(&fixture.database));
    let fact_id = fixture
        .stage_fixture_report(2, &score_only_contrast())
        .await;
    let mut fact: StageFact =
        serde_json::from_value(value(&fixture.store, &fixture.worker, "artifact", &fact_id).await)
            .unwrap();
    fact.payload["environment_digest"] = json!(d("drifted-environment"));
    fact = fact.seal().unwrap();
    put_artifact(
        &fixture.store,
        &fixture.worker,
        &fact_id,
        &serde_json::to_value(fact).unwrap(),
    )
    .await;
    let before = logical_snapshot(&fixture.database);
    assert!(
        matches!(fixture.close(&fact_id).await, Err(Error::Conflict(ref message)) if message == "development report differs from cycle scope")
    );
    assert_eq!(before, logical_snapshot(&fixture.database));
}

#[tokio::test]
async fn completed_runs_keep_the_original_historical_read_behavior() {
    let fixture = Fixture::new().await;
    let claim = fixture.claimed().await;
    let material = StepMaterial::new(&fixture).await;
    let model = EditingModelPort::new("first");
    let runner = LedgerDevRunner::new(&fixture, true);
    let journal = fixture.journal();
    let request = || material.request(&fixture, &claim.id, 1, true);
    let original = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        request(),
    )
    .await
    .unwrap();
    assert_eq!(original.outcome, ConsolidationRunOutcome::Candidate);
    let cycle: DevelopmentCycleRecord = serde_json::from_value(
        value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &claim.cycle_ids[1],
        )
        .await,
    )
    .unwrap();
    let mut session = fixture.store.session().await.unwrap();
    session
        .delete(&fixture.admin, "artifact", &cycle.report_fact_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = logical_snapshot(&fixture.database);
    let repeated = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        None,
        None,
        None,
        request(),
    )
    .await
    .unwrap();
    assert_eq!(repeated, original);
    let mut parent = fixture.parent_skill.clone();
    parent.content.push_str(" AG081_SECRET_PARENT");
    let mut changed = request();
    changed.parent_skill = &parent;
    assert!(
        matches!(MonitoringCoordinator::run_consolidation(&fixture.worker, &fixture.store, &claim.id, None, None, None, changed).await, Err(Error::Conflict(ref message)) if message == "completed consolidation input differs")
    );
    assert_eq!(before, logical_snapshot(&fixture.database));
}
