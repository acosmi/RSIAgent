//! E09 same-task comparison practice (v4.2 §8.5, V093, V094.b): K=1 default,
//! Admin-registered K=3, `PracticeAttemptSetV1`, and the guarantee that an
//! attempt is never a development cycle or an independent sample. Real SQLite
//! store, the registered pure-function runner, no model, no provider, zero
//! monetary cost. The contrast path needs runners whose scores differ between
//! attempts; the deterministic registered runner cannot produce that, so
//! `ContrastRunner` writes honest E03 receipts with chosen outputs.

use async_trait::async_trait;
use evo_core::curriculum::{CurriculumControlProfileV1, LearnerStateV2};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::strategy::PracticePlan;
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::curriculum::{
    CurriculumSourceArtifactV1, CurriculumSourceKindV1, DevelopmentCycleReceiptV1,
    PersistentCurriculumCoordinator, curriculum_state_storage_id,
};
use evo_engine::curriculum_profiles::{
    RegisteredPureFunctionProfileV1, registered_profile_source_bodies,
};
use evo_engine::development::{
    DEVELOPMENT_RUN_RECEIPT_SCHEMA, DevelopmentControlV1, DevelopmentCostState,
    DevelopmentEvidenceScope, DevelopmentExecutionReceiptV1, DevelopmentRunReceiptV1,
    DevelopmentSide, DevelopmentTaskSpecV1, EXECUTION_RECEIPT_KIND, ExecutionReceiptIssueRequest,
    GraderReceiptIssueRequest, RUN_RECEIPT_KIND, RegisteredDevelopmentRunner,
    RegisteredExecutionRequestV1, RegisteredTargetInputV1, execution_budget_call_id,
    execution_request_digest, issue_execution_receipt, issue_grader_receipt,
    load_development_control, load_execution_receipt, register_development_control,
    registered_runner_digest, run_receipt_id, storage_id,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentRunReport, DevelopmentRunRequest,
    OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournal, OptimizationJournalStage,
    PairedTaskResult, StageDependency, StageFact, StageFactKind, StoreOptimizationJournal,
    verify_development_observation,
};
use evo_engine::practice::{
    PRACTICE_ATTEMPT_SET_SCHEMA, PRACTICE_REGISTRATION_SCHEMA, PracticeAttemptSetV1,
    PracticeOutcomeV1, PracticeRegistrationV1, PracticeRunRequest, load_practice_attempt_set,
    load_practice_registration, practice_attempt_request, practice_attempt_request_id,
    practice_attempt_set_storage_id, practice_registration_storage_id, register_practice,
    run_practice_set, select_contrast,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::CleanupState;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    }
}

fn authority(id: &str, family: &str) -> StoredTraceAuthority {
    let body = format!("trusted-run-{id}");
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: family.into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: family.into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.clone(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
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

fn control(id: &str, scope: DevelopmentEvidenceScope) -> DevelopmentControlV1 {
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = grader_spec();
    let tasks = tasks();
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: id.into(),
        namespace: NAMESPACE.into(),
        billing_scope: SCOPE.into(),
        root_budget_id: "dev-root".into(),
        executor_actor: "dev-executor".into(),
        grader_actor: "dev-grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "dev-manifest".into(),
        manifest_digest: String::new(),
        tasks,
        environment_digest: d("environment"),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        source_ids: vec!["run-a".into(), "run-b".into()],
        evidence_scope: scope,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

/// A control with three tasks: E03 itself does not cap the task count; the
/// two-task limit is the practice plan's.
fn three_task_control(id: &str) -> DevelopmentControlV1 {
    let mut control = control(
        id,
        DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
    );
    control.tasks.push(
        DevelopmentTaskSpecV1::registered(
            "task-c",
            "family-c",
            RegisteredTargetInputV1::ClampI64 {
                value: 9,
                min: 0,
                max: 4,
            },
        )
        .unwrap(),
    );
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

fn build_request(
    control: &DevelopmentControlV1,
    request_id: &str,
    episode_id: &str,
) -> DevelopmentRunRequest {
    DevelopmentRunRequest {
        request_id: request_id.into(),
        namespace: NAMESPACE.into(),
        purpose: Purpose::Development,
        episode_id: episode_id.into(),
        step: 1,
        attempt: 1,
        manifest: control.manifest().unwrap(),
        parent_bundle_digest: d("parent-bundle"),
        candidate_bundle_digest: d("candidate-bundle"),
        environment_digest: control.environment_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        rules_digest: control.rules_digest.clone(),
        tools_digest: control.tools_digest.clone(),
        revoke_watermark: 1,
        idempotency_key: format!("{request_id}-idem"),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    executor: Context,
    grader: Context,
    control: DevelopmentControlV1,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("practice.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
        let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
        let executor = Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap();
        let grader = Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap();
        for (id, family) in [("run-a", "family-a"), ("run-b", "family-b")] {
            store_trace_authority(&store, &host, &authority(id, family))
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, "initial-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "dev-root".into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "fixture-only".into(),
                    authorization_receipt_digest: d("admin-budget-receipt"),
                    per_call_cap_micros: 10,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let control = control(
            CONTROL_ID,
            DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        );
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        Self {
            _dir: dir,
            store,
            admin,
            executor,
            grader,
            control,
        }
    }

    fn runner(&self) -> RegisteredDevelopmentRunner {
        RegisteredDevelopmentRunner::with_clock(
            self.store.clone(),
            self.executor.clone(),
            self.grader.clone(),
            CONTROL_ID,
            600,
            Arc::new(|| 100),
        )
        .unwrap()
    }

    fn counting(&self) -> CountingRunner {
        CountingRunner {
            inner: self.runner(),
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            fail: None,
        }
    }

    fn request(&self) -> DevelopmentRunRequest {
        build_request(&self.control, "dev-request-1", "episode-1")
    }

    async fn group_calls(&self, episode_id: &str) -> Vec<evo_storage::budget::BudgetCallRecord> {
        let mut session = self.store.session().await.unwrap();
        let calls = session
            .budget_calls_for_group(&self.admin, SCOPE, episode_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        calls
    }

    async fn raw_artifact(&self, id: &str) -> serde_json::Value {
        let mut session = self.store.session().await.unwrap();
        let value = session
            .need::<serde_json::Value>(&self.admin, "artifact", id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        value
    }

    async fn artifacts(&self) -> Vec<serde_json::Value> {
        let mut session = self.store.session().await.unwrap();
        let values = session
            .list::<serde_json::Value>(&self.admin, "artifact")
            .await
            .unwrap();
        session.commit().await.unwrap();
        values
    }

    /// Commits the request/observation pair the way the E03 step and its
    /// recovery journal would: with run, watermark, and artifact extras.
    async fn commit_facts(
        &self,
        request: &DevelopmentRunRequest,
        report: &DevelopmentRunReport,
    ) -> (String, String) {
        let journal =
            StoreOptimizationJournal::new(self.store.clone(), self.admin.clone(), "admin").unwrap();
        let extras = vec![
            StageDependency {
                kind: "run".into(),
                id: "run-a".into(),
            },
            StageDependency {
                kind: "run".into(),
                id: "run-b".into(),
            },
            StageDependency {
                kind: "revoke_watermark".into(),
                id: "1".into(),
            },
            StageDependency {
                kind: "artifact".into(),
                id: "optgrant-fixture".into(),
            },
            StageDependency {
                kind: "artifact".into(),
                id: "optstage-step-fixture".into(),
            },
        ];
        let request_fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: NAMESPACE.into(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentRequestPrepared,
            request_id: request.request_id.clone(),
            input_digest: request.manifest.digest.clone(),
            output_digest: None,
            dependencies: extras.clone(),
            payload: serde_json::to_value(request).unwrap(),
        }
        .seal()
        .unwrap();
        let observed_fact = observed_fact(request, report, extras);
        journal.commit(request_fact.clone()).await.unwrap();
        journal.commit(observed_fact.clone()).await.unwrap();
        (request_fact.artifact_id, observed_fact.artifact_id)
    }
}

fn observed_fact(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
    extras: Vec<StageDependency>,
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
    dependencies.extend(evo_engine::development::typed_receipt_closure(report).unwrap());
    dependencies.extend(extras);
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: NAMESPACE.into(),
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

/// Wraps the registered runner: counts every call, records the request and can
/// fail one attempt without delegating (an unknown outcome, an exhausted
/// budget or another error).
struct CountingRunner {
    inner: RegisteredDevelopmentRunner,
    calls: AtomicUsize,
    seen: Mutex<Vec<(String, u32)>>,
    fail: Option<(u32, fn() -> Error)>,
}

impl CountingRunner {
    fn failing(mut self, attempt: u32, error: fn() -> Error) -> Self {
        self.fail = Some((attempt, error));
        self
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn seen_attempts(&self) -> Vec<u32> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, attempt)| *attempt)
            .collect()
    }
}

#[async_trait]
impl DevRunner for CountingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push((request.request_id.clone(), request.attempt));
        if let Some((attempt, error)) = self.fail
            && request.attempt == attempt
        {
            return Err(error());
        }
        self.inner.run(request).await
    }
}

/// Wraps the registered runner and forges its report after an honest run.
struct ForgingRunner {
    inner: RegisteredDevelopmentRunner,
    forge: fn(&mut DevelopmentRunReport),
    calls: AtomicUsize,
}

#[async_trait]
impl DevRunner for ForgingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut report = self.inner.run(request).await?;
        (self.forge)(&mut report);
        Ok(report)
    }
}

/// Wraps the registered runner and, just before delegating attempt `at`, either
/// revokes a source run or bumps the namespace watermark: the world changes
/// while a set is running.
struct DisturbingRunner {
    inner: RegisteredDevelopmentRunner,
    store: Store,
    admin: Context,
    at: u32,
    revoke: bool,
    calls: AtomicUsize,
}

#[async_trait]
impl DevRunner for DisturbingRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.attempt == self.at {
            if self.revoke {
                LifecycleCoordinator::revoke_source(
                    &self.admin,
                    &self.store,
                    "run-a",
                    "privacy",
                    200,
                )
                .await?;
            } else {
                let mut session = self.store.session().await?;
                session
                    .bump_watermark(&self.admin, "unrelated-bump")
                    .await?;
                session.commit().await?;
            }
        }
        self.inner.run(request).await
    }
}

/// A runner whose candidate output depends on (attempt, task). It issues
/// honest E03 evidence through the public issuing functions (budget row,
/// settlement, receipts, run receipt) so that the practice layer verifies it
/// exactly like a registered runner's; only the outputs, and so the scores,
/// differ between attempts. The deterministic registered runner cannot do that.
struct ContrastRunner {
    store: Store,
    executor: Context,
    grader: Context,
    correct: fn(u32, &str) -> bool,
    calls: AtomicUsize,
}

impl ContrastRunner {
    fn new(fixture: &Fixture, correct: fn(u32, &str) -> bool) -> Self {
        Self {
            store: fixture.store.clone(),
            executor: fixture.executor.clone(),
            grader: fixture.grader.clone(),
            correct,
            calls: AtomicUsize::new(0),
        }
    }

    async fn issue_side(
        &self,
        control: &DevelopmentControlV1,
        request: &DevelopmentRunRequest,
        task: &DevelopmentTaskSpecV1,
        side: DevelopmentSide,
        output: &str,
    ) -> evo_core::Result<DevelopmentExecutionReceiptV1> {
        let bundle_digest = match side {
            DevelopmentSide::Parent => request.parent_bundle_digest.clone(),
            DevelopmentSide::Candidate => request.candidate_bundle_digest.clone(),
        };
        let request_digest = execution_request_digest(control, request, task, side)?;
        let call_id = execution_budget_call_id(&request.request_id, &task.task_id, side)?;
        let stored = RegisteredExecutionRequestV1 {
            schema_version: REGISTERED_EXECUTION_REQUEST_SCHEMA.into(),
            control_id: control.id.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            task_id: task.task_id.clone(),
            side,
            input: task.input,
            input_digest: task.input_digest.clone(),
            bundle_digest,
            environment_digest: request.environment_digest.clone(),
            request_digest: request_digest.clone(),
            target_id: task.target_id.clone(),
            target_digest: control.target_digest.clone(),
            runner_digest: control.runner_digest.clone(),
            source_ids: control.source_ids.clone(),
        };
        let call = self
            .store
            .reserve_budget_call_with_sources(
                &self.executor,
                &BudgetCallReservation {
                    billing_scope: control.billing_scope.clone(),
                    call_id: call_id.clone(),
                    dispatch_group_id: request.episode_id.clone(),
                    stage: BudgetStage::DevelopmentExecution,
                    actual_input_digest: request_digest.clone(),
                    request_artifact: Some(BudgetArtifact::from_serializable(
                        REGISTERED_EXECUTION_REQUEST_SCHEMA,
                        &stored,
                    )?),
                    max_cost_micros: 1,
                    lease_token: format!("lease-{}", &fingerprint(&call_id)?[..32]),
                    lease_until: 700,
                    now: 100,
                },
                &control.source_ids,
            )
            .await?;
        let fence = BudgetCallFence {
            billing_scope: control.billing_scope.clone(),
            call_id: call_id.clone(),
            actual_input_digest: request_digest.clone(),
            lease_token: call.lease_token.clone(),
            lease_epoch: call.lease_epoch,
            now: 101,
        };
        let dispatch_id = self
            .store
            .begin_budget_dispatch(&self.executor, &fence)
            .await?
            .call
            .dispatch_id
            .ok_or(Error::Internal)?;
        let output_digest = hash(output.as_bytes());
        self.store
            .settle_registered_execution_call(
                &self.executor,
                &fence,
                &UsageCharge {
                    amount_micros: 0,
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    provider_request_id: format!("registered-pure-function:{dispatch_id}"),
                    usage_record_id: format!("usage-{}", &fingerprint(&call_id)?[..32]),
                    output_digest: output_digest.clone(),
                },
                &RegisteredExecutionSettlement {
                    schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
                    provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
                    call_id: call_id.clone(),
                    dispatch_id,
                    request_digest,
                    output_digest,
                    target_id: task.target_id.clone(),
                    target_digest: control.target_digest.clone(),
                    runner_digest: control.runner_digest.clone(),
                },
            )
            .await?;
        issue_execution_receipt(
            &self.executor,
            &self.store,
            ExecutionReceiptIssueRequest {
                control_id: control.id.clone(),
                request: request.clone(),
                task_id: task.task_id.clone(),
                side,
                output_utf8: output.into(),
                budget_call_id: call_id,
                issued_at_unix_seconds: 100,
            },
        )
        .await
    }
}

#[async_trait]
impl DevRunner for ContrastRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> evo_core::Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let control = load_development_control(&self.store, &self.executor, CONTROL_ID).await?;
        let mut results = Vec::new();
        let mut execution_ids = Vec::new();
        let mut grader_ids = Vec::new();
        let mut call_ids = Vec::new();
        let mut usage_ids = Vec::new();
        for manifest_task in &request.manifest.tasks {
            let task = control.task(&manifest_task.id)?;
            let output = if (self.correct)(request.attempt, &task.task_id) {
                format!("{{\"answer\":{}}}", task.expected_answer_json)
            } else {
                "{\"answer\":{\"clamped\":12345}}".to_string()
            };
            let parent = self
                .issue_side(&control, &request, task, DevelopmentSide::Parent, &output)
                .await?;
            let candidate = self
                .issue_side(
                    &control,
                    &request,
                    task,
                    DevelopmentSide::Candidate,
                    &output,
                )
                .await?;
            let grader = issue_grader_receipt(
                &self.grader,
                &self.store,
                GraderReceiptIssueRequest {
                    control_id: control.id.clone(),
                    request: request.clone(),
                    task_id: task.task_id.clone(),
                    parent_execution_receipt_id: parent.receipt_id.clone(),
                    candidate_execution_receipt_id: candidate.receipt_id.clone(),
                    scoring_budget_call_id: None,
                    issued_at_unix_seconds: 100,
                },
            )
            .await?;
            results.push(PairedTaskResult {
                task_id: task.task_id.clone(),
                parent_score_micros: grader.parent_score_micros,
                candidate_score_micros: grader.candidate_score_micros,
                parent_passed: grader.parent_passed,
                candidate_passed: grader.candidate_passed,
                parent_execution_id: parent.receipt_id.clone(),
                candidate_execution_id: candidate.receipt_id.clone(),
                grader_receipt_digest: fingerprint(&grader)?,
            });
            for receipt in [&parent, &candidate] {
                execution_ids.push(receipt.receipt_id.clone());
                call_ids.push(receipt.budget_call_id.clone());
                usage_ids.push(receipt.usage_record_id.clone());
            }
            grader_ids.push(grader.receipt_id.clone());
        }
        let run_id = run_receipt_id(&request.request_id)?;
        let run = DevelopmentRunReceiptV1 {
            schema_version: DEVELOPMENT_RUN_RECEIPT_SCHEMA.into(),
            id: run_id.clone(),
            control_id: control.id.clone(),
            namespace: control.namespace.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            execution_receipt_ids: execution_ids.clone(),
            grader_receipt_ids: grader_ids.clone(),
            budget_call_ids: call_ids,
            usage_record_ids: usage_ids.clone(),
            executor_actor: self.executor.actor().into(),
            issued_at_unix_seconds: 100,
        };
        let run_storage = storage_id(RUN_RECEIPT_KIND, &run_id)?;
        let mut session = self.store.session().await?;
        session
            .put(
                &self.executor,
                "artifact",
                &run_storage,
                self.executor.actor(),
                &serde_json::json!({
                    "schema_version": "rsia.typed_artifact_envelope.v1",
                    "id": run_storage,
                    "record_kind": RUN_RECEIPT_KIND,
                    "payload": run,
                }),
            )
            .await?;
        for source in &control.source_ids {
            session
                .put_edge(&self.executor, "artifact", &run_storage, "run", source)
                .await?;
        }
        for id in &execution_ids {
            session
                .put_edge(
                    &self.executor,
                    "artifact",
                    &run_storage,
                    "artifact",
                    &storage_id(EXECUTION_RECEIPT_KIND, id)?,
                )
                .await?;
        }
        session.commit().await?;
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results,
            execution_receipt_id: run_id,
            usage_record_ids: usage_ids,
            provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
        })
    }
}

fn plan_k1() -> PracticePlan {
    PracticePlan::new("cluster-1", vec!["task-a".into(), "task-b".into()], 1, None).unwrap()
}

fn plan_k3(authorization: &str) -> PracticePlan {
    PracticePlan::new(
        "cluster-1",
        vec!["task-a".into(), "task-b".into()],
        3,
        Some(d(authorization)),
    )
    .unwrap()
}

fn practice_request(set_id: &str, plan: PracticePlan) -> PracticeRunRequest {
    PracticeRunRequest {
        set_id: set_id.into(),
        control_id: CONTROL_ID.into(),
        plan,
        episode_id: format!("practice-episode-{set_id}"),
        step: 1,
        bundle_digest: d("practice-bundle"),
        revoke_watermark: 1,
        created_at: 10,
    }
}

async fn register_k3(fixture: &Fixture, plan: &PracticePlan) -> PracticeRegistrationV1 {
    let registration = PracticeRegistrationV1::for_plan(plan, "admin", 5).unwrap();
    register_practice(&fixture.admin, &fixture.store, registration.clone())
        .await
        .unwrap();
    registration
}

fn known_zero() -> DevelopmentCostState {
    DevelopmentCostState::Known {
        micros: 0,
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
    }
}

async fn finish_cleanup(fixture: &Fixture, source: &str) -> CleanupState {
    let mut status =
        LifecycleCoordinator::revoke_source(&fixture.admin, &fixture.store, source, "privacy", 200)
            .await
            .unwrap();
    for now in 201..400 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(
            &fixture.admin,
            &fixture.store,
            &status.job_id,
            8,
            now,
        )
        .await
        .unwrap();
    }
    status.state
}

// ---------------------------------------------------------------------------
// 1. K=1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn k1_runs_exactly_once_is_no_contrast_and_needs_no_registration() {
    let fixture = Fixture::new().await;
    let runner = fixture.counting();
    let request = practice_request("set-k1", plan_k1());
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(runner.calls(), 1);
    assert_eq!(runner.seen_attempts(), vec![1]);
    assert_eq!(set.attempts.len(), 1);
    assert_eq!(set.outcome, PracticeOutcomeV1::NoContrast);
    assert_eq!(set.registration_id, None);
    // Two task families, regardless of K.
    assert_eq!(set.independent_clusters, 2);
    assert!(!set.counts_as_independent_samples);
    assert_eq!(set.new_practice_attempts(), 1);
    let attempt = &set.attempts[0];
    assert_eq!(attempt.attempt, 1);
    assert!(!attempt.cache_hit);
    assert_eq!(attempt.cost_state, known_zero());
    assert_eq!(
        attempt
            .tasks
            .iter()
            .map(|task| (task.task_id.as_str(), task.parent_family.as_str()))
            .collect::<Vec<_>>(),
        vec![("task-a", "family-a"), ("task-b", "family-b")]
    );
    assert!(
        attempt
            .tasks
            .iter()
            .all(|task| task.passed && task.score_micros == 1_000_000)
    );
    // Stored as an immutable artifact and readable again.
    let stored = load_practice_attempt_set(&fixture.store, &fixture.admin, "set-k1")
        .await
        .unwrap();
    assert_eq!(fingerprint(&stored).unwrap(), fingerprint(&set).unwrap());
    assert_eq!(
        fixture
            .raw_artifact(&practice_attempt_set_storage_id("set-k1").unwrap())
            .await["schema_version"],
        PRACTICE_ATTEMPT_SET_SCHEMA
    );
}

#[tokio::test]
async fn k1_carrying_an_authorization_is_refused_before_any_call() {
    let fixture = Fixture::new().await;
    let runner = fixture.counting();
    let plan = PracticePlan::new(
        "cluster-1",
        vec!["task-a".into(), "task-b".into()],
        1,
        Some(d("stray-authorization")),
    )
    .unwrap();
    assert!(matches!(
        run_practice_set(
            &fixture.admin,
            &fixture.store,
            &runner,
            practice_request("set-k1-auth", plan)
        )
        .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(runner.calls(), 0);
}

// ---------------------------------------------------------------------------
// 2. Refusals: the runner is never called
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unregistered_misregistered_and_oversized_k3_plans_are_refused_before_any_call() {
    let fixture = Fixture::new().await;
    let runner = fixture.counting();
    let run = |request: PracticeRunRequest| {
        let (store, admin, runner) = (&fixture.store, &fixture.admin, &runner);
        async move { run_practice_set(admin, store, runner, request).await }
    };

    // K=3 that nobody registered.
    let unregistered = plan_k3("authorization-unregistered");
    assert!(matches!(
        run(practice_request("set-unregistered", unregistered.clone())).await,
        Err(Error::Forbidden)
    ));

    // A registration written by anyone but an Admin is refused, so K=3 stays
    // unregistered.
    let registration = PracticeRegistrationV1::for_plan(&unregistered, "admin", 5).unwrap();
    for (actor, role) in [
        ("practice-worker", Role::Worker),
        ("practice-host", Role::Host),
        ("practice-evaluator", Role::Evaluator),
        ("practice-agent", Role::Agent),
    ] {
        let ctx = Context::new(NAMESPACE, actor, role).unwrap();
        let mut forged = registration.clone();
        forged.registered_by = actor.into();
        assert!(matches!(
            register_practice(&ctx, &fixture.store, forged).await,
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            register_practice(&ctx, &fixture.store, registration.clone()).await,
            Err(Error::Forbidden)
        ));
    }
    // An Admin cannot register in someone else's name either.
    let mut other_name = registration.clone();
    other_name.registered_by = "someone-else".into();
    assert!(matches!(
        register_practice(&fixture.admin, &fixture.store, other_name).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        load_practice_registration(&fixture.store, &fixture.admin, &registration.id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        run(practice_request("set-unregistered", unregistered.clone())).await,
        Err(Error::Forbidden)
    ));

    // A registration whose plan digest is not its own plan's is refused.
    let mut wrong_digest = registration.clone();
    wrong_digest.plan_digest = d("another-plan");
    assert!(matches!(
        register_practice(&fixture.admin, &fixture.store, wrong_digest).await,
        Err(Error::Conflict(_))
    ));
    // K=1 is never registered.
    assert!(PracticeRegistrationV1::for_plan(&plan_k1(), "admin", 5).is_err());

    // Registered for one plan, the authorization cannot be spent on another.
    let plan = plan_k3("authorization-registered");
    let registered = register_k3(&fixture, &plan).await;
    let mut other_cluster = plan.clone();
    other_cluster.parent_cluster_id = "cluster-2".into();
    assert!(matches!(
        run(practice_request("set-other-cluster", other_cluster)).await,
        Err(Error::Conflict(_))
    ));
    let mut reordered = plan.clone();
    reordered.task_ids.reverse();
    assert!(matches!(
        run(practice_request("set-reordered", reordered)).await,
        Err(Error::Conflict(_))
    ));
    let mut one_task = plan.clone();
    one_task.task_ids.truncate(1);
    assert!(matches!(
        run(practice_request("set-one-task", one_task)).await,
        Err(Error::Conflict(_))
    ));
    // The same id cannot be registered for a different plan.
    let mut conflicting = PracticeRegistrationV1::for_plan(&plan, "admin", 5).unwrap();
    conflicting.created_at = 6;
    assert!(matches!(
        register_practice(&fixture.admin, &fixture.store, conflicting).await,
        Err(Error::Conflict(_))
    ));
    // Registration is idempotent for identical content.
    register_practice(&fixture.admin, &fixture.store, registered.clone())
        .await
        .unwrap();

    // Three tasks: a plan cannot hold them, and a control with three tasks
    // cannot be practised under a two-task plan.
    let mut three = plan_k3("authorization-three");
    three.task_ids.push("task-c".into());
    assert!(matches!(
        run(practice_request("set-three-tasks", three)).await,
        Err(Error::Invalid(_))
    ));
    register_development_control(
        &fixture.admin,
        &fixture.store,
        three_task_control("dev-control-three"),
    )
    .await
    .unwrap();
    let mut on_three = practice_request("set-three-control", plan.clone());
    on_three.control_id = "dev-control-three".into();
    assert!(matches!(
        run(on_three).await,
        Err(Error::Conflict(message)) if message.contains("tasks differ")
    ));
    let mut on_three_k1 = practice_request("set-three-control-k1", plan_k1());
    on_three_k1.control_id = "dev-control-three".into();
    assert!(matches!(run(on_three_k1).await, Err(Error::Conflict(_))));

    // Unknown control, fixture-scoped control, role, digest shape, watermark.
    let mut unknown_control = practice_request("set-unknown-control", plan_k1());
    unknown_control.control_id = "no-such-control".into();
    assert!(matches!(run(unknown_control).await, Err(Error::NotFound)));
    register_development_control(
        &fixture.admin,
        &fixture.store,
        control(
            "dev-control-fixture",
            DevelopmentEvidenceScope::ProgramFixture,
        ),
    )
    .await
    .unwrap();
    let mut fixture_control = practice_request("set-fixture-control", plan_k1());
    fixture_control.control_id = "dev-control-fixture".into();
    assert!(matches!(run(fixture_control).await, Err(Error::Forbidden)));
    let evaluator = Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap();
    assert!(matches!(
        run_practice_set(
            &evaluator,
            &fixture.store,
            &runner,
            practice_request("set-evaluator", plan_k1())
        )
        .await,
        Err(Error::Forbidden)
    ));
    let mut bad_bundle = practice_request("set-bad-bundle", plan_k1());
    bad_bundle.bundle_digest = "not-a-digest".into();
    assert!(matches!(run(bad_bundle).await, Err(Error::Invalid(_))));
    let mut drifted = practice_request("set-drifted", plan_k1());
    drifted.revoke_watermark = 99;
    assert!(matches!(run(drifted).await, Err(Error::Conflict(_))));

    assert_eq!(runner.calls(), 0);
    for set_id in [
        "set-unregistered",
        "set-other-cluster",
        "set-three-tasks",
        "set-three-control",
        "set-drifted",
    ] {
        assert!(matches!(
            load_practice_attempt_set(&fixture.store, &fixture.admin, set_id).await,
            Err(Error::NotFound)
        ));
    }
    assert!(
        fixture
            .group_calls("practice-episode-set-drifted")
            .await
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// 3. K=3 with the real deterministic runner
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registered_k3_with_the_deterministic_runner_keeps_every_attempt_and_no_contrast() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-k3");
    let registration = register_k3(&fixture, &plan).await;
    let runner = fixture.counting();
    let request = practice_request("set-k3", plan);
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();

    assert_eq!(runner.calls(), 3);
    assert_eq!(runner.seen_attempts(), vec![1, 2, 3]);
    assert_eq!(set.outcome, PracticeOutcomeV1::NoContrast);
    assert_eq!(set.attempts.len(), 3);
    assert_eq!(
        set.registration_id.as_deref(),
        Some(registration.id.as_str())
    );
    assert_eq!(set.independent_clusters, 2);
    assert!(!set.counts_as_independent_samples);
    assert_eq!(set.new_practice_attempts(), 3);
    set.validate().unwrap();

    let request_ids: BTreeSet<&str> = set
        .attempts
        .iter()
        .map(|attempt| attempt.request_id.as_str())
        .collect();
    assert_eq!(request_ids.len(), 3);
    for (index, attempt) in set.attempts.iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap();
        assert_eq!(attempt.attempt, number);
        assert_eq!(
            attempt.request_id,
            practice_attempt_request_id("set-k3", number).unwrap()
        );
        assert_eq!(
            attempt.execution_receipt_id,
            run_receipt_id(&attempt.request_id).unwrap()
        );
        assert!(!attempt.cache_hit);
        // Four executions of zero cost: two tasks times two runner sides.
        assert_eq!(attempt.cost_state, known_zero());
        for (task, reference) in attempt.tasks.iter().zip(&set.attempts[0].tasks) {
            // Deterministic: every attempt agrees, and the digest is the stored
            // Candidate receipt's, not anything the report claims.
            assert_eq!(task, reference);
            let receipt = load_execution_receipt(
                &fixture.store,
                &fixture.admin,
                &evo_engine::development::execution_receipt_id(
                    &attempt.request_id,
                    &task.task_id,
                    DevelopmentSide::Candidate,
                )
                .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(receipt.output_digest, task.output_digest);
        }
    }

    // Shared root budget, development stage rows only: Practice is unused.
    let calls = fixture.group_calls(&request.episode_id).await;
    assert_eq!(calls.len(), 12);
    assert!(
        calls
            .iter()
            .all(|call| call.stage == BudgetStage::DevelopmentExecution)
    );
    let root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.spent_micros, root.reserved_micros), (0, 0));

    // Edges: the control's sources, every attempt's run receipt and Candidate
    // receipts, and the registration reach the set.
    let set_storage = practice_attempt_set_storage_id("set-k3").unwrap();
    let edge = ("artifact".to_string(), set_storage.clone());
    let mut session = fixture.store.session().await.unwrap();
    for source in ["run-a", "run-b"] {
        assert!(
            session
                .dependents(&fixture.admin, "run", source)
                .await
                .unwrap()
                .contains(&edge)
        );
    }
    assert!(
        session
            .dependents(
                &fixture.admin,
                "artifact",
                &storage_id(evo_engine::development::CONTROL_KIND, CONTROL_ID).unwrap()
            )
            .await
            .unwrap()
            .contains(&edge)
    );
    for attempt in &set.attempts {
        assert!(
            session
                .dependents(
                    &fixture.admin,
                    "artifact",
                    &storage_id(RUN_RECEIPT_KIND, &attempt.execution_receipt_id).unwrap()
                )
                .await
                .unwrap()
                .contains(&edge)
        );
        for task in &attempt.tasks {
            let candidate = evo_engine::development::execution_receipt_id(
                &attempt.request_id,
                &task.task_id,
                DevelopmentSide::Candidate,
            )
            .unwrap();
            assert!(
                session
                    .dependents(
                        &fixture.admin,
                        "artifact",
                        &storage_id(EXECUTION_RECEIPT_KIND, &candidate).unwrap()
                    )
                    .await
                    .unwrap()
                    .contains(&edge)
            );
        }
    }
    assert!(
        session
            .dependents(
                &fixture.admin,
                "artifact",
                &practice_registration_storage_id(&registration.id).unwrap()
            )
            .await
            .unwrap()
            .contains(&edge)
    );
    session.commit().await.unwrap();

    // The registration is an Admin artifact with its own schema.
    let stored = fixture
        .raw_artifact(&practice_registration_storage_id(&registration.id).unwrap())
        .await;
    assert_eq!(stored["schema_version"], PRACTICE_REGISTRATION_SCHEMA);
    assert_eq!(stored["k"], 3);
    assert_eq!(stored["registered_by"], "admin");
    assert_eq!(
        load_practice_registration(&fixture.store, &fixture.executor, &registration.id)
            .await
            .unwrap(),
        registration
    );
}

#[tokio::test]
async fn a_worker_may_run_a_registered_set_and_a_new_set_never_reuses_an_old_attempt() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-two-sets");
    register_k3(&fixture, &plan).await;
    let worker = Context::new(NAMESPACE, "practice-worker", Role::Worker).unwrap();
    let runner = fixture.counting();
    let first = run_practice_set(
        &worker,
        &fixture.store,
        &runner,
        practice_request("set-one", plan.clone()),
    )
    .await
    .unwrap();
    // A deliberate new practice on the same tasks and bundle: a new set id.
    let mut again = practice_request("set-two", plan);
    again.episode_id = first.episode_id.clone();
    let second = run_practice_set(&worker, &fixture.store, &runner, again.clone())
        .await
        .unwrap();
    assert_eq!(runner.calls(), 6);
    assert!(second.attempts.iter().all(|attempt| !attempt.cache_hit));
    let ids: BTreeSet<&str> = first
        .attempts
        .iter()
        .chain(&second.attempts)
        .map(|attempt| attempt.request_id.as_str())
        .collect();
    assert_eq!(ids.len(), 6);
    // Both sets landed in one dispatch group: 24 fresh rows, none reused.
    assert_eq!(fixture.group_calls(&again.episode_id).await.len(), 24);
    // More sets and more attempts never raise the number of independent
    // clusters: changing the set id (the "seed") adds no cluster and no n.
    assert_eq!(first.independent_clusters, 2);
    assert_eq!(second.independent_clusters, 2);
    assert!(!first.counts_as_independent_samples && !second.counts_as_independent_samples);
}

// ---------------------------------------------------------------------------
// 4. The contrast path (fixture runners with differing scores)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn k3_with_a_better_second_attempt_names_the_contrast_and_keeps_every_attempt() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-contrast");
    register_k3(&fixture, &plan).await;
    let runner = ContrastRunner::new(&fixture, |attempt, _task| attempt == 2);
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &runner,
        practice_request("set-contrast", plan),
    )
    .await
    .unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 3);
    // Higher score is better; the worse attempt is the earliest of the worst.
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Contrast {
            better_attempt: 2,
            worse_attempt: 1,
            task_id: "task-a".into(),
        }
    );
    assert_eq!(set.attempts.len(), 3);
    assert_eq!(
        set.attempts
            .iter()
            .map(|attempt| attempt.tasks[0].score_micros)
            .collect::<Vec<_>>(),
        vec![0, 1_000_000, 0]
    );
    assert_eq!(
        set.attempts
            .iter()
            .map(|attempt| attempt.tasks[0].passed)
            .collect::<Vec<_>>(),
        vec![false, true, false]
    );
    // The outputs differ, so the digests do: the evidence is the receipts'.
    assert_ne!(
        set.attempts[0].tasks[0].output_digest,
        set.attempts[1].tasks[0].output_digest
    );
    assert_eq!(
        set.attempts[0].tasks[0].output_digest,
        set.attempts[2].tasks[0].output_digest
    );
    // Even with a contrast the set is no independent sample.
    assert_eq!(set.independent_clusters, 2);
    assert!(!set.counts_as_independent_samples);
    let stored = load_practice_attempt_set(&fixture.store, &fixture.admin, "set-contrast")
        .await
        .unwrap();
    assert_eq!(stored.outcome, set.outcome);
    stored.validate().unwrap();
}

#[tokio::test]
async fn the_contrast_names_the_first_task_whose_attempts_differ() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-contrast-b");
    register_k3(&fixture, &plan).await;
    // task-a agrees everywhere; only attempt 3 gets task-b right.
    let runner = ContrastRunner::new(&fixture, |attempt, task| task == "task-a" || attempt == 3);
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &runner,
        practice_request("set-contrast-b", plan),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Contrast {
            better_attempt: 3,
            worse_attempt: 1,
            task_id: "task-b".into(),
        }
    );
    assert_eq!(select_contrast(&set.attempts), set.outcome);
}

#[tokio::test]
async fn identical_attempts_from_a_fixture_runner_are_no_contrast_and_k_is_never_raised() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-no-contrast");
    register_k3(&fixture, &plan).await;
    let runner = ContrastRunner::new(&fixture, |_attempt, _task| false);
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &runner,
        practice_request("set-no-contrast", plan),
    )
    .await
    .unwrap();
    // Every attempt failed identically: no valid difference, and no fourth try.
    assert_eq!(set.outcome, PracticeOutcomeV1::NoContrast);
    assert_eq!(set.attempts.len(), 3);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 3);
    assert!(set.attempts.iter().all(|attempt| {
        attempt
            .tasks
            .iter()
            .all(|task| !task.passed && task.score_micros == 0)
    }));
}

// ---------------------------------------------------------------------------
// 5. Idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_set_id_and_request_returns_the_stored_set_and_a_different_request_conflicts() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-idempotent");
    register_k3(&fixture, &plan).await;
    let runner = fixture.counting();
    let request = practice_request("set-idem", plan.clone());
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(runner.calls(), 3);
    let rows = fixture.group_calls(&request.episode_id).await.len();
    assert_eq!(rows, 12);

    let again = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(fingerprint(&again).unwrap(), fingerprint(&set).unwrap());
    // A Worker reconnecting gets the same answer; nothing is sent.
    let worker = Context::new(NAMESPACE, "practice-worker", Role::Worker).unwrap();
    let from_worker = run_practice_set(&worker, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&from_worker).unwrap(),
        fingerprint(&set).unwrap()
    );
    // A reconnect that stamps a new time is still the same actual request.
    let mut restamped = request.clone();
    restamped.created_at = 99;
    let from_restamp = run_practice_set(&fixture.admin, &fixture.store, &runner, restamped)
        .await
        .unwrap();
    assert_eq!(from_restamp.created_at, 10);
    assert_eq!(
        fingerprint(&from_restamp).unwrap(),
        fingerprint(&set).unwrap()
    );
    assert_eq!(runner.calls(), 3);
    assert_eq!(fixture.group_calls(&request.episode_id).await.len(), rows);

    // The same id with any other request is a Conflict and sends nothing.
    let mut other_bundle = request.clone();
    other_bundle.bundle_digest = d("another-bundle");
    let mut other_step = request.clone();
    other_step.step = 2;
    let mut other_episode = request.clone();
    other_episode.episode_id = "another-episode".into();
    let mut other_k = request.clone();
    other_k.plan = plan_k1();
    for changed in [other_bundle, other_step, other_episode, other_k] {
        assert!(matches!(
            run_practice_set(&fixture.admin, &fixture.store, &runner, changed).await,
            Err(Error::Conflict(_))
        ));
    }
    assert_eq!(runner.calls(), 3);
    let stored = load_practice_attempt_set(&fixture.store, &fixture.admin, "set-idem")
        .await
        .unwrap();
    assert_eq!(fingerprint(&stored).unwrap(), fingerprint(&set).unwrap());
}

// ---------------------------------------------------------------------------
// 6. Cache hit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_attempt_whose_receipts_already_exist_is_a_cache_hit_and_not_a_new_practice() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-cache");
    register_k3(&fixture, &plan).await;
    let request = practice_request("set-cache", plan);
    // Somebody already ran attempt 1's exact request.
    let first = practice_attempt_request(&fixture.control, &request, 1).unwrap();
    let direct = fixture.runner().run(first.clone()).await.unwrap();
    assert_eq!(fixture.group_calls(&request.episode_id).await.len(), 4);

    let runner = fixture.counting();
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert!(set.attempts[0].cache_hit);
    assert!(!set.attempts[1].cache_hit);
    assert!(!set.attempts[2].cache_hit);
    assert_eq!(set.new_practice_attempts(), 2);
    assert_eq!(set.attempts.len(), 3);
    assert_eq!(set.attempts[0].request_id, first.request_id);
    assert_eq!(
        set.attempts[0].execution_receipt_id,
        direct.execution_receipt_id
    );
    // Only attempts 2 and 3 added budget rows: 4 reused + 8 new.
    assert_eq!(fixture.group_calls(&request.episode_id).await.len(), 12);
    // The reused attempt still counts toward nothing independent.
    assert_eq!(set.independent_clusters, 2);
    assert!(!set.counts_as_independent_samples);
    assert_eq!(set.outcome, PracticeOutcomeV1::NoContrast);

    // A fully executed set re-requested under a new id is all new practice.
    let mut fresh = practice_request("set-cache-fresh", set.plan.clone());
    fresh.episode_id = request.episode_id.clone();
    let fresh_set = run_practice_set(&fixture.admin, &fixture.store, &runner, fresh)
        .await
        .unwrap();
    assert_eq!(fresh_set.new_practice_attempts(), 3);
}

// ---------------------------------------------------------------------------
// 7. A practice set is not a development cycle
// ---------------------------------------------------------------------------

fn curriculum_source(
    id: &str,
    source_kind: CurriculumSourceKindV1,
    subject_digest: String,
    body: serde_json::Value,
) -> CurriculumSourceArtifactV1 {
    CurriculumSourceArtifactV1 {
        schema_version: CurriculumSourceArtifactV1::SCHEMA.into(),
        id: id.into(),
        source_kind,
        data_use: DataUse::Development,
        subject_digest,
        body_digest: fingerprint(&body).unwrap(),
        body,
        dependency_ids: vec![],
    }
}

async fn curriculum(fixture: &Fixture) -> PersistentCurriculumCoordinator {
    let coordinator =
        PersistentCurriculumCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
            .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            serde_json::json!({"buckets":["clamp-boundary"]}),
        ))
        .await
        .unwrap();
    for (id, body) in registered_profile_source_bodies() {
        let kind = if id.contains("target") || id.contains("source") {
            CurriculumSourceKindV1::TargetSpec
        } else if id.contains("oracle") {
            CurriculumSourceKindV1::OracleSpec
        } else {
            CurriculumSourceKindV1::RunnerSpec
        };
        coordinator
            .register_source(curriculum_source(
                &id,
                kind,
                fingerprint(&body).unwrap(),
                body,
            ))
            .await
            .unwrap();
    }
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let profile = CurriculumControlProfileV1::offline_default(
        "pure_function_test_proposal.v1",
        d("task-space"),
        registered.oracle_digest.clone(),
        registered.runner_digest.clone(),
    )
    .unwrap();
    let state = LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: "learner-state".into(),
        profile_id: "pure_function_test_proposal.v1".into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: fixture.control.environment_digest.clone(),
        grader_digest: fixture.control.grader_digest.clone(),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    };
    coordinator
        .register_profile_and_state(profile, state)
        .await
        .unwrap();
    coordinator
}

fn count_schema(artifacts: &[serde_json::Value], schema: &str) -> usize {
    artifacts
        .iter()
        .filter(|value| value["schema_version"] == schema)
        .count()
}

fn count_schema_prefix(artifacts: &[serde_json::Value], prefix: &str) -> usize {
    artifacts
        .iter()
        .filter(|value| {
            value["schema_version"]
                .as_str()
                .is_some_and(|schema| schema.starts_with(prefix))
        })
        .count()
}

#[tokio::test]
async fn practice_sets_write_no_stage_fact_and_add_no_e12_or_e13_cycle() {
    let fixture = Fixture::new().await;
    // One real development cycle: request/observation facts and E12's record.
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let coordinator = curriculum(&fixture).await;
    let cycle_receipt =
        |id: &str, request_fact: &str, observed_fact: &str| DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: id.into(),
            state_id: "learner-state".into(),
            development_request_fact_id: request_fact.into(),
            development_observed_fact_id: observed_fact.into(),
        };
    let before_state = coordinator
        .record_cycle(cycle_receipt(
            "cycle-1",
            &request_fact_id,
            &observed_fact_id,
        ))
        .await
        .unwrap();
    assert_eq!(before_state.completed_cycles.len(), 1);
    let state_id = curriculum_state_storage_id("learner-state").unwrap();
    let state_before = fixture.raw_artifact(&state_id).await;
    let artifacts_before = fixture.artifacts().await;
    assert_eq!(
        count_schema(&artifacts_before, OPTIMIZATION_STAGE_FACT_SCHEMA),
        2
    );
    assert_eq!(
        count_schema_prefix(&artifacts_before, "rsia.monitoring."),
        0
    );

    // Practice on the very same control, episode and step as that cycle.
    let plan = plan_k3("authorization-cycles");
    register_k3(&fixture, &plan).await;
    let runner = fixture.counting();
    let mut k3 = practice_request("set-cycles-k3", plan);
    k3.episode_id = "episode-1".into();
    let mut k1 = practice_request("set-cycles-k1", plan_k1());
    k1.episode_id = "episode-1".into();
    let set_k3 = run_practice_set(&fixture.admin, &fixture.store, &runner, k3)
        .await
        .unwrap();
    let set_k1 = run_practice_set(&fixture.admin, &fixture.store, &runner, k1)
        .await
        .unwrap();
    assert_eq!(runner.calls(), 4);

    // No fact of any kind was written for any attempt.
    let artifacts_after = fixture.artifacts().await;
    assert_eq!(
        count_schema(&artifacts_after, OPTIMIZATION_STAGE_FACT_SCHEMA),
        2
    );
    assert_eq!(count_schema_prefix(&artifacts_after, "rsia.monitoring."), 0);
    // Exactly the real cycle's request and observation facts, no attempt's.
    for kind in ["development_request_prepared", "development_observed"] {
        let count = |artifacts: &[serde_json::Value]| {
            artifacts
                .iter()
                .filter(|value| value["kind"] == kind)
                .count()
        };
        assert_eq!(count(&artifacts_before), 1);
        assert_eq!(count(&artifacts_after), 1);
    }
    assert_eq!(
        count_schema(&artifacts_after, PRACTICE_ATTEMPT_SET_SCHEMA),
        2
    );
    assert_eq!(
        count_schema(&artifacts_after, PRACTICE_REGISTRATION_SCHEMA),
        1
    );

    // E12: the learner state did not move, and replaying the one real cycle
    // still yields exactly one.
    assert_eq!(fixture.raw_artifact(&state_id).await, state_before);
    let replay = coordinator
        .record_cycle(cycle_receipt(
            "cycle-1",
            &request_fact_id,
            &observed_fact_id,
        ))
        .await
        .unwrap();
    assert_eq!(replay.completed_cycles.len(), 1);
    assert_eq!(
        replay.development_fact_ids,
        before_state.development_fact_ids
    );
    // A practice set is not a development fact: E12 cannot record it.
    let set_storage = practice_attempt_set_storage_id("set-cycles-k3").unwrap();
    let run_storage =
        storage_id(RUN_RECEIPT_KIND, &set_k3.attempts[0].execution_receipt_id).unwrap();
    assert!(
        coordinator
            .record_cycle(cycle_receipt("cycle-2", &set_storage, &run_storage))
            .await
            .is_err()
    );
    assert!(
        coordinator
            .record_cycle(cycle_receipt("cycle-3", &run_storage, &set_storage))
            .await
            .is_err()
    );
    assert_eq!(fixture.raw_artifact(&state_id).await, state_before);

    // The real cycle still verifies, extra rows in its dispatch group or not.
    let view = verify_development_observation(
        &fixture.store,
        &fixture.admin,
        &request_fact_id,
        &observed_fact_id,
    )
    .await
    .unwrap();
    assert_eq!(view.outcomes.len(), 2);
    assert!(fixture.group_calls("episode-1").await.len() > 4);
    assert_eq!(set_k1.independent_clusters, set_k3.independent_clusters);
}

// ---------------------------------------------------------------------------
// 8. Uncertain, exhausted budget, errors: stop, keep, never resend
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_uncertain_second_attempt_stops_the_set_and_is_never_resent() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-uncertain");
    register_k3(&fixture, &plan).await;
    // Cancelled: the lease was lost, so whether the call ran is unknown.
    let runner = fixture.counting().failing(2, || Error::Cancelled);
    let request = practice_request("set-uncertain", plan);
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "uncertain_attempt:2".into()
        }
    );
    // Attempt 1 is kept; attempt 3 never ran.
    assert_eq!(set.attempts.len(), 1);
    assert_eq!(set.attempts[0].attempt, 1);
    assert_eq!(runner.calls(), 2);
    assert_eq!(runner.seen_attempts(), vec![1, 2]);
    assert_eq!(set.independent_clusters, 2);

    // Re-running neither resends attempt 2 nor starts attempt 3.
    let again = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    assert_eq!(fingerprint(&again).unwrap(), fingerprint(&set).unwrap());
    assert_eq!(runner.calls(), 2);
    let stored = load_practice_attempt_set(&fixture.store, &fixture.admin, "set-uncertain")
        .await
        .unwrap();
    assert_eq!(stored.outcome, set.outcome);
    assert_eq!(fixture.group_calls(&request.episode_id).await.len(), 4);
}

#[tokio::test]
async fn an_exhausted_budget_or_an_error_stops_the_set_with_what_finished() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-stop");
    register_k3(&fixture, &plan).await;

    let exhausted = fixture.counting().failing(2, || Error::Budget);
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &exhausted,
        practice_request("set-budget", plan.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "budget_exhausted:2".into()
        }
    );
    assert_eq!(set.attempts.len(), 1);
    assert_eq!(exhausted.calls(), 2);

    // The very first attempt failing keeps nothing but still records the stop.
    let broken = fixture
        .counting()
        .failing(1, || Error::Conflict("runner refused".into()));
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &broken,
        practice_request("set-error", plan.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "attempt_error:1:conflict".into()
        }
    );
    assert!(set.attempts.is_empty());
    assert_eq!(broken.calls(), 1);
    set.validate().unwrap();
    let again = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &broken,
        practice_request("set-error", plan),
    )
    .await
    .unwrap();
    assert_eq!(fingerprint(&again).unwrap(), fingerprint(&set).unwrap());
    assert_eq!(broken.calls(), 1);
}

#[tokio::test]
async fn a_forged_report_is_never_recorded_as_an_attempt() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-forged");
    register_k3(&fixture, &plan).await;

    // The report claims a worse score than the stored, server-graded receipts.
    let understated = ForgingRunner {
        inner: fixture.runner(),
        forge: |report| {
            report.results[0].candidate_score_micros = 0;
            report.results[0].candidate_passed = false;
        },
        calls: AtomicUsize::new(0),
    };
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &understated,
        practice_request("set-forged-score", plan.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "attempt_unverified:1:conflict".into()
        }
    );
    assert!(set.attempts.is_empty());
    assert_eq!(understated.calls.load(Ordering::SeqCst), 1);

    // A self-declared fixture never becomes practice evidence.
    let fixture_claim = ForgingRunner {
        inner: fixture.runner(),
        forge: |report| report.provenance = DevelopmentExecutionProvenance::Fixture,
        calls: AtomicUsize::new(0),
    };
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &fixture_claim,
        practice_request("set-forged-fixture", plan.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "attempt_unverified:1:forbidden".into()
        }
    );
    assert!(set.attempts.is_empty());

    // A report naming some other run receipt is not this attempt's closure.
    let wrong_run = ForgingRunner {
        inner: fixture.runner(),
        forge: |report| {
            report.execution_receipt_id = run_receipt_id("another-request").unwrap();
        },
        calls: AtomicUsize::new(0),
    };
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &wrong_run,
        practice_request("set-forged-run", plan),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "attempt_unverified:1:conflict".into()
        }
    );
    assert!(set.attempts.is_empty());
}

/// Attempt 2 finds the world changed: the runner refuses it, the set stops, and
/// the writing transaction re-checks the live sources, so nothing is stored.
async fn run_with_disturbance(revoke: bool) -> evo_core::Result<PracticeAttemptSetV1> {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-disturbed");
    register_k3(&fixture, &plan).await;
    let runner = DisturbingRunner {
        inner: fixture.runner(),
        store: fixture.store.clone(),
        admin: fixture.admin.clone(),
        at: 2,
        revoke,
        calls: AtomicUsize::new(0),
    };
    let outcome = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &runner,
        practice_request("set-disturbed", plan),
    )
    .await;
    // Attempt 3 never ran, and no set was stored after the world had changed.
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        load_practice_attempt_set(&fixture.store, &fixture.admin, "set-disturbed").await,
        Err(Error::NotFound)
    ));
    outcome
}

#[tokio::test]
async fn a_source_revoked_during_the_run_leaves_no_set_behind() {
    assert!(matches!(
        run_with_disturbance(true).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn a_watermark_moved_during_the_run_leaves_no_set_behind() {
    assert!(matches!(
        run_with_disturbance(false).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn evidence_a_runner_produced_under_another_control_is_not_this_sets_evidence() {
    let fixture = Fixture::new().await;
    // A second registered control with the same tasks, environment and grader.
    register_development_control(
        &fixture.admin,
        &fixture.store,
        control(
            "dev-control-b",
            DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        ),
    )
    .await
    .unwrap();
    let other = CountingRunner {
        inner: RegisteredDevelopmentRunner::with_clock(
            fixture.store.clone(),
            fixture.executor.clone(),
            fixture.grader.clone(),
            "dev-control-b",
            600,
            Arc::new(|| 100),
        )
        .unwrap(),
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
        fail: None,
    };
    // The set names dev-control, but the runner spent dev-control-b's rows.
    let set = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &other,
        practice_request("set-other-control", plan_k1()),
    )
    .await
    .unwrap();
    assert_eq!(
        set.outcome,
        PracticeOutcomeV1::Incomplete {
            reason: "attempt_unverified:1:conflict".into()
        }
    );
    assert!(set.attempts.is_empty());
    assert_eq!(other.calls(), 1);
}

// ---------------------------------------------------------------------------
// 9. Revocation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_a_source_redacts_the_set_and_keeps_the_registration() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-revoke");
    let registration = register_k3(&fixture, &plan).await;
    let runner = fixture.counting();
    let request = practice_request("set-revoke", plan.clone());
    let set = run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone())
        .await
        .unwrap();
    // A set that stopped before its first attempt carries no receipt edge; it
    // still depends on the control's sources.
    let broken = fixture.counting().failing(1, || Error::Internal);
    let empty = run_practice_set(
        &fixture.admin,
        &fixture.store,
        &broken,
        practice_request("set-revoke-empty", plan.clone()),
    )
    .await
    .unwrap();
    assert!(empty.attempts.is_empty());
    assert_eq!(runner.calls(), 3);

    let set_storage = practice_attempt_set_storage_id("set-revoke").unwrap();
    let empty_storage = practice_attempt_set_storage_id("set-revoke-empty").unwrap();
    let registration_storage = practice_registration_storage_id(&registration.id).unwrap();
    let registration_before = fixture.raw_artifact(&registration_storage).await;
    assert_eq!(
        fixture.raw_artifact(&set_storage).await["schema_version"],
        PRACTICE_ATTEMPT_SET_SCHEMA
    );

    // Logical revocation alone already fails closed: nothing is returned or
    // started, but the stored bytes are not yet cleaned.
    let status = LifecycleCoordinator::revoke_source(
        &fixture.admin,
        &fixture.store,
        "run-a",
        "privacy",
        200,
    )
    .await
    .unwrap();
    assert!(matches!(
        run_practice_set(&fixture.admin, &fixture.store, &runner, request.clone()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        run_practice_set(
            &fixture.admin,
            &fixture.store,
            &runner,
            practice_request("set-after-revoke", plan.clone())
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(runner.calls(), 3);
    assert_eq!(broken.calls(), 1);

    // Cleanup runs to completion: every practice node is classified.
    let mut status = status;
    for now in 201..400 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(
            &fixture.admin,
            &fixture.store,
            &status.job_id,
            8,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);

    for storage in [&set_storage, &empty_storage] {
        let redacted = fixture.raw_artifact(storage).await;
        assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
        assert_eq!(redacted["original_schema"], PRACTICE_ATTEMPT_SET_SCHEMA);
        assert!(redacted.get("attempts").is_none());
        assert!(redacted.get("outcome").is_none());
    }
    // The Admin registration survives; so do the E03 receipts.
    assert_eq!(
        fixture.raw_artifact(&registration_storage).await,
        registration_before
    );
    assert_eq!(
        load_practice_registration(&fixture.store, &fixture.admin, &registration.id)
            .await
            .unwrap(),
        registration
    );
    let receipt = fixture
        .raw_artifact(
            &storage_id(
                EXECUTION_RECEIPT_KIND,
                &evo_engine::development::execution_receipt_id(
                    &set.attempts[0].request_id,
                    "task-a",
                    DevelopmentSide::Candidate,
                )
                .unwrap(),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(receipt["schema_version"], "rsia.typed_artifact_envelope.v1");

    // A redacted set is reported as redacted and is never regenerated.
    assert!(matches!(
        load_practice_attempt_set(&fixture.store, &fixture.admin, "set-revoke").await,
        Err(Error::Conflict(message)) if message.contains("redacted")
    ));
    assert!(matches!(
        run_practice_set(&fixture.admin, &fixture.store, &runner, request).await,
        Err(Error::Conflict(message)) if message.contains("redacted")
    ));
    assert_eq!(runner.calls(), 3);
}

#[tokio::test]
async fn a_revocation_that_reaches_the_registration_preserves_it() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-preserve");
    let registration = register_k3(&fixture, &plan).await;
    let storage = practice_registration_storage_id(&registration.id).unwrap();
    let before = fixture.raw_artifact(&storage).await;
    // Put the registration inside the revocation closure on purpose: the
    // cleanup must classify it (a preserved Admin fact), not stall on it.
    let mut session = fixture.store.session().await.unwrap();
    session
        .put_edge(&fixture.admin, "artifact", &storage, "run", "run-a")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        finish_cleanup(&fixture, "run-a").await,
        CleanupState::Complete
    );
    assert_eq!(fixture.raw_artifact(&storage).await, before);
}

// ---------------------------------------------------------------------------
// Stored sets are re-validated
// ---------------------------------------------------------------------------

async fn stored_set_k3(
    fixture: &Fixture,
    set_id: &str,
    authorization: &str,
) -> PracticeAttemptSetV1 {
    let plan = plan_k3(authorization);
    register_k3(fixture, &plan).await;
    run_practice_set(
        &fixture.admin,
        &fixture.store,
        &fixture.counting(),
        practice_request(set_id, plan),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_set_never_counts_as_independent_samples_and_a_forged_body_is_refused() {
    let fixture = Fixture::new().await;
    let set = stored_set_k3(&fixture, "set-forge", "authorization-forge").await;
    set.validate().unwrap();

    let mut independent = set.clone();
    independent.counts_as_independent_samples = true;
    assert!(independent.validate().is_err());
    // K (or anything but the task families) inflating the cluster count.
    let mut inflated = set.clone();
    inflated.independent_clusters = 6;
    assert!(inflated.validate().is_err());
    let mut none = set.clone();
    none.independent_clusters = 0;
    assert!(none.validate().is_err());
    // A contrast no attempt supports.
    let mut forged_contrast = set.clone();
    forged_contrast.outcome = PracticeOutcomeV1::Contrast {
        better_attempt: 2,
        worse_attempt: 1,
        task_id: "task-a".into(),
    };
    assert!(forged_contrast.validate().is_err());
    // An incomplete set that somehow ran every certain attempt.
    let mut stalled = set.clone();
    stalled.outcome = PracticeOutcomeV1::Incomplete {
        reason: "uncertain_attempt:3".into(),
    };
    assert!(stalled.validate().is_err());
    // A finished set missing an attempt, and attempts out of order.
    let mut short = set.clone();
    short.attempts.pop();
    assert!(short.validate().is_err());
    let mut reordered = set.clone();
    reordered.attempts.swap(0, 2);
    assert!(reordered.validate().is_err());
    // K=3 without its registration, and K=1 claiming one.
    let mut unregistered = set.clone();
    unregistered.registration_id = None;
    assert!(unregistered.validate().is_err());
    // Attempt ids must derive from the set.
    let mut foreign = set.clone();
    foreign.attempts[1].request_id = practice_attempt_request_id("another-set", 2).unwrap();
    assert!(foreign.validate().is_err());

    // A forged body written over the stored set is refused when read back.
    let storage = practice_attempt_set_storage_id("set-forge").unwrap();
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&fixture.admin, "artifact", &storage, "admin", &independent)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        load_practice_attempt_set(&fixture.store, &fixture.admin, "set-forge").await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn set_and_registration_storage_ids_and_attempt_requests_are_stable() {
    let fixture = Fixture::new().await;
    let plan = plan_k3("authorization-stable");
    let request = practice_request("set-stable", plan.clone());
    for number in 1..=3 {
        let one = practice_attempt_request(&fixture.control, &request, number).unwrap();
        let two = practice_attempt_request(&fixture.control, &request, number).unwrap();
        assert_eq!(fingerprint(&one).unwrap(), fingerprint(&two).unwrap());
        assert_eq!(one.attempt, number);
        assert_eq!(one.parent_bundle_digest, one.candidate_bundle_digest);
        assert_eq!(one.manifest.digest, fixture.control.manifest_digest);
        assert_eq!(one.purpose, Purpose::Development);
    }
    assert!(practice_attempt_request(&fixture.control, &request, 0).is_err());
    assert!(practice_attempt_request(&fixture.control, &request, 4).is_err());
    assert_ne!(
        practice_attempt_set_storage_id("a").unwrap(),
        practice_attempt_set_storage_id("b").unwrap()
    );
    assert!(practice_registration_storage_id("not-a-digest").is_err());
    let registration = PracticeRegistrationV1::for_plan(&plan, "admin", 5).unwrap();
    assert_eq!(registration.id, plan.authorization_digest.clone().unwrap());
    assert_eq!(registration.plan_digest, fingerprint(&plan).unwrap());
    registration.binds_plan(&plan).unwrap();
}
