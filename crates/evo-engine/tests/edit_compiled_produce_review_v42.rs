//! AG-116 (E06/E07 stored review view, plan §6.5 L688; V070 L1578 sub-scope;
//! §5.1 L455, §5.2 L459): the `EditCompiled` stage fact that the production step
//! path writes reaches `review_stored` with an exact diff, and a one-byte change
//! of that stored fact is refused.
//!
//! Every stage fact reviewed here is written by `run_optimization_step` through
//! the Store journal; no `StepPrepared` or `EditCompiled` is assembled by hand
//! (plan L101: a fixture's predetermined answer is no substitute for the real
//! loop). The only direct writes are the source, grant and watermark authority
//! seeded as `seed_authority` in tests/optimization.rs does, and, in E2, the
//! tamper itself. The model and the development runner are the explicit
//! deterministic fixtures of tests/optimization.rs (Fixture provenance). Nothing
//! here covers a real model, the CLI/HTTP review path, other stage facts beyond
//! `StepPrepared`, the whole of E06/E07/V070, or any benefit.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    EditSuggestion, MergeRecord, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EditApplyReport, EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA,
    SkillEditBatch, SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext,
    skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::formal_authority::FormalCompilerInputBinding;
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentSelectionDecision, DevelopmentTask,
    OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournalStage, OptimizationStepOutcome,
    OptimizationStepRequest, PairedTaskResult, StageFact, StageFactKind,
    edit_compiled_input_digest, run_optimization_step,
};
use evo_engine::review::{
    ReviewDetail, ReviewGap, ReviewKind, ReviewPayload, ReviewRequest, ReviewResponse,
    review_stored,
};
use evo_storage::Store;
use serde_json::Value;
use std::collections::BTreeSet;
use trusted_compiler_inputs::StoreOptimizationJournal;

// ---------------------------------------------------------------------------
// Fixtures copied from tests/optimization.rs (run_request, report,
// FixtureDevRunner, EditingFixtureModelPort, optimization_evidence,
// optimization_trace, seed_authority, and the g1 request construction). They are
// copied, not shared, so that file stays untouched.
// ---------------------------------------------------------------------------

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

fn report(request: &DevelopmentRunRequest) -> DevelopmentRunReport {
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
                candidate_passed: true,
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
        Ok(report(&request))
    }
}

struct EditingFixtureModelPort;

#[async_trait]
impl ModelPort for EditingFixtureModelPort {
    // This is an explicit deterministic fixture adapter. Its receipts retain
    // Fixture provenance; the production broker verifies the full use itself.
    async fn dispatch_bound(
        &self,
        request: evo_engine::model::BoundModelRequest,
    ) -> evo_core::Result<evo_engine::model::ModelResponse> {
        request.validate()?;
        self.dispatch(request.request).await
    }

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

/// The source, grant and watermark authority a real host would have stored
/// before the step runs (tests/optimization.rs `seed_authority`).
async fn seed_authority(store: &Store, request: &OptimizationStepRequest<'_>) {
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
    let mut session = store.session().await.unwrap();
    // The one change from the copied seed: the watermark digest is a hash, the
    // format every production writer uses (revocation, import), because stored
    // review requires a 64-hex watermark digest. Storage itself only checks it
    // is an identifier, so the copied plain string was accepted but unreviewable.
    session
        .bump_watermark(&host, &hash(b"initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// The owned inputs of the g1 test's `make_request` closure.
struct StepInputs {
    evidence: EvidenceSet,
    source_selection: SourceSelection,
    source_bindings: Vec<TrustedSourceBinding>,
    parent: SkillSnapshot,
    baseline: SkillSnapshot,
    allowed: Vec<EvidenceRef>,
    edit_context: TrustedEditContext,
    edit_template: SkillEditBatch,
    profile: Profile,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver_patch: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
}

impl StepInputs {
    fn new() -> Self {
        let parent = SkillSnapshot {
            content: "old".into(),
            applicability: "applies".into(),
            counterexample: "counter".into(),
            required_capabilities: vec![],
            dependencies: vec![],
        };
        let allowed = vec![
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
        Self {
            evidence: optimization_evidence(),
            source_selection: SourceSelection {
                roots: vec![],
                run_ids: vec!["run-failure".into(), "run-success".into()],
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            },
            source_bindings: vec![
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
            ],
            baseline: parent.clone(),
            parent,
            allowed,
            edit_context,
            edit_template,
            profile: Profile {
                id: "profile-a".into(),
                evolution_enabled: true,
                parent_digest: hash(b"parent"),
                baseline_digest: hash(b"baseline"),
            },
            parent_strategy: Strategy::default(),
            baseline_strategy: Strategy::default(),
            improver_patch: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
        }
    }

    fn request(&self) -> OptimizationStepRequest<'_> {
        trusted_compiler_inputs::request(OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.source_bindings,
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
                parent_skill_digest: skill_snapshot_digest(&self.parent).unwrap(),
                bundle_digest: hash(b"parent-bundle"),
                source_closure: self.allowed.clone(),
                model_digest: hash(b"fixture-model"),
                tools_digest: hash(b"tools"),
                rules_digest: hash(b"rules"),
                sampling_digest: hash(b"sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &self.parent,
            edit_context: &self.edit_context,
            edit_batch_template: self.edit_template.clone(),
            protected_ranges: &[],
            bundle_context: BundleCompileContext {
                profile: &self.profile,
                baseline: &self.baseline,
                parent_strategy: &self.parent_strategy,
                baseline_strategy: &self.baseline_strategy,
                improver_patch: &self.improver_patch,
                caps: &self.caps,
                revoked: &self.revoked,
            },
            development_request: run_request(),
            allow_rank_call: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Production run and stored-fact access.
// ---------------------------------------------------------------------------

/// One store after one production step that reached a development candidate.
/// `compiled` and `prepared` are read back from the store exactly as the
/// producer wrote them; `report` and `bundle_digest` are what the step returned.
struct Produced {
    store: Store,
    admin: Context,
    compiled: StageFact,
    prepared: StageFact,
    report: EditApplyReport,
    bundle_digest: String,
    // Declared last so the database file outlives the open store.
    _directory: tempfile::TempDir,
}

async fn produce() -> Produced {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("produce-review.sqlite3"))
        .await
        .unwrap();
    let inputs = StepInputs::new();
    seed_authority(&store, &inputs.request()).await;
    let worker = Context::new("tenant", "worker", Role::Worker).unwrap();
    let journal = StoreOptimizationJournal::new(store.clone(), worker, "worker").unwrap();
    let outcome = run_optimization_step(
        Some(&EditingFixtureModelPort),
        Some(&FixtureDevRunner),
        Some(&journal),
        inputs.request(),
    )
    .await
    .unwrap();
    let OptimizationStepOutcome::Candidate {
        edit,
        bundle,
        selection,
    } = outcome
    else {
        panic!("the production step did not reach a development candidate")
    };
    assert_eq!(
        selection.decision,
        DevelopmentSelectionDecision::AcceptCandidate
    );
    let admin = Context::new("tenant", "reviewer", Role::Admin).unwrap();
    let facts = stage_facts(&store, &admin).await;
    let compiled = only(&facts, StageFactKind::EditCompiled);
    let prepared = only(&facts, StageFactKind::StepPrepared);
    Produced {
        store,
        admin,
        compiled,
        prepared,
        report: edit.report,
        bundle_digest: bundle.digest,
        _directory: directory,
    }
}

/// Every optimization stage fact in the namespace, as stored.
async fn stage_facts(store: &Store, admin: &Context) -> Vec<StageFact> {
    let mut session = store.session().await.unwrap();
    let bodies: Vec<Value> = session.list(admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    bodies
        .into_iter()
        .filter(|body| {
            body.get("schema_version").and_then(Value::as_str)
                == Some(OPTIMIZATION_STAGE_FACT_SCHEMA)
        })
        .map(|body| serde_json::from_value(body).unwrap())
        .collect()
}

fn only(facts: &[StageFact], kind: StageFactKind) -> StageFact {
    let found: Vec<_> = facts.iter().filter(|fact| fact.kind == kind).collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one production {kind:?} fact"
    );
    found[0].clone()
}

/// The compiled batch, rebuilt only from what the producer stored: the edit
/// template of its `StepPrepared` and the merged evidence and edits of its
/// `EditCompiled`.
fn stored_batch(prepared: &StageFact, compiled: &StageFact) -> SkillEditBatch {
    let mut batch: SkillEditBatch =
        serde_json::from_value(prepared.payload["edit_template"].clone()).unwrap();
    let merge: MergeRecord = serde_json::from_value(compiled.payload["merge"].clone()).unwrap();
    batch.evidence = merge.evidence;
    batch.edits = merge.edits;
    batch
}

async fn review(produced: &Produced, id: &str, detail: ReviewDetail) -> ReviewResponse {
    review_stored(
        &produced.admin,
        &produced.store,
        ReviewRequest {
            kind: ReviewKind::StageFact,
            id: id.into(),
            detail,
        },
    )
    .await
    .unwrap()
}

async fn read_body(produced: &Produced, id: &str) -> Value {
    let mut session = produced.store.session().await.unwrap();
    let body: Value = session
        .get(&produced.admin, "artifact", id)
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    body
}

/// A direct database write of the stored body, as an attacker with write access
/// to the store (not any product path) would make it. Edges are left as stored.
async fn write_body(produced: &Produced, id: &str, body: &Value) {
    let mut session = produced.store.session().await.unwrap();
    session
        .put(&produced.admin, "artifact", id, "worker", body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// The gaps every complete stored stage review carries: what review never binds.
fn unbound_by_design() -> Vec<ReviewGap> {
    vec![
        ReviewGap::CostUnobserved,
        ReviewGap::HostApplicationUnbound,
        ReviewGap::ApprovalUnbound,
        ReviewGap::RollbackTargetUnbound,
    ]
}

/// Replaces exactly one byte of the text the first edit inserts.
fn change_one_byte(edit: &mut SkillTextEdit) {
    let (TextEditOperation::Insert { text } | TextEditOperation::Replace { text }) =
        &mut edit.operation
    else {
        panic!("the production edit inserts no text")
    };
    let at = text
        .find('[')
        .expect("the fixture model inserts a bracketed instruction");
    text.replace_range(at..at + 1, "(");
}

fn assert_refused(response: &ReviewResponse, what: &str) {
    assert!(
        response.payload.is_none() && !response.diff_complete,
        "{what} was reviewed: {:?}",
        response.gaps
    );
    assert!(
        response.gaps.contains(&ReviewGap::MaterialInvalid),
        "{what} was not refused as material_invalid: {:?}",
        response.gaps
    );
}

fn assert_exact_diff(response: &ReviewResponse, produced: &Produced) {
    assert!(
        !response.gaps.contains(&ReviewGap::MaterialInvalid),
        "a production-written EditCompiled fact was refused: {:?}",
        response.gaps
    );
    assert_eq!(response.gaps, unbound_by_design());
    assert!(
        response.diff_complete,
        "expected a complete exact diff of the production fact: {:?}",
        response.gaps
    );
    let Some(ReviewPayload::StageFact(view)) = &response.payload else {
        panic!("no stage payload for the production fact")
    };
    assert!(view.facts.iter().any(|fact| {
        fact.kind == StageFactKind::EditCompiled
            && fact.artifact_id == produced.compiled.artifact_id
    }));
    assert!(view.facts.iter().any(|fact| {
        fact.kind == StageFactKind::StepPrepared
            && fact.artifact_id == produced.prepared.artifact_id
    }));
    assert_eq!(view.sources.len(), 2);
    assert!(view.prepared.is_some());
    assert_eq!(
        view.bundle
            .as_ref()
            .map(|bundle| bundle.bundle_digest.as_str()),
        Some(produced.bundle_digest.as_str())
    );
    let diff = view
        .diff
        .as_ref()
        .expect("the review payload holds no diff");
    assert_eq!(diff.status, produced.report.status);
    assert_eq!(diff.changed_bytes, Some(produced.report.changed_bytes));
    assert!(!diff.edits.is_empty());
    assert_eq!(diff.edits.len(), produced.report.edits.len());
    for (shown, applied) in diff.edits.iter().zip(&produced.report.edits) {
        assert_eq!(shown.field, applied.field);
        assert_eq!(shown.start, Some(applied.start));
        assert_eq!(shown.end, Some(applied.end));
        assert_eq!(
            shown.removed.display.as_deref(),
            Some(applied.removed_text.as_str())
        );
        assert_eq!(
            shown.inserted.display.as_deref(),
            Some(applied.inserted_text.as_str())
        );
        assert_eq!(
            shown.inserted.original_field_sha256.as_deref(),
            Some(applied.inserted_digest.as_str())
        );
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e1_a_production_written_edit_compiled_fact_reviews_to_its_exact_diff() {
    let produced = produce().await;
    let compiled = &produced.compiled;
    let prepared = &produced.prepared;
    assert_eq!(compiled.stage, OptimizationJournalStage::EditCompile);
    // The stored key is the sealed canonical stage id, not the id the producer
    // assembles before `seal`; a review of the pre-seal form finds nothing.
    assert!(compiled.artifact_id.starts_with("optstage-"));
    let pre_seal = format!(
        "{}-{}-{}-edit",
        compiled.episode_id, compiled.step, compiled.attempt
    );
    assert!(matches!(
        review_stored(
            &produced.admin,
            &produced.store,
            ReviewRequest {
                kind: ReviewKind::StageFact,
                id: pre_seal,
                detail: ReviewDetail::Exact,
            },
        )
        .await,
        Err(Error::NotFound)
    ));
    // The stored fact is the candidate the step returned, bound to its own
    // StepPrepared under the same compiler request and binding.
    assert_eq!(
        compiled.payload["report"],
        serde_json::to_value(&produced.report).unwrap()
    );
    assert_eq!(
        compiled.payload["bundle"]["digest"].as_str(),
        Some(produced.bundle_digest.as_str())
    );
    assert!(
        compiled.dependencies.iter().any(
            |dependency| dependency.kind == "artifact" && dependency.id == prepared.artifact_id
        )
    );
    assert_eq!(
        compiled.payload["compiler_request"],
        prepared.payload["compiler_request"]
    );
    assert_eq!(
        compiled.payload["compiler_binding"],
        prepared.payload["compiler_binding"]
    );

    // Plan §5.2 L459 / §5.1 L455: the input_digest the producer wrote is the one
    // shared rule over the complete compiled batch and its compiler binding, both
    // taken from the payloads the producer stored.
    let batch = stored_batch(prepared, compiled);
    let binding: FormalCompilerInputBinding =
        serde_json::from_value(compiled.payload["compiler_binding"].clone()).unwrap();
    assert_eq!(
        compiled.input_digest,
        edit_compiled_input_digest(&batch, Some(&binding)).unwrap()
    );
    // The same formula spelled out independently, and it is never the bare-batch
    // digest that review compared before AG-108.
    assert_eq!(
        compiled.input_digest,
        fingerprint(&(&batch, &binding)).unwrap()
    );
    assert_ne!(compiled.input_digest, fingerprint(&batch).unwrap());

    // V070 L1578 / §6.5 L688: the stored review presents the exact effective diff.
    let exact = review(&produced, &compiled.artifact_id, ReviewDetail::Exact).await;
    assert_exact_diff(&exact, &produced);

    // The default detail keeps the material but shows no text, so it is never a
    // complete diff.
    let metadata = review(&produced, &compiled.artifact_id, ReviewDetail::Metadata).await;
    assert!(
        metadata.payload.is_some() && !metadata.diff_complete,
        "the default detail lost the production fact: {:?}",
        metadata.gaps
    );
    assert!(!metadata.gaps.contains(&ReviewGap::MaterialInvalid));
}

#[tokio::test]
async fn e2_a_one_byte_change_of_the_production_edit_compiled_fact_is_refused() {
    let produced = produce().await;
    let id = produced.compiled.artifact_id.clone();
    let original = read_body(&produced, &id).await;
    assert_exact_diff(
        &review(&produced, &id, ReviewDetail::Exact).await,
        &produced,
    );

    let mut merge: MergeRecord =
        serde_json::from_value(produced.compiled.payload["merge"].clone()).unwrap();
    change_one_byte(&mut merge.edits[0]);
    let mut changed = produced.compiled.clone();
    changed.payload["merge"] = serde_json::to_value(merge).unwrap();

    // (a) Resealed: the payload digest is refreshed, so the fact is internally
    // consistent; only the producer's input_digest no longer matches the batch.
    let resealed = changed.clone().seal().unwrap();
    assert_eq!(resealed.artifact_id, id);
    assert_eq!(resealed.input_digest, produced.compiled.input_digest);
    assert_ne!(resealed.output_digest, produced.compiled.output_digest);
    write_body(&produced, &id, &serde_json::to_value(resealed).unwrap()).await;
    assert_refused(
        &review(&produced, &id, ReviewDetail::Exact).await,
        "a resealed one-byte change",
    );

    // (b) Written as is: the stored payload digest is stale.
    assert_eq!(changed.output_digest, produced.compiled.output_digest);
    write_body(&produced, &id, &serde_json::to_value(changed).unwrap()).await;
    assert_refused(
        &review(&produced, &id, ReviewDetail::Exact).await,
        "an unsealed one-byte change",
    );

    // Control: the producer's own bytes, restored, review completely again, so
    // each refusal above came from the one changed byte.
    write_body(&produced, &id, &original).await;
    assert_exact_diff(
        &review(&produced, &id, ReviewDetail::Exact).await,
        &produced,
    );
}

#[tokio::test]
async fn e3_the_production_step_prepared_fact_reviews_to_its_complete_prepared_material() {
    let produced = produce().await;
    let response = review(
        &produced,
        &produced.prepared.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    assert_eq!(
        response.gaps,
        unbound_by_design(),
        "the production StepPrepared fact was not fully bound"
    );
    // A StepPrepared closure holds no compiled edit, so there is no diff to
    // complete; its parent, baseline and strategies are all bound and shown.
    assert!(!response.diff_complete);
    let Some(ReviewPayload::StageFact(view)) = &response.payload else {
        panic!("no stage payload for the production StepPrepared fact")
    };
    assert_eq!(view.facts.len(), 1);
    assert_eq!(view.facts[0].kind, StageFactKind::StepPrepared);
    assert_eq!(view.facts[0].artifact_id, produced.prepared.artifact_id);
    assert_eq!(view.sources.len(), 2);
    assert!(view.diff.is_none() && view.bundle.is_none() && view.merge.is_none());
    let prepared = view
        .prepared
        .as_ref()
        .expect("the production StepPrepared material was not bound");
    assert_eq!(prepared.parent.content.display.as_deref(), Some("old"));
    assert_eq!(prepared.baseline.content.display.as_deref(), Some("old"));
    assert_eq!(
        Some(prepared.declared_parent_digest.as_str()),
        produced.prepared.payload["profile"]["parent_digest"].as_str()
    );
    assert_eq!(
        Some(prepared.declared_baseline_digest.as_str()),
        produced.prepared.payload["profile"]["baseline_digest"].as_str()
    );
}
