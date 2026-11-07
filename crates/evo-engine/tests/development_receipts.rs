//! E03 typed development receipts: registered pure-function runner, the
//! observation gate, E12 consumption, and revocation. Real SQLite store,
//! no model, no provider, zero monetary cost.

#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::curriculum::{CurriculumControlProfileV1, LearnerStateV2};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::curriculum::{
    CurriculumSourceArtifactV1, CurriculumSourceKindV1, DevelopmentCycleReceiptV1,
    PersistentCurriculumCoordinator,
};
use evo_engine::curriculum_profiles::{
    RegisteredPureFunctionProfileV1, registered_profile_source_bodies,
};
use evo_engine::development::{
    DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA, DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2,
    DevelopmentControlV1, DevelopmentCostState, DevelopmentEvidenceScope,
    DevelopmentExecutionReceiptVersion, DevelopmentSide, DevelopmentTaskSpecV1,
    EXECUTION_OUTPUT_KIND, EXECUTION_RECEIPT_KIND, ExecutionReceiptIssueRequest,
    GraderReceiptIssueRequest, RegisteredDevelopmentRunner, RegisteredExecutionRequestV1,
    RegisteredTargetInputV1, execution_budget_call_id, execution_receipt_id,
    execution_request_digest, grader_receipt_id, issue_execution_receipt, issue_grader_receipt,
    legacy_execution_receipt_id, load_execution_output, load_execution_receipt,
    load_grader_receipt, read_execution_receipt_in_session, register_development_control,
    registered_runner_digest, storage_id, typed_receipt_closure,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentRunReport, DevelopmentRunRequest,
    OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournal, OptimizationJournalStage, StageDependency,
    StageFact, StageFactKind, StoreOptimizationJournal, verify_development_observation,
};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
    REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2, ReadRegisteredExecutionSettlement,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::CleanupState;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        candidate_bundle_digest: trusted_compiler_inputs::software_candidate(
            NAMESPACE,
            &control.environment_digest,
            "registered development fixture",
        )
        .digest,
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
    candidate_bundle_digest: String,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("development.sqlite3"))
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
        let candidate = trusted_compiler_inputs::software_candidate_current(
            &store,
            &executor,
            &control.environment_digest,
            "registered development fixture",
            1,
        )
        .await
        .unwrap();
        trusted_compiler_inputs::persist_bundle_current(
            &store,
            &executor,
            &candidate,
            &control.environment_digest,
            1,
        )
        .await
        .unwrap();
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        // Registration is idempotent for identical content only.
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        let mut changed = control.clone();
        changed.created_at_unix_seconds = 2;
        assert!(matches!(
            register_development_control(&admin, &store, changed).await,
            Err(Error::Conflict(_))
        ));
        Self {
            _dir: dir,
            store,
            admin,
            executor,
            grader,
            control,
            candidate_bundle_digest: candidate.digest,
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

    fn request(&self) -> DevelopmentRunRequest {
        let mut request = build_request(&self.control, "dev-request-1", "episode-1");
        request.candidate_bundle_digest = self.candidate_bundle_digest.clone();
        request
    }

    /// Commits the request/observation pair the way the E03 step and its
    /// recovery journal would: with run, watermark, and artifact extras.
    async fn commit_facts(
        &self,
        request: &DevelopmentRunRequest,
        report: &DevelopmentRunReport,
    ) -> (String, String) {
        let inputs = trusted_compiler_inputs::bundle_use_current(
            &self.store,
            &self.admin,
            &request.candidate_bundle_digest,
            &request.environment_digest,
            request.revoke_watermark,
        )
        .await
        .unwrap();
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
        let mut request_dependencies = extras.clone();
        request_dependencies.push(StageDependency {
            kind: "environment".into(),
            id: request.environment_digest.clone(),
        });
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
            dependencies: request_dependencies,
            payload: serde_json::to_value(request).unwrap(),
        }
        .seal()
        .unwrap();
        let observed_fact = observed_fact(request, report, extras);
        journal
            .commit_source_bound(request_fact.clone(), &inputs)
            .await
            .unwrap();
        journal
            .commit_source_bound(observed_fact.clone(), &inputs)
            .await
            .unwrap();
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
    dependencies.extend(typed_receipt_closure(report).unwrap());
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

/// Overwrites the stored observation with an attacker-shaped body at the same
/// canonical id (raw storage access), then returns the gate's verdict.
async fn gate_with_forged_observation(
    fixture: &Fixture,
    request_fact_id: &str,
    honest: &StageFact,
    forged: &StageFact,
) -> Result<(), Error> {
    assert_eq!(honest.artifact_id, forged.artifact_id);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &forged.artifact_id,
            "admin",
            forged,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let verdict = verify_development_observation(
        &fixture.store,
        &fixture.admin,
        request_fact_id,
        &forged.artifact_id,
    )
    .await
    .map(|_| ());
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &honest.artifact_id,
            "admin",
            honest,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    verdict
}

#[tokio::test]
async fn registered_runner_issues_zero_cost_receipts_and_the_gate_verifies_them() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    assert_eq!(
        report.provenance,
        DevelopmentExecutionProvenance::RegisteredPureFunction
    );
    assert_eq!(report.results.len(), 2);
    assert_eq!(report.usage_record_ids.len(), 4);
    for result in &report.results {
        // Same registered pure function on both sides: honest identical pairs.
        assert_eq!(result.parent_score_micros, 1_000_000);
        assert_eq!(result.candidate_score_micros, 1_000_000);
        assert!(result.parent_passed && result.candidate_passed);
        for (side, receipt_id) in [
            (DevelopmentSide::Parent, &result.parent_execution_id),
            (DevelopmentSide::Candidate, &result.candidate_execution_id),
        ] {
            assert_eq!(
                receipt_id,
                &execution_receipt_id(&request.request_id, &result.task_id, side).unwrap()
            );
            let receipt = load_execution_receipt(&fixture.store, &fixture.admin, receipt_id)
                .await
                .unwrap();
            assert_eq!(receipt.control_id, CONTROL_ID);
            assert_eq!(receipt.executor_actor, "dev-executor");
            assert_eq!(receipt.executor_role, Role::Worker);
            assert_eq!(receipt.side, side);
            assert_eq!(
                receipt.cost_state,
                DevelopmentCostState::Known {
                    micros: 0,
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                }
            );
            let output =
                load_execution_output(&fixture.store, &fixture.admin, &receipt.output_artifact_id)
                    .await
                    .unwrap();
            assert_eq!(hash(output.output_utf8.as_bytes()), receipt.output_digest);
            let call = fixture
                .store
                .budget_call(&fixture.admin, SCOPE, &receipt.budget_call_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(call.state, BudgetCallState::Finalized);
            assert!(call.execution_closed);
            assert_eq!(call.stage, BudgetStage::DevelopmentExecution);
            assert_eq!(call.dispatch_group_id, "episode-1");
            assert_eq!(call.actual_cost_micros, Some(0));
            assert_eq!(call.actual_pricing_version.as_deref(), Some("pricing-v1"));
            assert_eq!(call.execution_provenance, None);
            let ReadRegisteredExecutionSettlement::Current(fact) =
                ReadRegisteredExecutionSettlement::from_call(&call).unwrap()
            else {
                panic!("the real writer must record a Current settlement");
            };
            let settlement = fact.settlement;
            assert_eq!(
                settlement.provenance,
                RegisteredExecutionProvenance::RegisteredPureFunction
            );
            assert_eq!(
                settlement.runner_digest,
                registered_runner_digest().unwrap()
            );
        }
        let grader = load_grader_receipt(
            &fixture.store,
            &fixture.admin,
            &grader_receipt_id(&request.request_id, &result.task_id).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(fingerprint(&grader).unwrap(), result.grader_receipt_digest);
        assert_eq!(grader.grader_actor, "dev-grader");
        assert_eq!(grader.grader_version, "exact-json-v1");
    }
    let root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 0);
    assert_eq!(root.reserved_micros, 0);

    // Re-running the same request reuses every receipt and budget row.
    let again = fixture.runner().run(request.clone()).await.unwrap();
    assert_eq!(fingerprint(&again).unwrap(), fingerprint(&report).unwrap());
    let mut session = fixture.store.session().await.unwrap();
    let calls = session
        .budget_calls_for_group(&fixture.admin, SCOPE, "episode-1")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(calls.len(), 4);

    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let view = verify_development_observation(
        &fixture.store,
        &fixture.admin,
        &request_fact_id,
        &observed_fact_id,
    )
    .await
    .unwrap();
    assert_eq!(view.outcomes.len(), 2);
    assert_eq!(view.outcomes[0].parent_family, "family-a");
    assert_eq!(view.outcomes[1].parent_family, "family-b");
    assert!(view.outcomes.iter().all(|outcome| outcome.candidate_passed));
    let worker_view = verify_development_observation(
        &fixture.store,
        &Context::new(NAMESPACE, "any-worker", Role::Worker).unwrap(),
        &request_fact_id,
        &observed_fact_id,
    )
    .await
    .unwrap();
    assert_eq!(worker_view.request_id, request.request_id);

    // Caller-supplied scores that differ from the server-computed result.
    let honest = observed_fact(&request, &report, vec![]);
    let mut forged_report = report.clone();
    forged_report.results[0].parent_score_micros = 0;
    forged_report.results[0].parent_passed = false;
    let forged = observed_fact(&request, &forged_report, vec![]);
    assert!(matches!(
        gate_with_forged_observation(&fixture, &request_fact_id, &honest, &forged).await,
        Err(Error::Conflict(message)) if message.contains("scores differ")
    ));
    // A fixture self-declaration never grants, even with real receipt ids.
    let mut fixture_report = report.clone();
    fixture_report.provenance = DevelopmentExecutionProvenance::Fixture;
    let forged = observed_fact(&request, &fixture_report, vec![]);
    assert!(matches!(
        gate_with_forged_observation(&fixture, &request_fact_id, &honest, &forged).await,
        Err(Error::Forbidden)
    ));
    // Dropping the typed closure edges is rejected.
    let mut without_closure = honest.clone();
    without_closure
        .dependencies
        .retain(|dependency| dependency.kind != "artifact");
    let without_closure = without_closure.seal().unwrap();
    assert!(matches!(
        gate_with_forged_observation(&fixture, &request_fact_id, &honest, &without_closure).await,
        Err(Error::Conflict(message)) if message.contains("typed receipt closure")
    ));

    // A watermark bump is drift (Conflict); a tombstone is Forbidden.
    let mut session = fixture.store.session().await.unwrap();
    session
        .bump_watermark(&fixture.admin, "unrelated-bump")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        verify_development_observation(
            &fixture.store,
            &fixture.admin,
            &request_fact_id,
            &observed_fact_id,
        )
        .await,
        Err(Error::Conflict(_))
    ));
    LifecycleCoordinator::revoke_source(&fixture.admin, &fixture.store, "run-b", "privacy", 200)
        .await
        .unwrap();
    assert!(matches!(
        verify_development_observation(
            &fixture.store,
            &fixture.admin,
            &request_fact_id,
            &observed_fact_id,
        )
        .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn receipts_from_a_different_control_are_a_conflict() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, _) = fixture.commit_facts(&request, &report).await;

    let other = control(
        "dev-control-b",
        DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
    );
    register_development_control(&fixture.admin, &fixture.store, other.clone())
        .await
        .unwrap();
    let other_runner = RegisteredDevelopmentRunner::with_clock(
        fixture.store.clone(),
        fixture.executor.clone(),
        fixture.grader.clone(),
        "dev-control-b",
        600,
        Arc::new(|| 100),
    )
    .unwrap();
    let mut other_request = build_request(&other, "dev-request-2", "episode-2");
    other_request.candidate_bundle_digest = fixture.candidate_bundle_digest.clone();
    let other_report = other_runner.run(other_request.clone()).await.unwrap();
    // The same request id can never be re-executed under a different control.
    assert!(matches!(
        other_runner.run(request.clone()).await,
        Err(Error::Conflict(_))
    ));

    let honest = observed_fact(&request, &report, vec![]);
    let mut mixed = report.clone();
    mixed.results[0].candidate_execution_id =
        other_report.results[0].candidate_execution_id.clone();
    let forged = observed_fact(&request, &mixed, vec![]);
    assert!(matches!(
        gate_with_forged_observation(&fixture, &request_fact_id, &honest, &forged).await,
        Err(Error::Conflict(message)) if message.contains("different controls")
    ));
}

#[tokio::test]
async fn roles_and_actor_separation_are_enforced_at_issuance() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let parent_id = report.results[0].parent_execution_id.clone();
    let call_id =
        execution_budget_call_id(&request.request_id, "task-a", DevelopmentSide::Parent).unwrap();
    let output = load_execution_output(
        &fixture.store,
        &fixture.admin,
        &load_execution_receipt(&fixture.store, &fixture.admin, &parent_id)
            .await
            .unwrap()
            .output_artifact_id,
    )
    .await
    .unwrap();
    let issue = |ctx: Context| {
        let issue = ExecutionReceiptIssueRequest {
            control_id: CONTROL_ID.into(),
            request: request.clone(),
            task_id: "task-a".into(),
            side: DevelopmentSide::Parent,
            output_utf8: output.output_utf8.clone(),
            budget_call_id: call_id.clone(),
            issued_at_unix_seconds: 100,
        };
        let store = &fixture.store;
        async move { issue_execution_receipt(&ctx, store, issue).await }
    };
    // The executor re-issuing identical evidence is idempotent.
    assert_eq!(
        issue(fixture.executor.clone()).await.unwrap().receipt_id,
        parent_id
    );
    // Admin is rejected even when it carries the executor's actor name.
    assert!(matches!(
        issue(Context::new(NAMESPACE, "dev-executor", Role::Admin).unwrap()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        issue(fixture.admin.clone()).await,
        Err(Error::Forbidden)
    ));
    // A Worker that is not the registered executor is rejected.
    assert!(matches!(
        issue(Context::new(NAMESPACE, "other-worker", Role::Worker).unwrap()).await,
        Err(Error::Forbidden)
    ));

    let grade = |ctx: Context| {
        let issue = GraderReceiptIssueRequest {
            control_id: CONTROL_ID.into(),
            request: request.clone(),
            task_id: "task-a".into(),
            parent_execution_receipt_id: report.results[0].parent_execution_id.clone(),
            candidate_execution_receipt_id: report.results[0].candidate_execution_id.clone(),
            scoring_budget_call_id: None,
            issued_at_unix_seconds: 100,
        };
        let store = &fixture.store;
        async move { issue_grader_receipt(&ctx, store, issue).await }
    };
    // The executor cannot grade by swapping to the Evaluator role.
    assert!(matches!(
        grade(Context::new(NAMESPACE, "dev-executor", Role::Evaluator).unwrap()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        grade(Context::new(NAMESPACE, "optimizer", Role::Evaluator).unwrap()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        grade(fixture.admin.clone()).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        grade(Context::new(NAMESPACE, "dev-grader", Role::Worker).unwrap()).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        fingerprint(&grade(fixture.grader.clone()).await.unwrap()).unwrap(),
        report.results[0].grader_receipt_digest
    );

    // A control whose executor and grader coincide cannot be registered.
    let mut same_actor = control(
        "dev-control-same",
        DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
    );
    same_actor.grader_actor = same_actor.executor_actor.clone();
    assert!(
        register_development_control(&fixture.admin, &fixture.store, same_actor)
            .await
            .is_err()
    );
    // The registering Admin cannot be the executor or the grader.
    assert!(matches!(
        register_development_control(
            &Context::new(NAMESPACE, "dev-executor", Role::Admin).unwrap(),
            &fixture.store,
            control(
                "dev-control-admin",
                DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
            ),
        )
        .await,
        Err(Error::Forbidden)
    ));
    // A fixture-scoped control can be registered but never executed as evidence.
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
    let fixture_runner = RegisteredDevelopmentRunner::with_clock(
        fixture.store.clone(),
        fixture.executor.clone(),
        fixture.grader.clone(),
        "dev-control-fixture",
        600,
        Arc::new(|| 100),
    )
    .unwrap();
    assert!(matches!(
        fixture_runner
            .run(build_request(
                &fixture.control,
                "dev-request-f",
                "episode-f"
            ))
            .await,
        Err(Error::Forbidden)
    ));
    // The runner itself refuses a shared executor/grader identity.
    assert!(matches!(
        RegisteredDevelopmentRunner::with_clock(
            fixture.store.clone(),
            fixture.executor.clone(),
            Context::new(NAMESPACE, "dev-executor", Role::Evaluator).unwrap(),
            CONTROL_ID,
            600,
            Arc::new(|| 100),
        ),
        Err(Error::Forbidden)
    ));
}

/// Reserves and dispatches one budget row as the executor, returning its fence.
async fn reserve_dispatched(
    fixture: &Fixture,
    call_id: &str,
    group: &str,
    stage: BudgetStage,
    input: &str,
) -> (BudgetCallFence, String) {
    let call = fixture
        .store
        .reserve_budget_call(
            &fixture.executor,
            &BudgetCallReservation {
                billing_scope: SCOPE.into(),
                call_id: call_id.into(),
                dispatch_group_id: group.into(),
                stage,
                actual_input_digest: input.into(),
                request_artifact: None,
                max_cost_micros: 1,
                lease_token: format!("{call_id}-lease"),
                lease_until: 1_000,
                now: 100,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 101,
    };
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&fixture.executor, &fence)
        .await
        .unwrap();
    (fence, dispatched.call.dispatch_id.unwrap())
}

async fn assert_development_reservation_released(fixture: &Fixture, reason: &str, held: i64) {
    let request = fixture.request();
    let task = &fixture.control.tasks[0];
    let call_id =
        execution_budget_call_id(&request.request_id, &task.task_id, DevelopmentSide::Parent)
            .unwrap();
    let call = fixture
        .store
        .budget_call(&fixture.executor, SCOPE, &call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Released);
    assert_eq!(call.terminal_reason.as_deref(), Some(reason));
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.executor, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        held
    );
    let receipt_id =
        execution_receipt_id(&request.request_id, &task.task_id, DevelopmentSide::Parent).unwrap();
    assert!(matches!(
        load_execution_receipt(&fixture.store, &fixture.executor, &receipt_id).await,
        Err(Error::NotFound)
    ));
    // A valid clock on retry does not turn the Released row into a free rerun.
    assert!(matches!(
        fixture.runner().run(request).await,
        Err(Error::Cancelled)
    ));
    assert_eq!(
        fixture
            .store
            .budget_call(&fixture.executor, SCOPE, &call_id)
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert!(matches!(
        load_execution_receipt(&fixture.store, &fixture.executor, &receipt_id).await,
        Err(Error::NotFound)
    ));
}

// Stage A: public runner + public clocks; no injected target error or new storage API.
#[tokio::test]
async fn ag054_public_development_concurrency_refusal_releases_one_micro() {
    let fixture = Fixture::new().await;
    reserve_dispatched(
        &fixture,
        "held",
        "held-group",
        BudgetStage::Reflection,
        &d("held"),
    )
    .await;
    assert!(matches!(
        fixture.runner().run(fixture.request()).await,
        Err(Error::Conflict(_))
    ));
    assert_development_reservation_released(
        &fixture,
        "development_pre_dispatch_v1.root_concurrency_limit",
        1,
    )
    .await;
}

#[tokio::test]
async fn ag054_public_development_lease_refusal_releases_one_micro() {
    let fixture = Fixture::new().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let runner = RegisteredDevelopmentRunner::with_clock(
        fixture.store.clone(),
        fixture.executor.clone(),
        fixture.grader.clone(),
        CONTROL_ID,
        600,
        Arc::new(move || {
            if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                100
            } else {
                800
            }
        }),
    )
    .unwrap();
    assert!(matches!(
        runner.run(fixture.request()).await,
        Err(Error::Cancelled)
    ));
    assert_development_reservation_released(
        &fixture,
        "development_pre_dispatch_v1.lease_expired",
        0,
    )
    .await;
}

fn clock_failure_sqlite_snapshot(path: &std::path::Path) -> serde_json::Value {
    let script = r#"import json,sqlite3,struct,sys
from pathlib import Path
con=sqlite3.connect(Path(sys.argv[1]).resolve().as_uri()+'?mode=ro',uri=True)
con.text_factory=bytes
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def quoted(name):
    return '"'+name.replace('"','""')+'"'
def cell(kind,value):
    kind=kind.decode('ascii')
    if kind in ('text','blob'):
        data=value.hex()
    elif kind=='integer':
        data=str(value)
    elif kind=='real':
        data=struct.pack('>d',value).hex()
    elif kind=='null':
        data=''
    else:
        raise ValueError('unknown SQLite storage class')
    return [kind,data]
result={}
for (raw_name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    name=raw_name.decode('utf-8')
    columns=[row[1].decode('utf-8') for row in con.execute('PRAGMA table_info('+quoted(name)+')')]
    projection=','.join('typeof('+quoted(column)+'),'+quoted(column) for column in columns)
    rows=con.execute('SELECT '+projection+' FROM '+quoted(name)).fetchall()
    result[name]={'columns':columns,'rows':sorted([[cell(row[i],row[i+1]) for i in range(0,len(row),2)] for row in rows])}
con.rollback()
con.close()
print(json.dumps(result,sort_keys=True))
"#;
    let output = std::process::Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn assert_development_clock_refusal_preserves_reservation(negative: i64) {
    let fixture = Fixture::new().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let failure_snapshot = Arc::new(std::sync::Mutex::new(None));
    let captured = failure_snapshot.clone();
    let path = fixture._dir.path().join("development.sqlite3");
    let clock_path = path.clone();
    let clock_reads = reads.clone();
    let runner = RegisteredDevelopmentRunner::with_clock(
        fixture.store.clone(),
        fixture.executor.clone(),
        fixture.grader.clone(),
        CONTROL_ID,
        600,
        Arc::new(move || {
            if clock_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                100
            } else {
                *captured.lock().unwrap() = Some(clock_failure_sqlite_snapshot(&clock_path));
                negative
            }
        }),
    )
    .unwrap();
    let request = fixture.request();
    let error = runner.run(request.clone()).await.unwrap_err();
    assert!(matches!(error, Error::ClockRegressed), "{error:?}");
    assert_eq!(error.to_string(), "clock_regressed");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    let before = failure_snapshot.lock().unwrap().clone().unwrap();
    assert_eq!(clock_failure_sqlite_snapshot(&path), before);

    let task = &fixture.control.tasks[0];
    let call_id =
        execution_budget_call_id(&request.request_id, &task.task_id, DevelopmentSide::Parent)
            .unwrap();
    let call = fixture
        .store
        .budget_call(&fixture.executor, SCOPE, &call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Reserved);
    assert_eq!(call.terminal_reason, None);
    assert_eq!(call.dispatch_id, None);
    assert_eq!(call.actual_cost_micros, None);
    assert_eq!(call.response_artifact, None);
    let root = fixture
        .store
        .root_budget(&fixture.executor, SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.reserved_micros, 1);
    assert_eq!(root.spent_micros, 0);
    let receipt_id =
        execution_receipt_id(&request.request_id, &task.task_id, DevelopmentSide::Parent).unwrap();
    assert!(matches!(
        load_execution_receipt(&fixture.store, &fixture.executor, &receipt_id).await,
        Err(Error::NotFound)
    ));
    // No implicit retry or release follows a rejected clock sample.
    fixture.store.close().await;
}

#[tokio::test]
async fn ag129_second_clock_failure_preserves_reserved_micro_without_new_records() {
    assert_development_clock_refusal_preserves_reservation(-1).await;
}

#[tokio::test]
async fn ag129_minimum_second_clock_preserves_reserved_micro_without_new_records() {
    assert_development_clock_refusal_preserves_reservation(i64::MIN).await;
}

#[tokio::test]
async fn budget_rows_must_be_closed_development_rows_with_a_registered_settlement() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let output_utf8 = "{\"answer\":{\"clamped\":-1}}".to_string();
    let output_digest = hash(output_utf8.as_bytes());
    let issue = |call_id: String| {
        issue_execution_receipt(
            &fixture.executor,
            &fixture.store,
            ExecutionReceiptIssueRequest {
                control_id: CONTROL_ID.into(),
                request: request.clone(),
                task_id: "task-a".into(),
                side: DevelopmentSide::Parent,
                output_utf8: output_utf8.clone(),
                budget_call_id: call_id,
                issued_at_unix_seconds: 100,
            },
        )
    };
    let charge = |usage: &str| UsageCharge {
        amount_micros: 0,
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        provider_request_id: "registered-pure-function:test".into(),
        usage_record_id: usage.into(),
        output_digest: output_digest.clone(),
    };
    let settlement =
        |call_id: &str, dispatch_id: &str, input: &str| RegisteredExecutionSettlement {
            schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
            provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
            call_id: call_id.into(),
            dispatch_id: dispatch_id.into(),
            request_digest: input.into(),
            output_digest: output_digest.clone(),
            target_id: "reference_host.clamp_i64.v1".into(),
            target_digest: fixture.control.target_digest.clone(),
            runner_digest: fixture.control.runner_digest.clone(),
        };

    // Wrong stage: the storage layer refuses the registered settlement.
    let (fence, dispatch_id) = reserve_dispatched(
        &fixture,
        "call-stage",
        "episode-1",
        BudgetStage::Reflection,
        &d("x"),
    )
    .await;
    assert!(matches!(
        fixture
            .store
            .settle_registered_execution_call(
                &fixture.executor,
                &fence,
                &charge("usage-stage"),
                &settlement("call-stage", &dispatch_id, &d("x")),
            )
            .await,
        Err(Error::Conflict(_))
    ));
    // Nonzero cost is never a registered pure-function execution.
    let mut paid = charge("usage-stage");
    paid.amount_micros = 1;
    assert!(matches!(
        fixture
            .store
            .settle_registered_execution_call(
                &fixture.executor,
                &fence,
                &paid,
                &settlement("call-stage", &dispatch_id, &d("x")),
            )
            .await,
        Err(Error::Invalid(_))
    ));
    // Admin cannot settle a registered execution row.
    assert!(matches!(
        fixture
            .store
            .settle_registered_execution_call(
                &fixture.admin,
                &fence,
                &charge("usage-stage"),
                &settlement("call-stage", &dispatch_id, &d("x")),
            )
            .await,
        Err(Error::Forbidden)
    ));
    // Finalized through the generic path and closed: still the wrong stage.
    fixture
        .store
        .finalize_budget_call(&fixture.executor, &fence, &charge("usage-stage"))
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.executor,
            SCOPE,
            "call-stage",
            &dispatch_id,
            "test_cleanup",
            102,
        )
        .await
        .unwrap();
    assert!(matches!(
        issue("call-stage".into()).await,
        Err(Error::Conflict(_))
    ));

    // Wrong dispatch group and wrong input digest: settled, but unbound.
    let (fence, dispatch_id) = reserve_dispatched(
        &fixture,
        "call-group",
        "other-episode",
        BudgetStage::DevelopmentExecution,
        &d("y"),
    )
    .await;
    fixture
        .store
        .settle_registered_execution_call(
            &fixture.executor,
            &fence,
            &charge("usage-group"),
            &settlement("call-group", &dispatch_id, &d("y")),
        )
        .await
        .unwrap();
    assert!(matches!(
        issue("call-group".into()).await,
        Err(Error::Conflict(_))
    ));
    let (fence, dispatch_id) = reserve_dispatched(
        &fixture,
        "call-input",
        "episode-1",
        BudgetStage::DevelopmentExecution,
        &d("z"),
    )
    .await;
    fixture
        .store
        .settle_registered_execution_call(
            &fixture.executor,
            &fence,
            &charge("usage-input"),
            &settlement("call-input", &dispatch_id, &d("z")),
        )
        .await
        .unwrap();
    assert!(matches!(
        issue("call-input".into()).await,
        Err(Error::Conflict(_))
    ));

    // Finalized without the registered settlement and not closed: unbound.
    let (fence, _dispatch_id) = reserve_dispatched(
        &fixture,
        "call-open",
        "episode-1",
        BudgetStage::DevelopmentExecution,
        &d("w"),
    )
    .await;
    fixture
        .store
        .finalize_budget_call(&fixture.executor, &fence, &charge("usage-open"))
        .await
        .unwrap();
    assert!(matches!(
        issue("call-open".into()).await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        issue("missing-call".into()).await,
        Err(Error::NotFound)
    ));
}

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

#[tokio::test]
async fn e12_record_cycle_accepts_a_real_registered_cycle_and_revocation_blocks_it() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    let coordinator = curriculum(&fixture).await;
    let state = coordinator
        .record_cycle(DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: "cycle-1".into(),
            state_id: "learner-state".into(),
            development_request_fact_id: request_fact_id.clone(),
            development_observed_fact_id: observed_fact_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(state.completed_cycles.len(), 1);
    let cycle = &state.completed_cycles[0];
    assert_eq!(cycle.cycle_id, "development-cycle-episode-1-1-1");
    assert_eq!(cycle.cluster_ids, vec!["family-a", "family-b"]);
    assert_eq!(cycle.successful_clusters, 2);
    assert_eq!(cycle.paired_gain_micros, vec![0, 0]);
    assert_eq!(cycle.environment_digest, fixture.control.environment_digest);
    assert_eq!(cycle.grader_digest, fixture.control.grader_digest);
    assert!(state.development_fact_ids.contains(&request_fact_id));
    assert!(state.development_fact_ids.contains(&observed_fact_id));
    assert!(state.failure_clusters.is_empty());

    // Idempotent replay of the same cycle receipt.
    let replay = coordinator
        .record_cycle(DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: "cycle-1".into(),
            state_id: "learner-state".into(),
            development_request_fact_id: request_fact_id.clone(),
            development_observed_fact_id: observed_fact_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(replay.completed_cycles.len(), 1);

    // Revoking a source run blocks the receipts and any further derived cycle.
    LifecycleCoordinator::revoke_source(&fixture.admin, &fixture.store, "run-a", "privacy", 300)
        .await
        .unwrap();
    assert!(matches!(
        verify_development_observation(
            &fixture.store,
            &fixture.admin,
            &request_fact_id,
            &observed_fact_id,
        )
        .await,
        Err(Error::Forbidden)
    ));
    assert!(
        coordinator
            .record_cycle(DevelopmentCycleReceiptV1 {
                schema_version: "rsia.development_cycle_receipt.v1".into(),
                id: "cycle-2".into(),
                state_id: "learner-state".into(),
                development_request_fact_id: request_fact_id,
                development_observed_fact_id: observed_fact_id,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revocation_cleanup_traverses_receipts_and_redacts_outputs() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
    verify_development_observation(
        &fixture.store,
        &fixture.admin,
        &request_fact_id,
        &observed_fact_id,
    )
    .await
    .unwrap();
    let parent = load_execution_receipt(
        &fixture.store,
        &fixture.admin,
        &report.results[0].parent_execution_id,
    )
    .await
    .unwrap();

    let mut status = LifecycleCoordinator::revoke_source(
        &fixture.admin,
        &fixture.store,
        "run-a",
        "privacy",
        200,
    )
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
    assert_eq!(status.state, CleanupState::Complete);

    let mut session = fixture.store.session().await.unwrap();
    let output: serde_json::Value = session
        .need(
            &fixture.admin,
            "artifact",
            &storage_id(EXECUTION_OUTPUT_KIND, &parent.output_artifact_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(output["schema_version"], "rsia.redacted.v1");
    let receipt: serde_json::Value = session
        .need(
            &fixture.admin,
            "artifact",
            &storage_id(EXECUTION_RECEIPT_KIND, &parent.receipt_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["schema_version"], "rsia.typed_artifact_envelope.v1");
    let observed: serde_json::Value = session
        .need(&fixture.admin, "artifact", &observed_fact_id)
        .await
        .unwrap();
    assert_eq!(observed["schema_version"], "rsia.redacted.v1");
    session.commit().await.unwrap();

    assert!(matches!(
        verify_development_observation(
            &fixture.store,
            &fixture.admin,
            &request_fact_id,
            &observed_fact_id,
        )
        .await,
        Err(Error::Conflict(message)) if message.contains("redacted")
    ));
    assert!(matches!(
        load_execution_output(&fixture.store, &fixture.admin, &parent.output_artifact_id).await,
        Err(Error::Conflict(_))
    ));
    assert!(fixture.runner().run(request).await.is_err());
}

async fn reserve_development_parent(fixture: &Fixture) -> evo_storage::budget::BudgetCallRecord {
    let request = fixture.request();
    let task = &fixture.control.tasks[0];
    let side = DevelopmentSide::Parent;
    let request_digest = execution_request_digest(&fixture.control, &request, task, side).unwrap();
    let stored_request = RegisteredExecutionRequestV1 {
        schema_version: REGISTERED_EXECUTION_REQUEST_SCHEMA.into(),
        control_id: fixture.control.id.clone(),
        request_id: request.request_id.clone(),
        episode_id: request.episode_id.clone(),
        task_id: task.task_id.clone(),
        side,
        input: task.input,
        input_digest: task.input_digest.clone(),
        bundle_digest: request.parent_bundle_digest.clone(),
        environment_digest: request.environment_digest.clone(),
        request_digest: request_digest.clone(),
        target_id: task.target_id.clone(),
        target_digest: fixture.control.target_digest.clone(),
        runner_digest: fixture.control.runner_digest.clone(),
        source_ids: fixture.control.source_ids.clone(),
    };
    fixture
        .store
        .reserve_budget_call_with_sources(
            &fixture.executor,
            &BudgetCallReservation {
                billing_scope: SCOPE.into(),
                call_id: execution_budget_call_id(&request.request_id, &task.task_id, side)
                    .unwrap(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::DevelopmentExecution,
                actual_input_digest: request_digest,
                request_artifact: Some(
                    BudgetArtifact::from_serializable(
                        REGISTERED_EXECUTION_REQUEST_SCHEMA,
                        &stored_request,
                    )
                    .unwrap(),
                ),
                max_cost_micros: 1,
                lease_token: "preexisting".into(),
                lease_until: 1000,
                now: 100,
            },
            &fixture.control.source_ids,
        )
        .await
        .unwrap()
}

fn development_fence(call: &evo_storage::budget::BudgetCallRecord) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 100,
    }
}

#[tokio::test]
async fn ag054_public_dispatched_and_unknown_development_calls_are_never_refunded() {
    for uncertain in [false, true] {
        let fixture = Fixture::new().await;
        let call = reserve_development_parent(&fixture).await;
        let fence = development_fence(&call);
        fixture
            .store
            .begin_budget_dispatch(&fixture.executor, &fence)
            .await
            .unwrap();
        if uncertain {
            fixture
                .store
                .mark_budget_call_uncertain(&fixture.executor, &fence, "unknown execution")
                .await
                .unwrap();
        }
        let before = fixture
            .store
            .budget_call(&fixture.executor, SCOPE, &call.call_id)
            .await
            .unwrap()
            .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let runner = RegisteredDevelopmentRunner::with_clock(
            fixture.store.clone(),
            fixture.executor.clone(),
            fixture.grader.clone(),
            CONTROL_ID,
            600,
            Arc::new(move || {
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    100
                } else {
                    -1
                }
            }),
        )
        .unwrap();
        assert!(runner.run(fixture.request()).await.is_err());
        assert_eq!(
            fixture
                .store
                .budget_call(&fixture.executor, SCOPE, &call.call_id)
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert_eq!(
            fixture
                .store
                .root_budget(&fixture.executor, SCOPE)
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            1
        );
        let receipt = execution_receipt_id(
            &fixture.request().request_id,
            &fixture.control.tasks[0].task_id,
            DevelopmentSide::Parent,
        )
        .unwrap();
        assert!(matches!(
            load_execution_receipt(&fixture.store, &fixture.executor, &receipt).await,
            Err(Error::NotFound)
        ));
    }
}

#[tokio::test]
async fn ag054_public_cancelled_development_call_retries_keep_terminal_facts() {
    let fixture = Fixture::new().await;
    let call = reserve_development_parent(&fixture).await;
    let terminal = fixture
        .store
        .cancel_budget_call(
            &fixture.executor,
            &development_fence(&call),
            "development_pre_dispatch_v1.source_revoked",
        )
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            fixture.runner().run(fixture.request()).await,
            Err(Error::Cancelled)
        ));
        assert_eq!(
            fixture
                .store
                .budget_call(&fixture.executor, SCOPE, &call.call_id)
                .await
                .unwrap()
                .unwrap(),
            terminal
        );
        assert_eq!(
            fixture
                .store
                .root_budget(&fixture.executor, SCOPE)
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            0
        );
    }
}

#[tokio::test]
async fn ag054_public_receipt_clock_failure_after_begin_keeps_finalized_zero_cost_facts() {
    let fixture = Fixture::new().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let failure_snapshot = Arc::new(std::sync::Mutex::new(None));
    let captured = failure_snapshot.clone();
    let path = fixture._dir.path().join("development.sqlite3");
    let clock_path = path.clone();
    let runner = RegisteredDevelopmentRunner::with_clock(
        fixture.store.clone(),
        fixture.executor.clone(),
        fixture.grader.clone(),
        CONTROL_ID,
        600,
        Arc::new(move || {
            if reads.fetch_add(1, Ordering::SeqCst) < 2 {
                100
            } else {
                *captured.lock().unwrap() = Some(clock_failure_sqlite_snapshot(&clock_path));
                -1
            }
        }),
    )
    .unwrap();
    let error = runner.run(fixture.request()).await.unwrap_err();
    assert!(matches!(error, Error::ClockRegressed), "{error:?}");
    assert_eq!(error.to_string(), "clock_regressed");
    let before = failure_snapshot.lock().unwrap().clone().unwrap();
    assert_eq!(clock_failure_sqlite_snapshot(&path), before);
    let call_id = execution_budget_call_id(
        &fixture.request().request_id,
        &fixture.control.tasks[0].task_id,
        DevelopmentSide::Parent,
    )
    .unwrap();
    let call = fixture
        .store
        .budget_call(&fixture.executor, SCOPE, &call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert!(call.dispatch_id.is_some());
    assert_eq!(call.actual_cost_micros, Some(0));
    assert!(call.response_artifact.is_some());
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.executor, SCOPE)
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        0
    );
    assert!(
        !call
            .terminal_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("development_pre_dispatch_v1."))
    );
    let receipt_id = execution_receipt_id(
        &fixture.request().request_id,
        &fixture.control.tasks[0].task_id,
        DevelopmentSide::Parent,
    )
    .unwrap();
    assert!(matches!(
        load_execution_receipt(&fixture.store, &fixture.executor, &receipt_id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_runner_rechecks_the_reserved_bundle_before_dispatch_without_a_watermark_change()
{
    for switch_pointer in [false, true] {
        let fixture = Fixture::new().await;
        let request = fixture.request();
        let mut s = fixture.store.session().await.unwrap();
        let original: evo_engine::compiler::CurrentAuthenticatedBundleRecord = s
            .need(
                &fixture.executor,
                "artifact",
                &format!("formal-bundle-{}", request.candidate_bundle_digest),
            )
            .await
            .unwrap();
        let watermark = s.watermark(&fixture.executor).await.unwrap();
        s.commit().await.unwrap();
        let paused = Arc::new(tokio::sync::Notify::new());
        let clock_paused = paused.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let (resume, waiting) = std::sync::mpsc::channel();
        let waiting = std::sync::Mutex::new(waiting);
        let runner = RegisteredDevelopmentRunner::with_clock(
            fixture.store.clone(),
            fixture.executor.clone(),
            fixture.grader.clone(),
            CONTROL_ID,
            600,
            Arc::new(move || {
                if reads.fetch_add(1, Ordering::SeqCst) == 1 {
                    // The second clock read follows the committed reserve and
                    // precedes the actual dispatch boundary. No target runs here.
                    clock_paused.notify_one();
                    if tokio::task::block_in_place(|| {
                        waiting
                            .lock()
                            .unwrap()
                            .recv_timeout(std::time::Duration::from_secs(30))
                    })
                    .is_err()
                    {
                        return -1;
                    }
                }
                100
            }),
        )
        .unwrap();
        let running_request = request.clone();
        let running = tokio::spawn(async move { runner.run(running_request).await });
        tokio::time::timeout(std::time::Duration::from_secs(30), paused.notified())
            .await
            .unwrap();
        let call_id = execution_budget_call_id(
            &request.request_id,
            &fixture.control.tasks[0].task_id,
            DevelopmentSide::Parent,
        )
        .unwrap();
        let mut s = fixture.store.session().await.unwrap();
        let reserved = s
            .budget_call(&fixture.admin, SCOPE, &call_id)
            .await
            .unwrap()
            .unwrap();
        let was_reserved_without_dispatch =
            reserved.state == BudgetCallState::Reserved && reserved.dispatch_id.is_none();
        if switch_pointer {
            let id = format!("profile-pointer-{}", original.input.profile.id);
            let pointer = evo_engine::release_store::PersistentProfilePointer {
                id: id.clone(),
                schema_version: evo_engine::release_store::PROFILE_POINTER_SCHEMA.into(),
                profile_id: original.input.profile.id.clone(),
                active_release_id: None,
                active_bundle_digest: None,
                epoch: 1,
            };
            s.put(
                &fixture.admin,
                "pointer",
                &id,
                fixture.admin.actor(),
                &pointer,
            )
            .await
            .unwrap();
            let operation = format!(
                "formal.admin.content.v1.{}",
                fingerprint(&(NAMESPACE, "pointer", &id)).unwrap()
            );
            let entity = fingerprint(&(
                NAMESPACE,
                "pointer",
                &id,
                evo_engine::formal_authority::content_digest(&pointer).unwrap(),
            ))
            .unwrap();
            s.audit(&fixture.admin, &operation, &entity).await.unwrap();
        }
        let after_pointer_watermark = s.watermark(&fixture.admin).await.unwrap();
        s.commit().await.unwrap();
        resume.send(()).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), running)
            .await
            .unwrap()
            .unwrap();
        assert!(
            was_reserved_without_dispatch,
            "fault placement must follow the real Reserved commit"
        );
        assert_eq!(after_pointer_watermark, watermark);
        let call = fixture
            .store
            .budget_call(&fixture.admin, SCOPE, &call_id)
            .await
            .unwrap()
            .unwrap();
        let root = fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.reserved_micros, 0);
        assert_eq!(root.spent_micros, 0);
        if switch_pointer {
            assert!(matches!(result, Err(Error::Conflict(ref message))
                if message == "compiler authority changed before continuation"));
            assert_eq!(call.state, BudgetCallState::Released);
            assert_eq!(
                call.terminal_reason.as_deref(),
                Some("development_pre_dispatch_v1.compiler_inputs_changed")
            );
            assert_eq!(call.dispatch_id, None);
            assert_eq!(call.actual_cost_micros, None);
            let receipt_id = execution_receipt_id(
                &request.request_id,
                &fixture.control.tasks[0].task_id,
                DevelopmentSide::Parent,
            )
            .unwrap();
            assert!(matches!(
                load_execution_receipt(&fixture.store, &fixture.executor, &receipt_id).await,
                Err(Error::NotFound)
            ));
        } else {
            let report = result.unwrap();
            assert_eq!(
                report.provenance,
                DevelopmentExecutionProvenance::RegisteredPureFunction
            );
            assert_eq!(report.results.len(), fixture.control.tasks.len());
            assert_eq!(
                report.usage_record_ids.len(),
                fixture.control.tasks.len() * 2
            );
            assert_eq!(call.state, BudgetCallState::Finalized);
            assert_eq!(call.actual_cost_micros, Some(0));
            assert!(call.execution_closed);
            assert!(call.dispatch_id.is_some());
        }
        fixture.store.close().await;
    }
}

#[test]
fn ag129_snapshot_preserves_nul_text_real_bits_and_storage_classes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite3");
    let output = std::process::Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(
            "import math,sqlite3,sys\n\
             con=sqlite3.connect(sys.argv[1])\n\
             con.execute('CREATE TABLE samples(value)')\n\
             values=['prefix\\x00a','prefix\\x00b',7,'7',1.0,math.nextafter(1.0,math.inf),None,b'',b'\\x00']\n\
             con.executemany('INSERT INTO samples(value) VALUES(?)',[(value,) for value in values])\n\
             con.commit()\n\
             con.close()\n",
        )
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let snapshot = clock_failure_sqlite_snapshot(&path);
    let rows = snapshot["samples"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 9);
    let unique: std::collections::BTreeSet<_> =
        rows.iter().map(serde_json::Value::to_string).collect();
    assert_eq!(unique.len(), 9);
    for expected in [
        serde_json::json!([["text", "7072656669780061"]]),
        serde_json::json!([["text", "7072656669780062"]]),
        serde_json::json!([["integer", "7"]]),
        serde_json::json!([["text", "37"]]),
        serde_json::json!([["real", "3ff0000000000000"]]),
        serde_json::json!([["real", "3ff0000000000001"]]),
        serde_json::json!([["null", ""]]),
        serde_json::json!([["blob", ""]]),
        serde_json::json!([["blob", "00"]]),
    ] {
        assert!(
            rows.contains(&expected),
            "lost logical SQLite value {expected}"
        );
    }
}

#[tokio::test]
async fn current_bound_pair_reloads_task_control_grader_and_metered_receipts() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact, observation) = fixture.commit_facts(&request, &report).await;
    let healthy =
        verify_development_observation(&fixture.store, &fixture.admin, &request_fact, &observation)
            .await
            .unwrap();
    assert_eq!(healthy.outcomes.len(), fixture.control.tasks.len());
    for result in &report.results {
        let parent =
            load_execution_receipt(&fixture.store, &fixture.admin, &result.parent_execution_id)
                .await
                .unwrap();
        let candidate = load_execution_receipt(
            &fixture.store,
            &fixture.admin,
            &result.candidate_execution_id,
        )
        .await
        .unwrap();
        assert_eq!(parent.task_id, candidate.task_id);
        assert_eq!(parent.control_id, fixture.control.id);
        assert_eq!(candidate.control_id, fixture.control.id);
        assert_eq!(parent.side, DevelopmentSide::Parent);
        assert_eq!(candidate.side, DevelopmentSide::Candidate);
        let grade = load_grader_receipt(
            &fixture.store,
            &fixture.admin,
            &grader_receipt_id(&request.request_id, &result.task_id).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(fingerprint(&grade).unwrap(), result.grader_receipt_digest);
        for execution in [parent, candidate] {
            let call = fixture
                .store
                .budget_call(&fixture.admin, SCOPE, &execution.budget_call_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(call.state, BudgetCallState::Finalized);
            assert_eq!(call.actual_cost_micros, Some(0));
            assert!(call.execution_closed);
            let ReadRegisteredExecutionSettlement::Current(fact) =
                ReadRegisteredExecutionSettlement::from_call(&call).unwrap()
            else {
                panic!("the real writer must record a Current settlement");
            };
            assert_eq!(
                fact.settlement.runner_digest,
                registered_runner_digest().unwrap()
            );
        }
    }
    let before_root = fixture
        .store
        .root_budget(&fixture.admin, SCOPE)
        .await
        .unwrap()
        .unwrap();
    let before_audit = fixture.store.verify_audit(&fixture.admin).await.unwrap();
    let mut changed = report.clone();
    changed.results[0].grader_receipt_digest = d("another legitimate-looking grader receipt");
    let honest = observed_fact(&request, &report, vec![]);
    let forged = observed_fact(&request, &changed, vec![]);
    assert!(matches!(
        gate_with_forged_observation(&fixture, &request_fact, &honest, &forged).await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap(),
        before_root
    );
    assert_eq!(
        fixture.store.verify_audit(&fixture.admin).await.unwrap(),
        before_audit
    );
    verify_development_observation(&fixture.store, &fixture.admin, &request_fact, &observation)
        .await
        .unwrap();
}

// A real registered request/reserve/dispatch fixture. Each test chooses the
// actual public financial writer; this helper never constructs a Current fact.
async fn reserve_execution(
    fixture: &Fixture,
    request: &DevelopmentRunRequest,
    task_id: &str,
    side: DevelopmentSide,
    output_utf8: &str,
) -> (BudgetCallFence, UsageCharge, RegisteredExecutionSettlement) {
    let task = fixture.control.task(task_id).unwrap();
    let request_digest = execution_request_digest(&fixture.control, request, task, side).unwrap();
    let call_id = execution_budget_call_id(&request.request_id, task_id, side).unwrap();
    let frozen = RegisteredExecutionRequestV1 {
        schema_version: REGISTERED_EXECUTION_REQUEST_SCHEMA.into(),
        control_id: fixture.control.id.clone(),
        request_id: request.request_id.clone(),
        episode_id: request.episode_id.clone(),
        task_id: task_id.into(),
        side,
        input: task.input,
        input_digest: task.input_digest.clone(),
        bundle_digest: match side {
            DevelopmentSide::Parent => request.parent_bundle_digest.clone(),
            DevelopmentSide::Candidate => request.candidate_bundle_digest.clone(),
        },
        environment_digest: request.environment_digest.clone(),
        request_digest: request_digest.clone(),
        target_id: task.target_id.clone(),
        target_digest: fixture.control.target_digest.clone(),
        runner_digest: fixture.control.runner_digest.clone(),
        source_ids: fixture.control.source_ids.clone(),
    };
    let call = fixture
        .store
        .reserve_budget_call(
            &fixture.executor,
            &BudgetCallReservation {
                billing_scope: SCOPE.into(),
                call_id: call_id.clone(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::DevelopmentExecution,
                actual_input_digest: request_digest.clone(),
                request_artifact: Some(
                    BudgetArtifact::from_serializable(REGISTERED_EXECUTION_REQUEST_SCHEMA, &frozen)
                        .unwrap(),
                ),
                max_cost_micros: 1,
                lease_token: format!("{call_id}-lease"),
                lease_until: 1_000,
                now: 100,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: call_id.clone(),
        actual_input_digest: request_digest.clone(),
        lease_token: call.lease_token,
        lease_epoch: call.lease_epoch,
        now: 101,
    };
    let dispatch = fixture
        .store
        .begin_budget_dispatch(&fixture.executor, &fence)
        .await
        .unwrap();
    let output_digest = hash(output_utf8.as_bytes());
    (
        fence,
        UsageCharge {
            amount_micros: 0,
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
            provider_request_id: format!("registered-pure-function:{call_id}"),
            usage_record_id: format!("usage-{call_id}"),
            output_digest: output_digest.clone(),
        },
        RegisteredExecutionSettlement {
            schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
            provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
            call_id,
            dispatch_id: dispatch.call.dispatch_id.unwrap(),
            request_digest,
            output_digest,
            target_id: task.target_id.clone(),
            target_digest: fixture.control.target_digest.clone(),
            runner_digest: fixture.control.runner_digest.clone(),
        },
    )
}

#[tokio::test]
async fn settlement_records_actual_actor_and_role_without_caller_fields() {
    let fixture = Fixture::new().await;
    let output_digest = hash(br#"{"answer":{"clamped":-1}}"#);
    for (index, settler) in [
        fixture.executor.clone(),
        Context::new(NAMESPACE, "another-registered-worker", Role::Worker).unwrap(),
        Context::new(NAMESPACE, "registered-host", Role::Host).unwrap(),
    ]
    .iter()
    .enumerate()
    {
        let call_id = format!("ag183-writer-{index}");
        let request_digest = d(&format!("ag183-input-{index}"));
        let (fence, dispatch_id) = reserve_dispatched(
            &fixture,
            &call_id,
            &format!("ag183-group-{index}"),
            BudgetStage::DevelopmentExecution,
            &request_digest,
        )
        .await;
        let submission = RegisteredExecutionSettlement {
            schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
            provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
            call_id: call_id.clone(),
            dispatch_id,
            request_digest,
            output_digest: output_digest.clone(),
            target_id: "reference_host.clamp_i64.v1".into(),
            target_digest: fixture.control.target_digest.clone(),
            runner_digest: fixture.control.runner_digest.clone(),
        };
        let mut forged = serde_json::to_value(&submission).unwrap();
        forged["settler_actor"] = serde_json::json!(fixture.control.executor_actor);
        forged["settler_role"] = serde_json::json!("worker");
        let error = serde_json::from_value::<RegisteredExecutionSettlement>(forged).unwrap_err();
        assert!(error.to_string().contains("unknown field `settler_actor`"));
        let charge = UsageCharge {
            amount_micros: 0,
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
            provider_request_id: "registered-pure-function:ag183".into(),
            usage_record_id: format!("ag183-writer-usage-{index}"),
            output_digest: output_digest.clone(),
        };
        let before = fixture
            .store
            .budget_call(&fixture.admin, SCOPE, &call_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            fixture
                .store
                .settle_registered_execution_call(&fixture.admin, &fence, &charge, &submission)
                .await,
            Err(Error::Forbidden)
        ));
        assert_eq!(
            fixture
                .store
                .budget_call(&fixture.admin, SCOPE, &call_id)
                .await
                .unwrap()
                .unwrap(),
            before
        );
        let settled = fixture
            .store
            .settle_registered_execution_call(settler, &fence, &charge, &submission)
            .await
            .unwrap();
        assert_eq!(settled.state, BudgetCallState::Finalized);
        assert_eq!(settled.actual_cost_micros, Some(0));
        assert!(settled.execution_closed);
        assert_eq!(
            settled.response_artifact.as_ref().unwrap().schema_version,
            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2
        );
        let ReadRegisteredExecutionSettlement::Current(fact) =
            ReadRegisteredExecutionSettlement::from_call(&settled).unwrap()
        else {
            panic!("new writer must persist its actual caller facts");
        };
        assert_eq!(fact.settler_actor, settler.actor());
        assert_eq!(fact.settler_role, settler.role());
        assert_eq!(fact.settlement, submission);
        let mut model_row = settled.clone();
        model_row.execution_provenance =
            Some(evo_storage::budget::BudgetExecutionProvenance::ExternalProvider);
        assert!(matches!(
            ReadRegisteredExecutionSettlement::from_call(&model_row),
            Err(Error::Conflict(message)) if message == "registered execution settlement is invalid"
        ));
        if index > 0 {
            assert_ne!(fact.settler_actor, fixture.control.executor_actor);
        }
        let root = fixture
            .store
            .root_budget(&fixture.admin, SCOPE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.spent_micros, 0);
        assert_eq!(root.reserved_micros, 0);
    }
    // Financial writer facts are not execution receipts or a control/executor match.
    fixture.store.close().await;
}

#[tokio::test]
async fn settlement_preserves_legacy_bytes_and_current_identity_on_replay() {
    let fixture = Fixture::new().await;
    let call_id = "ag183-replay";
    let request_digest = d("ag183-replay-input");
    let output_digest = hash(br#"{"answer":{"clamped":-1}}"#);
    let (fence, dispatch_id) = reserve_dispatched(
        &fixture,
        call_id,
        "ag183-replay-group",
        BudgetStage::DevelopmentExecution,
        &request_digest,
    )
    .await;
    let submission = RegisteredExecutionSettlement {
        schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
        provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
        call_id: call_id.into(),
        dispatch_id,
        request_digest,
        output_digest: output_digest.clone(),
        target_id: "reference_host.clamp_i64.v1".into(),
        target_digest: fixture.control.target_digest.clone(),
        runner_digest: fixture.control.runner_digest.clone(),
    };
    let charge = UsageCharge {
        amount_micros: 0,
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        provider_request_id: "registered-pure-function:ag183-replay".into(),
        usage_record_id: "ag183-replay-usage".into(),
        output_digest,
    };
    let first = fixture
        .store
        .settle_registered_execution_call(&fixture.executor, &fence, &charge, &submission)
        .await
        .unwrap();
    let path = fixture._dir.path().join("development.sqlite3");
    let current_before = clock_failure_sqlite_snapshot(&path);
    for resumer in [
        Context::new(NAMESPACE, "another-resumer", Role::Worker).unwrap(),
        Context::new(NAMESPACE, fixture.executor.actor(), Role::Host).unwrap(),
    ] {
        let replay = fixture
            .store
            .settle_registered_execution_call(&resumer, &fence, &charge, &submission)
            .await
            .unwrap();
        assert_eq!(replay, first);
        let ReadRegisteredExecutionSettlement::Current(fact) =
            ReadRegisteredExecutionSettlement::from_call(&replay).unwrap()
        else {
            panic!("replay cannot downgrade the current settlement");
        };
        assert_eq!(fact.settler_actor, fixture.executor.actor());
        assert_eq!(fact.settler_role, fixture.executor.role());
    }
    assert_eq!(clock_failure_sqlite_snapshot(&path), current_before);

    // Explicitly derived v1 wire fixture, not a claim of a native historical run.
    // Only its response payload is replaced; no current actor fact is fabricated.
    let legacy = submission.artifact().unwrap();
    let legacy_path = fixture._dir.path().join("derived-v1.sqlite3");
    let copied = std::process::Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(r#"
import sqlite3,sys
source,destination,schema,digest,body,scope,call_id=sys.argv[1:]
con=sqlite3.connect(source)
con.execute('VACUUM INTO ?', (destination,))
con.close()
con=sqlite3.connect(destination)
changed=con.execute('UPDATE root_budget_calls SET response_artifact_schema=?,response_artifact_digest=?,response_artifact_body=? WHERE billing_scope=? AND call_id=?',(schema,digest,body,scope,call_id))
assert changed.rowcount==1
con.commit()
con.close()
"#)
        .arg(&path).arg(&legacy_path)
        .arg(&legacy.schema_version).arg(&legacy.digest).arg(&legacy.body)
        .arg(SCOPE).arg(call_id)
        .output().unwrap();
    assert!(copied.status.success(), "{copied:?}");
    let old_store = Store::open(&legacy_path).await.unwrap();
    let old = old_store
        .budget_call(&fixture.admin, SCOPE, call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.response_artifact.as_ref(), Some(&legacy));
    assert!(
        matches!(ReadRegisteredExecutionSettlement::from_call(&old).unwrap(), ReadRegisteredExecutionSettlement::Legacy(value) if value == submission)
    );
    let mut model_legacy = old.clone();
    model_legacy.execution_provenance =
        Some(evo_storage::budget::BudgetExecutionProvenance::ExternalProvider);
    assert!(
        matches!(ReadRegisteredExecutionSettlement::from_call(&model_legacy).unwrap(), ReadRegisteredExecutionSettlement::Legacy(value) if value == submission)
    );
    let old_before = clock_failure_sqlite_snapshot(&legacy_path);
    let replay = old_store
        .settle_registered_execution_call(
            &Context::new(NAMESPACE, "legacy-resumer", Role::Host).unwrap(),
            &fence,
            &charge,
            &submission,
        )
        .await
        .unwrap();
    assert_eq!(replay, old);
    assert_eq!(replay.response_artifact.as_ref().unwrap().body, legacy.body);
    assert!(matches!(
        ReadRegisteredExecutionSettlement::from_call(&replay).unwrap(),
        ReadRegisteredExecutionSettlement::Legacy(_)
    ));
    assert_eq!(clock_failure_sqlite_snapshot(&legacy_path), old_before);
    assert_eq!(
        fixture
            .store
            .budget_call(&fixture.admin, SCOPE, call_id)
            .await
            .unwrap()
            .unwrap(),
        first
    );
    old_store.close().await;

    // This is a real generic financial close, not a registered execution witness.
    // Its self-consistent v1 payload remains Legacy for every legal resumer.
    for (index, model_settler) in [
        fixture.admin.clone(),
        Context::new(NAMESPACE, "generic-financial-host", Role::Host).unwrap(),
    ]
    .iter()
    .enumerate()
    {
        let model_call_id = format!("ag183-generic-legacy-{index}");
        let request_artifact = BudgetArtifact::from_serializable(
            "rsia.model_request.artifact.v1",
            &serde_json::json!({"request_id":model_call_id,"source_closure":[],"input":"ag183-financial-replay"}),
        )
        .unwrap();
        let reserved = fixture
            .store
            .reserve_budget_call(
                &fixture.executor,
                &BudgetCallReservation {
                    billing_scope: SCOPE.into(),
                    call_id: model_call_id.clone(),
                    dispatch_group_id: format!("ag183-generic-group-{index}"),
                    stage: BudgetStage::DevelopmentExecution,
                    actual_input_digest: request_artifact.digest.clone(),
                    request_artifact: Some(request_artifact),
                    max_cost_micros: 1,
                    lease_token: format!("ag183-generic-lease-{index}"),
                    lease_until: 1_000,
                    now: 100,
                },
            )
            .await
            .unwrap();
        let model_fence = BudgetCallFence {
            billing_scope: SCOPE.into(),
            call_id: model_call_id.clone(),
            actual_input_digest: reserved.actual_input_digest.clone(),
            lease_token: reserved.lease_token,
            lease_epoch: reserved.lease_epoch,
            now: 101,
        };
        let dispatched = fixture
            .store
            .begin_budget_dispatch(&fixture.executor, &model_fence)
            .await
            .unwrap()
            .call;
        let legacy_submission = RegisteredExecutionSettlement {
            call_id: model_call_id,
            dispatch_id: dispatched.dispatch_id.unwrap(),
            request_digest: model_fence.actual_input_digest.clone(),
            ..submission.clone()
        };
        let legacy_artifact = legacy_submission.artifact().unwrap();
        let legacy_charge = UsageCharge {
            provider_request_id: format!("generic-financial:{index}"),
            usage_record_id: format!("ag183-generic-usage-{index}"),
            ..charge.clone()
        };
        let settled = fixture
            .store
            .settle_model_budget_call(
                model_settler,
                &model_fence,
                &legacy_charge,
                &evo_storage::budget::ModelCallSettlementEvidence {
                    provenance: evo_storage::budget::BudgetExecutionProvenance::ExternalProvider,
                    actual_model_digest: Some(d("ag183-generic-model")),
                    transport_artifact: BudgetArtifact::from_serializable(
                        "test.transport.v1",
                        &serde_json::json!({"provider_request_id":legacy_charge.provider_request_id}),
                    )
                    .unwrap(),
                    usable_response: Some(legacy_artifact.clone()),
                    blocked_response: BudgetArtifact::from_serializable(
                        "test.response.v1",
                        &serde_json::json!({"state":"blocked"}),
                    )
                    .unwrap(),
                    forced_block_reason: None,
                },
            )
            .await
            .unwrap();
        assert!(settled.response_usable);
        assert_eq!(settled.call.actual_cost_micros, Some(0));
        assert!(!settled.call.execution_closed);
        assert_eq!(
            settled.call.execution_provenance,
            Some(evo_storage::budget::BudgetExecutionProvenance::ExternalProvider)
        );
        let closed = fixture
            .store
            .close_budget_call_execution(
                model_settler,
                SCOPE,
                &legacy_submission.call_id,
                &legacy_submission.dispatch_id,
                "ag183_financial_close",
                102,
            )
            .await
            .unwrap();
        assert!(closed.execution_closed);
        assert_eq!(closed.response_artifact.as_ref(), Some(&legacy_artifact));
        let financial_before = clock_failure_sqlite_snapshot(&path);
        for resumer in [
            fixture.executor.clone(),
            Context::new(NAMESPACE, "generic-legacy-resumer", Role::Host).unwrap(),
        ] {
            let replay = fixture
                .store
                .settle_registered_execution_call(
                    &resumer,
                    &model_fence,
                    &legacy_charge,
                    &legacy_submission,
                )
                .await
                .unwrap();
            assert_eq!(replay, closed);
            assert_eq!(replay.response_artifact.as_ref(), Some(&legacy_artifact));
            assert!(
                matches!(ReadRegisteredExecutionSettlement::from_call(&replay).unwrap(), ReadRegisteredExecutionSettlement::Legacy(value) if value == legacy_submission)
            );
        }
        assert_eq!(clock_failure_sqlite_snapshot(&path), financial_before);
    }
    fixture.store.close().await;
}

#[tokio::test]
async fn issuer_recomputes_frozen_target_output_before_receipt() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let healthy = fixture.runner().run(request.clone()).await.unwrap();
    let id = &healthy.results[0].parent_execution_id;
    let mut session = fixture.store.session().await.unwrap();
    let read = read_execution_receipt_in_session(
        &mut session,
        &fixture.admin,
        &request,
        "task-a",
        DevelopmentSide::Parent,
        DevelopmentExecutionReceiptVersion::V2,
    )
    .await
    .unwrap();
    assert_eq!(
        read.state,
        evo_storage::PersistentContractState::CurrentVerified
    );
    assert_eq!(read.receipt.receipt_id, *id);
    assert_eq!(
        read.receipt.schema_version,
        DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2
    );
    assert_eq!(read.current_receipt_id, *id);
    assert_eq!(
        read.legacy_receipt_id,
        legacy_execution_receipt_id(&request.request_id, "task-a", DevelopmentSide::Parent)
            .unwrap()
    );
    assert_ne!(read.legacy_receipt_id, read.current_receipt_id);
    session.commit().await.unwrap();
    let path = fixture._dir.path().join("development.sqlite3");
    for (index, task_id) in ["task-a", "task-b"].iter().enumerate() {
        let mut wrong_request = request.clone();
        wrong_request.request_id = format!("ag183-wrong-output-{index}");
        let wrong_output = r#"{"answer":{"clamped":12345}}"#;
        let (fence, charge, settlement) = reserve_execution(
            &fixture,
            &wrong_request,
            task_id,
            DevelopmentSide::Parent,
            wrong_output,
        )
        .await;
        let accounted = fixture
            .store
            .settle_registered_execution_call(&fixture.executor, &fence, &charge, &settlement)
            .await
            .unwrap();
        assert_eq!(
            accounted.output_digest.as_deref(),
            Some(hash(wrong_output.as_bytes()).as_str())
        );
        assert_eq!(accounted.actual_cost_micros, Some(0));
        assert!(accounted.execution_closed);
        let before = clock_failure_sqlite_snapshot(&path);
        assert!(matches!(
            issue_execution_receipt(&fixture.executor, &fixture.store, ExecutionReceiptIssueRequest {
                control_id: CONTROL_ID.into(), request: wrong_request, task_id: (*task_id).into(),
                side: DevelopmentSide::Parent, output_utf8: wrong_output.into(),
                budget_call_id: accounted.call_id.clone(), issued_at_unix_seconds: 100,
            }).await,
            Err(Error::Conflict(message)) if message == "registered execution output differs from frozen target"
        ));
        assert_eq!(clock_failure_sqlite_snapshot(&path), before);
        assert_eq!(
            fixture
                .store
                .budget_call(&fixture.admin, SCOPE, &accounted.call_id)
                .await
                .unwrap()
                .unwrap(),
            accounted
        );
    }
    fixture.store.close().await;
}

#[tokio::test]
async fn issuer_rejects_another_settler_without_rewriting_accounted_call() {
    let fixture = Fixture::new().await;
    let path = fixture._dir.path().join("development.sqlite3");
    let output = r#"{"answer":{"clamped":-1}}"#;
    for (index, settler) in [
        Context::new(NAMESPACE, "another-real-worker", Role::Worker).unwrap(),
        Context::new(NAMESPACE, "another-real-host", Role::Host).unwrap(),
    ]
    .iter()
    .enumerate()
    {
        let mut request = fixture.request();
        request.request_id = format!("ag183-another-settler-{index}");
        let (fence, charge, settlement) = reserve_execution(
            &fixture,
            &request,
            "task-a",
            DevelopmentSide::Parent,
            output,
        )
        .await;
        let accounted = fixture
            .store
            .settle_registered_execution_call(settler, &fence, &charge, &settlement)
            .await
            .unwrap();
        let ReadRegisteredExecutionSettlement::Current(fact) =
            ReadRegisteredExecutionSettlement::from_call(&accounted).unwrap()
        else {
            panic!("actual registered writer");
        };
        assert_eq!(fact.settler_actor, settler.actor());
        assert_eq!(fact.settler_role, settler.role());
        let before = clock_failure_sqlite_snapshot(&path);
        assert!(matches!(
            issue_execution_receipt(&fixture.executor, &fixture.store, ExecutionReceiptIssueRequest {
                control_id: CONTROL_ID.into(), request, task_id: "task-a".into(),
                side: DevelopmentSide::Parent, output_utf8: output.into(),
                budget_call_id: accounted.call_id.clone(), issued_at_unix_seconds: 100,
            }).await,
            Err(Error::Conflict(message)) if message == "registered execution settler differs from registered executor"
        ));
        let replay = fixture
            .store
            .settle_registered_execution_call(&fixture.executor, &fence, &charge, &settlement)
            .await
            .unwrap();
        assert_eq!(replay, accounted);
        assert_eq!(clock_failure_sqlite_snapshot(&path), before);
        assert_eq!(accounted.state, BudgetCallState::Finalized);
        assert_eq!(accounted.actual_cost_micros, Some(0));
        assert!(accounted.execution_closed);
    }
    // Same actual actor may settle as Host and issue as Worker. A reader's role
    // is not the settler's role, and the receipt retains the first real role.
    let mut request = fixture.request();
    request.request_id = "ag183-same-actor-host".into();
    let (fence, charge, settlement) = reserve_execution(
        &fixture,
        &request,
        "task-a",
        DevelopmentSide::Parent,
        output,
    )
    .await;
    let host = Context::new(NAMESPACE, fixture.executor.actor(), Role::Host).unwrap();
    fixture
        .store
        .settle_registered_execution_call(&host, &fence, &charge, &settlement)
        .await
        .unwrap();
    let receipt = issue_execution_receipt(
        &fixture.executor,
        &fixture.store,
        ExecutionReceiptIssueRequest {
            control_id: CONTROL_ID.into(),
            request: request.clone(),
            task_id: "task-a".into(),
            side: DevelopmentSide::Parent,
            output_utf8: output.into(),
            budget_call_id: fence.call_id,
            issued_at_unix_seconds: 100,
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.executor_actor, fixture.executor.actor());
    assert_eq!(receipt.executor_role, Role::Host);
    let mut session = fixture.store.session().await.unwrap();
    let read = read_execution_receipt_in_session(
        &mut session,
        &fixture.grader,
        &request,
        "task-a",
        DevelopmentSide::Parent,
        DevelopmentExecutionReceiptVersion::V2,
    )
    .await
    .unwrap();
    assert_eq!(
        read.state,
        evo_storage::PersistentContractState::CurrentVerified
    );
    assert_eq!(read.receipt.executor_role, Role::Host);
    session.commit().await.unwrap();
    fixture.store.close().await;
}

#[tokio::test]
async fn generic_model_response_never_becomes_current_execution_evidence() {
    let fixture = Fixture::new().await;
    let path = fixture._dir.path().join("development.sqlite3");
    let output = r#"{"answer":{"clamped":-1}}"#;
    // Both supported generic financial roles/provenances remain legal. Even a
    // self-consistent caller V2 body cannot turn that writer into registered execution.
    for (index, (writer, provenance)) in [
        (
            fixture.admin.clone(),
            evo_storage::budget::BudgetExecutionProvenance::ExternalProvider,
        ),
        (
            fixture.admin.clone(),
            evo_storage::budget::BudgetExecutionProvenance::Fixture,
        ),
        (
            Context::new(NAMESPACE, "model-financial-host", Role::Host).unwrap(),
            evo_storage::budget::BudgetExecutionProvenance::ExternalProvider,
        ),
        (
            Context::new(NAMESPACE, "model-financial-host", Role::Host).unwrap(),
            evo_storage::budget::BudgetExecutionProvenance::Fixture,
        ),
    ]
    .iter()
    .enumerate()
    {
        let mut request = fixture.request();
        request.request_id = format!("ag183-generic-v2-{index}");
        let (fence, charge, submission) = reserve_execution(
            &fixture,
            &request,
            "task-a",
            DevelopmentSide::Parent,
            output,
        )
        .await;
        let forged = BudgetArtifact::from_serializable(
            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2,
            &evo_storage::budget::RegisteredExecutionSettlementV2 {
                schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2.into(),
                settler_actor: fixture.executor.actor().into(),
                settler_role: Role::Worker,
                settlement: submission.clone(),
            },
        )
        .unwrap();
        let model = fixture
            .store
            .settle_model_budget_call(
                writer,
                &fence,
                &charge,
                &evo_storage::budget::ModelCallSettlementEvidence {
                    provenance: *provenance,
                    actual_model_digest: Some(d("generic-model-financial-fixture")),
                    transport_artifact: BudgetArtifact::from_serializable(
                        "test.transport.v1",
                        &serde_json::json!({"provider_request_id":charge.provider_request_id}),
                    )
                    .unwrap(),
                    usable_response: Some(forged.clone()),
                    blocked_response: BudgetArtifact::from_serializable(
                        "test.response.v1",
                        &serde_json::json!({"state":"blocked"}),
                    )
                    .unwrap(),
                    forced_block_reason: None,
                },
            )
            .await
            .unwrap();
        assert!(model.response_usable);
        assert_eq!(model.call.execution_provenance, Some(*provenance));
        let accounted = fixture
            .store
            .close_budget_call_execution(
                writer,
                SCOPE,
                &fence.call_id,
                &submission.dispatch_id,
                "ag183_model_financial_close",
                102,
            )
            .await
            .unwrap();
        assert_eq!(accounted.response_artifact.as_ref(), Some(&forged));
        assert_eq!(accounted.actual_cost_micros, Some(0));
        assert!(accounted.execution_closed);
        let before = clock_failure_sqlite_snapshot(&path);
        let expected = if *provenance == evo_storage::budget::BudgetExecutionProvenance::Fixture {
            "execution receipt lacks a matching closed development budget call"
        } else {
            "registered execution settlement is invalid"
        };
        assert!(matches!(
            issue_execution_receipt(&fixture.executor, &fixture.store, ExecutionReceiptIssueRequest {
                control_id: CONTROL_ID.into(), request, task_id: "task-a".into(),
                side: DevelopmentSide::Parent, output_utf8: output.into(), budget_call_id: fence.call_id.clone(),
                issued_at_unix_seconds: 100,
            }).await,
            Err(Error::Conflict(message)) if message == expected
        ));
        assert_eq!(clock_failure_sqlite_snapshot(&path), before);
        assert_eq!(
            fixture
                .store
                .budget_call(&fixture.admin, SCOPE, &fence.call_id)
                .await
                .unwrap()
                .unwrap(),
            accounted
        );
    }
    fixture.store.close().await;
}

// AG-183 V104 (§5.1.1 item 7): object rows the old producer natively wrote, captured
// byte for byte from the registered runner of this file's own Fixture at commit
// 7b285b9c603b567648dd0df579c2c9100e3f1c99 (pure-function V1 producer, capture
// backup sha256 e6377d577867f891ca5eaf330e508d7c3abcdf097be45625455aa6dac867f31d).
// Order: per task (a, b) the parent receipt, its output, the candidate receipt, its
// output and the grader receipt, then the run receipt. Never regenerated here.
/// The request those rows were produced for, as that commit's API serialized it.
const NATIVE_7B285B9_PURE_REQUEST: &str = r#"{"attempt":1,"candidate_bundle_digest":"c69a7d061fe68a35d66b88b8566476bbafae26eb783507b022197fc1966398e9","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","episode_id":"episode-1","grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","idempotency_key":"dev-request-1-idem","manifest":{"digest":"1d9be8cd6389d8f9204e2fc66bcc17518524c85a39ab3c2acdbe200a1ca03774","id":"dev-manifest","tasks":[{"id":"task-a","input_digest":"4b1cd29e86ac99565f4c19e46a6803fdd3a189ea82eba26f9263153a7ce1e5bd","parent_family":"family-a"},{"id":"task-b","input_digest":"a577fd498cd0f7aa4af7f772f9e03f7904453337cb21b3d7945f73ddc444a1e4","parent_family":"family-b"}]},"namespace":"n","parent_bundle_digest":"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd","purpose":"development","request_id":"dev-request-1","revoke_watermark":1,"rules_digest":"6c621d1a05138a7888d37d9269a9da8e2e11e4aced2f6cfd24b05ab1b9e61bb0","step":1,"tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281"}"#;
/// 11 object rows: (namespace, kind, id, owner, revision, body).
const NATIVE_7B285B9_PURE_ROWS: [(&str, &str, &str, &str, i64, &str); 11] = [
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb","record_kind":"development_execution_receipt_v1","payload":{"schema_version":"rsia.development_execution_receipt.v1","receipt_id":"devexec-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-a","side":"parent","input_digest":"4b1cd29e86ac99565f4c19e46a6803fdd3a189ea82eba26f9263153a7ce1e5bd","bundle_digest":"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","request_digest":"e293395460d59855c7213159326c7d13fccab2e310f0d95950080e57a5699959","budget_call_id":"devcall-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","dispatch_id":"bc483077-6c20-480f-950c-83454b11839b","usage_record_id":"devusage-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62","output_artifact_id":"devout-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","cost_state":{"state":"known","micros":0,"currency":"USD","pricing_version":"pricing-v1"},"execution_provenance":"registered_pure_function","revoke_watermark":1,"source_ids":["run-a","run-b"],"executor_actor":"dev-executor","executor_role":"worker","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-e30568d8fecc7c066a4fbb30dd9740b7599faa5b0150990bd600f26a0e46057a",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-e30568d8fecc7c066a4fbb30dd9740b7599faa5b0150990bd600f26a0e46057a","record_kind":"development_execution_output_v1","payload":{"schema_version":"rsia.development_execution_output.v1","id":"devout-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","control_id":"dev-control","request_id":"dev-request-1","task_id":"task-a","side":"parent","output_utf8":"{\"answer\":{\"clamped\":-1}}","output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62"}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b","record_kind":"development_execution_receipt_v1","payload":{"schema_version":"rsia.development_execution_receipt.v1","receipt_id":"devexec-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-a","side":"candidate","input_digest":"4b1cd29e86ac99565f4c19e46a6803fdd3a189ea82eba26f9263153a7ce1e5bd","bundle_digest":"c69a7d061fe68a35d66b88b8566476bbafae26eb783507b022197fc1966398e9","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","request_digest":"2279e899c83f6f4b8c1b5e8a54b6aad820ccd674b01ec991eca824fb1aa783cc","budget_call_id":"devcall-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","dispatch_id":"6b15fd15-eb9a-40b6-b85b-b625ce783f2f","usage_record_id":"devusage-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62","output_artifact_id":"devout-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","cost_state":{"state":"known","micros":0,"currency":"USD","pricing_version":"pricing-v1"},"execution_provenance":"registered_pure_function","revoke_watermark":1,"source_ids":["run-a","run-b"],"executor_actor":"dev-executor","executor_role":"worker","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-18d86ed5f4c6dfeac46c5eb500f019e4af2215cbdec445669c8ba0dcd001c020",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-18d86ed5f4c6dfeac46c5eb500f019e4af2215cbdec445669c8ba0dcd001c020","record_kind":"development_execution_output_v1","payload":{"schema_version":"rsia.development_execution_output.v1","id":"devout-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","control_id":"dev-control","request_id":"dev-request-1","task_id":"task-a","side":"candidate","output_utf8":"{\"answer\":{\"clamped\":-1}}","output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62"}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "dev-grader",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4","record_kind":"development_grader_receipt_v1","payload":{"schema_version":"rsia.development_grader_receipt.v1","receipt_id":"devgrade-b42dd4e5d5f4578b43644e68923332561c01c9b026b1e43702bdbb0d48b76da9","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-a","parent_execution_receipt_id":"devexec-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","candidate_execution_receipt_id":"devexec-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","parent_output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62","candidate_output_digest":"775020695f409fbe0aea28e542460777bb8224e85d7b9b05773384969960de62","oracle_digest":"07a5ee4619f9a38d3b463a9137cfb467f0f49c469af6f37d90f4e732b6d24f5a","grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","grader_version":"exact-json-v1","parent_score_micros":1000000,"candidate_score_micros":1000000,"parent_passed":true,"candidate_passed":true,"scored_output_pair_digest":"61c9a943498b687b9d2c0b66095e9203f56121b7a620dd3bf12694292289d044","scoring_budget_call_id":null,"revoke_watermark":1,"source_ids":["run-a","run-b"],"grader_actor":"dev-grader","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507","record_kind":"development_execution_receipt_v1","payload":{"schema_version":"rsia.development_execution_receipt.v1","receipt_id":"devexec-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-b","side":"parent","input_digest":"a577fd498cd0f7aa4af7f772f9e03f7904453337cb21b3d7945f73ddc444a1e4","bundle_digest":"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","request_digest":"865721ad3f6d88d7db1ac9efcfba2223f4ab8316b84d03e1b0f3d8f33922c39b","budget_call_id":"devcall-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","dispatch_id":"b7ae85b4-9e6d-4c42-a3ab-fa035477e06d","usage_record_id":"devusage-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa","output_artifact_id":"devout-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","cost_state":{"state":"known","micros":0,"currency":"USD","pricing_version":"pricing-v1"},"execution_provenance":"registered_pure_function","revoke_watermark":1,"source_ids":["run-a","run-b"],"executor_actor":"dev-executor","executor_role":"worker","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-cae37c0e7c69fe02b22a0a89a3bbbf821db208790f5e0b474ab8e30e3a491dea",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-cae37c0e7c69fe02b22a0a89a3bbbf821db208790f5e0b474ab8e30e3a491dea","record_kind":"development_execution_output_v1","payload":{"schema_version":"rsia.development_execution_output.v1","id":"devout-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","control_id":"dev-control","request_id":"dev-request-1","task_id":"task-b","side":"parent","output_utf8":"{\"answer\":{\"clamped\":3}}","output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa"}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb","record_kind":"development_execution_receipt_v1","payload":{"schema_version":"rsia.development_execution_receipt.v1","receipt_id":"devexec-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-b","side":"candidate","input_digest":"a577fd498cd0f7aa4af7f772f9e03f7904453337cb21b3d7945f73ddc444a1e4","bundle_digest":"c69a7d061fe68a35d66b88b8566476bbafae26eb783507b022197fc1966398e9","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","request_digest":"b516de1d6b5cb03529afb5ec499c0f0c3c235acef838b93b51cc0d531df86708","budget_call_id":"devcall-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","dispatch_id":"bdc1c3cb-b160-4f29-af23-24142c344ce5","usage_record_id":"devusage-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa","output_artifact_id":"devout-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","cost_state":{"state":"known","micros":0,"currency":"USD","pricing_version":"pricing-v1"},"execution_provenance":"registered_pure_function","revoke_watermark":1,"source_ids":["run-a","run-b"],"executor_actor":"dev-executor","executor_role":"worker","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-92b6e07b532f41fff34bebe10865b660a8e065d2b0c9398f669fcb58ca7e9913",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-92b6e07b532f41fff34bebe10865b660a8e065d2b0c9398f669fcb58ca7e9913","record_kind":"development_execution_output_v1","payload":{"schema_version":"rsia.development_execution_output.v1","id":"devout-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","control_id":"dev-control","request_id":"dev-request-1","task_id":"task-b","side":"candidate","output_utf8":"{\"answer\":{\"clamped\":3}}","output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa"}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "dev-grader",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34","record_kind":"development_grader_receipt_v1","payload":{"schema_version":"rsia.development_grader_receipt.v1","receipt_id":"devgrade-ee8d625372f72d630d8aa5c7557b6a10913fe8eca8929e58e91b0219a390d1f1","control_id":"dev-control","namespace":"n","episode_id":"episode-1","step":1,"attempt":1,"request_id":"dev-request-1","task_id":"task-b","parent_execution_receipt_id":"devexec-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","candidate_execution_receipt_id":"devexec-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a","parent_output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa","candidate_output_digest":"f2aaaf435a06e6ffdb92925f30ae14673a3ed5e4f7c793c44b4197afb36222aa","oracle_digest":"07a5ee4619f9a38d3b463a9137cfb467f0f49c469af6f37d90f4e732b6d24f5a","grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","grader_version":"exact-json-v1","parent_score_micros":1000000,"candidate_score_micros":1000000,"parent_passed":true,"candidate_passed":true,"scored_output_pair_digest":"01f02f96fffecfb0500b75c47dacbf3bac245bd7442ee31512a04699385a878c","scoring_budget_call_id":null,"revoke_watermark":1,"source_ids":["run-a","run-b"],"grader_actor":"dev-grader","issued_at_unix_seconds":100}}"#,
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "dev-executor",
        1,
        r#"{"schema_version":"rsia.typed_artifact_envelope.v1","id":"e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e","record_kind":"development_run_receipt_v1","payload":{"schema_version":"rsia.development_run_receipt.v1","id":"devrun-8df78fb2c1aab041e1bce0100ae96187c626445b2cdb051dad0f891d712f9344","control_id":"dev-control","namespace":"n","request_id":"dev-request-1","episode_id":"episode-1","step":1,"attempt":1,"execution_receipt_ids":["devexec-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","devexec-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","devexec-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","devexec-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a"],"grader_receipt_ids":["devgrade-b42dd4e5d5f4578b43644e68923332561c01c9b026b1e43702bdbb0d48b76da9","devgrade-ee8d625372f72d630d8aa5c7557b6a10913fe8eca8929e58e91b0219a390d1f1"],"budget_call_ids":["devcall-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","devcall-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","devcall-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","devcall-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a"],"usage_record_ids":["devusage-db8b3e9eb82dd767bb56838e8a2639cb5de980e130dd3fcfee67571a5f9f2609","devusage-28ae6ad03ae7fd71360d20e50c59837b4baea8dd937820168850d4bba4bc3581","devusage-8133972033c6782b39ec7ffc4ea550b9e299e4559049d5e28e70167693a98c83","devusage-1c6ebf2cc794f4b14a650ed0728f4ba548c765f32d019e389e79a2731556b88a"],"executor_actor":"dev-executor","issued_at_unix_seconds":100}}"#,
    ),
];
/// Every dependency edge these rows own: (namespace, src_kind, src_id, dst_kind, dst_id).
const NATIVE_7B285B9_PURE_EDGES: [(&str, &str, &str, &str, &str); 46] = [
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "artifact",
        "budget-ref-5e4effce2d3c100e9839373908829b3b",
    ),
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "artifact",
        "e03dev-cae37c0e7c69fe02b22a0a89a3bbbf821db208790f5e0b474ab8e30e3a491dea",
    ),
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-18d86ed5f4c6dfeac46c5eb500f019e4af2215cbdec445669c8ba0dcd001c020",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-18d86ed5f4c6dfeac46c5eb500f019e4af2215cbdec445669c8ba0dcd001c020",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-2261bc39676d29e5d143a3fd9fc56c8f4c690c6d6567de40170f78dbbf3d205e",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-264a971fb3c5db68914c923845f8625abb4848d06d6e5342fff278e08e5e9ea4",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-92b6e07b532f41fff34bebe10865b660a8e065d2b0c9398f669fcb58ca7e9913",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-92b6e07b532f41fff34bebe10865b660a8e065d2b0c9398f669fcb58ca7e9913",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "artifact",
        "e03dev-0b45169812221a319377f95ca1b1fc42746e040da50e3efd798f7f8146bbd507",
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-a13b1ef2c8e8d2f7345f69e694a3f3324149436ff317af0ee92ffe352a468a34",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "artifact",
        "budget-ref-28f396d691131f0d8c5831a5d701a2e5",
    ),
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "artifact",
        "e03dev-e30568d8fecc7c066a4fbb30dd9740b7599faa5b0150990bd600f26a0e46057a",
    ),
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-c8e9a537a2362a3a97ad9c2235b356faad249d9d50618a0c9bc8233053f2e3bb",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-cae37c0e7c69fe02b22a0a89a3bbbf821db208790f5e0b474ab8e30e3a491dea",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-cae37c0e7c69fe02b22a0a89a3bbbf821db208790f5e0b474ab8e30e3a491dea",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-e30568d8fecc7c066a4fbb30dd9740b7599faa5b0150990bd600f26a0e46057a",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-e30568d8fecc7c066a4fbb30dd9740b7599faa5b0150990bd600f26a0e46057a",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "artifact",
        "budget-ref-c933de659f5baf9bba2bee6762bc5cca",
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "artifact",
        "e03dev-92b6e07b532f41fff34bebe10865b660a8e065d2b0c9398f669fcb58ca7e9913",
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-f14a8a6fb981e2d1cbd1c7f9a76f51b554fc6053f0814ec7de500e583cc82cdb",
        "run",
        "run-b",
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "artifact",
        "budget-ref-d3ca2274400db0b5abd234e4e800c3ef",
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "artifact",
        "e03dev-18d86ed5f4c6dfeac46c5eb500f019e4af2215cbdec445669c8ba0dcd001c020",
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "artifact",
        "e03dev-fa1131c7be6e139e0c27f0e9db40e1318c58b9b9d8c4bb81a27533cca04684ae",
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "run",
        "run-a",
    ),
    (
        "n",
        "artifact",
        "e03dev-f5bcdcda41a4816c359afb84946af4c70ae2c688b4ad6d4f6a696d2fc935268b",
        "run",
        "run-b",
    ),
];

/// Inserts native rows and the edges they own exactly as stored, bypassing every
/// current writer.
async fn insert_native(
    path: &std::path::Path,
    rows: &[(&str, &str, &str, &str, i64, &str)],
    edges: &[(&str, &str, &str, &str, &str)],
) {
    let pool = evo_storage::sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    for (namespace, kind, id, owner, revision, body) in rows {
        evo_storage::sqlx::query(
            "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES(?,?,?,?,?,?)",
        )
        .bind(namespace)
        .bind(kind)
        .bind(id)
        .bind(owner)
        .bind(body)
        .bind(revision)
        .execute(&pool)
        .await
        .unwrap();
    }
    for (namespace, src_kind, src_id, dst_kind, dst_id) in edges {
        evo_storage::sqlx::query(
            "INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)",
        )
        .bind(namespace)
        .bind(src_kind)
        .bind(src_id)
        .bind(dst_kind)
        .bind(dst_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

fn native_payload(body: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(body).unwrap()["payload"].clone()
}

#[tokio::test]
async fn legacy_receipts_and_stage_cache_never_gain_current_authority() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let report = fixture.runner().run(request.clone()).await.unwrap();
    let (request_fact, observed) = fixture.commit_facts(&request, &report).await;
    verify_development_observation(&fixture.store, &fixture.admin, &request_fact, &observed)
        .await
        .unwrap();
    let current_id = report.results[0].parent_execution_id.clone();
    assert_eq!(
        load_execution_receipt(&fixture.store, &fixture.admin, &current_id)
            .await
            .unwrap()
            .schema_version,
        DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2
    );
    let current_storage = storage_id(EXECUTION_RECEIPT_KIND, &current_id).unwrap();
    let mut session = fixture.store.session().await.unwrap();
    let original: serde_json::Value = session
        .need(&fixture.admin, "artifact", &current_storage)
        .await
        .unwrap();
    session.commit().await.unwrap();
    // The native 7b285b9 V1 receipt of this exact request and side, byte for
    // byte. It can never add authority even alongside real current facts.
    let path = fixture._dir.path().join("development.sqlite3");
    insert_native(&path, &NATIVE_7B285B9_PURE_ROWS[..1], &[]).await;
    let legacy: evo_engine::development::DevelopmentExecutionReceiptV1 =
        serde_json::from_value(native_payload(NATIVE_7B285B9_PURE_ROWS[0].5)).unwrap();
    assert_eq!(legacy.schema_version, DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA);
    assert_eq!(
        legacy.receipt_id,
        legacy_execution_receipt_id(&request.request_id, "task-a", DevelopmentSide::Parent)
            .unwrap()
    );
    let before = clock_failure_sqlite_snapshot(&path);
    let mut session = fixture.store.session().await.unwrap();
    let historical = read_execution_receipt_in_session(
        &mut session,
        &fixture.admin,
        &request,
        "task-a",
        DevelopmentSide::Parent,
        DevelopmentExecutionReceiptVersion::V1,
    )
    .await
    .unwrap();
    assert_eq!(
        historical.state,
        evo_storage::PersistentContractState::Legacy
    );
    assert_eq!(
        historical.receipt.schema_version,
        DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA
    );
    assert_eq!(historical.legacy_receipt_id, legacy.receipt_id);
    assert_eq!(historical.current_receipt_id, current_id);
    session.commit().await.unwrap();
    assert_eq!(
        serde_json::to_value(
            load_execution_receipt(&fixture.store, &fixture.admin, &legacy.receipt_id)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
    assert!(
        matches!(fixture.runner().run(request.clone()).await, Err(Error::Conflict(message)) if message == "development execution source_legacy; new request_id required")
    );
    let mut only_new_idempotency = request.clone();
    only_new_idempotency.idempotency_key = "new-idempotency-is-not-a-new-run".into();
    assert!(
        matches!(fixture.runner().run(only_new_idempotency).await, Err(Error::Conflict(message)) if message == "development execution source_legacy; new request_id required")
    );
    assert_eq!(clock_failure_sqlite_snapshot(&path), before);
    let mut legacy_report = report.clone();
    legacy_report.results[0].parent_execution_id = legacy.receipt_id.clone();
    let honest = observed_fact(&request, &report, vec![]);
    let forged = observed_fact(&request, &legacy_report, vec![]);
    assert!(
        matches!(gate_with_forged_observation(&fixture, &request_fact, &honest, &forged).await,
        Err(Error::Conflict(message)) if message == "development execution source_legacy; new request_id required")
    );
    // An invalid V2 does not fall back to the readable Legacy at its other ID.
    for (schema, extra, wrong_id, invalid) in [
        (
            "rsia.development_execution_receipt.v999",
            false,
            false,
            true,
        ),
        (DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2, true, false, false),
        (DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2, false, true, false),
    ] {
        let mut changed = original.clone();
        changed["payload"]["schema_version"] = serde_json::json!(schema);
        if extra {
            changed["payload"]["not_a_receipt_field"] = serde_json::json!(true);
        }
        if wrong_id {
            changed["payload"]["receipt_id"] = serde_json::json!(legacy.receipt_id);
        }
        let mut session = fixture.store.session().await.unwrap();
        session
            .put(
                &fixture.admin,
                "artifact",
                &current_storage,
                "admin",
                &changed,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        let read = load_execution_receipt(&fixture.store, &fixture.admin, &current_id).await;
        if invalid {
            assert!(
                matches!(read, Err(Error::Invalid(message)) if message == "unsupported development execution receipt schema")
            );
        } else {
            let expected = if wrong_id {
                "development execution receipt identity mismatch"
            } else {
                "development execution receipt is invalid"
            };
            assert!(matches!(read, Err(Error::Conflict(message)) if message == expected));
        }
        let mut session = fixture.store.session().await.unwrap();
        session
            .put(
                &fixture.admin,
                "artifact",
                &current_storage,
                "admin",
                &original,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    let mut explicit_new = request.clone();
    explicit_new.request_id = "ag183-explicit-new-execution".into();
    let new_report = fixture.runner().run(explicit_new.clone()).await.unwrap();
    assert_ne!(new_report.execution_receipt_id, report.execution_receipt_id);
    assert_ne!(new_report.results[0].parent_execution_id, current_id);
    assert_eq!(
        load_execution_receipt(&fixture.store, &fixture.admin, &current_id)
            .await
            .unwrap()
            .schema_version,
        DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2
    );
    assert_eq!(
        load_execution_receipt(&fixture.store, &fixture.admin, &legacy.receipt_id)
            .await
            .unwrap()
            .schema_version,
        DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA
    );
    fixture.store.close().await;

    // A supported generic writer genuinely produced this historical V1 financial
    // response, without any execution receipt. It is not a native old E03 run.
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let output = r#"{"answer":{"clamped":-1}}"#;
    let (fence, charge, submission) = reserve_execution(
        &fixture,
        &request,
        "task-a",
        DevelopmentSide::Parent,
        output,
    )
    .await;
    let artifact = submission.artifact().unwrap();
    fixture
        .store
        .settle_model_budget_call(
            &fixture.admin,
            &fence,
            &charge,
            &evo_storage::budget::ModelCallSettlementEvidence {
                provenance: evo_storage::budget::BudgetExecutionProvenance::ExternalProvider,
                actual_model_digest: Some(d("legacy-financial-model")),
                transport_artifact: BudgetArtifact::from_serializable(
                    "test.transport.v1",
                    &serde_json::json!({"provider_request_id":charge.provider_request_id}),
                )
                .unwrap(),
                usable_response: Some(artifact.clone()),
                blocked_response: BudgetArtifact::from_serializable(
                    "test.response.v1",
                    &serde_json::json!({"state":"blocked"}),
                )
                .unwrap(),
                forced_block_reason: None,
            },
        )
        .await
        .unwrap();
    let accounted = fixture
        .store
        .close_budget_call_execution(
            &fixture.admin,
            SCOPE,
            &fence.call_id,
            &submission.dispatch_id,
            "ag183_legacy_financial_close",
            102,
        )
        .await
        .unwrap();
    let path = fixture._dir.path().join("development.sqlite3");
    let before = clock_failure_sqlite_snapshot(&path);
    assert!(
        matches!(fixture.runner().run(request).await, Err(Error::Conflict(message)) if message == "development execution source_legacy; new request_id required")
    );
    let financial_replay = fixture
        .store
        .settle_registered_execution_call(&fixture.executor, &fence, &charge, &submission)
        .await
        .unwrap();
    assert_eq!(financial_replay, accounted);
    assert_eq!(financial_replay.response_artifact.as_ref(), Some(&artifact));
    assert_eq!(clock_failure_sqlite_snapshot(&path), before);
    fixture.store.close().await;

    // The whole native 7b285b9 run of this Fixture's request: statically readable
    // as Legacy, with its current successor ID derivable from the same input and
    // version; never rerun or upgraded under its request ID, while a new request
    // ID runs current; and cleaned by revocation like any execution evidence.
    let fixture = Fixture::new().await;
    let request = fixture.request();
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        serde_json::from_str::<serde_json::Value>(NATIVE_7B285B9_PURE_REQUEST).unwrap()
    );
    let path = fixture._dir.path().join("development.sqlite3");
    insert_native(&path, &NATIVE_7B285B9_PURE_ROWS, &NATIVE_7B285B9_PURE_EDGES).await;
    let run = native_payload(NATIVE_7B285B9_PURE_ROWS[10].5);
    assert_eq!(
        serde_json::to_value(
            evo_engine::development::load_run_receipt(
                &fixture.store,
                &fixture.admin,
                run["id"].as_str().unwrap()
            )
            .await
            .unwrap()
        )
        .unwrap(),
        run
    );
    let sides = [
        ("task-a", DevelopmentSide::Parent, 0),
        ("task-a", DevelopmentSide::Candidate, 2),
        ("task-b", DevelopmentSide::Parent, 5),
        ("task-b", DevelopmentSide::Candidate, 7),
    ];
    for (task, side, row) in sides {
        let receipt = native_payload(NATIVE_7B285B9_PURE_ROWS[row].5);
        let output = native_payload(NATIVE_7B285B9_PURE_ROWS[row + 1].5);
        let id = receipt["receipt_id"].as_str().unwrap();
        assert_eq!(
            serde_json::to_value(
                load_execution_receipt(&fixture.store, &fixture.admin, id)
                    .await
                    .unwrap()
            )
            .unwrap(),
            receipt
        );
        assert_eq!(
            serde_json::to_value(
                load_execution_output(
                    &fixture.store,
                    &fixture.admin,
                    receipt["output_artifact_id"].as_str().unwrap()
                )
                .await
                .unwrap()
            )
            .unwrap(),
            output
        );
        let mut session = fixture.store.session().await.unwrap();
        let read = read_execution_receipt_in_session(
            &mut session,
            &fixture.admin,
            &request,
            task,
            side,
            DevelopmentExecutionReceiptVersion::V1,
        )
        .await
        .unwrap();
        assert_eq!(read.state, evo_storage::PersistentContractState::Legacy);
        assert_eq!(read.legacy_receipt_id, id);
        assert_eq!(
            read.current_receipt_id,
            execution_receipt_id(&request.request_id, task, side).unwrap()
        );
        assert_ne!(read.current_receipt_id, read.legacy_receipt_id);
        assert!(matches!(
            read_execution_receipt_in_session(
                &mut session,
                &fixture.admin,
                &request,
                task,
                side,
                DevelopmentExecutionReceiptVersion::V2,
            )
            .await,
            Err(Error::NotFound)
        ));
        session.commit().await.unwrap();
    }
    for (task, row) in [("task-a", 4), ("task-b", 9)] {
        assert_eq!(
            serde_json::to_value(
                load_grader_receipt(
                    &fixture.store,
                    &fixture.admin,
                    &grader_receipt_id(&request.request_id, task).unwrap()
                )
                .await
                .unwrap()
            )
            .unwrap(),
            native_payload(NATIVE_7B285B9_PURE_ROWS[row].5)
        );
    }
    let before = clock_failure_sqlite_snapshot(&path);
    assert!(
        matches!(fixture.runner().run(request.clone()).await, Err(Error::Conflict(message)) if message == "development execution source_legacy; new request_id required")
    );
    assert_eq!(clock_failure_sqlite_snapshot(&path), before);
    let mut successor = request.clone();
    successor.request_id = "ag183-native-successor".into();
    let current = fixture.runner().run(successor.clone()).await.unwrap();
    for (task, side, _) in sides {
        let id = execution_receipt_id(&successor.request_id, task, side).unwrap();
        assert!(
            current
                .results
                .iter()
                .any(|result| result.parent_execution_id == id
                    || result.candidate_execution_id == id)
        );
        assert_eq!(
            load_execution_receipt(&fixture.store, &fixture.admin, &id)
                .await
                .unwrap()
                .schema_version,
            DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2
        );
    }
    let mut status = LifecycleCoordinator::revoke_source(
        &fixture.admin,
        &fixture.store,
        "run-a",
        "privacy",
        200,
    )
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
    assert_eq!(status.state, CleanupState::Complete);
    // The outputs are redacted; the receipts, grader receipts and run receipt are
    // the preserved accounting history, still exactly the native bytes.
    let mut session = fixture.store.session().await.unwrap();
    for (index, (_, kind, id, _, _, body)) in NATIVE_7B285B9_PURE_ROWS.iter().enumerate() {
        let stored = session
            .object_row(&fixture.admin, kind, id)
            .await
            .unwrap()
            .unwrap();
        if [1, 3, 6, 8].contains(&index) {
            let value: serde_json::Value = serde_json::from_str(&stored.body).unwrap();
            assert_eq!(value["schema_version"], "rsia.redacted.v1", "{id}");
        } else {
            assert_eq!(stored.body, *body, "{id}");
        }
    }
    session.commit().await.unwrap();
    fixture.store.close().await;
}
