use async_trait::async_trait;
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::hash;
use evo_core::optimization::{
    EditSuggestion, ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, ReflectionBatch, ReflectionBatchKind, SkillFailureDiagnosis,
    SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role};
use evo_core::{
    Strategy,
    contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot},
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentSelectionDecision, DevelopmentTask,
    OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournal, OptimizationJournalStage,
    OptimizationStepOutcome, OptimizationStepRequest, PairedTaskResult, StageFact, StageFactKind,
    StoreOptimizationJournal, SuggestionOutcome, run_development, run_optimization_step,
    select_development,
};
use evo_storage::Store;
use std::collections::BTreeSet;
use std::sync::Mutex;

fn model_request() -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: "request-a".into(),
            namespace: "tenant".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-a".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent"),
            bundle_digest: hash(b"bundle"),
            source_closure: vec![EvidenceRef {
                id: "run-a".into(),
                digest: hash(b"run-a"),
            }],
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::Evidence,
            label: "failure".into(),
            content: "complete fixture payload".into(),
        }],
    )
    .unwrap()
}

struct FixtureModelPort;

#[async_trait]
impl ModelPort for FixtureModelPort {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let output = "[]".to_string();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: "fixture-response".into(),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: "fixture-call".into(),
                dispatch_id: "fixture-dispatch".into(),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: "fixture-provider-request".into(),
                usage_record_id: "fixture-usage".into(),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

#[tokio::test]
async fn missing_model_port_is_an_explicit_gap() {
    let batch = ReflectionBatch {
        id: "reflection-failure".into(),
        kind: ReflectionBatchKind::Failure,
        traces: vec![],
        independent_parent_families: BTreeSet::new(),
        source_closure: model_request().source_closure,
    };
    let error = evo_engine::optimization::request_suggestions(
        None,
        model_request(),
        std::slice::from_ref(&batch),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::NotFound));

    let fixture = evo_engine::optimization::request_suggestions(
        Some(&FixtureModelPort),
        model_request(),
        &[batch],
    )
    .await
    .unwrap();
    assert!(matches!(fixture, SuggestionOutcome::NoChange { .. }));
}

#[test]
fn model_response_cannot_change_frozen_model_or_output_bytes() {
    let request = model_request();
    let receipt = ModelExecutionReceipt {
        call_id: "fixture-call".into(),
        dispatch_id: "fixture-dispatch".into(),
        root_budget_id: "fixture-budget".into(),
        provider_request_id: "fixture-provider-request".into(),
        usage_record_id: "fixture-usage".into(),
        provenance: ModelExecutionProvenance::Fixture,
    };
    let wrong_model = ModelResponse::Completed {
        request_id: request.request_id.clone(),
        response_id: "fixture-response".into(),
        actual_model_digest: hash(b"other-model"),
        input_digest: request.input_digest.clone(),
        output: "[]".into(),
        output_digest: hash(b"[]"),
        execution_receipt: receipt.clone(),
    };
    assert!(wrong_model.validate_against(&request).is_err());

    let wrong_output = ModelResponse::Completed {
        request_id: request.request_id.clone(),
        response_id: "fixture-response".into(),
        actual_model_digest: request.model_digest.clone(),
        input_digest: request.input_digest.clone(),
        output: "[]".into(),
        output_digest: hash(b"different"),
        execution_receipt: receipt,
    };
    assert!(wrong_output.validate_against(&request).is_err());

    let contradictory_cancel = ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: "stopped after transport returned".into(),
        dispatch: RejectedDispatch::NotDispatched,
    };
    assert!(contradictory_cancel.validate_against(&request).is_err());
}

fn run_request() -> DevelopmentRunRequest {
    DevelopmentRunRequest {
        request_id: "dev-request".into(),
        namespace: "tenant".into(),
        purpose: Purpose::Development,
        episode_id: "episode-a".into(),
        step: 1,
        attempt: 1,
        manifest: DevelopmentManifest::build(
            "manifest-a",
            vec![
                DevelopmentTask {
                    id: "task-a".into(),
                    parent_family: "family-a".into(),
                    input_digest: hash(b"task-a"),
                },
                DevelopmentTask {
                    id: "task-b".into(),
                    parent_family: "family-b".into(),
                    input_digest: hash(b"task-b"),
                },
            ],
        )
        .unwrap(),
        parent_bundle_digest: hash(b"parent-bundle"),
        candidate_bundle_digest: hash(b"candidate-bundle"),
        environment_digest: hash(b"environment"),
        grader_digest: hash(b"grader"),
        rules_digest: hash(b"rules"),
        tools_digest: hash(b"tools"),
        revoke_watermark: 1,
        idempotency_key: "dev-idempotency".into(),
    }
}

fn report(request: &DevelopmentRunRequest, preserve: bool) -> DevelopmentRunReport {
    DevelopmentRunReport {
        request_id: request.request_id.clone(),
        manifest_digest: request.manifest.digest.clone(),
        parent_bundle_digest: request.parent_bundle_digest.clone(),
        candidate_bundle_digest: request.candidate_bundle_digest.clone(),
        environment_digest: request.environment_digest.clone(),
        grader_digest: request.grader_digest.clone(),
        results: vec![
            PairedTaskResult {
                task_id: "task-a".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: 700_000,
                parent_passed: true,
                candidate_passed: preserve,
                parent_execution_id: "parent-a".into(),
                candidate_execution_id: "candidate-a".into(),
                grader_receipt_digest: hash(b"grade-a"),
            },
            PairedTaskResult {
                task_id: "task-b".into(),
                parent_score_micros: 400_000,
                candidate_score_micros: 600_000,
                parent_passed: false,
                candidate_passed: true,
                parent_execution_id: "parent-b".into(),
                candidate_execution_id: "candidate-b".into(),
                grader_receipt_digest: hash(b"grade-b"),
            },
        ],
        execution_receipt_id: "fixture-execution".into(),
        usage_record_ids: vec!["fixture-usage".into()],
        provenance: DevelopmentExecutionProvenance::Fixture,
    }
}

struct FixtureDevRunner;

#[async_trait]
impl DevRunner for FixtureDevRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Ok(report(&request, true))
    }
}

struct EditingFixtureModelPort;

#[async_trait]
impl ModelPort for EditingFixtureModelPort {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
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
                    text: format!(" [{}]", strategy.instruction),
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
                root_budget_id: "fixture-budget".into(),
                provider_request_id: format!("fixture-provider-{id}"),
                usage_record_id: format!("fixture-usage-{id}"),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

#[derive(Default)]
struct FixtureJournal {
    facts: Mutex<Vec<StageFact>>,
    recovery_facts: Mutex<Vec<StageFact>>,
}

#[async_trait]
impl OptimizationJournal for FixtureJournal {
    async fn claim(&self, fact: StageFact) -> Result<bool> {
        self.commit(fact).await?;
        Ok(true)
    }
    async fn lookup(&self, id: &str) -> Result<Option<StageFact>> {
        Ok(self
            .facts
            .lock()
            .unwrap()
            .iter()
            .chain(self.recovery_facts.lock().unwrap().iter())
            .find(|f| f.artifact_id == id)
            .cloned())
    }
    async fn check_live(&self, _: &[String], _: u64) -> Result<()> {
        Ok(())
    }
    async fn verify_sources(
        &self,
        _: &SourceSelection,
        _: &EvidenceSet,
        _: &[TrustedSourceBinding],
        _: &[OptimizationTrace],
        _: u64,
    ) -> Result<()> {
        Ok(())
    }
    async fn commit(&self, fact: StageFact) -> Result<()> {
        let recovery = matches!(
            fact.kind,
            StageFactKind::StepPrepared
                | StageFactKind::StepCompleted
                | StageFactKind::DispatchPrepared
                | StageFactKind::DispatchObserved
        );
        if recovery {
            self.recovery_facts.lock().unwrap().push(fact);
        } else {
            self.facts.lock().unwrap().push(fact);
        }
        Ok(())
    }
}

fn optimization_evidence() -> EvidenceSet {
    EvidenceSet::build(
        "evidence-set",
        vec![
            EvidenceMember {
                source_id: "run-failure".into(),
                content_digest: hash(b"run-failure"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            EvidenceMember {
                source_id: "run-success".into(),
                content_digest: hash(b"run-success"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
        ],
        SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            parsed_ok: 2,
            bytes_read: 22,
            ..SourceCoverage::default()
        },
        ["family-a".into(), "family-b".into()].into_iter().collect(),
    )
    .unwrap()
}

fn optimization_trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill-a".into(),
            bundle_digest: hash(b"parent-bundle"),
            request_digest: hash(b"task-request"),
            rule_id: Some("rule-a".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture defect".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

#[tokio::test]
async fn g1_step_requires_journal_and_reaches_development_candidate_without_active_state() {
    let evidence = optimization_evidence();
    let parent = SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let baseline = parent.clone();
    let allowed = [
        EvidenceRef {
            id: "run-failure".into(),
            digest: hash(b"run-failure"),
        },
        EvidenceRef {
            id: "run-success".into(),
            digest: hash(b"run-success"),
        },
    ];
    let edit_context = TrustedEditContext::new(
        "tenant",
        "profile-a",
        "skill-a",
        "v1",
        hash(b"parent"),
        hash(b"baseline"),
        &parent,
        allowed.clone(),
    )
    .unwrap();
    let edit_template = SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: "tenant".into(),
        profile_id: "profile-a".into(),
        skill_id: "skill-a".into(),
        skill_version: "v1".into(),
        input_digest: skill_snapshot_digest(&parent).unwrap(),
        approved_parent_digest: hash(b"parent"),
        safe_baseline_digest: hash(b"baseline"),
        evidence: EvidenceClosure {
            support: vec![],
            counterexamples: vec![],
            dependencies: vec![],
        },
        edits: vec![],
    };
    let profile = Profile {
        id: "profile-a".into(),
        evolution_enabled: true,
        parent_digest: hash(b"parent"),
        baseline_digest: hash(b"baseline"),
    };
    let parent_strategy = Strategy::default();
    let baseline_strategy = Strategy::default();
    let improver_patch = ImproverPatch::default();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let revoked = BTreeSet::new();
    let journal = FixtureJournal::default();
    let source_selection = SourceSelection {
        roots: vec![],
        run_ids: vec!["run-failure".into(), "run-success".into()],
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    };
    let source_bindings = vec![
        TrustedSourceBinding {
            source_id: "run-failure".into(),
            source_digest: hash(b"run-failure"),
            parent_family: "family-a".into(),
        },
        TrustedSourceBinding {
            source_id: "run-success".into(),
            source_digest: hash(b"run-success"),
            parent_family: "family-b".into(),
        },
    ];
    let make_request = || OptimizationStepRequest {
        evidence: &evidence,
        source_selection: &source_selection,
        source_bindings: &source_bindings,
        traces: vec![
            optimization_trace("run-failure", "family-a", TraceOutcome::TaskFailure),
            optimization_trace("run-success", "family-b", TraceOutcome::Success),
        ],
        model_context: ModelRequestContext {
            request_id: "optimizer-request".into(),
            namespace: "tenant".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-a".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: skill_snapshot_digest(&parent).unwrap(),
            bundle_digest: hash(b"parent-bundle"),
            source_closure: allowed.to_vec(),
            model_digest: hash(b"fixture-model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        },
        parent_skill: &parent,
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
        development_request: run_request(),
        allow_rank_call: false,
    };
    let missing = run_optimization_step(
        Some(&EditingFixtureModelPort),
        Some(&FixtureDevRunner),
        None,
        make_request(),
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, Error::NotFound));

    let denied_selection = SourceSelection {
        allow_model_excerpts: false,
        ..source_selection.clone()
    };
    let mut denied_request = make_request();
    denied_request.source_selection = &denied_selection;
    let denied = run_optimization_step(
        Some(&EditingFixtureModelPort),
        Some(&FixtureDevRunner),
        Some(&journal),
        denied_request,
    )
    .await
    .unwrap();
    assert!(matches!(denied, OptimizationStepOutcome::Rejected { .. }));
    assert_eq!(journal.facts.lock().unwrap().len(), 1);
    journal.facts.lock().unwrap().clear();
    journal.recovery_facts.lock().unwrap().clear();

    let outcome = run_optimization_step(
        Some(&EditingFixtureModelPort),
        Some(&FixtureDevRunner),
        Some(&journal),
        make_request(),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, OptimizationStepOutcome::Candidate { .. }));
    assert_eq!(journal.facts.lock().unwrap().len(), 8);
    exercise_sqlite_recovery(&make_request).await;
    exercise_rank_and_authority(&make_request).await;
    exercise_late_revocation(&make_request).await;
    exercise_terminal_outcomes(&make_request).await;
    exercise_missing_watermark(&make_request).await;
}

#[tokio::test]
async fn store_journal_is_idempotent_conflict_detecting_and_reloadable() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("journal.sqlite3"))
        .await
        .unwrap();
    let context = Context::new("tenant", "worker", Role::Worker).unwrap();
    let journal = StoreOptimizationJournal::new(store, context, "optimizer-worker").unwrap();
    let fact = StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: "episode-a-1-1-test".into(),
        namespace: "tenant".into(),
        episode_id: "episode-a".into(),
        step: 1,
        attempt: 1,
        stage: OptimizationJournalStage::ReflectFailure,
        kind: StageFactKind::RequestPrepared,
        request_id: "request-a".into(),
        input_digest: hash(b"input"),
        output_digest: None,
        dependencies: vec![],
        payload: serde_json::json!({"full": "payload"}),
    };
    let fact = fact.seal().unwrap();
    journal.commit(fact.clone()).await.unwrap();
    journal.commit(fact.clone()).await.unwrap();
    assert_eq!(
        journal.reload(&fact.artifact_id).await.unwrap().payload,
        fact.payload
    );

    let mut claimed = fact.clone();
    claimed.request_id = "claim-request".into();
    let claimed = claimed.seal().unwrap();
    assert!(journal.claim(claimed.clone()).await.unwrap());
    assert!(!journal.claim(claimed).await.unwrap());
    let mut conflicting = fact;
    conflicting.payload = serde_json::json!({"full": "different"});
    assert!(journal.commit(conflicting.clone()).await.is_err());
    assert!(journal.commit(conflicting.seal().unwrap()).await.is_err());
}

#[tokio::test]
async fn development_selection_uses_same_complete_manifest_and_never_formal_types() {
    let request = run_request();
    let accepted = run_development(Some(&FixtureDevRunner), request.clone())
        .await
        .unwrap();
    assert_eq!(
        accepted.decision,
        DevelopmentSelectionDecision::AcceptCandidate
    );

    let not_preserved = select_development(&request, &report(&request, false)).unwrap();
    assert_eq!(
        not_preserved.decision,
        DevelopmentSelectionDecision::KeepIncumbent
    );

    let missing = run_development(None, request).await.unwrap_err();
    assert!(matches!(missing, Error::NotFound));
}

// These ports are explicit fixtures, with counters measuring dispatches, not model quality.
#[derive(Default)]
struct CountModel(std::sync::atomic::AtomicUsize);
#[async_trait]
impl ModelPort for CountModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        EditingFixtureModelPort.dispatch(request).await
    }
}
#[derive(Default)]
struct CountRunner(std::sync::atomic::AtomicUsize);
#[async_trait]
impl DevRunner for CountRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        FixtureDevRunner.run(request).await
    }
}

struct CrashJournal {
    inner: StoreOptimizationJournal,
    kind: StageFactKind,
    stage: OptimizationJournalStage,
    failed: std::sync::atomic::AtomicBool,
}
#[async_trait]
impl OptimizationJournal for CrashJournal {
    async fn commit(&self, fact: StageFact) -> Result<()> {
        if (fact.kind == self.kind && fact.stage == self.stage)
            || self.failed.load(std::sync::atomic::Ordering::SeqCst)
        {
            self.failed.store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(Error::Conflict(
                "fixture crash before durable commit".into(),
            ));
        }
        self.inner.commit(fact).await
    }
    async fn claim(&self, fact: StageFact) -> Result<bool> {
        self.inner.claim(fact).await
    }
    async fn lookup(&self, id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(id).await
    }
    async fn check_live(&self, ids: &[String], w: u64) -> Result<()> {
        self.inner.check_live(ids, w).await
    }
    async fn verify_sources(
        &self,
        s: &SourceSelection,
        e: &EvidenceSet,
        b: &[TrustedSourceBinding],
        t: &[OptimizationTrace],
        w: u64,
    ) -> Result<()> {
        self.inner.verify_sources(s, e, b, t, w).await
    }
}

async fn seed_authority(store: &Store, request: &OptimizationStepRequest<'_>) {
    seed_authority_with_watermark(store, request, true).await;
}
async fn seed_authority_with_watermark(
    store: &Store,
    request: &OptimizationStepRequest<'_>,
    initialize: bool,
) {
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
    };
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    for trace in &request.traces {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: trace.run_id.clone(),
                body: trace.run_id.as_bytes().to_vec(),
                parent_family: trace.parent_family.clone(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: trace.clone(),
            excerpt_start: 0,
            excerpt_end: trace.run_id.len(),
        };
        store_trace_authority(store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(store, &host, request.source_selection)
        .await
        .unwrap();
    if !initialize {
        return;
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&host, "initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn exercise_sqlite_recovery<'a>(make_request: impl Fn() -> OptimizationStepRequest<'a>) {
    use std::sync::atomic::Ordering::SeqCst;
    // Fail after all model responses, after Dev dispatch, or immediately before full terminal.
    for (kind, stage, expected_uncertain) in [
        (
            StageFactKind::DispatchObserved,
            OptimizationJournalStage::ReflectFailure,
            true,
        ),
        (
            StageFactKind::EditCompiled,
            OptimizationJournalStage::EditCompile,
            false,
        ),
        (
            StageFactKind::DevelopmentRequestPrepared,
            OptimizationJournalStage::Development,
            false,
        ),
        (
            StageFactKind::DispatchObserved,
            OptimizationJournalStage::Development,
            true,
        ),
        (
            StageFactKind::StepCompleted,
            OptimizationJournalStage::Merge,
            false,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovery.sqlite3");
        let store = Store::open(&path).await.unwrap();
        seed_authority(&store, &make_request()).await;
        let context = Context::new("tenant", "worker", Role::Worker).unwrap();
        let first = CrashJournal {
            inner: StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap(),
            kind,
            stage,
            failed: Default::default(),
        };
        let model = CountModel::default();
        let runner = CountRunner::default();
        assert!(
            run_optimization_step(Some(&model), Some(&runner), Some(&first), make_request())
                .await
                .is_err()
        );
        assert_eq!(
            model.0.load(SeqCst),
            if stage == OptimizationJournalStage::ReflectFailure {
                1
            } else {
                2
            }
        );
        drop(first);
        drop(store);
        let reopened = Store::open(&path).await.unwrap();
        let journal =
            StoreOptimizationJournal::new(reopened.clone(), context.clone(), "worker").unwrap();
        let resumed_model = CountModel::default();
        let resumed_runner = CountRunner::default();
        let outcome = run_optimization_step(
            Some(&resumed_model),
            Some(&resumed_runner),
            Some(&journal),
            make_request(),
        )
        .await
        .unwrap();
        assert_eq!(
            resumed_model.0.load(SeqCst),
            0,
            "restart must reuse model results"
        );
        assert_eq!(
            resumed_runner.0.load(SeqCst),
            usize::from(matches!(
                kind,
                StageFactKind::EditCompiled | StageFactKind::DevelopmentRequestPrepared
            ))
        );
        if expected_uncertain {
            assert!(matches!(outcome, OptimizationStepOutcome::Uncertain { .. }));
        } else {
            let OptimizationStepOutcome::Candidate {
                edit,
                bundle,
                selection,
            } = outcome
            else {
                panic!("expected complete candidate")
            };
            assert!(edit.output.content.contains("["));
            assert!(!bundle.digest.is_empty());
            assert_eq!(
                selection.decision,
                DevelopmentSelectionDecision::AcceptCandidate
            );
            let recovered = run_optimization_step(None, None, Some(&journal), make_request())
                .await
                .unwrap();
            let OptimizationStepOutcome::Candidate {
                edit: again,
                bundle: again_bundle,
                selection: again_selection,
            } = recovered
            else {
                panic!("candidate must be reconstructible")
            };
            assert_eq!(
                serde_json::to_value(&edit.report).unwrap(),
                serde_json::to_value(&again.report).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&edit.patch).unwrap(),
                serde_json::to_value(&again.patch).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&edit.output).unwrap(),
                serde_json::to_value(&again.output).unwrap()
            );
            assert_eq!(bundle.digest, again_bundle.digest);
            assert_eq!(
                serde_json::to_value(&selection).unwrap(),
                serde_json::to_value(&again_selection).unwrap()
            );
        }
        let mut changed = make_request();
        changed.allow_rank_call = true;
        assert!(matches!(
            run_optimization_step(
                Some(&resumed_model),
                Some(&resumed_runner),
                Some(&journal),
                changed
            )
            .await,
            Err(Error::Conflict(_))
        ));
        let mut session = reopened.session().await.unwrap();
        let dependent = session
            .dependents(&context, "run", "run-success")
            .await
            .unwrap();
        assert!(!dependent.is_empty());
        // Non-primary source2 deletion blocks reads and all further candidate production.
        session
            .delete(&context, "run", "run-success")
            .await
            .unwrap();
        session
            .put(
                &context,
                "tombstone",
                "run-success",
                "worker",
                &serde_json::json!({"deleted":true}),
            )
            .await
            .unwrap();
        session
            .bump_watermark(&context, "source2-deleted")
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(
            run_optimization_step(
                Some(&resumed_model),
                Some(&resumed_runner),
                Some(&journal),
                make_request()
            )
            .await
            .is_err()
        );
        assert_eq!(resumed_model.0.load(SeqCst), 0);
        for (kind, id) in dependent {
            if kind == "artifact" && id.starts_with("optstage-") {
                assert!(journal.reload(&id).await.is_err());
            }
        }
        // Minimal billing/raw execution history remains physically present after logical revocation.
        let mut session = reopened.session().await.unwrap();
        let facts: Vec<serde_json::Value> = session.list(&context, "artifact").await.unwrap();
        assert!(facts.iter().any(|v| v["kind"]
            == if stage == OptimizationJournalStage::ReflectFailure {
                "dispatch_prepared"
            } else {
                "dispatch_observed"
            }));
        session.commit().await.unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("falsegrant.sqlite3"))
        .await
        .unwrap();
    seed_authority(&store, &make_request()).await;
    let journal = StoreOptimizationJournal::new(
        store,
        Context::new("tenant", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    let model = CountModel::default();
    let runner = CountRunner::default();
    let mut request = make_request();
    let denied = SourceSelection {
        allow_model_excerpts: false,
        ..request.source_selection.clone()
    };
    request.source_selection = &denied;
    assert!(matches!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), request)
            .await
            .unwrap(),
        OptimizationStepOutcome::Rejected { .. }
    ));
    assert_eq!(model.0.load(SeqCst), 0);
    assert_eq!(runner.0.load(SeqCst), 0);
}

struct RankingFixture {
    invalid: bool,
    ranks: std::sync::atomic::AtomicUsize,
    reflections: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ModelPort for RankingFixture {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        use std::sync::atomic::Ordering::SeqCst;
        if request.stage == ModelStage::Rank {
            self.ranks.fetch_add(1, SeqCst);
            let output = if self.invalid {
                "[\"invented-id\"]"
            } else {
                "[\"fix-rule-0\",\"preserve-rule-0\"]"
            }
            .to_string();
            return Ok(ModelResponse::Completed {
                request_id: request.request_id,
                response_id: "rank-result".into(),
                actual_model_digest: request.model_digest,
                input_digest: request.input_digest,
                output_digest: hash(output.as_bytes()),
                output,
                execution_receipt: ModelExecutionReceipt {
                    call_id: "rank-call".into(),
                    dispatch_id: "rank-dispatch".into(),
                    root_budget_id: "fixture-budget".into(),
                    provider_request_id: "rank-provider".into(),
                    usage_record_id: "rank-usage".into(),
                    provenance: ModelExecutionProvenance::Fixture,
                },
            });
        }
        self.reflections.fetch_add(1, SeqCst);
        let mut response = EditingFixtureModelPort.dispatch(request).await?;
        if let ModelResponse::Completed {
            output,
            output_digest,
            ..
        } = &mut response
        {
            let original: Vec<EditSuggestion> = serde_json::from_str(output).unwrap();
            let items = (0..3)
                .map(|i| {
                    let mut item = original[0].clone();
                    item.id = format!("{}-{i}", item.id);
                    item
                })
                .collect::<Vec<_>>();
            *output = serde_json::to_string(&items).unwrap();
            *output_digest = hash(output.as_bytes());
        }
        Ok(response)
    }
}

async fn exercise_rank_and_authority<'a>(make_request: impl Fn() -> OptimizationStepRequest<'a>) {
    use std::sync::atomic::Ordering::SeqCst;
    for invalid in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("rank.sqlite3"))
            .await
            .unwrap();
        seed_authority(&store, &make_request()).await;
        let context = Context::new("tenant", "worker", Role::Worker).unwrap();
        let journal =
            StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
        let model = RankingFixture {
            invalid,
            ranks: Default::default(),
            reflections: Default::default(),
        };
        let runner = CountRunner::default();
        let mut request = make_request();
        request.allow_rank_call = true;
        request.model_context.request_id = "R".repeat(120);
        let outcome = run_optimization_step(Some(&model), Some(&runner), Some(&journal), request)
            .await
            .unwrap();
        assert_eq!(model.ranks.load(SeqCst), 1);
        assert_eq!(model.reflections.load(SeqCst), 2);
        if invalid {
            assert!(matches!(outcome, OptimizationStepOutcome::Rejected { .. }));
            assert_eq!(runner.0.load(SeqCst), 0);
        } else {
            assert!(matches!(outcome, OptimizationStepOutcome::Candidate { .. }));
            assert_eq!(runner.0.load(SeqCst), 1);
        }
        let mut request = make_request();
        request.allow_rank_call = true;
        request.model_context.request_id = "R".repeat(120);
        run_optimization_step(None, None, Some(&journal), request)
            .await
            .unwrap();
        assert_eq!(model.ranks.load(SeqCst), 1);
        // Same episode/step with a different case-sensitive request base is isolated.
        let mut request = make_request();
        request.allow_rank_call = true;
        request.model_context.request_id = "r".repeat(120);
        let second_case =
            run_optimization_step(Some(&model), Some(&runner), Some(&journal), request)
                .await
                .unwrap();
        if !invalid {
            assert!(matches!(
                second_case,
                OptimizationStepOutcome::Candidate { .. }
            ));
        }
        assert_eq!(model.ranks.load(SeqCst), 2);
        let mut session = store.session().await.unwrap();
        let facts: Vec<serde_json::Value> = session.list(&context, "artifact").await.unwrap();
        for fact in facts
            .iter()
            .filter(|v| v["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA)
        {
            assert!(fact["artifact_id"].as_str().unwrap().len() < 128);
        }
        session.commit().await.unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("authority.sqlite3"))
        .await
        .unwrap();
    let context = Context::new("tenant", "worker", Role::Worker).unwrap();
    let journal = StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
    let model = CountModel::default();
    let runner = CountRunner::default();
    // Caller-constructed evidence/bindings alone never confer trust.
    assert!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), make_request())
            .await
            .is_err()
    );
    assert_eq!(model.0.load(SeqCst), 0);
    seed_authority(&store, &make_request()).await;
    let grant_id = format!(
        "optgrant-{}",
        evo_core::fingerprint(make_request().source_selection).unwrap()
    );
    let mut session = store.session().await.unwrap();
    session
        .delete(&context, "artifact", &grant_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), make_request())
            .await
            .is_err()
    );
    assert_eq!(model.0.load(SeqCst), 0);
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    evo_engine::evidence::store_source_selection(&store, &host, make_request().source_selection)
        .await
        .unwrap();
    let mut forged = make_request();
    forged.traces[0].excerpt = "unbacked bytes".into();
    assert!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), forged)
            .await
            .is_err()
    );
    assert_eq!(model.0.load(SeqCst), 0);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&context, "unrelated-revocation")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), make_request())
            .await
            .is_err()
    );
    assert_eq!(model.0.load(SeqCst), 0);
}

#[tokio::test]
async fn host_source_authority_rejects_import_and_wrong_excerpt_range() {
    use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("authority.sqlite3"))
        .await
        .unwrap();
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    let mut authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "run-failure".into(),
            body: b"run-failure".to_vec(),
            parent_family: "family-a".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::UnverifiedImport,
            purpose: Purpose::Development,
        },
        trace: optimization_trace("run-failure", "family-a", TraceOutcome::TaskFailure),
        excerpt_start: 0,
        excerpt_end: 11,
    };
    assert!(
        store_trace_authority(&store, &host, &authority)
            .await
            .is_err()
    );
    authority.record.execution_attestation = ExecutionAttestation::TrustedHost;
    authority.excerpt_end = 10;
    assert!(
        store_trace_authority(&store, &host, &authority)
            .await
            .is_err()
    );
    authority.excerpt_end = 11;
    let worker = Context::new("tenant", "worker", Role::Worker).unwrap();
    assert!(
        store_trace_authority(&store, &worker, &authority)
            .await
            .is_err()
    );
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session.delete(&host, "run", "run-failure").await.unwrap();
    session.redact_cache(&host, "run-failure").await.unwrap();
    session.commit().await.unwrap();
    assert!(
        store_trace_authority(&store, &host, &authority)
            .await
            .is_err()
    );
}

struct RevokingModel {
    store: Store,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ModelPort for RevokingModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let response = EditingFixtureModelPort.dispatch(request).await?;
        let host = Context::new("tenant", "host", Role::Host)?;
        let mut session = self.store.session().await?;
        session
            .put(
                &host,
                "tombstone",
                "run-success",
                "host",
                &serde_json::json!({"revoked":true}),
            )
            .await?;
        session
            .bump_watermark(&host, "late-source2-revocation")
            .await?;
        session.commit().await?;
        Ok(response)
    }
}
async fn exercise_late_revocation<'a>(make_request: impl Fn() -> OptimizationStepRequest<'a>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("late.sqlite3"))
        .await
        .unwrap();
    seed_authority(&store, &make_request()).await;
    let context = Context::new("tenant", "worker", Role::Worker).unwrap();
    let journal = StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
    let model = RevokingModel {
        store: store.clone(),
        calls: Default::default(),
    };
    let runner = CountRunner::default();
    assert!(
        run_optimization_step(Some(&model), Some(&runner), Some(&journal), make_request())
            .await
            .is_err()
    );
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(runner.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    let mut session = store.session().await.unwrap();
    let facts: Vec<serde_json::Value> = session.list(&context, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert!(facts.iter().any(|v| v["kind"] == "dispatch_observed"));
    assert!(!facts.iter().any(|v| v["kind"] == "terminal_candidate"));
}

struct TerminalFixture {
    mode: u8,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ModelPort for TerminalFixture {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.mode == 2 {
            return Ok(ModelResponse::Uncertain {
                request_id: request.request_id,
                dispatch_id: "unknown-dispatch".into(),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: None,
                usage_record_id: None,
            });
        }
        let mut response = FixtureModelPort.dispatch(request).await?;
        if self.mode == 1
            && let ModelResponse::Completed {
                output,
                output_digest,
                ..
            } = &mut response
        {
            *output = "invalid json".into();
            *output_digest = hash(output.as_bytes());
        }
        Ok(response)
    }
}
async fn exercise_terminal_outcomes<'a>(make_request: impl Fn() -> OptimizationStepRequest<'a>) {
    for mode in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("terminal.sqlite3"))
            .await
            .unwrap();
        seed_authority(&store, &make_request()).await;
        let context = Context::new("tenant", "worker", Role::Worker).unwrap();
        let journal =
            StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
        let model = TerminalFixture {
            mode,
            calls: Default::default(),
        };
        let runner = CountRunner::default();
        for _ in 0..2 {
            let outcome =
                run_optimization_step(Some(&model), Some(&runner), Some(&journal), make_request())
                    .await
                    .unwrap();
            match mode {
                0 => assert!(matches!(outcome, OptimizationStepOutcome::NoChange { .. })),
                1 => assert!(matches!(outcome, OptimizationStepOutcome::Rejected { .. })),
                _ => assert!(matches!(outcome, OptimizationStepOutcome::Uncertain { .. })),
            }
        }
        assert_eq!(
            model.calls.load(std::sync::atomic::Ordering::SeqCst),
            if mode == 0 { 2 } else { 1 }
        );
        assert_eq!(runner.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        if mode == 2 {
            let mut session = store.session().await.unwrap();
            let facts: Vec<serde_json::Value> = session.list(&context, "artifact").await.unwrap();
            session.commit().await.unwrap();
            assert!(!facts.iter().any(|v| v["kind"] == "terminal_rejected"));
            assert!(facts.iter().any(|v|v["kind"]=="step_completed" && v["payload"]["status"]=="uncertain"));
        }
    }
    // A reflection-build error is itself a durable terminal, before any paid model work.
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("build-error.sqlite3"))
        .await
        .unwrap();
    seed_authority(&store, &make_request()).await;
    let context = Context::new("tenant", "worker", Role::Worker).unwrap();
    let journal = StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
    let model = CountModel::default();
    let runner = CountRunner::default();
    for _ in 0..2 {
        let mut request = make_request();
        request.traces.push(request.traces[0].clone());
        assert!(matches!(
            run_optimization_step(Some(&model), Some(&runner), Some(&journal), request)
                .await
                .unwrap(),
            OptimizationStepOutcome::Rejected { .. }
        ));
    }
    assert_eq!(model.0.load(std::sync::atomic::Ordering::SeqCst), 0);
}

async fn exercise_missing_watermark<'a>(make_request: impl Fn() -> OptimizationStepRequest<'a>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("missing-watermark.sqlite3"))
        .await
        .unwrap();
    // Restore source and grant objects without their watermark; never interpret this as an empty new namespace.
    seed_authority_with_watermark(&store, &make_request(), false).await;
    let context = Context::new("tenant", "worker", Role::Worker).unwrap();
    let fact = StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: "tenant".into(),
        episode_id: "episode-a".into(),
        step: 1,
        attempt: 1,
        stage: OptimizationJournalStage::ReflectFailure,
        kind: StageFactKind::DispatchObserved,
        request_id: "restored-stage".into(),
        input_digest: hash(b"frozen-input"),
        output_digest: None,
        dependencies: vec![
            evo_engine::optimization::StageDependency {
                kind: "run".into(),
                id: "run-success".into(),
            },
            evo_engine::optimization::StageDependency {
                kind: "revoke_watermark".into(),
                id: "1".into(),
            },
        ],
        payload: serde_json::json!({"restored":true}),
    }
    .seal()
    .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(&context, "artifact", &fact.artifact_id, "worker", &fact)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let journal = StoreOptimizationJournal::new(store.clone(), context.clone(), "worker").unwrap();
    assert!(
        matches!(journal.reload(&fact.artifact_id).await,Err(Error::Conflict(message)) if message.contains("missing source revoke watermark"))
    );
    let model = CountModel::default();
    let runner = CountRunner::default();
    assert!(
        matches!(run_optimization_step(Some(&model),Some(&runner),Some(&journal),make_request()).await,Err(Error::Conflict(message)) if message.contains("missing source revoke watermark"))
    );
    assert_eq!(model.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    let mut session = store.session().await.unwrap();
    assert!(session.watermark(&context).await.unwrap().is_none());
    session.commit().await.unwrap();
}

#[tokio::test]
async fn legacy_run_is_explicitly_unsupported_and_never_overwritten() {
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, load_stored_source, store_trace_authority,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("legacy.sqlite3"))
        .await
        .unwrap();
    let host = Context::new("tenant", "host", Role::Host).unwrap();
    let legacy = serde_json::json!({"id":"run-failure","used":true,"status":"complete"});
    let mut session = store.session().await.unwrap();
    session
        .put(&host, "run", "run-failure", "host", &legacy)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut session = store.session().await.unwrap();
    assert!(
        matches!(load_stored_source(&mut session,&host,"run-failure").await,Err(Error::Invalid(message)) if message.contains("unsupported or unverified legacy"))
    );
    session.commit().await.unwrap();
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "run-failure".into(),
            body: b"run-failure".to_vec(),
            parent_family: "family-a".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: optimization_trace("run-failure", "family-a", TraceOutcome::TaskFailure),
        excerpt_start: 0,
        excerpt_end: 11,
    };
    assert!(
        matches!(store_trace_authority(&store,&host,&authority).await,Err(Error::Invalid(message)) if message.contains("unsupported or unverified legacy"))
    );
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .need::<serde_json::Value>(&host, "run", "run-failure")
            .await
            .unwrap(),
        legacy
    );
    session.commit().await.unwrap();
}
