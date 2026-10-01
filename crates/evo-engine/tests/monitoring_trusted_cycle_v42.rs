//! E13 increment (plan v4.2, AG-028): the consolidation trigger on cycle close,
//! cycles closed on E03 evidence (the observation gate) instead of a report's own
//! provenance, the guard against a consolidation's development run becoming a new
//! cycle, and the registered runner as the consolidation's development runner.
//!
//! The fixtures are copied from `tests/development_receipts.rs` (the registered
//! control and runner, the request/observation fact pair) and from
//! `tests/monitoring_consolidation_v42.rs` (environment, hand-written fixture
//! cycles, the optimization step material), because test crates cannot import one
//! another.
//!
//! Environment type of every test here: the monitoring environment is recorded
//! from a Host run that applied no trusted execution receipt, so its evidence scope
//! is `ProgramFixture` (a `TrustedExecution` environment needs an Active release,
//! which needs a formal approval). The label a proposal gets from the cycles and the
//! environment (`TrustedExecution` only when the environment is `TrustedExecution`
//! and no cycle is a fixture) is therefore `ProgramFixture` here; what these tests
//! pin is the cycle-level input of that rule, `DevelopmentCycleRecord::provenance`
//! (the derivation matrix itself is unit-tested in `monitoring.rs`).

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
    CONTROL_KIND, DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    EXECUTION_RECEIPT_KIND, RUN_RECEIPT_KIND, RegisteredDevelopmentRunner, RegisteredTargetInputV1,
    register_development_control, registered_runner_digest, storage_id, typed_receipt_closure,
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
    ConsolidationRunOutcome, ConsolidationScopeIndex, ConsolidationTrigger, CycleTriggerOutcome,
    DEVELOPMENT_CYCLE_SCHEMA, DevelopmentCycleRecord, EnvironmentEvidenceScope,
    MonitoringCoordinator, RecordEnvironmentRequest, RecordedEnvironment,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepRequest, PairedTaskResult,
    StageDependency, StageFact, StageFactKind, StoreOptimizationJournal,
    verify_development_observation,
};
use evo_engine::release_store::{PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{RootBudgetAuthorization, RootBudgetRecord};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const TENANT: &str = "tenant";
const SCOPE: &str = "billing-scope";
const ROOT: &str = "root-budget";
const CONTROL_ID: &str = "dev-control";
const REDACTED: &str = "rsia.redacted.v1";
const CLAIM_PREFIX: &str = "consolidation-claim-";
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
        let store = Store::open(&dir.path().join("trusted-cycle.sqlite3"))
            .await
            .unwrap();
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

    async fn scope_index(&self) -> ConsolidationScopeIndex {
        let scope_id = self.scope_id().await;
        let mut session = self.store.session().await.unwrap();
        let index = session
            .need(&self.worker, "artifact", &scope_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        index
    }

    async fn claim(&self, claim_id: &str) -> ConsolidationClaim {
        let mut session = self.store.session().await.unwrap();
        let claim = session
            .need(&self.worker, "artifact", claim_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        claim
    }

    async fn cycles(&self) -> Vec<Value> {
        artifacts_with_schema(&self.store, &self.worker, DEVELOPMENT_CYCLE_SCHEMA).await
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

    /// A hand-written observation that declares `provenance` (never a fixture), sits
    /// in the episode of the honest run `honest` and names that run's real usage
    /// records, so that nothing but the E03 gate can tell it from a real one. Its
    /// receipts are opaque objects under their own ids (the old fixture style), it
    /// claims a clean improvement, and it optionally carries a request half.
    async fn stage_forged(
        &self,
        honest: &Staged,
        provenance: DevelopmentExecutionProvenance,
        with_request_half: bool,
    ) -> String {
        let mut request = honest.request.clone();
        request.request_id = format!(
            "forged-{}-{}",
            format!("{provenance:?}").to_lowercase(),
            if with_request_half {
                "paired"
            } else {
                "lonely"
            }
        );
        request.idempotency_key = format!("{}-idem", request.request_id);
        let mut receipts = vec![format!("{}-run-receipt", request.request_id)];
        let results: Vec<PairedTaskResult> = request
            .manifest
            .tasks
            .iter()
            .map(|task| {
                let parent = format!("{}-parent-{}", request.request_id, task.id);
                let candidate = format!("{}-candidate-{}", request.request_id, task.id);
                receipts.push(parent.clone());
                receipts.push(candidate.clone());
                PairedTaskResult {
                    task_id: task.id.clone(),
                    parent_score_micros: 0,
                    candidate_score_micros: 1_000_000,
                    parent_passed: false,
                    candidate_passed: true,
                    parent_execution_id: parent,
                    candidate_execution_id: candidate,
                    grader_receipt_digest: d(&format!("{}-grade-{}", request.request_id, task.id)),
                }
            })
            .collect();
        let report = DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: request.manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results,
            execution_receipt_id: receipts[0].clone(),
            usage_record_ids: honest.report.usage_record_ids.clone(),
            provenance,
        };
        for id in &receipts {
            put_artifact(
                &self.store,
                &self.worker,
                id,
                &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","fixture":true}),
            )
            .await;
        }
        let journal = self.journal();
        if with_request_half {
            journal.commit(request_fact(&request)).await.unwrap();
        }
        let observed = observed_fact(&request, &report, false);
        journal.commit(observed.clone()).await.unwrap();
        observed.artifact_id
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

// ---------------------------------------------------------------------------
// Consolidation step material (copied from tests/monitoring_consolidation_v42.rs)
// ---------------------------------------------------------------------------

/// Model fixture: one bounded insertion per reflection batch. The receipt names the
/// claim's root budget so the root-binding wrapper accepts it.
struct EditingModelPort {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelPort for EditingModelPort {
    async fn dispatch(&self, request: ModelRequest) -> evo_core::Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
        let (id, batch_id, field, start) = match request.stage {
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
            counterexamples: if request.stage == ModelStage::ReflectSuccess {
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
                    text: format!(" [{} trusted-cycle]", strategy.instruction),
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

    /// The claim's optimization request. Its development request is the registered
    /// control's manifest, environment, grader, rules and tools, so the registered
    /// runner accepts it.
    fn request<'a>(
        &'a self,
        fixture: &'a Fixture,
        claim_id: &str,
        sequence: u32,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.selection,
            source_bindings: &self.bindings,
            traces: fixture
                .authorities
                .iter()
                .map(|authority| authority.trace.clone())
                .collect(),
            model_context: ModelRequestContext {
                request_id: format!("consolidation-request-{sequence}"),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                stage: ModelStage::Merge,
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
                environment_digest: fixture.control.environment_digest.clone(),
                grader_digest: fixture.control.grader_digest.clone(),
                rules_digest: fixture.control.rules_digest.clone(),
                tools_digest: fixture.control.tools_digest.clone(),
                revoke_watermark: 1,
                idempotency_key: format!("consolidation-idempotency-{sequence}"),
            },
            allow_rank_call: false,
        }
    }
}

/// A claimed fixture run to its terminal record by the real registered runner.
struct Consolidated {
    fixture: Fixture,
    claim: ConsolidationClaim,
    outcome: ConsolidationRunOutcome,
    /// The Development stage the consolidation step journaled under its claim id.
    request_fact_id: String,
    observed_fact_id: String,
}

impl Consolidated {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let claim = fixture.claimed().await;
        let material = StepMaterial::new(&fixture).await;
        let model = EditingModelPort {
            calls: AtomicUsize::new(0),
        };
        let runner = fixture.runner();
        let journal = fixture.journal();
        let record = MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &claim.id,
            Some(&model),
            Some(&runner),
            Some(&journal),
            material.request(&fixture, &claim.id, 1),
        )
        .await
        .unwrap();
        assert_eq!(model.calls.load(Ordering::SeqCst), 2);
        let stage_facts = artifacts_with_schema(
            &fixture.store,
            &fixture.worker,
            OPTIMIZATION_STAGE_FACT_SCHEMA,
        )
        .await;
        let of_kind = |kind: &str| -> String {
            let found: Vec<_> = stage_facts
                .iter()
                .filter(|fact| fact["kind"] == kind && fact["episode_id"] == json!(claim.id))
                .collect();
            assert_eq!(found.len(), 1, "one {kind} fact under the claim's episode");
            found[0]["artifact_id"].as_str().unwrap().to_string()
        };
        Self {
            request_fact_id: of_kind("development_request_prepared"),
            observed_fact_id: of_kind("development_observed"),
            claim,
            outcome: record.outcome,
            fixture,
        }
    }
}

async fn value(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(ctx, kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn artifacts_with_schema(store: &Store, ctx: &Context, schema: &str) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let all: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    all.into_iter()
        .filter(|item| item["schema_version"] == schema)
        .collect()
}

async fn put_artifact(store: &Store, ctx: &Context, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, "worker", body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn dependents_of(
    store: &Store,
    ctx: &Context,
    dst_kind: &str,
    dst_id: &str,
) -> Vec<(String, String)> {
    let mut session = store.session().await.unwrap();
    let dependents = session.dependents(ctx, dst_kind, dst_id).await.unwrap();
    session.commit().await.unwrap();
    dependents
}

fn dependents_contain(dependents: &[(String, String)], kind: &str, id: &str) -> bool {
    dependents
        .iter()
        .any(|(dependent_kind, dependent_id)| dependent_kind == kind && dependent_id == id)
}

/// The tombstone the revocation cleanup leaves under a redacted claim's own id.
fn claim_tombstone(claim: &ConsolidationClaim) -> Value {
    json!({
        "id": claim.id,
        "schema_version": REDACTED,
        "state": "source_revoked",
        "original_kind": "artifact",
        "original_schema": CONSOLIDATION_CLAIM_SCHEMA,
        "original_digest": d("the claim as it was"),
        "metadata": {},
    })
}

fn claimed_by(outcome: &CycleTriggerOutcome) -> (String, ConsolidationClaimState) {
    match &outcome.trigger {
        ConsolidationTrigger::Claimed { claim_id, state } => (claim_id.clone(), *state),
        other => panic!("expected a claim, got {other:?}"),
    }
}

/// Revokes `source` and drives the cleanup job until it stops moving.
async fn revoke_and_clean(fixture: &Fixture, source: &str) -> CleanupStatus {
    let mut status = LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: source.into(),
        },
        "source revoked",
        500,
    )
    .await
    .unwrap();
    for now in 501..800 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status =
            LifecycleStore::cleanup_step(&fixture.admin, &fixture.store, &status.job_id, 8, now)
                .await
                .unwrap();
    }
    status
}

// ---------------------------------------------------------------------------
// Test 1: a real registered runner, two trusted cycles, the trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_real_registered_runner_closes_two_trusted_cycles_and_the_second_triggers_a_no_contrast_claim()
 {
    let fixture = Fixture::new().await;
    let first = fixture.stage_trusted(1).await;
    let second = fixture.stage_trusted(2).await;

    // The E03 gate verifies each pair on its own: these are genuine observations.
    for staged in [&first, &second] {
        let view = verify_development_observation(
            &fixture.store,
            &fixture.worker,
            &staged.request_fact_id,
            &staged.observed_fact_id,
        )
        .await
        .unwrap();
        assert_eq!(view.outcomes.len(), 2);
    }

    let one = fixture
        .close_and_trigger(&first.observed_fact_id)
        .await
        .unwrap();
    assert_eq!(one.trigger, ConsolidationTrigger::NotEligible);
    // A trigger that is not eligible writes no claim.
    assert!(fixture.scope_index().await.claim_ids.is_empty());
    assert_eq!(one.cycle.ordinal, 1);
    assert_eq!(one.cycle.report_fact_id, first.observed_fact_id);
    assert_eq!(one.cycle.request_id, first.report.request_id);
    assert_eq!(
        one.cycle.execution_receipt_id,
        first.report.execution_receipt_id
    );
    // The label is decided by the gate: the verified receipts attest the registered
    // pure-function runner. The environment here is a ProgramFixture one (see the
    // module comment), so a proposal built on these cycles is ProgramFixture too.
    assert_eq!(
        one.cycle.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    assert_eq!(
        fixture.environment.environment.evidence_scope,
        EnvironmentEvidenceScope::ProgramFixture
    );
    // Both sides ran the same registered function: two honest identical pairs.
    assert_eq!(one.cycle.pairs.len(), 2);
    assert!(one.cycle.pairs.iter().all(|pair| {
        pair.parent_passed
            && pair.candidate_passed
            && pair.parent_score_micros == 1_000_000
            && pair.candidate_score_micros == 1_000_000
            && pair.class == ConsolidationClass::StableSuccess
    }));
    assert!(!one.cycle.has_contrast);
    assert_eq!(one.cycle.usage_record_ids.len(), 4);
    assert_eq!(one.cycle.sources.len(), 2);

    // The cycle depends on the typed run receipt, not on the report's own id for it.
    let typed_run = storage_id(RUN_RECEIPT_KIND, &first.report.execution_receipt_id).unwrap();
    assert!(dependents_contain(
        &dependents_of(&fixture.store, &fixture.worker, "artifact", &typed_run).await,
        "artifact",
        &one.cycle.id
    ));
    assert!(
        !dependents_contain(
            &dependents_of(
                &fixture.store,
                &fixture.worker,
                "artifact",
                &first.report.execution_receipt_id
            )
            .await,
            "artifact",
            &one.cycle.id
        ),
        "no edge to the report's raw run receipt id, which is not a stored object"
    );

    let two = fixture
        .close_and_trigger(&second.observed_fact_id)
        .await
        .unwrap();
    assert_eq!(two.cycle.ordinal, 2);
    assert_eq!(
        two.cycle.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    assert_eq!(two.cycle.scope_id, one.cycle.scope_id);
    // The real runner scores both sides alike: the generation is claimed, and it is
    // a no-contrast one.
    let (claim_id, state) = claimed_by(&two);
    assert_eq!(state, ConsolidationClaimState::NoContrast);
    assert!(claim_id.starts_with(CLAIM_PREFIX));
    let claim = fixture.claim(&claim_id).await;
    assert_eq!(claim.generation, 1);
    assert_eq!(claim.state, ConsolidationClaimState::NoContrast);
    assert_eq!(
        claim.cycle_ids,
        vec![one.cycle.id.clone(), two.cycle.id.clone()]
    );
    assert_eq!(claim.scope_id, one.cycle.scope_id);

    // It needs no dispatch: it finishes without a model or a runner.
    let record =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &claim_id)
            .await
            .unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::NoChange);
    assert!(record.budget_call_ids.is_empty());
    assert_eq!(
        fixture.claim(&claim_id).await.state,
        ConsolidationClaimState::CompletedNoChange
    );

    // Closing the same report again is idempotent and reports the claim as taken.
    let again = fixture
        .close_and_trigger(&second.observed_fact_id)
        .await
        .unwrap();
    assert_eq!(again.cycle, two.cycle);
    assert_eq!(
        again.trigger,
        ConsolidationTrigger::AlreadyClaimed { claim_id }
    );

    // Real zero-cost execution: nothing spent, nothing held.
    let root = fixture
        .store
        .root_budget(&fixture.worker, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.spent_micros, root.reserved_micros), (0, 0));
}

// ---------------------------------------------------------------------------
// Test 2: forged non-fixture facts are refused; a fixture keeps its path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fact_that_declares_itself_non_fixture_but_fails_the_e03_gate_is_refused_and_a_fixture_keeps_its_path()
 {
    let fixture = Fixture::new().await;
    let honest = fixture.stage_trusted(1).await;

    // Hand-written, raw-id receipts, a clean improvement, and the honest run's real
    // usage records (so the ledger cannot tell). Only the E03 gate can refuse it.
    for provenance in [
        DevelopmentExecutionProvenance::RegisteredPureFunction,
        DevelopmentExecutionProvenance::IsolatedRunner,
    ] {
        // Without a request half the gate has nothing to pair the observation with.
        let lonely = fixture.stage_forged(&honest, provenance, false).await;
        assert!(
            matches!(fixture.close(&lonely).await, Err(Error::NotFound)),
            "{provenance:?} without a request half"
        );
        assert!(matches!(
            fixture.close_and_trigger(&lonely).await,
            Err(Error::NotFound)
        ));
    }
    // With a hand-written request half the pair is formed, and the receipts it
    // names are not typed E03 receipts.
    for provenance in [
        DevelopmentExecutionProvenance::RegisteredPureFunction,
        DevelopmentExecutionProvenance::IsolatedRunner,
    ] {
        let paired = fixture.stage_forged(&honest, provenance, true).await;
        assert!(
            matches!(fixture.close(&paired).await, Err(Error::NotFound)),
            "{provenance:?} with a request half"
        );
    }

    // Honest receipts, but a report that claims a better score than the server
    // computed: the observation is overwritten at its canonical id with a payload
    // that differs from the honest one only in its scores.
    let honest_observed = observed_fact(&honest.request, &honest.report, true);
    assert_eq!(honest_observed.artifact_id, honest.observed_fact_id);
    let mut inflated = honest.report.clone();
    inflated.results[0].parent_score_micros = 0;
    inflated.results[0].parent_passed = false;
    let forged = observed_fact(&honest.request, &inflated, true);
    assert_eq!(forged.artifact_id, honest.observed_fact_id);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &forged.artifact_id,
            "admin",
            &forged,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        fixture.close(&honest.observed_fact_id).await,
        Err(Error::Conflict(message)) if message.contains("scores differ")
    ));
    // The typed receipt closure dropped from the observation: the raw ids alone are
    // not evidence.
    let mut without_closure = honest_observed.clone();
    without_closure
        .dependencies
        .retain(|dependency| !dependency.id.starts_with("e03dev-"));
    let without_closure = without_closure.seal().unwrap();
    assert_eq!(without_closure.artifact_id, honest.observed_fact_id);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &without_closure.artifact_id,
            "admin",
            &without_closure,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        fixture.close(&honest.observed_fact_id).await,
        Err(Error::Conflict(message)) if message.contains("typed receipt closure")
    ));
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &honest_observed.artifact_id,
            "admin",
            &honest_observed,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    // A real observation that declares itself a fixture takes the fixture path, where
    // its receipt ids must be stored under their own ids, which they are not.
    let mut request = honest.request.clone();
    request.episode_id = "declared-fixture-episode".into();
    request.request_id = "declared-fixture-request".into();
    let mut as_fixture = honest.report.clone();
    as_fixture.request_id = request.request_id.clone();
    as_fixture.provenance = DevelopmentExecutionProvenance::Fixture;
    let declared = observed_fact(&request, &as_fixture, false);
    fixture.journal().commit(declared.clone()).await.unwrap();
    assert!(matches!(
        fixture.close(&declared.artifact_id).await,
        Err(Error::NotFound)
    ));

    // None of the refusals wrote a cycle.
    assert!(fixture.cycles().await.is_empty());

    // The honest observation closes, and a hand-written fixture one still closes
    // through the original path and stays a fixture.
    let trusted = fixture.close(&honest.observed_fact_id).await.unwrap();
    assert_eq!(
        trusted.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    let fixture_cycle = fixture
        .close_fixture_cycle(7, &score_only_contrast())
        .await
        .unwrap();
    assert_eq!(
        fixture_cycle.provenance,
        DevelopmentExecutionProvenance::Fixture
    );
    // The original edge: the fixture cycle depends on its receipt under the id the
    // report names.
    assert!(dependents_contain(
        &dependents_of(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &fixture_cycle.execution_receipt_id
        )
        .await,
        "artifact",
        &fixture_cycle.id
    ));
    // Both kinds of cycle live in the one scope. Whether they hand a proposal the
    // `TrustedExecution` label is the existing derivation's business (any fixture
    // cycle or a fixture environment makes it `ProgramFixture`; unit-tested in
    // `monitoring.rs`).
    assert_eq!(fixture.cycles().await.len(), 2);
    assert_eq!(fixture.scope_index().await.cycle_ids.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 3: a consolidation's own development run is not a new cycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_development_run_of_a_consolidation_cannot_be_closed_as_a_new_cycle() {
    let run = Consolidated::new().await;
    let fixture = &run.fixture;
    // Real runner, no improvement: the step ends rejected, and journals its
    // Development stage under the claim's own id.
    assert_eq!(run.outcome, ConsolidationRunOutcome::Rejected);
    assert_eq!(
        fixture.claim(&run.claim.id).await.state,
        ConsolidationClaimState::CompletedRejected
    );
    // The fact is a genuine verified observation and agrees with the scope's
    // manifest, environment and grader; only its episode, which is the claim's id,
    // marks it as the consolidation's own.
    let view = verify_development_observation(
        &fixture.store,
        &fixture.worker,
        &run.request_fact_id,
        &run.observed_fact_id,
    )
    .await
    .unwrap();
    assert_eq!(view.episode_id, run.claim.id);
    assert_eq!(view.outcomes.len(), 2);
    assert_eq!(view.manifest_digest, fixture.manifest_digest);
    assert_eq!(
        view.environment_digest,
        fixture.environment.environment.environment_digest
    );
    assert_eq!(view.grader_digest, fixture.grader_digest);

    let cycles_before = fixture.scope_index().await.cycle_ids;
    assert_eq!(cycles_before.len(), 2);
    let direct = fixture.close(&run.observed_fact_id).await.map(|_| ());
    let triggered = fixture
        .close_and_trigger(&run.observed_fact_id)
        .await
        .map(|_| ());
    for refused in [direct, triggered] {
        assert!(
            matches!(&refused, Err(Error::Conflict(message)) if message.contains("consolidation")),
            "{refused:?}"
        );
    }
    let index = fixture.scope_index().await;
    assert_eq!(index.cycle_ids, cycles_before);
    assert_eq!(fixture.cycles().await.len(), 2);
    assert_eq!(index.claim_ids.len(), 1);
}

#[tokio::test]
async fn a_redacted_claim_still_keeps_its_consolidations_development_run_out_of_the_cycles() {
    let run = Consolidated::new().await;
    let fixture = &run.fixture;
    // The claim is redacted (its sources were revoked and cleaned) and leaves a
    // tombstone under its own id; the observation of its step is still stored.
    put_artifact(
        &fixture.store,
        &fixture.worker,
        &run.claim.id,
        &claim_tombstone(&run.claim),
    )
    .await;
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await["schema_version"],
        REDACTED
    );
    let refused = fixture.close(&run.observed_fact_id).await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message.contains("consolidation")),
        "{refused:?}"
    );
    assert!(matches!(
        fixture.close_and_trigger(&run.observed_fact_id).await,
        Err(Error::Conflict(message)) if message.contains("consolidation")
    ));
    assert_eq!(fixture.scope_index().await.cycle_ids.len(), 2);
    assert_eq!(fixture.cycles().await.len(), 2);
}

#[tokio::test]
async fn after_a_real_revocation_cleanup_the_consolidations_development_run_is_still_no_cycle() {
    let run = Consolidated::new().await;
    let fixture = &run.fixture;
    let status = revoke_and_clean(fixture, "source-1").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    // The claim and the step's facts are redacted together; whatever the verdict is
    // called, no cycle comes out of it.
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await["schema_version"],
        REDACTED
    );
    assert!(fixture.close(&run.observed_fact_id).await.is_err());
    assert!(
        fixture
            .close_and_trigger(&run.observed_fact_id)
            .await
            .is_err()
    );
    assert_eq!(fixture.scope_index().await.cycle_ids.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 4: the trigger sequence and its concurrency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_trigger_claims_one_generation_per_two_cycles() {
    let fixture = Fixture::new().await;
    let mut cycles = Vec::new();
    for n in 1..=4 {
        cycles.push(fixture.stage_trusted(n).await);
    }
    let mut outcomes = Vec::new();
    for staged in &cycles {
        outcomes.push(
            fixture
                .close_and_trigger(&staged.observed_fact_id)
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        outcomes.iter().map(|o| o.cycle.ordinal).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(outcomes[0].trigger, ConsolidationTrigger::NotEligible);
    let (first_claim, first_state) = claimed_by(&outcomes[1]);
    assert_eq!(first_state, ConsolidationClaimState::NoContrast);
    assert_eq!(
        outcomes[2].trigger,
        ConsolidationTrigger::AlreadyClaimed {
            claim_id: first_claim.clone()
        }
    );
    let (second_claim, second_state) = claimed_by(&outcomes[3]);
    assert_eq!(second_state, ConsolidationClaimState::NoContrast);
    assert_ne!(second_claim, first_claim);

    let first = fixture.claim(&first_claim).await;
    let second = fixture.claim(&second_claim).await;
    assert_eq!((first.generation, second.generation), (1, 2));
    assert_eq!(
        first.cycle_ids,
        vec![outcomes[0].cycle.id.clone(), outcomes[1].cycle.id.clone()]
    );
    assert_eq!(
        second.cycle_ids,
        vec![outcomes[2].cycle.id.clone(), outcomes[3].cycle.id.clone()]
    );
    let index = fixture.scope_index().await;
    assert_eq!(index.claim_ids.len(), 2);
    assert_eq!(index.claim_ids[&1], first_claim);
    assert_eq!(index.claim_ids[&2], second_claim);

    // The existing calls keep their own meaning next to the wrapper.
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &index.id)
            .await
            .unwrap(),
        ClaimOutcome::AlreadyClaimed(claim) if claim.id == second_claim
    ));
}

#[tokio::test]
async fn closing_four_cycles_without_the_trigger_skips_generation_one_for_good() {
    let fixture = Fixture::new().await;
    for n in 1..=4 {
        let staged = fixture.stage_trusted(n).await;
        fixture.close(&staged.observed_fact_id).await.unwrap();
    }
    let scope_id = fixture.scope_id().await;
    let ClaimOutcome::Claimed(claim) =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap()
    else {
        panic!("the newest generation was not claimed")
    };
    // Only the newest complete generation is ever claimed.
    assert_eq!(claim.generation, 2);
    let index = fixture.scope_index().await;
    assert_eq!(index.claim_ids.keys().copied().collect::<Vec<_>>(), vec![2]);
}

/// Closes the named facts at once, each on its own task of a multi-thread runtime.
async fn close_and_trigger_together(
    fixture: &Fixture,
    fact_ids: &[&str],
) -> Vec<CycleTriggerOutcome> {
    let tasks: Vec<_> = fact_ids
        .iter()
        .map(|fact_id| {
            let ctx = fixture.worker.clone();
            let store = fixture.store.clone();
            let request = fixture.cycle_request((*fact_id).into());
            tokio::spawn(async move {
                MonitoringCoordinator::close_development_cycle_and_trigger(&ctx, &store, request)
                    .await
            })
        })
        .collect();
    let mut outcomes = Vec::new();
    for task in tasks {
        outcomes.push(task.await.unwrap().unwrap());
    }
    outcomes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_closes_of_one_generation_produce_exactly_one_claim() {
    // The same second cycle closed twice at once: one caller claims, the other sees
    // the claim; both report the one cycle.
    let fixture = Fixture::new().await;
    let first = fixture.stage_trusted(1).await;
    let second = fixture.stage_trusted(2).await;
    fixture
        .close_and_trigger(&first.observed_fact_id)
        .await
        .unwrap();
    let both = close_and_trigger_together(
        &fixture,
        &[&second.observed_fact_id, &second.observed_fact_id],
    )
    .await;
    assert_eq!(both[0].cycle, both[1].cycle);
    let claimed: Vec<_> = both
        .iter()
        .filter(|outcome| matches!(outcome.trigger, ConsolidationTrigger::Claimed { .. }))
        .collect();
    assert_eq!(claimed.len(), 1, "{both:?}");
    let (claim_id, _) = claimed_by(claimed[0]);
    assert!(both.iter().any(|outcome| {
        outcome.trigger
            == ConsolidationTrigger::AlreadyClaimed {
                claim_id: claim_id.clone(),
            }
    }));
    assert_eq!(fixture.scope_index().await.claim_ids.len(), 1);

    // Two different cycles of the next generation closed at once: whichever order
    // they commit in, generation two is claimed by exactly one of the callers.
    let third = fixture.stage_trusted(3).await;
    let fourth = fixture.stage_trusted(4).await;
    let both = close_and_trigger_together(
        &fixture,
        &[&third.observed_fact_id, &fourth.observed_fact_id],
    )
    .await;
    let mut ordinals: Vec<_> = both.iter().map(|outcome| outcome.cycle.ordinal).collect();
    ordinals.sort();
    assert_eq!(ordinals, vec![3, 4]);
    let claims: Vec<_> = both
        .iter()
        .filter(|outcome| matches!(outcome.trigger, ConsolidationTrigger::Claimed { .. }))
        .collect();
    assert_eq!(claims.len(), 1, "{both:?}");
    let (second_generation, _) = claimed_by(claims[0]);
    let index = fixture.scope_index().await;
    assert_eq!(index.claim_ids.len(), 2);
    assert_eq!(index.claim_ids[&2], second_generation);
}

// ---------------------------------------------------------------------------
// Test 5: a revocation between two closes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_revocation_between_two_closes_rejects_the_second_cycle_and_keeps_the_first() {
    let fixture = Fixture::new().await;
    // Both cycles are staged while the runner can still execute: it refuses a
    // revoked control source.
    let first = fixture.stage_trusted(1).await;
    let second = fixture.stage_trusted(2).await;
    let one = fixture
        .close_and_trigger(&first.observed_fact_id)
        .await
        .unwrap();
    assert_eq!(one.trigger, ConsolidationTrigger::NotEligible);

    // The first cycle's source is revoked before the second close.
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-1".into(),
        },
        "source revoked between two cycles",
        500,
    )
    .await
    .unwrap();

    // Actual behavior: the revocation moved the namespace watermark, which freezes
    // the scope (the watermark is part of it), so the second cycle is refused. It is
    // an error, not a cycle with an `Unavailable` trigger.
    let refused = fixture.close_and_trigger(&second.observed_fact_id).await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message.contains("watermark")),
        "{refused:?}"
    );
    let index = fixture.scope_index().await;
    assert_eq!(index.cycle_ids, vec![one.cycle.id.clone()]);
    assert!(index.claim_ids.is_empty());
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &index.id)
            .await,
        Err(Error::Conflict(_))
    ));
    // Nor can the first cycle's own fact be closed again.
    assert!(fixture.close(&first.observed_fact_id).await.is_err());
}

#[tokio::test]
async fn a_claim_that_cannot_be_read_after_a_cycle_closes_leaves_the_cycle_and_reports_unavailable()
{
    // A claim that cannot be read when the trigger asks for it: the cycle is
    // already committed and stays; the trigger says why the claim was unavailable.
    // A revocation itself moves the watermark and stops the close first (above), so
    // the redaction that a cleanup leaves behind is written directly.
    let fixture = Fixture::new().await;
    let mut staged = Vec::new();
    for n in 1..=3 {
        staged.push(fixture.stage_trusted(n).await);
    }
    fixture
        .close_and_trigger(&staged[0].observed_fact_id)
        .await
        .unwrap();
    let second = fixture
        .close_and_trigger(&staged[1].observed_fact_id)
        .await
        .unwrap();
    let (claim_id, _) = claimed_by(&second);
    let claim = fixture.claim(&claim_id).await;
    put_artifact(
        &fixture.store,
        &fixture.worker,
        &claim_id,
        &claim_tombstone(&claim),
    )
    .await;

    let third = fixture
        .close_and_trigger(&staged[2].observed_fact_id)
        .await
        .unwrap();
    assert_eq!(third.cycle.ordinal, 3);
    assert!(
        matches!(
            &third.trigger,
            ConsolidationTrigger::Unavailable { reason } if reason.contains("redacted")
        ),
        "{:?}",
        third.trigger
    );
    // Nothing was rolled back.
    let index = fixture.scope_index().await;
    assert_eq!(index.cycle_ids.len(), 3);
    assert_eq!(index.cycle_ids[2], third.cycle.id);
    assert_eq!(fixture.cycles().await.len(), 3);
}

// ---------------------------------------------------------------------------
// Test 6: the registered runner as the consolidation's development runner
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_registered_runner_binds_to_the_root_of_its_control() {
    let fixture = Fixture::new().await;
    let runner = fixture.runner();
    let binding = runner.trusted_budget_binding(TENANT).await.unwrap();
    assert_eq!(
        binding,
        BudgetPortBinding {
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        }
    );

    // Another namespace is not the runner's own.
    assert!(matches!(
        runner.trusted_budget_binding("other-tenant").await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        runner.trusted_budget_binding("not a namespace").await,
        Err(Error::Invalid(_))
    ));
    // A runner for a control that was never registered has no binding to give.
    assert!(matches!(
        fixture
            .runner_for("unregistered-control")
            .trusted_budget_binding(TENANT)
            .await,
        Err(Error::NotFound)
    ));

    // A control whose root no longer matches the persisted billing scope: the
    // stored control is overwritten at its canonical id (raw storage access).
    let control_id = storage_id(CONTROL_KIND, CONTROL_ID).unwrap();
    let stored = value(&fixture.store, &fixture.admin, "artifact", &control_id).await;
    let mut drifted = stored.clone();
    drifted["payload"]["root_budget_id"] = json!("other-root");
    put_artifact(&fixture.store, &fixture.admin, &control_id, &drifted).await;
    assert!(matches!(
        runner.trusted_budget_binding(TENANT).await,
        Err(Error::Conflict(message)) if message.contains("root budget")
    ));
    put_artifact(&fixture.store, &fixture.admin, &control_id, &stored).await;
    assert_eq!(
        runner
            .trusted_budget_binding(TENANT)
            .await
            .unwrap()
            .root_budget_id,
        ROOT
    );
}

#[tokio::test]
async fn a_consolidation_refuses_a_runner_bound_to_another_root_and_the_right_one_runs_it() {
    let fixture = Fixture::new().await;
    let claim = fixture.claimed().await;
    let material = StepMaterial::new(&fixture).await;
    let model = EditingModelPort {
        calls: AtomicUsize::new(0),
    };

    // A second control under a second billing root. Its runner binds to that root,
    // which is not the claim's.
    store_second_root(&fixture).await;
    let other = fixture.runner_for("other-control");
    let other_binding = other.trusted_budget_binding(TENANT).await.unwrap();
    assert_eq!(other_binding.billing_scope, "other-scope");
    assert_eq!(other_binding.root_budget_id, "other-root");
    let journal = fixture.journal();
    let refused = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&other),
        Some(&journal),
        material.request(&fixture, &claim.id, 1),
    )
    .await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message.contains("budget binding")),
        "{refused:?}"
    );
    // Refused before anything was dispatched or the claim was taken.
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.claim(&claim.id).await.state,
        ConsolidationClaimState::Claimed
    );

    // The runner of the claim's own root drives it to a terminal record, spending
    // through the real ledger under the claim's dispatch group.
    let runner = fixture.runner();
    let record = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        material.request(&fixture, &claim.id, 1),
    )
    .await
    .unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::Rejected);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.worker, SCOPE, &claim.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(calls.len(), 4, "two tasks, two sides");
    assert!(calls.iter().all(|call| call.usage_record_id.is_some()));
    assert_eq!(record.budget_call_ids.len(), 4);
}

/// Authorizes a second billing root and registers a control under it.
async fn store_second_root(fixture: &Fixture) {
    fixture
        .store
        .authorize_root_budget(
            &fixture.admin,
            &RootBudgetAuthorization {
                root_budget_id: "other-root".into(),
                billing_scope: "other-scope".into(),
                allowed_namespaces: vec![TENANT.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: d("other-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let other = control(
        "other-control",
        "other-scope",
        "other-root",
        &fixture.control.environment_digest,
    );
    register_development_control(&fixture.admin, &fixture.store, other)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Revocation cleanup reaches a trusted cycle through its typed receipt edge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_a_source_cleans_trusted_cycles_to_complete() {
    let fixture = Fixture::new().await;
    let first = fixture.stage_trusted(1).await;
    let second = fixture.stage_trusted(2).await;
    let one = fixture
        .close_and_trigger(&first.observed_fact_id)
        .await
        .unwrap();
    let two = fixture
        .close_and_trigger(&second.observed_fact_id)
        .await
        .unwrap();
    let (claim_id, _) = claimed_by(&two);
    let scope_before = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &one.cycle.scope_id,
    )
    .await;
    let parent_receipt = storage_id(
        EXECUTION_RECEIPT_KIND,
        &first.report.results[0].parent_execution_id,
    )
    .unwrap();

    let status = revoke_and_clean(&fixture, "source-1").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    // The derived content is redacted: both cycles, the claim, the observations.
    for cycle in [&one.cycle, &two.cycle] {
        let body = value(&fixture.store, &fixture.worker, "artifact", &cycle.id).await;
        assert_eq!(body["schema_version"], REDACTED, "{body}");
        assert_eq!(body["original_schema"], DEVELOPMENT_CYCLE_SCHEMA, "{body}");
    }
    let claim_body = value(&fixture.store, &fixture.worker, "artifact", &claim_id).await;
    assert_eq!(claim_body["original_schema"], CONSOLIDATION_CLAIM_SCHEMA);
    for staged in [&first, &second] {
        let body = value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &staged.observed_fact_id,
        )
        .await;
        assert_eq!(body["schema_version"], REDACTED, "{body}");
    }
    // The scope index and the typed receipts (accounting and history) survive.
    assert_eq!(
        value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &one.cycle.scope_id
        )
        .await,
        scope_before
    );
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &parent_receipt).await["schema_version"],
        "rsia.typed_artifact_envelope.v1"
    );
    // Nothing more can be closed or claimed on the revoked scope.
    assert!(
        fixture
            .close_and_trigger(&first.observed_fact_id)
            .await
            .is_err()
    );
}
