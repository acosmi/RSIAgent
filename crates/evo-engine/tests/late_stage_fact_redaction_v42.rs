//! AG-045 (E08/§11, E03/E13): a stage fact that carries a model's answer and is
//! written after one of its source runs was revoked is stored redacted, and the
//! answer is nowhere in the database.
//!
//! Plan §11 says that when a source is deleted while a model request is in
//! flight, the late output can form no usable candidate while its real cost is
//! still booked, that the late result of an already dispatched call cannot regain
//! validity, and that the closure of a revocation holds the jobs, the caches and
//! the host receipts without a silent miss. AG-043 stopped the ledger from storing
//! such an output. The step journal was still open: a `DispatchObserved` fact is
//! exempt from the liveness check of the store journal on purpose (a late receipt
//! keeps the accounting truth), and its payload is the whole `ModelResponse`, so
//! the model's output was stored in plaintext, in the object and in the idempotency
//! cache, whenever the response was settled while the source was live (or replayed
//! from that settlement) and the fact landed after the revocation. A cleanup that is
//! `Running` redacts the fact later through the edges it carries; a cleanup that is
//! `Complete` never looks again, so the output stayed for good.
//!
//! The store journal now decides, inside the transaction that would write such a
//! fact (a `DispatchObserved` or `ResponseObserved` fact of a model stage), whether a
//! run it depends on is revoked, by the tombstone of that run and nothing else: not
//! the watermark (another source's revocation moves it, and must not redact an
//! unrelated fact), and by kind. The only tombstone that spares the run is the one
//! written for an artifact that shares its id (that revokes the artifact, not the
//! run); every other tombstone redacts the fact: one written for a run, one of a
//! kind `begin_revoke` does not write, and one that does not decode, which fails
//! closed like the other tombstone gates (AG-032, AG-038, AG-043). If a run is
//! revoked, the fact is stored as the `rsia.redacted.v1` object the cleanup would
//! have made of it, its dependency edges are kept, and its idempotency row is
//! written redacted, so the same fact is refused if it is committed again. Nothing
//! else changes: the write is accepted (the receipt is kept), the step's own
//! liveness check after it still fails the step, the ledger is the ledger's, and a
//! fact over live sources is stored exactly as committed.
//!
//! Reading such a fact back is an expected state after a revocation, not a storage
//! failure: `lookup` names it (a `Conflict` with a fixed message, as the exploration,
//! curriculum and replay stores name a redacted record) instead of `Internal`, and a
//! body that is neither a stage fact nor a tombstone stays `Internal`.
//!
//! "No plaintext" is read as physical absence where the marker is written by nothing
//! but the stage fact: the raw bytes of the database file and its write-ahead log,
//! which cover every table and the pages written and then overwritten, and the rows
//! of the budget ledger tables. Where the ledger itself stored the output while the
//! source was live (the broker variant) it is read through the store's own reads: the
//! body of an object of any kind, and the idempotency row of every fact the journal
//! was given. The controls (a fact over a live source) show that the scans do see a
//! stored output.
//!
//! Not covered (and not claimed): SQLite pages that held a plaintext written before
//! the revocation (the free pages and the write-ahead log of an output the broker
//! stored while the source was live), facts without a run dependency, the facts a
//! revoked source's other writers leave (the liveness check refuses them, as
//! before), and the scores-only facts of the development runner, which hold no
//! model output and are left as they are.
//!
//! Fixtures are copied from `tests/exploration_trust_v42.rs`,
//! `tests/late_response_revoked_v42.rs` and `tests/optimization.rs`; the originals
//! are untouched, but for one assertion of `tests/optimization.rs`
//! (`exercise_late_revocation`: its hand-written tombstone is the unreadable one
//! above, so the late `DispatchObserved` it checks is now the redacted object).

use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepOutcome,
    OptimizationStepRequest, PairedTaskResult, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal, SuggestionOutcome, run_optimization_step,
};
use evo_storage::Store;
use evo_storage::budget::{BudgetCallState, RootBudgetAuthorization};
use evo_storage::lifecycle::{
    CleanupState, CleanupStatus, REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone, read_control_plane_facts,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const NAMESPACE: &str = "n";
const SCOPE: &str = "scope-1";
const ROOT: &str = "root-1";
/// The runs of the step's source selection (sorted, unique); `RUNS[0]` is the one the
/// tests revoke.
const RUNS: [&str; 2] = ["run-failure", "run-success"];
/// A trusted run that is in no selection.
const OTHER_RUN: &str = "run-other";
const REDACTED: &str = "rsia.redacted.v1";
/// What the model answers: plaintext that a revocation must not leave in the database.
const MARKER: &str = "MODEL-OUTPUT-MARKER-late-stage-fact-5b3e90";
/// The `objects.kind` values a stage fact or a source can be stored under.
const OBJECT_KINDS: [&str; 11] = [
    "artifact",
    "run",
    "tombstone",
    "job",
    "release",
    "candidate",
    "pointer",
    "receipt",
    "evaluation",
    "budget",
    "reservation",
];

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn admin() -> Context {
    Context::new(NAMESPACE, "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NAMESPACE, "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new(NAMESPACE, "trusted-broker", Role::Host).unwrap()
}

// ---------------------------------------------------------------------------
// The trusted source closure and the step request (exploration_trust_v42)
// ---------------------------------------------------------------------------

fn trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

fn parent_skill() -> SkillSnapshot {
    SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    }
}

/// The three trusted runs, the selection over `RUNS`, one watermark bump (so the
/// store's watermark is the 1 the requests carry) and, when asked, the root budget
/// the broker bills.
async fn seeded_store(with_root: bool) -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("late-stage-fact.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        (RUNS[0], "family-a", TraceOutcome::TaskFailure),
        (RUNS[1], "family-b", TraceOutcome::Success),
        (OTHER_RUN, "family-c", TraceOutcome::Success),
    ] {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(&store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        &store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "initial").await.unwrap();
    session.commit().await.unwrap();
    if with_root {
        store
            .authorize_root_budget(
                &admin(),
                &RootBudgetAuthorization {
                    root_budget_id: ROOT.into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "price-v1".into(),
                    payment_subject: "payer-1".into(),
                    authorization_receipt_digest: d("admin-authorization"),
                    per_call_cap_micros: 20,
                    total_limit_micros: 100,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
    }
    (dir, path, store)
}

struct Parent {
    skill: SkillSnapshot,
    edit_context: TrustedEditContext,
}

/// Everything an `OptimizationStepRequest` borrows besides its parent.
struct Fixture {
    evidence: EvidenceSet,
    source_selection: SourceSelection,
    bindings: Vec<TrustedSourceBinding>,
    profile: Profile,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
    allowed: Vec<EvidenceRef>,
    manifest: DevelopmentManifest,
}

impl Fixture {
    fn new() -> Self {
        let member = |id: &str| EvidenceMember {
            source_id: id.into(),
            content_digest: hash(id.as_bytes()),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        };
        let binding = |id: &str, family: &str| TrustedSourceBinding {
            source_id: id.into(),
            source_digest: hash(id.as_bytes()),
            parent_family: family.into(),
        };
        Self {
            evidence: EvidenceSet::build(
                "evidence",
                vec![member(RUNS[0]), member(RUNS[1])],
                SourceCoverage {
                    discovery_exhausted: true,
                    files_known: true,
                    parsed_ok: 2,
                    bytes_read: 22,
                    ..SourceCoverage::default()
                },
                ["family-a".into(), "family-b".into()].into_iter().collect(),
            )
            .unwrap(),
            source_selection: SourceSelection {
                roots: vec![],
                run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            },
            bindings: vec![binding(RUNS[0], "family-a"), binding(RUNS[1], "family-b")],
            profile: Profile {
                id: "profile".into(),
                evolution_enabled: true,
                parent_digest: d("approved-parent"),
                baseline_digest: d("baseline"),
            },
            baseline: parent_skill(),
            parent_strategy: Strategy::default(),
            baseline_strategy: Strategy::default(),
            improver: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
            allowed: RUNS
                .iter()
                .map(|run| EvidenceRef {
                    id: (*run).into(),
                    digest: hash(run.as_bytes()),
                })
                .collect(),
            manifest: DevelopmentManifest::build(
                "manifest",
                ["task-a", "task-b"]
                    .iter()
                    .map(|task| DevelopmentTask {
                        id: (*task).into(),
                        parent_family: format!("family-{task}"),
                        input_digest: d(task),
                    })
                    .collect(),
            )
            .unwrap(),
        }
    }

    fn parent(&self) -> Parent {
        let skill = parent_skill();
        let edit_context = TrustedEditContext::new(
            NAMESPACE,
            "profile",
            "skill",
            "v1",
            d("approved-parent"),
            d("baseline"),
            &skill,
            self.allowed.clone(),
        )
        .unwrap();
        Parent {
            skill,
            edit_context,
        }
    }

    fn request<'a>(
        &'a self,
        parent: &'a Parent,
        episode: &str,
        tag: &str,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.source_selection,
            source_bindings: &self.bindings,
            traces: vec![
                trace(RUNS[0], "family-a", TraceOutcome::TaskFailure),
                trace(RUNS[1], "family-b", TraceOutcome::Success),
            ],
            model_context: ModelRequestContext {
                request_id: format!("optimizer-{tag}"),
                namespace: NAMESPACE.into(),
                purpose: Purpose::Development,
                stage: ModelStage::ReflectFailure,
                episode_id: episode.into(),
                step: 1,
                attempt: 1,
                parent_skill_digest: skill_snapshot_digest(&parent.skill).unwrap(),
                bundle_digest: d("parent-bundle"),
                source_closure: self.allowed.clone(),
                model_digest: d("model"),
                tools_digest: d("tools"),
                rules_digest: d("rules"),
                sampling_digest: d("sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &parent.skill,
            edit_context: &parent.edit_context,
            edit_batch_template: SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: NAMESPACE.into(),
                profile_id: "profile".into(),
                skill_id: "skill".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(&parent.skill).unwrap(),
                approved_parent_digest: d("approved-parent"),
                safe_baseline_digest: d("baseline"),
                evidence: EvidenceClosure {
                    support: vec![],
                    counterexamples: vec![],
                    dependencies: vec![],
                },
                edits: vec![],
            },
            protected_ranges: &[],
            bundle_context: BundleCompileContext {
                profile: &self.profile,
                baseline: &self.baseline,
                parent_strategy: &self.parent_strategy,
                baseline_strategy: &self.baseline_strategy,
                improver_patch: &self.improver,
                caps: &self.caps,
                revoked: &self.revoked,
            },
            development_request: DevelopmentRunRequest {
                request_id: format!("dev-{tag}"),
                namespace: NAMESPACE.into(),
                purpose: Purpose::Development,
                episode_id: episode.into(),
                step: 1,
                attempt: 1,
                manifest: self.manifest.clone(),
                parent_bundle_digest: d("parent-bundle"),
                candidate_bundle_digest: d(&format!("candidate-{tag}")),
                environment_digest: d("environment"),
                grader_digest: d("grader"),
                rules_digest: d("rules"),
                tools_digest: d("tools"),
                revoke_watermark: 1,
                idempotency_key: format!("dev-idempotency-{tag}"),
            },
            allow_rank_call: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The ports: the model that answers with the marker, what happens to the source
// while its call is in flight, the development runner, and a journal that
// remembers what it was given
// ---------------------------------------------------------------------------

/// The suggestions a reflection model returns for `request`. The marker is in the
/// text of the hypothesis, so it is in the `output` of the response and in every
/// object that stores the response.
fn suggestions_output(request: &ModelRequest) -> Result<String> {
    let parent: SkillSnapshot = serde_json::from_str(
        &request
            .input
            .iter()
            .find(|part| part.label == "parent-skill")
            .ok_or_else(|| Error::Invalid("fixture parent input missing".into()))?
            .content,
    )
    .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
    let source = request.source_closure[0].clone();
    let (id, batch_id, field, start) = match request.stage {
        ModelStage::ReflectFailure => (
            "repair-content",
            "reflection-failure",
            SkillTextField::Content,
            parent.content.len(),
        ),
        ModelStage::ReflectSuccess => (
            "preserve-applicability",
            "reflection-success",
            SkillTextField::Applicability,
            parent.applicability.len(),
        ),
        _ => return Err(Error::Invalid("unexpected fixture stage".into())),
    };
    let suggestion = EditSuggestion {
        id: id.into(),
        hypothesis: format!("bounded deterministic fixture repair {MARKER}"),
        batch_ids: vec![batch_id.into()],
        support: vec![source.clone()],
        counterexamples: (request.stage == ModelStage::ReflectSuccess)
            .then_some(source.clone())
            .into_iter()
            .collect(),
        dependencies: vec![source],
        edit: SkillTextEdit {
            field,
            start,
            end: start,
            expected_text_digest: hash(b""),
            exact_anchor: None,
            operation: TextEditOperation::Insert {
                text: " [repair]".into(),
            },
        },
    };
    Ok(serde_json::to_string(&vec![suggestion]).unwrap())
}

/// A model that answers every request with the marker and counts its calls.
#[derive(Default)]
struct MarkerModel(AtomicUsize);

impl MarkerModel {
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelPort for MarkerModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let output = suggestions_output(&request)?;
        Ok(ModelResponse::Completed {
            request_id: request.request_id.clone(),
            response_id: format!("response-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            input_digest: request.input_digest.clone(),
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: request.request_id.clone(),
                dispatch_id: format!("dispatch-{}", request.request_id),
                root_budget_id: ROOT.into(),
                provider_request_id: format!("provider-{}", request.request_id),
                usage_record_id: format!("usage-{}", request.request_id),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

/// A transport for the persistent broker: the marker, at a real cost of 10 micros.
struct MarkerTransport;

#[async_trait]
impl ModelTransport for MarkerTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            output: suggestions_output(request)?,
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
    }
}

/// Revokes the run `id` and runs its cleanup for `pages` steps of one edge each, or
/// to `Complete` when `pages` is `None`. Returns the job.
async fn revoke(store: &Store, id: &str, pages: Option<u32>) -> Result<CleanupStatus> {
    let mut status =
        LifecycleCoordinator::revoke_source(&admin(), store, id, "privacy", 12).await?;
    let mut now = 13;
    while pages.is_none_or(|pages| now < 13 + i64::from(pages)) {
        if status.state == CleanupState::Complete {
            break;
        }
        assert!(now < 400, "the cleanup did not finish: {status:?}");
        status =
            LifecycleCoordinator::continue_cleanup(&admin(), store, &status.job_id, 1, now).await?;
        now += 1;
    }
    Ok(status)
}

/// Runs the cleanup job of `status` to `Complete`, one edge at a time.
async fn finish_cleanup(store: &Store, mut status: CleanupStatus) -> CleanupStatus {
    for now in 400..800 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(&admin(), store, &status.job_id, 1, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None, "{status:?}");
    status
}

/// What happens to the sources while the first model call of a step is in flight,
/// after the model (or the broker) has answered and before the answer is journaled.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    Nothing,
    /// `RUNS[0]` is revoked and its cleanup has run for one edge only.
    RevokeAndStartCleanup,
    /// `RUNS[0]` is revoked and its cleanup has run to `Complete`.
    RevokeAndClean,
    /// A run that is in no selection is revoked: the watermark moves, no source of
    /// the step is revoked.
    RevokeUnrelatedRun,
    /// The tombstone of an artifact that shares the id of `RUNS[0]` is written, and
    /// the watermark moves as the revocation of an artifact moves it.
    RevokeArtifactWithTheSameId,
    /// `RUNS[0]` is revoked by a tombstone `begin_revoke` did not write (the
    /// hand-written `{"revoked":true}` of `tests/optimization.rs`), and the watermark
    /// moves. No cleanup job exists for it.
    RevokeWithAnUnreadableTombstone,
}

impl InFlight {
    async fn apply(self, store: &Store) -> Result<()> {
        match self {
            Self::Nothing => {}
            Self::RevokeAndStartCleanup => {
                revoke(store, RUNS[0], Some(1)).await?;
            }
            Self::RevokeAndClean => {
                revoke(store, RUNS[0], None).await?;
            }
            Self::RevokeUnrelatedRun => {
                revoke(store, OTHER_RUN, Some(0)).await?;
            }
            Self::RevokeArtifactWithTheSameId => {
                let mut session = store.session().await?;
                session
                    .put(
                        &admin(),
                        "tombstone",
                        RUNS[0],
                        "admin",
                        &artifact_tombstone(RUNS[0]),
                    )
                    .await?;
                session
                    .bump_watermark(&admin(), &d("artifact-revoked"))
                    .await?;
                session.commit().await?;
            }
            Self::RevokeWithAnUnreadableTombstone => {
                let mut session = store.session().await?;
                session
                    .put(
                        &admin(),
                        "tombstone",
                        RUNS[0],
                        "admin",
                        &json!({"revoked": true}),
                    )
                    .await?;
                session
                    .bump_watermark(&admin(), &d("unreadable-revoked"))
                    .await?;
                session.commit().await?;
            }
        }
        Ok(())
    }
}

/// What `begin_revoke` writes for an E16 import-source artifact with the id `id`.
fn artifact_tombstone(id: &str) -> RevokeTombstone {
    RevokeTombstone {
        id: id.into(),
        schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
        source_kind: "artifact".into(),
        reason: "privacy".into(),
        watermark_seq: 2,
        watermark_digest: d("artifact-revoked"),
        created_at: 12,
    }
}

/// The port of a step: answers through `inner` (a model, or the broker that bills
/// it), then, on its first call only, does `during` to the sources before it hands
/// the answer back. The answer was settled while the sources were live and is
/// journaled after the revocation.
struct InFlightModel<M> {
    store: Store,
    during: InFlight,
    inner: M,
    requests: Mutex<Vec<String>>,
}

impl<M> InFlightModel<M> {
    fn new(store: &Store, during: InFlight, inner: M) -> Self {
        Self {
            store: store.clone(),
            during,
            inner,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl<M: ModelPort> ModelPort for InFlightModel<M> {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.request_id.clone());
            requests.len() == 1
        };
        let response = self.inner.dispatch(request).await?;
        if first {
            self.during.apply(&self.store).await?;
        }
        Ok(response)
    }
}

/// An improving report that is honest about being a fixture. The step never reaches
/// it when its source is revoked; the controls do.
struct FixtureRunner;

#[async_trait]
impl DevRunner for FixtureRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        let results = request
            .manifest
            .tasks
            .iter()
            .map(|task| PairedTaskResult {
                task_id: task.id.clone(),
                parent_score_micros: 500_000,
                candidate_score_micros: 900_000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: format!("fixture-parent-{}", task.id),
                candidate_execution_id: format!("fixture-candidate-{}", task.id),
                grader_receipt_digest: d(&format!("fixture-grade-{}", task.id)),
            })
            .collect();
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results,
            execution_receipt_id: "fixture-development-execution".into(),
            usage_record_ids: vec!["fixture-development-usage".into()],
            provenance: DevelopmentExecutionProvenance::Fixture,
        })
    }
}

/// A store journal that remembers every fact it was given, so a test knows what was
/// written and can ask the store about exactly those facts.
struct RecordingJournal {
    inner: StoreOptimizationJournal,
    given: Mutex<Vec<StageFact>>,
}

impl RecordingJournal {
    fn new(store: &Store) -> Self {
        Self {
            inner: StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap(),
            given: Mutex::new(Vec::new()),
        }
    }

    fn given(&self) -> Vec<StageFact> {
        self.given.lock().unwrap().clone()
    }

    /// The model-call observations the step committed (not the runner's).
    fn model_observations(&self) -> Vec<StageFact> {
        self.given()
            .into_iter()
            .filter(|fact| {
                fact.kind == StageFactKind::DispatchObserved
                    && fact.stage != OptimizationJournalStage::Development
            })
            .collect()
    }
}

#[async_trait]
impl OptimizationJournal for RecordingJournal {
    async fn commit(&self, fact: StageFact) -> Result<()> {
        self.given.lock().unwrap().push(fact.clone());
        self.inner.commit(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(artifact_id).await
    }

    async fn claim(&self, fact: StageFact) -> Result<bool> {
        self.given.lock().unwrap().push(fact.clone());
        self.inner.claim(fact).await
    }

    async fn verify_sources(
        &self,
        selection: &SourceSelection,
        evidence: &EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> Result<()> {
        self.inner
            .verify_sources(selection, evidence, bindings, traces, watermark)
            .await
    }

    async fn check_live(&self, source_ids: &[String], watermark: u64) -> Result<()> {
        self.inner.check_live(source_ids, watermark).await
    }
}

// ---------------------------------------------------------------------------
// Reading the database
// ---------------------------------------------------------------------------

/// The stored body of one object, `None` when there is none.
async fn raw(store: &Store, kind: &str, id: &str) -> Option<Value> {
    let mut session = store.session().await.unwrap();
    let value = session.get::<Value>(&worker(), kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

/// Where `needle` can be read physically: in a row of a budget ledger table, or in
/// the raw bytes of the database file or its write-ahead log (every table, and the
/// pages that were written and then overwritten). Empty when it is nowhere.
/// (`late_response_revoked_v42`'s `found_in`.)
async fn found_in(path: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let facts = read_control_plane_facts(path).await.unwrap();
    for (table, rows) in &facts.root_budget_tables {
        let hits = rows.iter().filter(|row| row.contains(needle)).count();
        if hits > 0 {
            found.push(format!("table {table}: {hits} row(s)"));
        }
    }
    for (suffix, label) in [("", "file: database"), ("-wal", "file: write-ahead log")] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        if let Ok(bytes) = std::fs::read(PathBuf::from(name))
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(label.to_string());
        }
    }
    found
}

/// Whether the database file or its write-ahead log holds `committed` as the very
/// text `serde_json::to_string` gives (what `Session::put` stores, byte for byte).
async fn holds_as_text(path: &Path, committed: &StageFact) -> bool {
    let text = serde_json::to_string(committed).unwrap();
    found_in(path, &text)
        .await
        .iter()
        .any(|place| place.starts_with("file"))
}

/// Where `needle` can be read through the store's own reads: in the body of an
/// object of any kind, or in the stored response of the idempotency row of one of
/// `given`, the facts the journal was handed. Empty when it is nowhere.
async fn readable_in(store: &Store, needle: &str, given: &[StageFact]) -> Vec<String> {
    let mut found = Vec::new();
    let mut session = store.session().await.unwrap();
    for kind in OBJECT_KINDS {
        let bodies: Vec<Value> = session.list(&worker(), kind).await.unwrap();
        let hits = bodies
            .iter()
            .filter(|body| body.to_string().contains(needle))
            .count();
        if hits > 0 {
            found.push(format!("objects of kind {kind}: {hits} body(ies)"));
        }
    }
    for fact in given {
        if let Ok(Some(cached)) = session
            .cached::<StageFact, _>(&worker(), "optimization.stage", &fact.artifact_id, fact)
            .await
            && serde_json::to_string(&cached).unwrap().contains(needle)
        {
            found.push(format!("idempotency row of {}", fact.artifact_id));
        }
    }
    session.commit().await.unwrap();
    found
}

/// The state of the idempotency row of `fact`.
#[derive(Debug, PartialEq, Eq)]
enum CacheRow {
    Absent,
    Live,
    /// Present, with the digest of this fact, and redacted: a replay is refused.
    Redacted,
}

async fn cache_row(store: &Store, fact: &StageFact) -> CacheRow {
    let mut session = store.session().await.unwrap();
    let row = session
        .cached::<StageFact, _>(&worker(), "optimization.stage", &fact.artifact_id, fact)
        .await;
    session.commit().await.unwrap();
    match row {
        Ok(None) => CacheRow::Absent,
        Ok(Some(_)) => CacheRow::Live,
        Err(Error::Conflict(message)) if message.contains("subject was deleted") => {
            CacheRow::Redacted
        }
        Err(other) => panic!(
            "the idempotency row of {} is unreadable: {other:?}",
            fact.artifact_id
        ),
    }
}

/// Overwrites the stored body of the object `id` (an artifact), the way a store
/// holding another record would.
async fn put_raw(store: &Store, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(&worker(), "artifact", id, "worker", body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// What a read says of a stage fact that was redacted because its source was
/// revoked, word for word.
fn redacted_message(id: &str) -> String {
    format!("optimization stage fact {id} was redacted because its source was revoked")
}

/// `result` must be the named `Conflict` of a redacted stage fact `id`: not
/// `Internal`, not another `Conflict`, not a fact.
fn assert_names_redaction<T: std::fmt::Debug>(result: Result<T>, id: &str, what: &str) {
    match result {
        Err(Error::Conflict(message)) => assert_eq!(message, redacted_message(id), "{what}"),
        other => panic!("{what}: expected the named Conflict of {id}, got {other:?}"),
    }
}

/// Every object of the namespace that is a stage fact, in plaintext or redacted.
async fn stage_fact_bodies(store: &Store) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let bodies: Vec<Value> = session.list(&worker(), "artifact").await.unwrap();
    session.commit().await.unwrap();
    bodies
        .into_iter()
        .filter(|body| {
            body["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA
                || (body["schema_version"] == REDACTED
                    && body["original_schema"] == OPTIMIZATION_STAGE_FACT_SCHEMA)
        })
        .collect()
}

async fn plaintext_stage_facts(store: &Store) -> Vec<String> {
    stage_fact_bodies(store)
        .await
        .into_iter()
        .filter(|body| body["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA)
        .map(|body| body["artifact_id"].as_str().unwrap_or("?").to_owned())
        .collect()
}

/// What is wrong with the way `original`, a fact that landed after its source was
/// revoked, is stored: empty when it is the `rsia.redacted.v1` object of the fact
/// (state, kind, schema and digest of what it replaces, the receipt the books name),
/// holds none of its payload, keeps its edges, and its idempotency row is redacted.
async fn defects_of_redaction(store: &Store, original: &StageFact) -> Vec<String> {
    let mut defects = Vec::new();
    let Some(body) = raw(store, "artifact", &original.artifact_id).await else {
        return vec![format!("{} was not stored at all", original.artifact_id)];
    };
    if body["schema_version"] != REDACTED {
        defects.push(format!(
            "{} is stored as {}, not redacted",
            original.artifact_id, body["schema_version"]
        ));
        return defects;
    }
    let expectations = [
        ("id", json!(original.artifact_id)),
        ("state", json!("source_revoked")),
        ("original_kind", json!("artifact")),
        ("original_schema", json!(OPTIMIZATION_STAGE_FACT_SCHEMA)),
        (
            "original_digest",
            json!(hash(serde_json::to_string(original).unwrap().as_bytes())),
        ),
    ];
    for (field, expected) in expectations {
        if body[field] != expected {
            defects.push(format!("{field} is {}, not {expected}", body[field]));
        }
    }
    if body.get("payload").is_some() || body.get("dependencies").is_some() {
        defects.push("the redaction still holds the payload or the dependencies".into());
    }
    let metadata = &body["metadata"];
    if metadata["kind"] != json!(original.kind) {
        defects.push(format!("metadata kind is {}", metadata["kind"]));
    }
    // A model response keeps the ids of its receipt (the books and the audit name
    // them) and nothing of its output.
    if let Some(receipt) = original.payload.get("execution_receipt") {
        for id in [
            "call_id",
            "dispatch_id",
            "root_budget_id",
            "provider_request_id",
            "usage_record_id",
        ] {
            if metadata["historical_facts"]["execution_receipt"][id] != receipt[id] {
                defects.push(format!("the receipt's {id} was not kept"));
            }
        }
    }
    for dependency in original.dependencies.iter().filter(|d| d.kind == "run") {
        let mut session = store.session().await.unwrap();
        let edges = session
            .dependents(&worker(), "run", &dependency.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        if !edges.contains(&("artifact".to_owned(), original.artifact_id.clone())) {
            defects.push(format!("no edge to run {}", dependency.id));
        }
    }
    let row = cache_row(store, original).await;
    if row != CacheRow::Redacted {
        defects.push(format!("the idempotency row is {row:?}, not redacted"));
    }
    defects
}

/// Everything that is wrong with how the late observation `observation` was stored
/// and with what the database still holds of the model's output: every place the
/// output can be read (physically when `path` is given, and through the store's own
/// reads) and every defect of the redaction. Empty when the fact is the redacted
/// object, with its edges and a redacted idempotency row, and the output is nowhere.
async fn defects_of_late_storage(
    path: Option<&Path>,
    store: &Store,
    observation: &StageFact,
    given: &[StageFact],
) -> Vec<String> {
    let mut defects = Vec::new();
    if let Some(path) = path {
        for place in found_in(path, MARKER).await {
            defects.push(format!(
                "the model output is physically in the database: {place}"
            ));
        }
    }
    for place in readable_in(store, MARKER, given).await {
        defects.push(format!("the model output is readable: {place}"));
    }
    defects.extend(defects_of_redaction(store, observation).await);
    defects
}

// ---------------------------------------------------------------------------
// Facts for the journal-level cases
// ---------------------------------------------------------------------------

fn dep(kind: &str, id: &str) -> StageDependency {
    StageDependency {
        kind: kind.into(),
        id: id.into(),
    }
}

fn receipt_of(request_id: &str) -> ModelExecutionReceipt {
    ModelExecutionReceipt {
        call_id: format!("call-{request_id}"),
        dispatch_id: format!("dispatch-{request_id}"),
        root_budget_id: ROOT.into(),
        provider_request_id: format!("provider-{request_id}"),
        usage_record_id: format!("usage-{request_id}"),
        provenance: ModelExecutionProvenance::Fixture,
    }
}

/// The model's text of `request_id`: unique, and carrying the marker.
fn text_of(request_id: &str) -> String {
    format!("{MARKER}-{request_id}")
}

/// The three shapes of the whole `ModelResponse` that a dispatch observation holds,
/// each with the model's or the provider's text in it, and the outcome a step builds
/// from a completed one (what a response observation holds).
fn payloads_of(request_id: &str) -> Vec<(&'static str, Value)> {
    let receipt = receipt_of(request_id);
    let output = text_of(request_id);
    let completed = ModelResponse::Completed {
        request_id: request_id.into(),
        response_id: format!("response-{request_id}"),
        actual_model_digest: d("model"),
        input_digest: d(&format!("input-{request_id}")),
        output_digest: hash(output.as_bytes()),
        output: output.clone(),
        execution_receipt: receipt.clone(),
    };
    let rejected = ModelResponse::Rejected {
        request_id: request_id.into(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: format!("{output} (the provider's own words)"),
        dispatch: RejectedDispatch::Dispatched {
            receipt: receipt.clone(),
        },
    };
    let uncertain = ModelResponse::Uncertain {
        request_id: request_id.into(),
        dispatch_id: receipt.dispatch_id.clone(),
        root_budget_id: receipt.root_budget_id.clone(),
        provider_request_id: Some(receipt.provider_request_id.clone()),
        usage_record_id: Some(receipt.usage_record_id.clone()),
    };
    let outcome = SuggestionOutcome::Suggestions {
        items: vec![EditSuggestion {
            id: "repair-content".into(),
            hypothesis: output.clone(),
            batch_ids: vec!["reflection-failure".into()],
            support: vec![],
            counterexamples: vec![],
            dependencies: vec![],
            edit: SkillTextEdit {
                field: SkillTextField::Content,
                start: 0,
                end: 0,
                expected_text_digest: hash(b""),
                exact_anchor: None,
                operation: TextEditOperation::Insert {
                    text: output.clone(),
                },
            },
        }],
        response_id: format!("response-{request_id}"),
        output_digest: hash(output.as_bytes()),
        output,
        receipt,
    };
    vec![
        ("completed", serde_json::to_value(completed).unwrap()),
        ("rejected", serde_json::to_value(rejected).unwrap()),
        ("uncertain", serde_json::to_value(uncertain).unwrap()),
        ("suggestions", serde_json::to_value(outcome).unwrap()),
    ]
}

/// An observation as the step builds it: `payload`, the run dependencies and the
/// watermark of the step.
fn observation_of(
    payload: Value,
    request_id: &str,
    kind: StageFactKind,
    stage: OptimizationJournalStage,
    runs: &[&str],
    watermark: Option<u64>,
) -> StageFact {
    let mut dependencies: Vec<_> = runs.iter().map(|run| dep("run", run)).collect();
    dependencies.extend(watermark.map(|seq| dep("revoke_watermark", &seq.to_string())));
    dependencies.push(dep("artifact", "optstage-step-prepared"));
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: NAMESPACE.into(),
        episode_id: "episode-journal".into(),
        step: 1,
        attempt: 1,
        stage,
        kind,
        request_id: request_id.into(),
        input_digest: d(&format!("input-{request_id}")),
        output_digest: None,
        dependencies,
        payload,
    }
    .seal()
    .unwrap()
}

/// A model-call observation holding a completed `ModelResponse`.
fn observation(
    request_id: &str,
    kind: StageFactKind,
    stage: OptimizationJournalStage,
    runs: &[&str],
    watermark: Option<u64>,
) -> StageFact {
    let (_, payload) = payloads_of(request_id).remove(0);
    observation_of(payload, request_id, kind, stage, runs, watermark)
}

/// The dispatch observation of a reflection call over `runs` at the step's watermark.
fn dispatch_observed(request_id: &str, runs: &[&str]) -> StageFact {
    observation(
        request_id,
        StageFactKind::DispatchObserved,
        OptimizationJournalStage::ReflectFailure,
        runs,
        Some(1),
    )
}

fn journal(store: &Store) -> StoreOptimizationJournal {
    StoreOptimizationJournal::new(store.clone(), worker(), "worker").unwrap()
}

/// What is wrong with a fact that is stored exactly as it was committed: empty when
/// the object is the fact and its idempotency row is live and holds it.
async fn defects_of_plain_storage(store: &Store, committed: &StageFact) -> Vec<String> {
    let mut defects = Vec::new();
    match raw(store, "artifact", &committed.artifact_id).await {
        None => defects.push(format!("{} was not stored", committed.artifact_id)),
        Some(body) => match serde_json::from_value::<StageFact>(body) {
            Err(error) => defects.push(format!("the stored object is not the fact: {error}")),
            Ok(stored) => {
                if fingerprint(&stored).unwrap() != fingerprint(committed).unwrap() {
                    defects.push("the stored fact differs from the committed one".into());
                }
            }
        },
    }
    if cache_row(store, committed).await != CacheRow::Live {
        defects.push("the idempotency row is not live".into());
    }
    defects
}

// ===========================================================================
// 1. The late observation, at the journal
// ===========================================================================

/// `RUNS[0]` is revoked, the cleanup has run as far as `cleanup` says, and only then
/// does a dispatch observation over the sources land (a response settled while they
/// were live, or the replay of that settlement). It is accepted (the receipt is
/// kept) and stored redacted, with its edges and a redacted idempotency row, and the
/// output is nowhere. A replay of the same fact is refused and writes nothing.
async fn a_late_observation_leaves_no_output(cleanup: Option<u32>) {
    let what = format!("cleanup pages: {cleanup:?}");
    let (_dir, path, store) = seeded_store(false).await;
    let journal = journal(&store);
    let status = revoke(&store, RUNS[0], cleanup).await.unwrap();
    assert_eq!(
        status.state,
        if cleanup.is_some() {
            CleanupState::Running
        } else {
            CleanupState::Complete
        },
        "{what}: {status:?}"
    );

    let late = dispatch_observed("request-late", &RUNS);
    let given = std::slice::from_ref(&late);
    journal.commit(late.clone()).await.unwrap();
    let defects = defects_of_late_storage(Some(&path), &store, &late, given).await;
    assert!(
        defects.is_empty(),
        "{what}: the late observation is not stored redacted:\n{defects:#?}"
    );

    // The same fact again is a replay of a redacted subject: refused, nothing written.
    let again = journal.commit(late.clone()).await;
    assert!(
        matches!(&again, Err(Error::Conflict(message)) if message.contains("subject was deleted")),
        "{what}: {again:?}"
    );
    assert_names_redaction(
        journal.lookup(&late.artifact_id).await,
        &late.artifact_id,
        &format!("{what}: lookup"),
    );
    let defects = defects_of_late_storage(Some(&path), &store, &late, given).await;
    assert!(
        defects.is_empty(),
        "{what}, after the replay:\n{defects:#?}"
    );

    // A running cleanup that reaches the fact afterwards leaves it as it is, and the
    // job still completes.
    if cleanup.is_some() {
        finish_cleanup(&store, status).await;
        let defects = defects_of_late_storage(Some(&path), &store, &late, given).await;
        assert!(
            defects.is_empty(),
            "{what}, after the cleanup completed:\n{defects:#?}"
        );
    }
}

#[tokio::test]
async fn an_observation_that_lands_after_the_cleanup_completed_is_stored_redacted() {
    a_late_observation_leaves_no_output(None).await;
}

#[tokio::test]
async fn an_observation_that_lands_while_the_cleanup_runs_is_stored_redacted() {
    a_late_observation_leaves_no_output(Some(1)).await;
}

/// Every stage a model call is journaled under is covered, for both kinds of
/// observation (the optimizer's reflection and ranking calls, and a consolidation's).
#[tokio::test]
async fn an_observation_of_every_model_stage_is_stored_redacted() {
    let (_dir, path, store) = seeded_store(false).await;
    let journal = journal(&store);
    revoke(&store, RUNS[0], None).await.unwrap();
    for stage in [
        OptimizationJournalStage::ReflectFailure,
        OptimizationJournalStage::ReflectSuccess,
        OptimizationJournalStage::Merge,
        OptimizationJournalStage::Rank,
        OptimizationJournalStage::Consolidate,
    ] {
        for kind in [
            StageFactKind::DispatchObserved,
            StageFactKind::ResponseObserved,
        ] {
            let fact = observation(
                &format!("request-{stage:?}-{kind:?}"),
                kind,
                stage,
                &RUNS,
                None,
            );
            journal.commit(fact.clone()).await.unwrap();
            let defects =
                defects_of_late_storage(Some(&path), &store, &fact, std::slice::from_ref(&fact))
                    .await;
            assert!(defects.is_empty(), "{stage:?} {kind:?}:\n{defects:#?}");
        }
    }
}

/// A response observation of a model stage is a model-output fact too. With the
/// watermark of its step it is still refused by the liveness check (nothing written);
/// without one nothing refuses it, and it is stored redacted.
#[tokio::test]
async fn a_response_observation_of_a_revoked_source_is_refused_or_stored_redacted() {
    let (_dir, path, store) = seeded_store(false).await;
    let journal = journal(&store);
    revoke(&store, RUNS[0], None).await.unwrap();

    let stamped = observation(
        "request-stamped",
        StageFactKind::ResponseObserved,
        OptimizationJournalStage::ReflectFailure,
        &RUNS,
        Some(1),
    );
    let refused = journal.commit(stamped.clone()).await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message.contains("watermark")),
        "{refused:?}"
    );
    assert!(
        raw(&store, "artifact", &stamped.artifact_id)
            .await
            .is_none()
    );
    assert_eq!(cache_row(&store, &stamped).await, CacheRow::Absent);

    let (_, outcome) = payloads_of("request-unstamped").remove(3);
    let unstamped = observation_of(
        outcome,
        "request-unstamped",
        StageFactKind::ResponseObserved,
        OptimizationJournalStage::ReflectFailure,
        &RUNS,
        None,
    );
    journal.commit(unstamped.clone()).await.unwrap();
    let defects = defects_of_late_storage(
        Some(&path),
        &store,
        &unstamped,
        std::slice::from_ref(&unstamped),
    )
    .await;
    assert!(
        defects.is_empty(),
        "the unstamped response observation is not stored redacted:\n{defects:#?}"
    );
}

/// The control: over live sources the observation is stored exactly as committed,
/// its idempotency row is live, and the scans see the output (so their silence in the
/// other cases means something).
#[tokio::test]
async fn an_observation_over_live_sources_is_stored_as_committed() {
    let (_dir, path, store) = seeded_store(false).await;
    let journal = journal(&store);
    let fact = dispatch_observed("request-live", &RUNS);
    journal.commit(fact.clone()).await.unwrap();
    let defects = defects_of_plain_storage(&store, &fact).await;
    assert!(defects.is_empty(), "{defects:#?}");
    let found = found_in(&path, MARKER).await;
    assert!(
        found.iter().any(|place| place.starts_with("file")),
        "the physical scan does not see a stored output: {found:?}"
    );
    let found = readable_in(&store, MARKER, std::slice::from_ref(&fact)).await;
    assert!(
        found.iter().any(|place| place.starts_with("objects"))
            && found.iter().any(|place| place.starts_with("idempotency")),
        "the logical scan does not see a stored output: {found:?}"
    );
    assert!(
        holds_as_text(&path, &fact).await,
        "the stored body is not the committed fact, byte for byte"
    );
    // The same fact again is the idempotent no-op it always was.
    journal.commit(fact.clone()).await.unwrap();
    assert!(defects_of_plain_storage(&store, &fact).await.is_empty());
}

/// A tombstone is keyed by the id of its source alone and records the kind it was
/// written for. An observation that depends on a run is spared only by the tombstone
/// of an artifact that shares the run's id (that is another object's revocation).
/// Every other tombstone redacts it: one written for a run, one of a kind
/// `begin_revoke` does not write, one that is not a tombstone `begin_revoke` writes
/// at all (the hand-written `{"revoked":true}` of the older fixtures, and a body of
/// no known shape). That is the fail-closed rule of the other tombstone gates.
#[tokio::test]
async fn only_the_tombstone_of_an_artifact_with_the_same_id_spares_a_run_observation() {
    let (_dir, path, store) = seeded_store(false).await;
    let journal = journal(&store);
    let cases = [
        ("artifact", false),
        ("run", true),
        ("blob", true),
        ("legacy", true),
        ("garbage", true),
    ];
    for (label, redacted) in cases {
        let id = format!("run-shared-{label}");
        let body = match label {
            "artifact" | "run" | "blob" => serde_json::to_value(RevokeTombstone {
                source_kind: label.into(),
                ..artifact_tombstone(&id)
            })
            .unwrap(),
            "legacy" => json!({"revoked": true}),
            _ => json!({"id": id, "garbage": true}),
        };
        let mut session = store.session().await.unwrap();
        session
            .put(&admin(), "tombstone", &id, "admin", &body)
            .await
            .unwrap();
        session.commit().await.unwrap();

        let fact = dispatch_observed(&format!("request-{label}"), &[RUNS[1], &id]);
        journal.commit(fact.clone()).await.unwrap();
        let defects = if redacted {
            defects_of_redaction(&store, &fact).await
        } else {
            defects_of_plain_storage(&store, &fact).await
        };
        assert!(
            defects.is_empty(),
            "a tombstone written as {label} ({body}):\n{defects:#?}"
        );
        let found = found_in(&path, &format!("{MARKER}-request-{label}")).await;
        assert_eq!(
            found.iter().any(|place| place.starts_with("file")),
            !redacted,
            "a tombstone written as {label}: {found:?}"
        );
    }
}

/// The decision is the tombstone, not the watermark: the revocation of a run the
/// observation does not depend on moves the watermark, and the observation is stored
/// as committed (every consumer still checks the watermark when it reads it).
#[tokio::test]
async fn the_revocation_of_an_unrelated_run_does_not_redact_an_observation() {
    let (_dir, _path, store) = seeded_store(false).await;
    let journal = journal(&store);
    revoke(&store, OTHER_RUN, None).await.unwrap();
    let fact = dispatch_observed("request-unrelated", &RUNS);
    journal.commit(fact.clone()).await.unwrap();
    let defects = defects_of_plain_storage(&store, &fact).await;
    assert!(defects.is_empty(), "{defects:#?}");
}

/// The scores-only observation of the development runner holds no model output and
/// is not handled, whatever its sources.
#[tokio::test]
async fn the_development_runners_scores_only_observation_is_left_as_it_is() {
    let (_dir, _path, store) = seeded_store(false).await;
    let journal = journal(&store);
    revoke(&store, RUNS[0], None).await.unwrap();
    let mut fact = observation(
        "request-development",
        StageFactKind::DispatchObserved,
        OptimizationJournalStage::Development,
        &RUNS,
        Some(1),
    );
    fact.payload = json!({"request_id": "request-development", "results": [
        {"task_id": "task-a", "parent_score_micros": 500_000, "candidate_score_micros": 900_000}],
        "provenance": "fixture"});
    let fact = fact.seal().unwrap();
    journal.commit(fact.clone()).await.unwrap();
    let defects = defects_of_plain_storage(&store, &fact).await;
    assert!(defects.is_empty(), "{defects:#?}");
}

/// The redaction made at write time is the object the cleanup makes of the same
/// fact: one construction, whichever comes first, for every shape a model
/// observation takes (the three `ModelResponse` shapes and the outcome of a step).
#[tokio::test]
async fn a_redaction_made_at_write_time_is_the_one_the_cleanup_makes() {
    for (shape, payload) in payloads_of("request-equal") {
        let kind = if shape == "suggestions" {
            StageFactKind::ResponseObserved
        } else {
            StageFactKind::DispatchObserved
        };
        let fact = observation_of(
            payload,
            "request-equal",
            kind,
            OptimizationJournalStage::ReflectFailure,
            &RUNS,
            None,
        );

        // Committed while live, then revoked and cleaned.
        let (_dir, _path, store) = seeded_store(false).await;
        journal(&store).commit(fact.clone()).await.unwrap();
        let status = revoke(&store, RUNS[0], Some(1)).await.unwrap();
        finish_cleanup(&store, status).await;
        let by_the_cleanup = raw(&store, "artifact", &fact.artifact_id).await.unwrap();
        assert_eq!(by_the_cleanup["schema_version"], REDACTED, "{shape}");
        assert_eq!(
            cache_row(&store, &fact).await,
            CacheRow::Redacted,
            "{shape}"
        );

        // Revoked and cleaned, then committed.
        let (_dir, path, store) = seeded_store(false).await;
        revoke(&store, RUNS[0], None).await.unwrap();
        journal(&store).commit(fact.clone()).await.unwrap();
        let at_write_time = raw(&store, "artifact", &fact.artifact_id).await.unwrap();

        assert_eq!(at_write_time, by_the_cleanup, "{shape}");
        let defects =
            defects_of_late_storage(Some(&path), &store, &fact, std::slice::from_ref(&fact)).await;
        assert!(defects.is_empty(), "{shape}:\n{defects:#?}");
        // The audit chain is intact and records the redacted write.
        assert!(store.verify_audit(&admin()).await.unwrap() > 0, "{shape}");
    }
}

// ===========================================================================
// 2. The late observation, through a step
// ===========================================================================

/// Everything a step over the revoked source leaves behind, checked in one place:
/// the step is no candidate, the model was called once and never again, the late
/// observation is stored redacted and the model's output is nowhere.
struct Ran {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    journal: RecordingJournal,
    fixture: Fixture,
    parent: Parent,
    result: Result<OptimizationStepOutcome>,
}

async fn run_step(during: InFlight, episode: &str) -> (Ran, Vec<String>) {
    let (dir, path, store) = seeded_store(false).await;
    let fixture = Fixture::new();
    let parent = fixture.parent();
    let journal = RecordingJournal::new(&store);
    let model = InFlightModel::new(&store, during, MarkerModel::default());
    let result = run_optimization_step(
        Some(&model),
        Some(&FixtureRunner),
        Some(&journal),
        fixture.request(&parent, episode, "late"),
    )
    .await;
    let requests = model.requests();
    (
        Ran {
            _dir: dir,
            path,
            store,
            journal,
            fixture,
            parent,
            result,
        },
        requests,
    )
}

impl Ran {
    /// The one model observation the step committed.
    fn the_observation(&self) -> StageFact {
        let mut observations = self.journal.model_observations();
        assert_eq!(observations.len(), 1, "{observations:#?}");
        observations.remove(0)
    }

    /// A replay of the step with a model that must not be called.
    async fn replay(&self, episode: &str) -> (Result<OptimizationStepOutcome>, usize) {
        let counting = MarkerModel::default();
        let journal = journal(&self.store);
        let result = run_optimization_step(
            Some(&counting),
            Some(&FixtureRunner),
            Some(&journal),
            self.fixture.request(&self.parent, episode, "late"),
        )
        .await;
        (result, counting.calls())
    }

    /// What is wrong with the storage of `observation`, the output and the redaction.
    async fn defects_of(&self, observation: &StageFact) -> Vec<String> {
        defects_of_late_storage(
            Some(&self.path),
            &self.store,
            observation,
            &self.journal.given(),
        )
        .await
    }
}

/// The response was settled while the source was live, the revocation and its whole
/// cleanup came before the step journaled it, and nothing will clean the fact
/// afterwards: it must be redacted when it is written.
#[tokio::test]
async fn a_response_that_lands_after_the_cleanup_completed_leaves_no_output() {
    let (late, requests) = run_step(InFlight::RevokeAndClean, "episode-a").await;
    assert_eq!(requests.len(), 1, "one model call");
    // The step ended on its own liveness check: no candidate, no outcome.
    assert!(
        matches!(&late.result, Err(Error::Conflict(_))),
        "{:?}",
        late.result
    );
    let observation = late.the_observation();
    assert!(observation.payload.to_string().contains(MARKER));
    let defects = late.defects_of(&observation).await;
    assert!(
        defects.is_empty(),
        "the late observation is not stored redacted and leaves the output:\n{defects:#?}"
    );
    // Nothing of the step is left in plaintext at Complete: not the late fact either.
    assert_eq!(
        plaintext_stage_facts(&late.store).await,
        Vec::<String>::new(),
        "stage facts still in plaintext"
    );

    // A replay finds the sources revoked before it can reach the model.
    let (replay, calls) = late.replay("episode-a").await;
    assert!(replay.is_err(), "{replay:?}");
    assert_eq!(calls, 0, "the replay dispatched again");
    let defects = late.defects_of(&observation).await;
    assert!(defects.is_empty(), "after the replay:\n{defects:#?}");
}

/// The same while the cleanup is still running: the fact is redacted when it is
/// written, not when the cleanup reaches it, and the job then completes.
#[tokio::test]
async fn a_response_that_lands_while_the_cleanup_runs_leaves_no_output() {
    let (late, requests) = run_step(InFlight::RevokeAndStartCleanup, "episode-b").await;
    assert_eq!(requests.len(), 1, "one model call");
    assert!(
        matches!(&late.result, Err(Error::Conflict(_))),
        "{:?}",
        late.result
    );
    let status = revoke(&late.store, RUNS[0], Some(0)).await.unwrap();
    assert_eq!(status.state, CleanupState::Running, "{status:?}");
    let observation = late.the_observation();
    let defects = late.defects_of(&observation).await;
    assert!(
        defects.is_empty(),
        "the late observation is not stored redacted while the cleanup runs:\n{defects:#?}"
    );

    let (replay, calls) = late.replay("episode-b").await;
    assert!(replay.is_err(), "{replay:?}");
    assert_eq!(calls, 0, "the replay dispatched again");

    // The cleanup goes on and completes; every stage fact of the step is redacted.
    finish_cleanup(&late.store, status).await;
    assert_eq!(
        plaintext_stage_facts(&late.store).await,
        Vec::<String>::new(),
        "stage facts still in plaintext at Complete"
    );
    let defects = late.defects_of(&observation).await;
    assert!(
        defects.is_empty(),
        "after the cleanup completed:\n{defects:#?}"
    );
}

/// The fixture of `tests/optimization.rs::exercise_late_revocation` at the level of
/// this file: the revocation in flight is a tombstone `begin_revoke` did not write
/// (it does not decode). Every other tombstone gate takes such a tombstone for a
/// revocation, and so does the journal: the late answer is stored redacted and is
/// nowhere in the database.
#[tokio::test]
async fn an_unreadable_tombstone_redacts_the_late_fact_of_a_step() {
    let (late, requests) = run_step(InFlight::RevokeWithAnUnreadableTombstone, "episode-h").await;
    assert_eq!(requests.len(), 1, "one model call");
    assert!(
        matches!(&late.result, Err(Error::Conflict(_))),
        "{:?}",
        late.result
    );
    let observation = late.the_observation();
    assert!(observation.payload.to_string().contains(MARKER));
    let defects = late.defects_of(&observation).await;
    assert!(
        defects.is_empty(),
        "the late observation is not stored redacted and leaves the output:\n{defects:#?}"
    );
    let (replay, calls) = late.replay("episode-h").await;
    assert!(replay.is_err(), "{replay:?}");
    assert_eq!(calls, 0, "the replay dispatched again");
    let defects = late.defects_of(&observation).await;
    assert!(defects.is_empty(), "after the replay:\n{defects:#?}");
}

/// The controls of the step: a revocation that is not one of the step's sources
/// (another run, or an artifact that only shares an id) fails the step on its own
/// liveness check as before, and the observation is stored exactly as committed.
#[tokio::test]
async fn a_revocation_that_is_not_one_of_the_steps_sources_redacts_nothing() {
    for (during, episode) in [
        (InFlight::RevokeUnrelatedRun, "episode-c"),
        (InFlight::RevokeArtifactWithTheSameId, "episode-d"),
    ] {
        let (late, requests) = run_step(during, episode).await;
        assert_eq!(requests.len(), 1, "{during:?}: one model call");
        assert!(
            matches!(&late.result, Err(Error::Conflict(_))),
            "{during:?}: {:?}",
            late.result
        );
        let observation = late.the_observation();
        let defects = defects_of_plain_storage(&late.store, &observation).await;
        assert!(defects.is_empty(), "{during:?}:\n{defects:#?}");
    }
}

/// The control of the whole step: over live sources it completes, and every fact it
/// committed is stored exactly as committed, with a live idempotency row.
#[tokio::test]
async fn a_step_over_live_sources_stores_every_fact_as_committed() {
    let (late, requests) = run_step(InFlight::Nothing, "episode-e").await;
    assert_eq!(requests.len(), 2, "two reflection calls");
    assert!(
        matches!(&late.result, Ok(OptimizationStepOutcome::Candidate { .. })),
        "{:?}",
        late.result
    );
    let given = late.journal.given();
    assert!(given.len() >= 8, "{} facts", given.len());
    let mut seen = BTreeSet::new();
    for fact in &given {
        if !seen.insert(fact.artifact_id.clone()) {
            continue;
        }
        let defects = defects_of_plain_storage(&late.store, fact).await;
        assert!(
            defects.is_empty(),
            "{:?} {:?}:\n{defects:#?}",
            fact.kind,
            fact.stage
        );
    }
    // The model's output is stored, as sent, where the step needs it.
    assert_eq!(late.journal.model_observations().len(), 2);
    for observation in late.journal.model_observations() {
        assert!(
            holds_as_text(&late.path, &observation).await,
            "the stored observation is not the committed fact, byte for byte"
        );
    }
    let found = found_in(&late.path, MARKER).await;
    assert!(
        found.iter().any(|place| place.starts_with("file")),
        "the scan does not see a live output: {found:?}"
    );
}

// ===========================================================================
// 3. Reading a redacted stage fact back
// ===========================================================================

/// A stage fact that the cleanup redacted, or that the journal stored redacted
/// because its source was already revoked, is an expected state after a revocation.
/// A read names it: a `Conflict` with a fixed message, as the exploration,
/// curriculum and replay stores name a redacted record, and not `Internal`.
#[tokio::test]
async fn a_redacted_stage_fact_is_named_on_lookup_not_reported_as_internal() {
    // Redacted by the cleanup: committed while its sources were live, then revoked
    // and cleaned.
    let (_dir, _path, store) = seeded_store(false).await;
    let by_the_cleanup = journal(&store);
    let live = dispatch_observed("request-by-the-cleanup", &RUNS);
    by_the_cleanup.commit(live.clone()).await.unwrap();
    assert_eq!(
        by_the_cleanup
            .lookup(&live.artifact_id)
            .await
            .unwrap()
            .map(|fact| fact.artifact_id),
        Some(live.artifact_id.clone()),
        "a live fact is served as it was"
    );
    let status = revoke(&store, RUNS[0], Some(1)).await.unwrap();
    finish_cleanup(&store, status).await;
    let body = raw(&store, "artifact", &live.artifact_id).await.unwrap();
    assert_eq!(body["schema_version"], REDACTED);
    assert_names_redaction(
        by_the_cleanup.lookup(&live.artifact_id).await,
        &live.artifact_id,
        "redacted by the cleanup: lookup",
    );
    assert_names_redaction(
        by_the_cleanup.reload(&live.artifact_id).await,
        &live.artifact_id,
        "redacted by the cleanup: reload",
    );

    // Redacted at write time: revoked and cleaned first, then committed.
    let (_dir, _path, store) = seeded_store(false).await;
    let at_write_time = journal(&store);
    revoke(&store, RUNS[0], None).await.unwrap();
    let late = dispatch_observed("request-at-write-time", &RUNS);
    at_write_time.commit(late.clone()).await.unwrap();
    assert_names_redaction(
        at_write_time.lookup(&late.artifact_id).await,
        &late.artifact_id,
        "redacted at write time: lookup",
    );
    assert_names_redaction(
        at_write_time.reload(&late.artifact_id).await,
        &late.artifact_id,
        "redacted at write time: reload",
    );
    // An id nothing was stored under is still absent, not named.
    assert!(
        at_write_time
            .lookup("optstage-never-written")
            .await
            .unwrap()
            .is_none()
    );
}

/// Only the tombstone is named. A body that claims to be a stage fact and does not
/// decode, a body of no known shape, and a body that only looks like a tombstone are
/// corruption, and stay `Internal`.
#[tokio::test]
async fn a_stage_fact_body_that_is_neither_a_fact_nor_a_tombstone_stays_internal() {
    let (_dir, _path, store) = seeded_store(false).await;
    let journal = journal(&store);
    for (id, body) in [
        (
            "optstage-corrupt-fact",
            json!({"schema_version": OPTIMIZATION_STAGE_FACT_SCHEMA,
                   "artifact_id": "optstage-corrupt-fact", "junk": true}),
        ),
        ("optstage-corrupt-shape", json!({"hello": "world"})),
        (
            "optstage-corrupt-lookalike",
            json!({"schema_version": "rsia.redacted.v2", "id": "optstage-corrupt-lookalike"}),
        ),
    ] {
        put_raw(&store, id, &body).await;
        let result = journal.lookup(id).await;
        assert!(
            matches!(result, Err(Error::Internal)),
            "{id}: expected Internal, got {result:?}"
        );
    }
}

/// The step reads its own stage facts back when it replays. When one of them is a
/// tombstone (cleaned in a donor store, so a genuine one), the replay ends with the
/// named `Conflict` through the journal of the step (`RecoveryJournal::lookup` hands
/// the read on), and nothing is dispatched.
#[tokio::test]
async fn a_replay_that_reads_a_redacted_stage_fact_ends_with_the_named_conflict() {
    // The donor ran the same step; its stage facts are cleaned.
    let (donor, _) = run_step(InFlight::Nothing, "episode-r1").await;
    let status = revoke(&donor.store, RUNS[0], None).await.unwrap();
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");

    // The store under test holds the live step; a clean replay returns its candidate
    // from the books, without a dispatch.
    let (live, _) = run_step(InFlight::Nothing, "episode-r1").await;
    assert!(
        matches!(&live.result, Ok(OptimizationStepOutcome::Candidate { .. })),
        "{:?}",
        live.result
    );
    let (control, calls) = live.replay("episode-r1").await;
    assert!(
        matches!(&control, Ok(OptimizationStepOutcome::Candidate { .. })),
        "{control:?}"
    );
    assert_eq!(calls, 0);

    for kind in [StageFactKind::StepPrepared, StageFactKind::StepCompleted] {
        let id = live
            .journal
            .given()
            .into_iter()
            .find(|fact| fact.kind == kind)
            .unwrap_or_else(|| panic!("the step committed no {kind:?}"))
            .artifact_id;
        let original = raw(&live.store, "artifact", &id).await.unwrap();
        let tombstone = raw(&donor.store, "artifact", &id).await.unwrap();
        assert_eq!(tombstone["schema_version"], REDACTED, "{kind:?}");
        put_raw(&live.store, &id, &tombstone).await;

        let (replay, calls) = live.replay("episode-r1").await;
        assert_names_redaction(replay, &id, &format!("{kind:?}"));
        assert_eq!(calls, 0, "{kind:?}: the replay dispatched");

        put_raw(&live.store, &id, &original).await;
    }
}

// ===========================================================================
// 4. The same, billed by the persistent broker: the cost stays on the books
// ===========================================================================

/// The broker settles the call while the source is live (the response is usable,
/// its cost is booked, the ledger stores the output), then the source is revoked and
/// only afterwards is the answer journaled. The ledger keeps the cost and the
/// dispatch (the cleanup redacts what the call holds), and the journaled observation
/// is redacted and names the same receipt.
async fn the_cost_stays_on_the_books(during: InFlight, episode: &str) {
    let what = format!("{during:?}");
    let (_dir, path, store) = seeded_store(true).await;
    let fixture = Fixture::new();
    let parent = fixture.parent();
    let journal = RecordingJournal::new(&store);
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        MarkerTransport,
        BrokerConfig {
            billing_scope: SCOPE.into(),
            actor: "trusted-broker".into(),
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(|| 10),
    )
    .unwrap();
    let model = InFlightModel::new(&store, during, broker);
    let result = run_optimization_step(
        Some(&model),
        Some(&FixtureRunner),
        Some(&journal),
        fixture.request(&parent, episode, "billed"),
    )
    .await;
    assert!(
        matches!(&result, Err(Error::Conflict(_))),
        "{what}: {result:?}"
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 1, "{what}: one model call");

    // The late observation is redacted at once, whatever the cleanup has done. (Only
    // the logical scan: the broker stored the output while the source was live, and
    // what that wrote and the cleanup then overwrote is not claimed to be gone.)
    let observations = journal.model_observations();
    assert_eq!(observations.len(), 1, "{what}");
    let observation = &observations[0];
    let defects = defects_of_late_storage(None, &store, observation, &journal.given()).await;
    assert!(
        defects.is_empty(),
        "{what}: the late observation is not stored redacted:\n{defects:#?}"
    );

    // The cleanup goes to its end; the books and the journal agree on the receipt.
    let status = revoke(&store, RUNS[0], Some(0)).await.unwrap();
    finish_cleanup(&store, status).await;
    let call = store
        .budget_call(&host(), SCOPE, &requests[0])
        .await
        .unwrap()
        .expect("the call is on the books");
    assert_eq!(call.state, BudgetCallState::Finalized, "{what}");
    assert_eq!(call.actual_cost_micros, Some(10), "{what}");
    assert_eq!(call.actual_currency.as_deref(), Some("USD"), "{what}");
    assert!(call.execution_closed, "{what}");
    assert_eq!(call.response_usable, Some(false), "{what}");
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("source_revoked"),
        "{what}"
    );
    let root = store.root_budget(&admin(), SCOPE).await.unwrap().unwrap();
    assert_eq!((root.reserved_micros, root.spent_micros), (0, 10), "{what}");
    let receipt = &observation.payload["execution_receipt"];
    assert_eq!(receipt["call_id"], json!(call.call_id), "{what}");
    assert_eq!(receipt["dispatch_id"], json!(call.dispatch_id), "{what}");
    assert_eq!(
        receipt["provider_request_id"],
        json!(call.provider_request_id),
        "{what}"
    );
    assert_eq!(
        receipt["usage_record_id"],
        json!(call.usage_record_id),
        "{what}"
    );

    // Nowhere, through every read the store has: the ledger rows, any object, any
    // idempotency row.
    let ledger = read_control_plane_facts(&path).await.unwrap();
    assert!(
        ledger
            .root_budget_tables
            .values()
            .flatten()
            .all(|row| !row.contains(MARKER)),
        "{what}: the output is in the ledger"
    );
    let defects = defects_of_late_storage(None, &store, observation, &journal.given()).await;
    assert!(
        defects.is_empty(),
        "{what}, after the cleanup completed:\n{defects:#?}"
    );
    assert_eq!(
        plaintext_stage_facts(&store).await,
        Vec::<String>::new(),
        "{what}"
    );
}

#[tokio::test]
async fn the_cost_of_a_response_that_lands_after_the_cleanup_completed_stays_on_the_books() {
    the_cost_stays_on_the_books(InFlight::RevokeAndClean, "episode-f").await;
}

#[tokio::test]
async fn the_cost_of_a_response_that_lands_while_the_cleanup_runs_stays_on_the_books() {
    the_cost_stays_on_the_books(InFlight::RevokeAndStartCleanup, "episode-g").await;
}
