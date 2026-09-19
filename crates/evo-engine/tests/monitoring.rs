use async_trait::async_trait;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile, SkillSnapshot,
    SurfaceCoverage, SurfaceItem, SystemSnapshot,
};
use evo_core::evidence::{EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace, TraceOutcome,
    TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    TrustedEditContext, skill_snapshot_digest,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::broker::BudgetPortBinding;
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::monitoring::{
    ApplicationStatus, ClaimOutcome, CompleteDevelopmentCycleRequest, ConsolidationClaimState,
    ConsolidationDevRunner, ConsolidationModelPort, ConsolidationRunOutcome,
    EnvironmentEvidenceScope, MonitoringCoordinator, QualityStatus, RecordEnvironmentRequest,
    RecordedEnvironment,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationJournal,
    OptimizationJournalStage, OptimizationStepRequest, PairedTaskResult, StageDependency,
    StageFact, StageFactKind, StoreOptimizationJournal,
};
use evo_engine::release_store::{PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage, RootBudgetAuthorization,
    RootBudgetRecord,
};
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    host: Context,
    worker: Context,
    environment: RecordedEnvironment,
    authorities: Vec<StoredTraceAuthority>,
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
        let database = dir.path().join("monitoring.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let admin = context("admin", Role::Admin);
        let host = context("host", Role::Host);
        let worker = context("worker", Role::Worker);
        ReleaseStore::register_host_surface(
            &admin,
            &store,
            "surface",
            surface(),
            vec!["model".into()],
        )
        .await
        .unwrap();
        let snapshot = ReleaseStore::prepare_run(
            &host,
            &store,
            PrepareRunRequest {
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
                capability_level: CapabilityLevel::Attached,
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
            environment.observation.application_status,
            ApplicationStatus::NotSelected
        );
        assert_eq!(
            environment.observation.quality_status,
            QualityStatus::Unknown
        );
        assert_eq!(
            environment.environment.evidence_scope,
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
        Self {
            _dir: dir,
            database,
            store,
            admin,
            host,
            worker,
            environment,
            authorities,
            manifest,
            parent_skill: parent_skill.clone(),
            manifest_digest,
            grader_digest,
            strategy_digest,
            parent_skill_digest: skill_snapshot_digest(&parent_skill).unwrap(),
            parent_bundle_digest: d("parent-bundle"),
            budget,
        }
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

    async fn stage_report(
        &self,
        cycle: u32,
        contrast: bool,
        forbidden_dependency: bool,
        hidden_payload: bool,
    ) -> String {
        stage_report(
            &self.store,
            &self.worker,
            &self.environment,
            &self.manifest_digest,
            &self.grader_digest,
            &self.parent_bundle_digest,
            cycle,
            contrast,
            forbidden_dependency,
            hidden_payload,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn stage_report(
    store: &Store,
    worker: &Context,
    environment: &RecordedEnvironment,
    manifest_digest: &str,
    grader_digest: &str,
    parent_bundle_digest: &str,
    cycle: u32,
    contrast: bool,
    forbidden_dependency: bool,
    hidden_payload: bool,
) -> String {
    let parent_execution = format!("parent-execution-{cycle}");
    let candidate_execution = format!("candidate-execution-{cycle}");
    let runner_execution = format!("runner-execution-{cycle}");
    let grader_receipt = d(&format!("grader-receipt-{cycle}"));
    let report = DevelopmentRunReport {
        request_id: format!("development-request-{cycle}"),
        manifest_digest: manifest_digest.into(),
        parent_bundle_digest: parent_bundle_digest.into(),
        candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
        environment_digest: environment.environment.environment_digest.clone(),
        grader_digest: grader_digest.into(),
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
                &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":cycle,"fixture":true}),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let mut payload = serde_json::to_value(&report).unwrap();
    if hidden_payload {
        payload["hidden_formal_score"] = json!(900_000);
    }
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
    if forbidden_dependency {
        dependencies.push(StageDependency {
            kind: "oracle".into(),
            id: format!("hidden-oracle-{cycle}"),
        });
    }
    let fact = StageFact {
        schema_version: "rsia.optimization.stage_fact.v1".into(),
        artifact_id: "placeholder".into(),
        namespace: worker.namespace().into(),
        episode_id: format!("development-episode-{cycle}"),
        step: cycle,
        attempt: 1,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id: report.request_id,
        input_digest: d(&format!("development-input-{cycle}")),
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

struct CountingModelPort {
    calls: AtomicUsize,
    binding: BudgetPortBinding,
}

impl Default for CountingModelPort {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            binding: BudgetPortBinding {
                billing_scope: "billing-scope".into(),
                root_budget_id: "root-budget".into(),
            },
        }
    }
}

#[async_trait]
impl ModelPort for CountingModelPort {
    async fn dispatch(&self, _request: ModelRequest) -> evo_core::Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Conflict("model must not be dispatched".into()))
    }
}

#[async_trait]
impl ConsolidationModelPort for CountingModelPort {
    async fn trusted_budget_binding(
        &self,
        _namespace: &str,
    ) -> evo_core::Result<BudgetPortBinding> {
        Ok(self.binding.clone())
    }
}

struct RevokingModelPort {
    store: Store,
    admin: Context,
    calls: AtomicUsize,
}

#[async_trait]
impl ModelPort for RevokingModelPort {
    async fn dispatch(&self, request: ModelRequest) -> evo_core::Result<ModelResponse> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        if attempt > 0 {
            return Err(Error::Conflict(
                "revoking model was dispatched twice".into(),
            ));
        }
        LifecycleStore::begin_revoke(
            &self.admin,
            &self.store,
            TypedObjectRef {
                kind: "run".into(),
                id: "source-2".into(),
            },
            "revoked during consolidation",
            60,
        )
        .await?;
        let output = "[]".to_string();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: "revoked-response".into(),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: "revoked-call".into(),
                dispatch_id: "revoked-dispatch".into(),
                root_budget_id: "root-budget".into(),
                provider_request_id: "revoked-provider".into(),
                usage_record_id: "revoked-usage".into(),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

#[async_trait]
impl ConsolidationModelPort for RevokingModelPort {
    async fn trusted_budget_binding(
        &self,
        _namespace: &str,
    ) -> evo_core::Result<BudgetPortBinding> {
        Ok(BudgetPortBinding {
            billing_scope: "billing-scope".into(),
            root_budget_id: "root-budget".into(),
        })
    }
}

struct CountingDevRunner {
    calls: AtomicUsize,
    binding: BudgetPortBinding,
}

impl Default for CountingDevRunner {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            binding: BudgetPortBinding {
                billing_scope: "billing-scope".into(),
                root_budget_id: "root-budget".into(),
            },
        }
    }
}

#[async_trait]
impl DevRunner for CountingDevRunner {
    async fn run(&self, _request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Conflict("development runner must not run".into()))
    }
}

#[async_trait]
impl ConsolidationDevRunner for CountingDevRunner {
    async fn trusted_budget_binding(
        &self,
        _namespace: &str,
    ) -> evo_core::Result<BudgetPortBinding> {
        Ok(self.binding.clone())
    }
}

struct FailAfterStoreJournal<'a> {
    inner: &'a StoreOptimizationJournal,
    commits: AtomicUsize,
    fail_on_commit: usize,
}

#[async_trait]
impl OptimizationJournal for FailAfterStoreJournal<'_> {
    async fn commit(&self, fact: StageFact) -> evo_core::Result<()> {
        let index = self.commits.fetch_add(1, Ordering::SeqCst);
        if index == self.fail_on_commit {
            return Err(Error::NotFound);
        }
        self.inner.commit(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> evo_core::Result<Option<StageFact>> {
        self.inner.lookup(artifact_id).await
    }

    async fn claim(&self, fact: StageFact) -> evo_core::Result<bool> {
        self.inner.claim(fact).await
    }

    async fn verify_sources(
        &self,
        selection: &SourceSelection,
        evidence: &EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> evo_core::Result<()> {
        self.inner
            .verify_sources(selection, evidence, bindings, traces, watermark)
            .await
    }

    async fn check_live(&self, source_ids: &[String], watermark: u64) -> evo_core::Result<()> {
        self.inner.check_live(source_ids, watermark).await
    }
}

#[tokio::test]
async fn two_and_four_completed_cycles_have_one_stable_claim_per_generation() {
    let fixture = Fixture::new().await;
    let first_fact = fixture.stage_report(1, false, false, false).await;
    let first = MonitoringCoordinator::close_development_cycle(
        &fixture.worker,
        &fixture.store,
        fixture.cycle_request(first_fact.clone()),
    )
    .await
    .unwrap();
    assert_eq!(first.ordinal, 1);
    assert_eq!(first.provenance, DevelopmentExecutionProvenance::Fixture);
    let mut renamed = fixture.cycle_request(first_fact);
    renamed.skill_id = "renamed-skill".into();
    assert!(matches!(
        MonitoringCoordinator::close_development_cycle(&fixture.worker, &fixture.store, renamed)
            .await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &first.scope_id
        )
        .await
        .unwrap(),
        ClaimOutcome::NotEligible {
            completed_cycles: 1
        }
    ));
    let second_fact = fixture.stage_report(2, false, false, false).await;
    MonitoringCoordinator::close_development_cycle(
        &fixture.worker,
        &fixture.store,
        fixture.cycle_request(second_fact),
    )
    .await
    .unwrap();
    let (left, right) = tokio::join!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &first.scope_id
        ),
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &first.scope_id
        )
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ClaimOutcome::Claimed(_)))
            .count(),
        1
    );
    let claim = outcomes
        .into_iter()
        .find_map(|outcome| match outcome {
            ClaimOutcome::Claimed(claim) | ClaimOutcome::AlreadyClaimed(claim) => Some(claim),
            ClaimOutcome::NotEligible { .. } => None,
        })
        .unwrap();
    assert_eq!(claim.generation, 1);
    assert_eq!(claim.state, ConsolidationClaimState::NoContrast);
    let no_change =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &claim.id)
            .await
            .unwrap();
    assert_eq!(no_change.outcome, ConsolidationRunOutcome::NoChange);
    let root = fixture
        .store
        .root_budget(&fixture.worker, "billing-scope")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.spent_micros, root.reserved_micros), (0, 0));

    fixture.store.close().await;
    let reopened = Store::open(&fixture.database).await.unwrap();
    for cycle in 3..=4 {
        let fact = stage_report(
            &reopened,
            &fixture.worker,
            &fixture.environment,
            &fixture.manifest_digest,
            &fixture.grader_digest,
            &fixture.parent_bundle_digest,
            cycle,
            true,
            false,
            false,
        )
        .await;
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &reopened,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
    }
    let generation_two =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &reopened, &first.scope_id)
            .await
            .unwrap();
    let ClaimOutcome::Claimed(generation_two) = generation_two else {
        panic!("generation two was not claimed")
    };
    assert_eq!(generation_two.generation, 2);
    assert_eq!(generation_two.state, ConsolidationClaimState::Claimed);
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &reopened, &first.scope_id)
            .await
            .unwrap(),
        ClaimOutcome::AlreadyClaimed(claim) if claim.id == generation_two.id
    ));
}

#[tokio::test]
async fn second_source_revocation_blocks_claim_without_erasing_cycles() {
    let fixture = Fixture::new().await;
    let mut scope_id = String::new();
    let mut cycle_ids = Vec::new();
    for cycle in 1..=2 {
        let fact = fixture.stage_report(cycle, true, false, false).await;
        let record = MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
        scope_id = record.scope_id;
        cycle_ids.push(record.id);
    }
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-2".into(),
        },
        "source two revoked",
        20,
    )
    .await
    .unwrap();
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await,
        Err(Error::Conflict(_))
    ));
    let mut session = fixture.store.session().await.unwrap();
    for id in cycle_ids {
        assert!(
            session
                .get::<serde_json::Value>(&fixture.worker, "artifact", &id)
                .await
                .unwrap()
                .is_some()
        );
    }
    session.commit().await.unwrap();
}

#[tokio::test]
async fn environment_drift_invalidates_old_scope_and_unknown_is_not_harmful() {
    let fixture = Fixture::new().await;
    let fact = fixture.stage_report(1, true, false, false).await;
    let cycle = MonitoringCoordinator::close_development_cycle(
        &fixture.worker,
        &fixture.store,
        fixture.cycle_request(fact),
    )
    .await
    .unwrap();
    let snapshot = ReleaseStore::prepare_run(
        &fixture.host,
        &fixture.store,
        PrepareRunRequest {
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
            capability_level: CapabilityLevel::Attached,
        },
    )
    .await
    .unwrap();
    ReleaseStore::record_applied_request(
        &fixture.host,
        &fixture.store,
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
        &fixture.host,
        &fixture.store,
        RecordEnvironmentRequest {
            run_id: "host-run-2".into(),
            development_manifest_digest: fixture.manifest_digest.clone(),
            grader_digest: fixture.grader_digest.clone(),
            generation_strategy_digest: fixture.strategy_digest.clone(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(current.observation.quality_status, QualityStatus::Unknown);
    MonitoringCoordinator::record_environment_drift(
        &fixture.admin,
        &fixture.store,
        &fixture.environment.environment.id,
        &current.environment.id,
        &cycle.scope_id,
        "model deployment changed",
    )
    .await
    .unwrap();
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(
            &fixture.worker,
            &fixture.store,
            &cycle.scope_id
        )
        .await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn hidden_formal_payloads_and_dependencies_never_enter_cycle_history() {
    let fixture = Fixture::new().await;
    let forbidden = fixture.stage_report(1, true, true, false).await;
    assert!(matches!(
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(forbidden)
        )
        .await,
        Err(Error::Forbidden)
    ));
    let hidden = fixture.stage_report(2, true, false, true).await;
    assert!(matches!(
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(hidden)
        )
        .await,
        Err(Error::Invalid(_))
    ));
    let outsider = Context::new("other-tenant", "worker", Role::Worker).unwrap();
    let fact = fixture.stage_report(3, true, false, false).await;
    assert!(
        MonitoringCoordinator::close_development_cycle(
            &outsider,
            &fixture.store,
            fixture.cycle_request(fact)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn real_e03_no_change_and_zero_budget_paths_never_dispatch_or_write_active() {
    let fixture = Fixture::new().await;
    let mut scope_id = String::new();
    for cycle in 1..=2 {
        let fact = fixture.stage_report(cycle, true, false, false).await;
        let record = MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
        scope_id = record.scope_id;
    }
    let ClaimOutcome::Claimed(first_claim) =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap()
    else {
        panic!("first consolidation generation was not claimed")
    };
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
        "tenant",
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
        namespace: "tenant".into(),
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
    let profile = Profile {
        id: "profile".into(),
        evolution_enabled: true,
        parent_digest: approved_parent,
        baseline_digest: safe_baseline,
    };
    let baseline = fixture.parent_skill.clone();
    let parent_strategy = Strategy::default();
    let baseline_strategy = Strategy::default();
    let improver_patch = ImproverPatch::default();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let revoked = BTreeSet::new();
    let journal = StoreOptimizationJournal::new(
        fixture.store.clone(),
        fixture.worker.clone(),
        fixture.worker.actor(),
    )
    .unwrap();
    let model = CountingModelPort::default();
    let runner = CountingDevRunner::default();
    let make_request = |claim_id: &str, sequence: u32| OptimizationStepRequest {
        evidence: &evidence,
        source_selection: &selection,
        source_bindings: &bindings,
        traces: vec![],
        model_context: ModelRequestContext {
            request_id: format!("consolidation-request-{sequence}"),
            namespace: "tenant".into(),
            purpose: Purpose::Development,
            stage: ModelStage::Merge,
            episode_id: claim_id.into(),
            step: sequence,
            attempt: 1,
            parent_skill_digest: fixture.parent_skill_digest.clone(),
            bundle_digest: fixture.parent_bundle_digest.clone(),
            source_closure: allowed.clone(),
            model_digest: d("fixture-model"),
            tools_digest: d("tools"),
            rules_digest: d("rules"),
            sampling_digest: d("sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        },
        parent_skill: &fixture.parent_skill,
        edit_context: &edit_context,
        edit_batch_template: edit_template.clone(),
        protected_ranges: &[],
        bundle_context: BundleCompileContext {
            profile: &profile,
            baseline: &baseline,
            parent_strategy: &parent_strategy,
            baseline_strategy: &baseline_strategy,
            improver_patch: &improver_patch,
            caps: &caps,
            revoked: &revoked,
        },
        development_request: DevelopmentRunRequest {
            request_id: format!("consolidation-development-{sequence}"),
            namespace: "tenant".into(),
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
    };
    let wrong_model = CountingModelPort {
        calls: AtomicUsize::new(0),
        binding: BudgetPortBinding {
            billing_scope: "billing-scope".into(),
            root_budget_id: "wrong-root".into(),
        },
    };
    assert!(matches!(
        MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &first_claim.id,
            Some(&wrong_model),
            Some(&runner),
            Some(&journal),
            make_request(&first_claim.id, 1),
        )
        .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(wrong_model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    let wrong_runner = CountingDevRunner {
        calls: AtomicUsize::new(0),
        binding: BudgetPortBinding {
            billing_scope: "wrong-scope".into(),
            root_budget_id: "root-budget".into(),
        },
    };
    assert!(matches!(
        MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &first_claim.id,
            Some(&model),
            Some(&wrong_runner),
            Some(&journal),
            make_request(&first_claim.id, 1),
        )
        .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(wrong_runner.calls.load(Ordering::SeqCst), 0);
    let prior_call = fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-scope".into(),
                call_id: "prior-dispatched-call".into(),
                dispatch_group_id: first_claim.id.clone(),
                stage: BudgetStage::Consolidation,
                actual_input_digest: d("prior-dispatched-input"),
                request_artifact: None,
                max_cost_micros: 10,
                lease_token: "prior-dispatched-lease".into(),
                lease_until: 100,
                now: 20,
            },
        )
        .await
        .unwrap();
    let prior_fence = BudgetCallFence {
        billing_scope: prior_call.billing_scope.clone(),
        call_id: prior_call.call_id.clone(),
        actual_input_digest: prior_call.actual_input_digest.clone(),
        lease_token: prior_call.lease_token.clone(),
        lease_epoch: prior_call.lease_epoch,
        now: 21,
    };
    fixture
        .store
        .begin_budget_dispatch(&fixture.worker, &prior_fence)
        .await
        .unwrap();
    let failing_journal = FailAfterStoreJournal {
        inner: &journal,
        commits: AtomicUsize::new(0),
        fail_on_commit: 2,
    };
    let first = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &first_claim.id,
        Some(&model),
        Some(&runner),
        Some(&failing_journal),
        make_request(&first_claim.id, 1),
    )
    .await
    .unwrap();
    assert_eq!(first.outcome, ConsolidationRunOutcome::Uncertain);
    assert_eq!(first.reason, "optimization_step_not_found");
    assert_eq!(first.budget_call_ids, vec!["prior-dispatched-call"]);
    assert_eq!(
        fixture
            .store
            .budget_call(&fixture.worker, "billing-scope", "prior-dispatched-call")
            .await
            .unwrap()
            .unwrap()
            .state,
        BudgetCallState::Dispatched
    );
    let repeated = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &first_claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        make_request(&first_claim.id, 1),
    )
    .await
    .unwrap();
    assert_eq!(repeated, first);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);

    for cycle in 3..=4 {
        let fact = fixture.stage_report(cycle, true, false, false).await;
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
    }
    let ClaimOutcome::Claimed(second_claim) =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap()
    else {
        panic!("second consolidation generation was not claimed")
    };
    let (left, right) = tokio::join!(
        MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &second_claim.id,
            Some(&model),
            Some(&runner),
            Some(&journal),
            make_request(&second_claim.id, 2),
        ),
        MonitoringCoordinator::run_consolidation(
            &fixture.worker,
            &fixture.store,
            &second_claim.id,
            Some(&model),
            Some(&runner),
            Some(&journal),
            make_request(&second_claim.id, 2),
        )
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left, right);
    assert_eq!(left.outcome, ConsolidationRunOutcome::NoChange);
    for cycle in 5..=6 {
        let fact = fixture.stage_report(cycle, true, false, false).await;
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
    }
    let ClaimOutcome::Claimed(third_claim) =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap()
    else {
        panic!("third consolidation generation was not claimed")
    };
    let mut budget_fill_calls = Vec::new();
    for index in 0..10 {
        let max_cost_micros = if index == 9 { 90 } else { 100 };
        budget_fill_calls.push(
            fixture
                .store
                .reserve_budget_call(
                    &fixture.worker,
                    &BudgetCallReservation {
                        billing_scope: "billing-scope".into(),
                        call_id: format!("budget-fill-{index}"),
                        dispatch_group_id: "budget-fill-group".into(),
                        stage: BudgetStage::Consolidation,
                        actual_input_digest: d(&format!("budget-fill-input-{index}")),
                        request_artifact: None,
                        max_cost_micros,
                        lease_token: format!("budget-fill-lease-{index}"),
                        lease_until: 100,
                        now: 30,
                    },
                )
                .await
                .unwrap(),
        );
    }
    let blocked = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &third_claim.id,
        Some(&model),
        Some(&runner),
        Some(&journal),
        make_request(&third_claim.id, 3),
    )
    .await
    .unwrap();
    assert_eq!(blocked.outcome, ConsolidationRunOutcome::BlockedBudget);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    for call in budget_fill_calls {
        fixture
            .store
            .cancel_budget_call(
                &fixture.worker,
                &BudgetCallFence {
                    billing_scope: call.billing_scope,
                    call_id: call.call_id,
                    actual_input_digest: call.actual_input_digest,
                    lease_token: call.lease_token,
                    lease_epoch: call.lease_epoch,
                    now: 31,
                },
                "release test budget",
            )
            .await
            .unwrap();
    }
    for cycle in 7..=8 {
        let fact = fixture.stage_report(cycle, true, false, false).await;
        MonitoringCoordinator::close_development_cycle(
            &fixture.worker,
            &fixture.store,
            fixture.cycle_request(fact),
        )
        .await
        .unwrap();
    }
    let ClaimOutcome::Claimed(fourth_claim) =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap()
    else {
        panic!("fourth consolidation generation was not claimed")
    };
    let revoking_model = RevokingModelPort {
        store: fixture.store.clone(),
        admin: fixture.admin.clone(),
        calls: AtomicUsize::new(0),
    };
    let mut revoke_request = make_request(&fourth_claim.id, 4);
    revoke_request.traces = fixture
        .authorities
        .iter()
        .map(|authority| authority.trace.clone())
        .collect();
    let revoked = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &fourth_claim.id,
        Some(&revoking_model),
        Some(&runner),
        Some(&journal),
        revoke_request,
    )
    .await
    .unwrap();
    assert_eq!(revoked.outcome, ConsolidationRunOutcome::Revoked);
    assert_eq!(revoking_model.calls.load(Ordering::SeqCst), 1);
    let mut retry_request = make_request(&fourth_claim.id, 4);
    retry_request.traces = fixture
        .authorities
        .iter()
        .map(|authority| authority.trace.clone())
        .collect();
    let retry = MonitoringCoordinator::run_consolidation(
        &fixture.worker,
        &fixture.store,
        &fourth_claim.id,
        Some(&revoking_model),
        Some(&runner),
        Some(&journal),
        retry_request,
    )
    .await
    .unwrap();
    assert_eq!(retry, revoked);
    assert_eq!(revoking_model.calls.load(Ordering::SeqCst), 1);
    let mut session = fixture.store.session().await.unwrap();
    assert!(
        session
            .get::<serde_json::Value>(&fixture.worker, "pointer", "profile-pointer-profile")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}
