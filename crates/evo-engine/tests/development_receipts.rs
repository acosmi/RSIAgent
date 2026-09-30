//! E03 typed development receipts: registered pure-function runner, the
//! observation gate, E12 consumption, and revocation. Real SQLite store,
//! no model, no provider, zero monetary cost.

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
    DevelopmentControlV1, DevelopmentCostState, DevelopmentEvidenceScope, DevelopmentSide,
    DevelopmentTaskSpecV1, EXECUTION_OUTPUT_KIND, EXECUTION_RECEIPT_KIND,
    ExecutionReceiptIssueRequest, GraderReceiptIssueRequest, RegisteredDevelopmentRunner,
    RegisteredTargetInputV1, execution_budget_call_id, execution_receipt_id, grader_receipt_id,
    issue_execution_receipt, issue_grader_receipt, load_execution_output, load_execution_receipt,
    load_grader_receipt, register_development_control, registered_runner_digest, storage_id,
    typed_receipt_closure,
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
    BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage,
    REGISTERED_EXECUTION_SETTLEMENT_SCHEMA, RegisteredExecutionProvenance,
    RegisteredExecutionSettlement, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::CleanupState;
use std::sync::Arc;

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
        build_request(&self.control, "dev-request-1", "episode-1")
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
            let settlement = RegisteredExecutionSettlement::from_call(&call).unwrap();
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
    let other_request = build_request(&other, "dev-request-2", "episode-2");
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
