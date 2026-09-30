//! E13 increment (plan v4.2, AG-022): the revocation-cleanup closure of the
//! monitoring objects, pure pass/fail task pairing, the durable consolidation
//! proposal and its staging bridge.
//!
//! The fixtures are copied from `tests/monitoring.rs` (environment, cycles,
//! claims), `tests/optimization.rs` (the editing model port and the
//! development report) and `tests/development_receipts.rs` (driving a cleanup
//! job to `Complete`), because test crates cannot import one another.

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
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::monitoring::{
    CONSOLIDATION_PROPOSAL_SCHEMA, ClaimOutcome, CompleteDevelopmentCycleRequest,
    ConsolidationClaim, ConsolidationClaimState, ConsolidationDevRunner, ConsolidationModelPort,
    ConsolidationProposal, ConsolidationRunOutcome, ConsolidationRunRecord,
    DEVELOPMENT_CYCLE_SCHEMA, DevelopmentCycleRecord, ENVIRONMENT_DRIFT_SCHEMA,
    EnvironmentEvidenceScope, MonitoringCoordinator, RecordEnvironmentRequest, RecordedEnvironment,
    StagedConsolidationCandidate,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentSelection,
    DevelopmentSelectionDecision, DevelopmentTask, OptimizationJournal, OptimizationJournalStage,
    OptimizationStepRequest, PairedTaskResult, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal,
};
use evo_engine::release_store::{
    PrepareRunRequest, ReleaseCandidateRecord, ReleaseStore, TrustedHostExecutionEvidence,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, RootBudgetRecord,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

const TENANT: &str = "tenant";
const SCOPE: &str = "billing-scope";
const ROOT: &str = "root-budget";
const REDACTED: &str = "rsia.redacted.v1";

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

/// One paired task of a staged development report.
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

/// Identical before and after: no contrast at all.
fn no_contrast() -> Vec<ResultSpec> {
    vec![ResultSpec {
        task_id: "task-a",
        parent_score: 500_000,
        candidate_score: 500_000,
        parent_passed: true,
        candidate_passed: true,
    }]
}

struct Fixture {
    _dir: tempfile::TempDir,
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
        let store = Store::open(&dir.path().join("consolidation.sqlite3"))
            .await
            .unwrap();
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
            environment.environment.evidence_scope,
            EnvironmentEvidenceScope::ProgramFixture
        );
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
        Self {
            _dir: dir,
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

    /// Stages a development report for `cycle` and closes it as a cycle.
    async fn close_cycle(
        &self,
        cycle: u32,
        results: &[ResultSpec],
    ) -> Result<DevelopmentCycleRecord, Error> {
        let fact = self.stage_report(cycle, results).await;
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            self.cycle_request(fact),
        )
        .await
    }

    /// Closes two cycles with a contrast and claims the first generation.
    async fn claimed(&self) -> ConsolidationClaim {
        for cycle in 1..=2 {
            self.close_cycle(cycle, &score_only_contrast())
                .await
                .unwrap();
        }
        self.claim_generation().await
    }

    async fn claim_generation(&self) -> ConsolidationClaim {
        let scope_id = self.scope_id().await;
        let ClaimOutcome::Claimed(claim) =
            MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, &scope_id)
                .await
                .unwrap()
        else {
            panic!("the next consolidation generation was not claimed")
        };
        claim
    }

    async fn scope_id(&self) -> String {
        let mut session = self.store.session().await.unwrap();
        let scopes: Vec<Value> = session.list(&self.worker, "artifact").await.unwrap();
        session.commit().await.unwrap();
        let ids: Vec<_> = scopes
            .iter()
            .filter(|value| value["schema_version"] == "rsia.monitoring.consolidation_scope.v1")
            .map(|value| value["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 1, "exactly one consolidation scope");
        ids[0].clone()
    }

    /// Stages a DevelopmentObserved fact whose report pairs `results` for one
    /// cycle, with the fixture receipts the stage fact depends on.
    async fn stage_report(&self, cycle: u32, results: &[ResultSpec]) -> String {
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
            schema_version: "rsia.optimization.stage_fact.v1".into(),
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

    fn journal(&self) -> StoreOptimizationJournal {
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker").unwrap()
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
}

impl EditingModelPort {
    fn new(tag: &'static str) -> Self {
        Self {
            tag,
            calls: AtomicUsize::new(0),
        }
    }
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
}

impl LedgerDevRunner {
    fn new(fixture: &Fixture, improves: bool) -> Self {
        Self {
            store: fixture.store.clone(),
            worker: fixture.worker.clone(),
            improves,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DevRunner for LedgerDevRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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

/// A claimed fixture run to its first terminal record.
struct Run {
    fixture: Fixture,
    material: StepMaterial,
    claim: ConsolidationClaim,
    model: EditingModelPort,
    runner: LedgerDevRunner,
}

impl Run {
    async fn claimed(improves: bool) -> Self {
        let fixture = Fixture::new().await;
        let claim = fixture.claimed().await;
        let material = StepMaterial::new(&fixture).await;
        let runner = LedgerDevRunner::new(&fixture, improves);
        Self {
            fixture,
            material,
            claim,
            model: EditingModelPort::new("first"),
            runner,
        }
    }

    /// Runs `claim` through `run_consolidation` with the given ports.
    async fn run_with(
        &self,
        claim: &ConsolidationClaim,
        model: &dyn ConsolidationModelPort,
        journal: &dyn OptimizationJournal,
        sequence: u32,
        with_traces: bool,
    ) -> Result<ConsolidationRunRecord, Error> {
        MonitoringCoordinator::run_consolidation(
            &self.fixture.worker,
            &self.fixture.store,
            &claim.id,
            Some(model),
            Some(&self.runner),
            Some(journal),
            self.material
                .request(&self.fixture, &claim.id, sequence, with_traces),
        )
        .await
    }

    async fn run(&self, with_traces: bool) -> Result<ConsolidationRunRecord, Error> {
        let journal = self.fixture.journal();
        self.run_with(&self.claim, &self.model, &journal, 1, with_traces)
            .await
    }

    async fn proposal(&self) -> ConsolidationProposal {
        self.proposal_of(&self.claim).await
    }

    async fn proposal_of(&self, claim: &ConsolidationClaim) -> ConsolidationProposal {
        let mut session = self.fixture.store.session().await.unwrap();
        let proposal = session
            .need(
                &self.fixture.worker,
                "artifact",
                &format!("consolidation-proposal-{}", claim.id),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        proposal
    }

    async fn stage(
        &self,
        ctx: &Context,
        proposal: &ConsolidationProposal,
        candidate_id: &str,
    ) -> Result<StagedConsolidationCandidate, Error> {
        MonitoringCoordinator::stage_consolidation_candidate(
            ctx,
            &self.fixture.store,
            &proposal.id,
            candidate_id,
        )
        .await
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

async fn objects_of_kind(store: &Store, ctx: &Context, kind: &str) -> usize {
    let mut session = store.session().await.unwrap();
    let all: Vec<Value> = session.list(ctx, kind).await.unwrap();
    session.commit().await.unwrap();
    all.len()
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

/// Model port that revokes `source-2` while its first dispatch is in flight.
struct RevokingModelPort {
    store: Store,
    admin: Context,
    calls: AtomicUsize,
}

#[async_trait]
impl ModelPort for RevokingModelPort {
    async fn dispatch(&self, request: ModelRequest) -> evo_core::Result<ModelResponse> {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
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
                root_budget_id: ROOT.into(),
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
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        })
    }
}

/// Journal that fails its `fail_on_commit`-th commit, as a crash would.
struct FailingJournal {
    inner: StoreOptimizationJournal,
    commits: AtomicUsize,
    fail_on_commit: usize,
}

#[async_trait]
impl OptimizationJournal for FailingJournal {
    async fn commit(&self, fact: StageFact) -> evo_core::Result<()> {
        if self.commits.fetch_add(1, Ordering::SeqCst) == self.fail_on_commit {
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

/// Journal that reports a forged `StepCompleted` fact, to show the proposal only
/// accepts the terminal fact that really holds this candidate.
type Forge = fn(&mut StageFact);

struct ForgedTerminalJournal {
    inner: StoreOptimizationJournal,
    forge: Forge,
}

#[async_trait]
impl OptimizationJournal for ForgedTerminalJournal {
    async fn commit(&self, fact: StageFact) -> evo_core::Result<()> {
        self.inner.commit(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> evo_core::Result<Option<StageFact>> {
        let Some(mut fact) = self.inner.lookup(artifact_id).await? else {
            return Ok(None);
        };
        if fact.kind == StageFactKind::StepCompleted && fact.payload["status"] == "candidate" {
            (self.forge)(&mut fact);
            fact.output_digest = None;
            fact = fact.seal()?;
        }
        Ok(Some(fact))
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

fn dependents_contain(dependents: &[(String, String)], kind: &str, id: &str) -> bool {
    dependents
        .iter()
        .any(|(dependent_kind, dependent_id)| dependent_kind == kind && dependent_id == id)
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

// ---------------------------------------------------------------------------
// D4: pure pass/fail task pairing (development_cycle.v2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_pairs_follow_pass_fail_transitions_and_never_the_score() {
    let fixture = Fixture::new().await;
    let cycle = fixture
        .close_cycle(
            1,
            &[
                // Fails both times but the score rises: still persistent_fail.
                ResultSpec {
                    task_id: "persistent-fail",
                    parent_score: 100_000,
                    candidate_score: 400_000,
                    parent_passed: false,
                    candidate_passed: false,
                },
                // Passes then fails while the score rises: regressed.
                ResultSpec {
                    task_id: "regressed",
                    parent_score: 500_000,
                    candidate_score: 600_000,
                    parent_passed: true,
                    candidate_passed: false,
                },
                // Fails then passes while the score falls: improved.
                ResultSpec {
                    task_id: "improved",
                    parent_score: 500_000,
                    candidate_score: 100_000,
                    parent_passed: false,
                    candidate_passed: true,
                },
                // Passes both times while the score falls: stable_success.
                ResultSpec {
                    task_id: "stable-success",
                    parent_score: 900_000,
                    candidate_score: 800_000,
                    parent_passed: true,
                    candidate_passed: true,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(cycle.schema_version, "rsia.monitoring.development_cycle.v2");
    assert_eq!(cycle.schema_version, DEVELOPMENT_CYCLE_SCHEMA);
    let observed: Vec<_> = cycle
        .pairs
        .iter()
        .map(|pair| {
            (
                pair.task_id.as_str(),
                pair.parent_passed,
                pair.candidate_passed,
                pair.class,
                pair.parent_score_micros,
                pair.candidate_score_micros,
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            (
                "persistent-fail",
                false,
                false,
                ConsolidationClass::PersistentFail,
                100_000,
                400_000
            ),
            (
                "regressed",
                true,
                false,
                ConsolidationClass::Regressed,
                500_000,
                600_000
            ),
            (
                "improved",
                false,
                true,
                ConsolidationClass::Improved,
                500_000,
                100_000
            ),
            (
                "stable-success",
                true,
                true,
                ConsolidationClass::StableSuccess,
                900_000,
                800_000
            ),
        ]
    );
    assert!(cycle.has_contrast);
    // The stored record is the returned record, task ids included.
    assert_eq!(
        serde_json::to_value(&cycle).unwrap(),
        value(&fixture.store, &fixture.worker, "artifact", &cycle.id).await
    );
}

#[tokio::test]
async fn a_report_that_repeats_a_task_or_has_no_results_is_invalid() {
    let fixture = Fixture::new().await;
    let twice = ResultSpec {
        task_id: "task-a",
        parent_score: 500_000,
        candidate_score: 600_000,
        parent_passed: true,
        candidate_passed: true,
    };
    let duplicate = fixture
        .close_cycle(1, &[twice.clone(), twice.clone()])
        .await;
    assert!(
        matches!(&duplicate, Err(Error::Invalid(message)) if message.contains("repeats a task id")),
        "{duplicate:?}"
    );
    assert!(matches!(
        fixture.close_cycle(2, &[]).await,
        Err(Error::Invalid(_))
    ));
    let over_range = ResultSpec {
        candidate_score: 1_000_001,
        ..twice
    };
    assert!(matches!(
        fixture.close_cycle(3, &[over_range]).await,
        Err(Error::Invalid(_))
    ));
    // Nothing of the refused reports was recorded as a cycle.
    assert!(
        artifacts_with_schema(&fixture.store, &fixture.worker, DEVELOPMENT_CYCLE_SCHEMA)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn contrast_keeps_its_meaning_a_score_only_change_is_still_a_contrast() {
    // Score change only (both pass): contrast, although the class is stable_success.
    let scored = Fixture::new().await;
    let first = scored.close_cycle(1, &score_only_contrast()).await.unwrap();
    assert!(first.has_contrast);
    assert_eq!(first.pairs[0].class, ConsolidationClass::StableSuccess);
    scored.close_cycle(2, &no_contrast()).await.unwrap();
    let claim = scored.claim_generation().await;
    assert_eq!(claim.state, ConsolidationClaimState::Claimed);

    // Identical before and after in both cycles: no contrast, no dispatch.
    let flat = Fixture::new().await;
    for cycle in 1..=2 {
        let record = flat.close_cycle(cycle, &no_contrast()).await.unwrap();
        assert!(!record.has_contrast);
    }
    let ClaimOutcome::Claimed(claim) = MonitoringCoordinator::claim_consolidation(
        &flat.worker,
        &flat.store,
        &flat.scope_id().await,
    )
    .await
    .unwrap() else {
        panic!("generation one was not claimed")
    };
    assert_eq!(claim.state, ConsolidationClaimState::NoContrast);

    // A pass/fail difference at an unchanged score is a contrast too.
    let flipped = Fixture::new().await;
    let record = flipped
        .close_cycle(
            1,
            &[ResultSpec {
                task_id: "task-a",
                parent_score: 500_000,
                candidate_score: 500_000,
                parent_passed: false,
                candidate_passed: true,
            }],
        )
        .await
        .unwrap();
    assert!(record.has_contrast);
    assert_eq!(record.pairs[0].class, ConsolidationClass::Improved);
}

#[tokio::test]
async fn pre_v2_cycle_records_are_refused_explicitly_when_read() {
    let fixture = Fixture::new().await;
    fixture
        .close_cycle(1, &score_only_contrast())
        .await
        .unwrap();
    let second = fixture
        .close_cycle(2, &score_only_contrast())
        .await
        .unwrap();
    let scope_id = second.scope_id.clone();

    // The v1 shape: a score-biased `pair_classes` list, no task ids.
    let mut v1 = serde_json::to_value(&second).unwrap();
    v1["schema_version"] = json!("rsia.monitoring.development_cycle.v1");
    v1.as_object_mut().unwrap().remove("pairs");
    v1["pair_classes"] = json!(["improved"]);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.worker, "artifact", &second.id, "worker", &v1)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let claim =
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await;
    assert!(
        matches!(&claim, Err(Error::Invalid(message)) if message.contains("pre-v2 development cycle")),
        "{claim:?}"
    );
    // Re-closing the same report reads the stored cycle and is refused the same way.
    let reclose = MonitoringCoordinator::close_development_cycle(
        &fixture.worker,
        &fixture.store,
        fixture.cycle_request(second.report_fact_id.clone()),
    )
    .await;
    assert!(
        matches!(&reclose, Err(Error::Invalid(message)) if message.contains("pre-v2 development cycle")),
        "{reclose:?}"
    );
    // No claim was written for a scope with an unreadable cycle.
    assert!(
        artifacts_with_schema(
            &fixture.store,
            &fixture.worker,
            "rsia.monitoring.consolidation_claim.v1"
        )
        .await
        .is_empty()
    );

    // Restoring the v2 record restores the claim: the refusal was about that record.
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.worker, "artifact", &second.id, "worker", &second)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        MonitoringCoordinator::claim_consolidation(&fixture.worker, &fixture.store, &scope_id)
            .await
            .unwrap(),
        ClaimOutcome::Claimed(_)
    ));
}

#[tokio::test]
async fn a_stored_pair_class_must_still_equal_its_pass_fail_outcomes() {
    let fixture = Fixture::new().await;
    fixture
        .close_cycle(1, &score_only_contrast())
        .await
        .unwrap();
    let second = fixture
        .close_cycle(2, &score_only_contrast())
        .await
        .unwrap();
    // Both tasks passed, so a stored `improved` (the old score-biased answer) is false.
    let mut tampered = serde_json::to_value(&second).unwrap();
    tampered["pairs"][0]["class"] = json!("improved");
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.worker, "artifact", &second.id, "worker", &tampered)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let claim = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &second.scope_id,
    )
    .await;
    assert!(
        matches!(&claim, Err(Error::Conflict(message)) if message.contains("pair class")),
        "{claim:?}"
    );
}

// ---------------------------------------------------------------------------
// D2: the consolidation proposal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_candidate_outcome_freezes_a_proposal_with_its_fields_and_edges() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    let record = run.run(true).await.unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::Candidate);
    let proposal = run.proposal().await;
    assert_eq!(record.proposal_id.as_deref(), Some(proposal.id.as_str()));
    assert!(!record.budget_call_ids.is_empty());

    assert_eq!(
        proposal.id,
        format!("consolidation-proposal-{}", run.claim.id)
    );
    assert_eq!(proposal.schema_version, CONSOLIDATION_PROPOSAL_SCHEMA);
    assert_eq!(proposal.claim_id, run.claim.id);
    assert_eq!(proposal.scope_id, run.claim.scope_id);
    assert_eq!(proposal.cycle_ids, run.claim.cycle_ids);
    assert_eq!(proposal.sources, run.claim.sources);
    assert_eq!(proposal.revoke_watermark, 1);
    assert_eq!(proposal.parent_skill_digest, fixture.parent_skill_digest);
    assert_eq!(proposal.parent_bundle_digest, fixture.parent_bundle_digest);
    assert_eq!(
        proposal.environment_digest,
        fixture.environment.environment.environment_digest
    );
    assert_eq!(
        proposal.provenance,
        EnvironmentEvidenceScope::ProgramFixture
    );

    // The candidate: a complete bundle whose skill the edit really changed.
    assert_eq!(
        proposal.candidate_bundle.digest,
        proposal.candidate_bundle_digest
    );
    assert_eq!(
        proposal.candidate_skill_digest,
        skill_snapshot_digest(&proposal.candidate_bundle.skill).unwrap()
    );
    assert_ne!(
        proposal.candidate_skill_digest,
        proposal.parent_skill_digest
    );
    assert!(proposal.candidate_bundle.skill.content.contains("first]"));

    // pairs_digest is the fingerprint of the claim's two cycles' pairs.
    let mut session = fixture.store.session().await.unwrap();
    let mut pairs = Vec::new();
    for cycle_id in &run.claim.cycle_ids {
        let cycle: DevelopmentCycleRecord = session
            .need(&fixture.worker, "artifact", cycle_id)
            .await
            .unwrap();
        pairs.push((cycle.id, cycle.pairs));
    }
    session.commit().await.unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(proposal.pairs_digest, fingerprint(&pairs).unwrap());

    // The terminal fact is the journal's StepCompleted fact for this very step
    // and holds the same candidate and selection the proposal froze.
    let fact = fixture
        .journal()
        .lookup(&proposal.terminal_fact_id)
        .await
        .unwrap()
        .expect("terminal fact exists");
    assert_eq!(fact.kind, StageFactKind::StepCompleted);
    assert_eq!(fact.stage, OptimizationJournalStage::Merge);
    assert_eq!(fact.episode_id, run.claim.id);
    assert_eq!(fact.payload["status"], "candidate");
    assert_eq!(
        fact.payload["bundle"]["digest"],
        json!(proposal.candidate_bundle_digest)
    );
    let selection: DevelopmentSelection =
        serde_json::from_value(fact.payload["selection"].clone()).unwrap();
    assert_eq!(
        selection.decision,
        DevelopmentSelectionDecision::AcceptCandidate
    );
    assert_eq!(
        selection.candidate_bundle_digest,
        proposal.candidate_bundle_digest
    );
    assert_eq!(proposal.selection_digest, fingerprint(&selection).unwrap());

    // Dependency edges point at the claim, the terminal fact and every source run.
    let on_claim = dependents_of(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await;
    assert!(dependents_contain(&on_claim, "artifact", &proposal.id));
    assert!(dependents_contain(&on_claim, "artifact", &record.id));
    let on_fact = dependents_of(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &proposal.terminal_fact_id,
    )
    .await;
    assert!(dependents_contain(&on_fact, "artifact", &proposal.id));
    for source in ["source-1", "source-2"] {
        let on_source = dependents_of(&fixture.store, &fixture.worker, "run", source).await;
        assert!(dependents_contain(&on_source, "artifact", &proposal.id));
    }

    // The claim is terminal and the stored run record names the proposal.
    let claim: ConsolidationClaim = serde_json::from_value(
        value(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await,
    )
    .unwrap();
    assert_eq!(claim.state, ConsolidationClaimState::CompletedCandidate);
    let stored = value(&fixture.store, &fixture.worker, "artifact", &record.id).await;
    assert_eq!(stored["proposal_id"], json!(proposal.id));
    // A replay of the same request is the same record and writes no second proposal.
    assert_eq!(run.run(true).await.unwrap(), record);
    assert_eq!(
        artifacts_with_schema(
            &fixture.store,
            &fixture.worker,
            CONSOLIDATION_PROPOSAL_SCHEMA
        )
        .await
        .len(),
        1
    );
}

#[tokio::test]
async fn only_a_candidate_outcome_leaves_a_proposal() {
    // NoChange: no eligible reflection batch.
    let no_change = Run::claimed(true).await;
    let record = no_change.run(false).await.unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::NoChange);
    assert_eq!(record.proposal_id, None);
    // A record without a proposal keeps the pre-proposal JSON shape.
    let stored = value(
        &no_change.fixture.store,
        &no_change.fixture.worker,
        "artifact",
        &record.id,
    )
    .await;
    assert!(stored.get("proposal_id").is_none());
    assert_no_proposal(&no_change).await;

    // Rejected: the development run did not improve on the parent.
    let rejected = Run::claimed(false).await;
    let record = rejected.run(true).await.unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::Rejected);
    assert_eq!(record.proposal_id, None);
    assert_eq!(rejected.runner.calls.load(Ordering::SeqCst), 1);
    assert_no_proposal(&rejected).await;

    // BlockedBudget: a stopped root dispatches nothing.
    let blocked = Run::claimed(true).await;
    blocked
        .fixture
        .store
        .stop_root_budget(&blocked.fixture.admin, SCOPE, "stop for test", 50)
        .await
        .unwrap();
    let record = blocked.run(true).await.unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::BlockedBudget);
    assert_eq!(record.proposal_id, None);
    assert_eq!(blocked.model.calls.load(Ordering::SeqCst), 0);
    assert_no_proposal(&blocked).await;

    // Uncertain: the journal failed before the step could complete.
    let uncertain = Run::claimed(true).await;
    let failing = FailingJournal {
        inner: uncertain.fixture.journal(),
        commits: AtomicUsize::new(0),
        fail_on_commit: 2,
    };
    let record = uncertain
        .run_with(&uncertain.claim, &uncertain.model, &failing, 1, false)
        .await
        .unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::Uncertain);
    assert_eq!(record.proposal_id, None);
    assert_no_proposal(&uncertain).await;

    // Revoked: a source was revoked while the model call was in flight.
    let revoked = Run::claimed(true).await;
    let revoking = RevokingModelPort {
        store: revoked.fixture.store.clone(),
        admin: revoked.fixture.admin.clone(),
        calls: AtomicUsize::new(0),
    };
    let journal = revoked.fixture.journal();
    let record = revoked
        .run_with(&revoked.claim, &revoking, &journal, 1, true)
        .await
        .unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::Revoked);
    assert_eq!(record.proposal_id, None);
    assert_no_proposal(&revoked).await;
}

async fn assert_no_proposal(run: &Run) {
    assert!(
        artifacts_with_schema(
            &run.fixture.store,
            &run.fixture.worker,
            CONSOLIDATION_PROPOSAL_SCHEMA
        )
        .await
        .is_empty()
    );
}

#[test]
fn run_records_written_before_proposals_existed_still_deserialize() {
    let old = json!({
        "id": "consolidation-run-old",
        "schema_version": "rsia.monitoring.consolidation_run.v1",
        "claim_id": "consolidation-claim-old",
        "input_digest": null,
        "outcome": "no_change",
        "reason": "two completed development cycles contained no before/after contrast",
        "budget_call_ids": [],
    });
    let record: ConsolidationRunRecord = serde_json::from_value(old).unwrap();
    assert_eq!(record.proposal_id, None);
}

#[tokio::test]
async fn re_entry_after_a_crash_between_proposal_and_terminal_record_reuses_the_proposal() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    let first = run.run(true).await.unwrap();
    let proposal = run.proposal().await;
    let proposal_body = value(&fixture.store, &fixture.worker, "artifact", &proposal.id).await;
    assert_eq!(first.proposal_id.as_deref(), Some(proposal.id.as_str()));

    // The terminal record and the claim's terminal state are written by one
    // transaction, so a crash right after the proposal commit leaves exactly:
    // proposal present, no run record, claim still Running. Undo those two writes.
    let crash = || async {
        let mut session = fixture.store.session().await.unwrap();
        session
            .delete(&fixture.worker, "artifact", &first.id)
            .await
            .unwrap();
        let mut claim: ConsolidationClaim = session
            .need(&fixture.worker, "artifact", &run.claim.id)
            .await
            .unwrap();
        claim.state = ConsolidationClaimState::Running;
        session
            .put(&fixture.worker, "artifact", &claim.id, "worker", &claim)
            .await
            .unwrap();
        session.commit().await.unwrap();
    };
    crash().await;

    // Until the run completes, the proposal is not stageable.
    let early = run
        .stage(&fixture.worker, &proposal, "early-candidate")
        .await;
    assert!(matches!(early, Err(Error::Conflict(_))), "{early:?}");

    // Re-entry: the journal replays the Candidate, no port runs again, the same
    // proposal is found (not rewritten) and the terminal record names it.
    let model_calls = run.model.calls.load(Ordering::SeqCst);
    let runner_calls = run.runner.calls.load(Ordering::SeqCst);
    let again = run.run(true).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(run.model.calls.load(Ordering::SeqCst), model_calls);
    assert_eq!(run.runner.calls.load(Ordering::SeqCst), runner_calls);
    assert_eq!(
        artifacts_with_schema(
            &fixture.store,
            &fixture.worker,
            CONSOLIDATION_PROPOSAL_SCHEMA
        )
        .await
        .len(),
        1
    );
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &proposal.id).await,
        proposal_body
    );
    let claim: ConsolidationClaim = serde_json::from_value(
        value(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await,
    )
    .unwrap();
    assert_eq!(claim.state, ConsolidationClaimState::CompletedCandidate);
    run.stage(&fixture.worker, &proposal, "candidate-after-recovery")
        .await
        .unwrap();

    // Different content under the proposal's identity is a conflict, not an overwrite.
    crash().await;
    let mut altered = proposal_body.clone();
    altered["pairs_digest"] = json!(d("another pairing"));
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.worker,
            "artifact",
            &proposal.id,
            "worker",
            &altered,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let conflict = run.run(true).await;
    assert!(
        matches!(&conflict, Err(Error::Conflict(message)) if message.contains("immutable")),
        "{conflict:?}"
    );
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &proposal.id).await,
        altered
    );
}

#[tokio::test]
async fn a_terminal_fact_that_does_not_hold_the_candidate_writes_no_proposal() {
    let forgeries: [(&str, Forge); 3] = [
        ("another status", |fact| {
            fact.payload["status"] = json!("rejected");
        }),
        ("another bundle", |fact| {
            fact.payload["bundle"]["digest"] = json!(d("another bundle"));
        }),
        ("another selection", |fact| {
            fact.payload["selection"]["candidate_total_micros"] = json!(1);
        }),
    ];
    for (name, forge) in forgeries {
        let run = Run::claimed(true).await;
        let forged = ForgedTerminalJournal {
            inner: run.fixture.journal(),
            forge,
        };
        let refused = run.run_with(&run.claim, &run.model, &forged, 1, true).await;
        assert!(
            matches!(&refused, Err(Error::Conflict(message)) if message.contains("terminal fact")),
            "{name}: {refused:?}"
        );
        assert_no_proposal(&run).await;
        // The claim is still running and no terminal record exists.
        let claim: ConsolidationClaim = serde_json::from_value(
            value(
                &run.fixture.store,
                &run.fixture.worker,
                "artifact",
                &run.claim.id,
            )
            .await,
        )
        .unwrap();
        assert_eq!(claim.state, ConsolidationClaimState::Running, "{name}");
        // With the honest journal the very same request replays the Candidate
        // without dispatching again and completes normally.
        let calls = run.model.calls.load(Ordering::SeqCst);
        let record = run.run(true).await.unwrap();
        assert_eq!(record.outcome, ConsolidationRunOutcome::Candidate, "{name}");
        assert_eq!(run.model.calls.load(Ordering::SeqCst), calls, "{name}");
        assert_eq!(record.proposal_id, Some(run.proposal().await.id), "{name}");
    }
}

// ---------------------------------------------------------------------------
// The staging bridge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn staging_writes_only_a_release_candidate_and_is_idempotent_and_conflict_checked() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    run.run(true).await.unwrap();
    let proposal = run.proposal().await;

    let staged = run
        .stage(&fixture.worker, &proposal, "candidate-1")
        .await
        .unwrap();
    let candidate: &ReleaseCandidateRecord = &staged.candidate;
    assert_eq!(staged.proposal_id, proposal.id);
    // The proposal's provenance is visible on the staging result and on the proposal.
    assert_eq!(staged.provenance, EnvironmentEvidenceScope::ProgramFixture);
    assert_eq!(
        proposal.provenance,
        EnvironmentEvidenceScope::ProgramFixture
    );
    assert_eq!(candidate.id, "candidate-1");
    assert_eq!(candidate.bundle_digest, proposal.candidate_bundle_digest);
    assert_eq!(candidate.environment_digest, proposal.environment_digest);
    assert_eq!(candidate.profile_id, "profile");
    assert_eq!(candidate.proposer_actor, "worker");
    assert_eq!(candidate.revoke_watermark, proposal.revoke_watermark);
    assert_eq!(
        candidate
            .sources
            .iter()
            .map(|source| (
                source.kind.as_str(),
                source.id.as_str(),
                source.content_digest.as_str()
            ))
            .collect::<Vec<_>>(),
        proposal
            .sources
            .iter()
            .map(|source| ("run", source.id.as_str(), source.content_digest.as_str()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        serde_json::to_value(candidate).unwrap(),
        value(&fixture.store, &fixture.worker, "artifact", "candidate-1").await
    );

    // Only a candidate exists: no release, no pointer, Active is untouched.
    assert_eq!(
        objects_of_kind(&fixture.store, &fixture.worker, "release").await,
        0
    );
    assert_eq!(
        objects_of_kind(&fixture.store, &fixture.worker, "pointer").await,
        0
    );
    let mut session = fixture.store.session().await.unwrap();
    assert!(
        session
            .get::<Value>(&fixture.worker, "pointer", "profile-pointer-profile")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
    // The candidate depends on its proposal and on the source runs.
    let on_proposal =
        dependents_of(&fixture.store, &fixture.worker, "artifact", &proposal.id).await;
    assert!(dependents_contain(&on_proposal, "artifact", "candidate-1"));
    let on_source = dependents_of(&fixture.store, &fixture.worker, "run", "source-1").await;
    assert!(dependents_contain(&on_source, "artifact", "candidate-1"));

    // Same candidate id, same content: idempotent.
    let again = run
        .stage(&fixture.worker, &proposal, "candidate-1")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&again.candidate).unwrap(),
        serde_json::to_value(candidate).unwrap()
    );
    assert_eq!(
        artifacts_with_schema(&fixture.store, &fixture.worker, "rsia.release_candidate.v1")
            .await
            .len(),
        1
    );
    // Same candidate id, different content (another proposer): a conflict.
    let other = run.stage(&fixture.admin, &proposal, "candidate-1").await;
    assert!(matches!(other, Err(Error::Conflict(_))), "{other:?}");

    // A second generation with a different edit is different content.
    for cycle in 3..=4 {
        fixture
            .close_cycle(cycle, &score_only_contrast())
            .await
            .unwrap();
    }
    let second_claim = fixture.claim_generation().await;
    assert_eq!(second_claim.generation, 2);
    let second_model = EditingModelPort::new("second");
    let journal = fixture.journal();
    let second_record = run
        .run_with(&second_claim, &second_model, &journal, 2, true)
        .await
        .unwrap();
    assert_eq!(
        second_record.outcome,
        ConsolidationRunOutcome::Candidate,
        "{second_record:?}"
    );
    let second_proposal = run.proposal_of(&second_claim).await;
    assert_ne!(
        second_proposal.candidate_bundle_digest,
        proposal.candidate_bundle_digest
    );
    let conflict = run
        .stage(&fixture.worker, &second_proposal, "candidate-1")
        .await;
    assert!(matches!(conflict, Err(Error::Conflict(_))), "{conflict:?}");
    let second_staged = run
        .stage(&fixture.worker, &second_proposal, "candidate-2")
        .await
        .unwrap();
    assert_eq!(
        second_staged.candidate.bundle_digest,
        second_proposal.candidate_bundle_digest
    );
    assert_eq!(
        artifacts_with_schema(&fixture.store, &fixture.worker, "rsia.release_candidate.v1")
            .await
            .len(),
        2
    );

    // A staged candidate enters the ordinary approval gates; nothing here grants Active.
    let other_admin = context("other-admin", Role::Admin);
    assert!(matches!(
        ReleaseStore::approve_verified(
            &other_admin,
            &fixture.store,
            "candidate-1",
            "no-such-report"
        )
        .await,
        Err(Error::NotFound)
    ));
    let proposer_as_admin = Context::new(TENANT, "worker", Role::Admin).unwrap();
    assert!(matches!(
        ReleaseStore::approve_verified(
            &proposer_as_admin,
            &fixture.store,
            "candidate-1",
            "no-such-report"
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        objects_of_kind(&fixture.store, &fixture.worker, "release").await,
        0
    );
    assert_eq!(
        objects_of_kind(&fixture.store, &fixture.worker, "pointer").await,
        0
    );
    // Only Worker and Admin may stage.
    let outsider = context("host", Role::Host);
    assert!(matches!(
        run.stage(&outsider, &proposal, "candidate-x").await,
        Err(Error::Forbidden)
    ));

    // Revoking a source blocks staging before any cleanup has run.
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-2".into(),
        },
        "source two revoked",
        700,
    )
    .await
    .unwrap();
    let refused = run.stage(&fixture.worker, &proposal, "candidate-3").await;
    assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    let mut session = fixture.store.session().await.unwrap();
    assert!(
        session
            .get::<Value>(&fixture.worker, "artifact", "candidate-3")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn a_proposal_the_terminal_record_does_not_name_is_not_stageable() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    let record = run.run(true).await.unwrap();
    let proposal = run.proposal().await;
    let forge = |outcome: ConsolidationRunOutcome,
                 state: ConsolidationClaimState,
                 proposal_id: Option<String>| {
        let record = ConsolidationRunRecord {
            outcome,
            proposal_id,
            ..record.clone()
        };
        async move {
            let mut session = fixture.store.session().await.unwrap();
            session
                .put(&fixture.worker, "artifact", &record.id, "worker", &record)
                .await
                .unwrap();
            let mut claim: ConsolidationClaim = session
                .need(&fixture.worker, "artifact", &record.claim_id)
                .await
                .unwrap();
            claim.state = state;
            session
                .put(&fixture.worker, "artifact", &claim.id, "worker", &claim)
                .await
                .unwrap();
            session.commit().await.unwrap();
        }
    };
    // A concurrent run that ended Uncertain left this proposal behind.
    forge(
        ConsolidationRunOutcome::Uncertain,
        ConsolidationClaimState::CompletedUncertain,
        None,
    )
    .await;
    let uncertain = run
        .stage(&fixture.worker, &proposal, "orphan-candidate")
        .await;
    assert!(
        matches!(&uncertain, Err(Error::Conflict(message)) if message.contains("recorded candidate outcome")),
        "{uncertain:?}"
    );
    // A Candidate record that names another proposal does not vouch for this one.
    forge(
        ConsolidationRunOutcome::Candidate,
        ConsolidationClaimState::CompletedCandidate,
        Some("consolidation-proposal-another".into()),
    )
    .await;
    let other = run
        .stage(&fixture.worker, &proposal, "orphan-candidate")
        .await;
    assert!(
        matches!(&other, Err(Error::Conflict(message)) if message.contains("recorded candidate outcome")),
        "{other:?}"
    );
    // The genuine record restores it.
    forge(
        ConsolidationRunOutcome::Candidate,
        ConsolidationClaimState::CompletedCandidate,
        Some(proposal.id.clone()),
    )
    .await;
    run.stage(&fixture.worker, &proposal, "orphan-candidate")
        .await
        .unwrap();
}

#[tokio::test]
async fn an_environment_drift_makes_the_proposal_unstageable() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    run.run(true).await.unwrap();
    let proposal = run.proposal().await;
    let current = record_drifted_environment(fixture).await;
    MonitoringCoordinator::record_environment_drift(
        &fixture.admin,
        &fixture.store,
        &fixture.environment.environment.id,
        &current.environment.id,
        &proposal.scope_id,
        "model deployment changed",
    )
    .await
    .unwrap();
    let refused = run
        .stage(&fixture.worker, &proposal, "drifted-candidate")
        .await;
    assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    assert_eq!(
        artifacts_with_schema(&fixture.store, &fixture.worker, "rsia.release_candidate.v1")
            .await
            .len(),
        0
    );
}

async fn record_drifted_environment(fixture: &Fixture) -> RecordedEnvironment {
    prepare_host_run(
        &fixture.host,
        &fixture.store,
        "host-run-2",
        "model-v2",
        "task-input-2",
    )
    .await;
    MonitoringCoordinator::record_environment_from_run(
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
    .unwrap()
}

// ---------------------------------------------------------------------------
// D1: the revocation-cleanup closure
// ---------------------------------------------------------------------------

fn assert_redacted(body: &Value, original_schema: &str) {
    assert_eq!(body["schema_version"], REDACTED, "{body}");
    assert_eq!(body["state"], "source_revoked", "{body}");
    assert_eq!(body["original_schema"], original_schema, "{body}");
}

#[tokio::test]
async fn revoking_a_source_cleans_the_monitoring_closure_to_complete() {
    let run = Run::claimed(true).await;
    let fixture = &run.fixture;
    let record = run.run(true).await.unwrap();
    let proposal = run.proposal().await;
    let staged = run
        .stage(&fixture.worker, &proposal, "candidate-1")
        .await
        .unwrap();

    // Environment identities, an observation and a drift fact hang off the Host run
    // snapshot. The snapshot of a run served by a release built from `source-1`
    // depends on it through release -> candidate -> run; that chain needs a formal
    // approval, so the dependency is written directly.
    let current = record_drifted_environment(fixture).await;
    let drift = MonitoringCoordinator::record_environment_drift(
        &fixture.admin,
        &fixture.store,
        &fixture.environment.environment.id,
        &current.environment.id,
        &run.claim.scope_id,
        "model deployment changed",
    )
    .await
    .unwrap();
    let mut session = fixture.store.session().await.unwrap();
    session
        .put_edge(
            &fixture.admin,
            "artifact",
            "run-snapshot-host-run-1",
            "run",
            "source-1",
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let (binding_ids, report_fact_ids) = {
        let mut bindings = Vec::new();
        let mut facts = Vec::new();
        let mut session = fixture.store.session().await.unwrap();
        for cycle_id in &run.claim.cycle_ids {
            let cycle: DevelopmentCycleRecord = session
                .need(&fixture.worker, "artifact", cycle_id)
                .await
                .unwrap();
            bindings.push(format!(
                "development-report-binding-{}",
                &fingerprint(&cycle.report_fact_id).unwrap()[..32]
            ));
            facts.push(cycle.report_fact_id);
        }
        session.commit().await.unwrap();
        (bindings, facts)
    };
    let scope_before = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &run.claim.scope_id,
    )
    .await;
    let run_before = value(&fixture.store, &fixture.worker, "artifact", &record.id).await;
    let drift_before = value(&fixture.store, &fixture.worker, "artifact", &drift.id).await;
    let other_environment_before = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &current.environment.id,
    )
    .await;
    assert_eq!(drift_before["schema_version"], ENVIRONMENT_DRIFT_SCHEMA);

    let status = revoke_and_clean(fixture, "source-1").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    // Redacted: everything derived from the revoked source.
    for cycle_id in &run.claim.cycle_ids {
        let body = value(&fixture.store, &fixture.worker, "artifact", cycle_id).await;
        assert_redacted(&body, DEVELOPMENT_CYCLE_SCHEMA);
    }
    for binding_id in &binding_ids {
        let body = value(&fixture.store, &fixture.worker, "artifact", binding_id).await;
        assert_redacted(&body, "rsia.monitoring.development_report_binding.v1");
    }
    let claim_body = value(&fixture.store, &fixture.worker, "artifact", &run.claim.id).await;
    assert_redacted(&claim_body, "rsia.monitoring.consolidation_claim.v1");
    let proposal_body = value(&fixture.store, &fixture.worker, "artifact", &proposal.id).await;
    assert_redacted(&proposal_body, CONSOLIDATION_PROPOSAL_SCHEMA);
    // The candidate bundle is gone; the provenance and the watermark remain as metadata.
    assert!(!proposal_body.to_string().contains("first]"));
    assert_eq!(proposal_body["metadata"]["provenance"], "program_fixture");
    assert_eq!(
        proposal_body["metadata"]["environment_digest"],
        json!(proposal.environment_digest)
    );
    let candidate_body = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &staged.candidate.id,
    )
    .await;
    assert_redacted(&candidate_body, "rsia.release_candidate.v1");
    let terminal_body = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &proposal.terminal_fact_id,
    )
    .await;
    assert_redacted(&terminal_body, "rsia.optimization.stage_fact.v1");
    let environment_body = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &fixture.environment.environment.id,
    )
    .await;
    assert_redacted(&environment_body, "rsia.monitoring.environment.v1");
    let observation_body = value(
        &fixture.store,
        &fixture.worker,
        "artifact",
        &fixture.environment.observation.id,
    )
    .await;
    assert_redacted(&observation_body, "rsia.monitoring.observation.v1");

    // Preserved: management, accounting and result facts, byte for byte.
    assert_eq!(
        value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &run.claim.scope_id
        )
        .await,
        scope_before
    );
    let run_after = value(&fixture.store, &fixture.worker, "artifact", &record.id).await;
    assert_eq!(run_after, run_before);
    assert_eq!(run_after["proposal_id"], json!(proposal.id));
    assert_eq!(
        value(&fixture.store, &fixture.worker, "artifact", &drift.id).await,
        drift_before
    );
    // Content the revoked source never reached is not touched.
    assert_eq!(
        value(
            &fixture.store,
            &fixture.worker,
            "artifact",
            &current.environment.id
        )
        .await,
        other_environment_before
    );
    // The spend the run reconciles is still in the ledger.
    assert!(!record.budget_call_ids.is_empty());
    for call_id in &record.budget_call_ids {
        let call = fixture
            .store
            .budget_call(&fixture.worker, SCOPE, call_id)
            .await
            .unwrap()
            .expect("budget call survives the revocation");
        assert!(call.usage_record_id.is_some());
    }
    let root = fixture
        .store
        .root_budget(&fixture.worker, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 10);

    // Fail closed afterwards: the generation cannot be claimed or executed again,
    // the redacted claim cannot be completed, the proposal cannot be staged.
    let reclaim = MonitoringCoordinator::claim_consolidation(
        &fixture.worker,
        &fixture.store,
        &run.claim.scope_id,
    )
    .await;
    assert!(matches!(reclaim, Err(Error::Conflict(_))), "{reclaim:?}");
    let rerun = run.run(true).await;
    assert!(
        matches!(&rerun, Err(Error::Conflict(message)) if message.contains("redacted")),
        "{rerun:?}"
    );
    let no_contrast =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &run.claim.id)
            .await;
    assert!(
        matches!(&no_contrast, Err(Error::Conflict(message)) if message.contains("redacted")),
        "{no_contrast:?}"
    );
    let restage = run
        .stage(&fixture.worker, &proposal, "candidate-after-cleanup")
        .await;
    assert!(
        matches!(&restage, Err(Error::Conflict(message)) if message.contains("redacted")),
        "{restage:?}"
    );
    // Nor can an already closed cycle be re-closed: derivation is blocked too.
    let late = MonitoringCoordinator::close_development_cycle(
        &fixture.worker,
        &fixture.store,
        fixture.cycle_request(report_fact_ids[0].clone()),
    )
    .await;
    assert!(matches!(late, Err(Error::Conflict(_))), "{late:?}");
}

#[tokio::test]
async fn an_unknown_monitoring_schema_still_fails_the_cleanup_job_closed() {
    let fixture = Fixture::new().await;
    let claim = fixture.claimed().await;
    assert_eq!(claim.generation, 1);
    // A monitoring-looking object nobody classified, hanging off a revoked run.
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.worker,
            "artifact",
            "forged-monitoring-object",
            "worker",
            &json!({"id":"forged-monitoring-object","schema_version":"rsia.monitoring.unknown.v1"}),
        )
        .await
        .unwrap();
    session
        .put_edge(
            &fixture.worker,
            "artifact",
            "forged-monitoring-object",
            "run",
            "source-1",
        )
        .await
        .unwrap();
    session.commit().await.unwrap();

    let status = revoke_and_clean(&fixture, "source-1").await;
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    let error = status.last_error.expect("a failed job names its cause");
    assert!(error.contains("blocked_unknown_scope"), "{error}");
    assert!(error.contains("rsia.monitoring.unknown.v1"), "{error}");
    // The job does not claim completion on a further step either.
    let again =
        LifecycleStore::cleanup_step(&fixture.admin, &fixture.store, &status.job_id, 8, 900)
            .await
            .unwrap();
    assert_eq!(again.state, CleanupState::Failed);
}
