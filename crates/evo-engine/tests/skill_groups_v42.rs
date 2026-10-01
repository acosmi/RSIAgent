//! V095.a program subrange: at most two isolated skill groups per job, serial
//! execution through the existing single-skill step, and a combined marker that
//! never stands for an approved, published or activated combination.
//!
//! The fixtures mirror `tests/optimization.rs`: a real SQLite store, Host-issued
//! source authority, fixture model and development runner ports with counters.

use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evidence::{EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{SkillGroupCandidate, build_combined_skill_candidate};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::groups::{
    GroupOutcome, GroupOutcomeKind, SkillGroupJobOutcome, SkillGroupJobRequest, SkillGroupSpec,
    run_skill_group_job,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationStepOutcome, OptimizationStepRequest, PairedTaskResult, StepTerminalClass,
    StoreOptimizationJournal, run_optimization_step,
};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use evo_storage::lifecycle::CleanupState;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

const NS: &str = "tenant";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// What distinguishes one group's request from another's.
#[derive(Clone)]
struct Spec {
    tag: String,
    skill_id: String,
    skill_version: String,
    content: String,
    failure_run: String,
    success_run: String,
    namespace: String,
}

impl Spec {
    /// Group `group-<tag>` edits `skill-<tag>` in episode `episode-<tag>`. It
    /// reads its own failing run and `shared-success`, which every group reads.
    fn new(tag: &str, failure_run: &str) -> Self {
        Self {
            tag: tag.into(),
            skill_id: format!("skill-{tag}"),
            skill_version: "v1".into(),
            content: format!("old-{tag}"),
            failure_run: failure_run.into(),
            success_run: "shared-success".into(),
            namespace: NS.into(),
        }
    }
}

/// Everything one `OptimizationStepRequest` borrows, owned in one place.
struct GroupFixture {
    group_id: String,
    episode_id: String,
    parent: SkillSnapshot,
    baseline: SkillSnapshot,
    selection: SourceSelection,
    records: Vec<StoredRunRecord>,
    evidence: EvidenceSet,
    bindings: Vec<TrustedSourceBinding>,
    traces: Vec<OptimizationTrace>,
    edit_context: TrustedEditContext,
    template: SkillEditBatch,
    profile: Profile,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver_patch: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
    development_request: DevelopmentRunRequest,
    model_context: ModelRequestContext,
}

fn trace(run: &str, family: &str, outcome: TraceOutcome, skill_id: &str) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: skill_id.into(),
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

fn build(spec: &Spec) -> GroupFixture {
    let tag = &spec.tag;
    let parent = SkillSnapshot {
        content: spec.content.clone(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let failure_family = format!("family-{tag}");
    let runs = [
        (
            spec.failure_run.as_str(),
            failure_family.as_str(),
            TraceOutcome::TaskFailure,
        ),
        (
            spec.success_run.as_str(),
            "family-shared",
            TraceOutcome::Success,
        ),
    ];
    let selection = SourceSelection {
        roots: vec![],
        run_ids: runs.iter().map(|(id, _, _)| (*id).into()).collect(),
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    };
    let records: Vec<StoredRunRecord> = runs
        .iter()
        .map(|(id, family, _)| StoredRunRecord {
            id: (*id).into(),
            body: id.as_bytes().to_vec(),
            parent_family: (*family).into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        })
        .collect();
    let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
    let bindings = runs
        .iter()
        .map(|(id, family, _)| TrustedSourceBinding {
            source_id: (*id).into(),
            source_digest: hash(id.as_bytes()),
            parent_family: (*family).into(),
        })
        .collect();
    let traces = runs
        .iter()
        .map(|(id, family, outcome)| trace(id, family, *outcome, &spec.skill_id))
        .collect();
    let allowed: Vec<EvidenceRef> = runs
        .iter()
        .map(|(id, _, _)| EvidenceRef {
            id: (*id).into(),
            digest: hash(id.as_bytes()),
        })
        .collect();
    let approved_parent_digest = hash(format!("parent-{}", spec.skill_id).as_bytes());
    let safe_baseline_digest = hash(format!("baseline-{}", spec.skill_id).as_bytes());
    let parent_digest = skill_snapshot_digest(&parent).unwrap();
    let parent_bundle_digest = hash(format!("parent-bundle-{tag}").as_bytes());
    let profile_id = format!("profile-{}", spec.skill_id);
    let edit_context = TrustedEditContext::new(
        spec.namespace.as_str(),
        profile_id.as_str(),
        spec.skill_id.as_str(),
        spec.skill_version.as_str(),
        approved_parent_digest.as_str(),
        safe_baseline_digest.as_str(),
        &parent,
        allowed.clone(),
    )
    .unwrap();
    let template = SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: spec.namespace.clone(),
        profile_id: profile_id.clone(),
        skill_id: spec.skill_id.clone(),
        skill_version: spec.skill_version.clone(),
        input_digest: parent_digest.clone(),
        approved_parent_digest: approved_parent_digest.clone(),
        safe_baseline_digest: safe_baseline_digest.clone(),
        evidence: EvidenceClosure {
            support: vec![],
            counterexamples: vec![],
            dependencies: vec![],
        },
        edits: vec![],
    };
    let episode_id = format!("episode-{tag}");
    let development_request = DevelopmentRunRequest {
        request_id: format!("dev-request-{tag}"),
        namespace: spec.namespace.clone(),
        purpose: Purpose::Development,
        episode_id: episode_id.clone(),
        step: 1,
        attempt: 1,
        manifest: DevelopmentManifest::build(
            format!("manifest-{tag}"),
            vec![
                DevelopmentTask {
                    id: format!("task-{tag}-1"),
                    parent_family: format!("family-{tag}"),
                    input_digest: hash(b"task-1"),
                },
                DevelopmentTask {
                    id: format!("task-{tag}-2"),
                    parent_family: "family-shared".into(),
                    input_digest: hash(b"task-2"),
                },
            ],
        )
        .unwrap(),
        parent_bundle_digest: parent_bundle_digest.clone(),
        candidate_bundle_digest: hash(b"candidate-bundle"),
        environment_digest: hash(b"environment"),
        grader_digest: hash(b"grader"),
        rules_digest: hash(b"rules"),
        tools_digest: hash(b"tools"),
        revoke_watermark: 1,
        idempotency_key: format!("dev-idempotency-{tag}"),
    };
    let model_context = ModelRequestContext {
        request_id: format!("request-{tag}"),
        namespace: spec.namespace.clone(),
        purpose: Purpose::Development,
        stage: ModelStage::ReflectFailure,
        episode_id: episode_id.clone(),
        step: 1,
        attempt: 1,
        parent_skill_digest: parent_digest,
        bundle_digest: parent_bundle_digest,
        source_closure: allowed,
        model_digest: hash(b"fixture-model"),
        tools_digest: hash(b"tools"),
        rules_digest: hash(b"rules"),
        sampling_digest: hash(b"sampling"),
        revoke_watermark: 1,
        max_suggestions: 4,
    };
    GroupFixture {
        group_id: format!("group-{tag}"),
        episode_id,
        baseline: parent.clone(),
        parent,
        selection,
        records,
        evidence,
        bindings,
        traces,
        edit_context,
        template,
        profile: Profile {
            id: profile_id,
            evolution_enabled: true,
            parent_digest: approved_parent_digest,
            baseline_digest: safe_baseline_digest,
        },
        parent_strategy: Strategy::default(),
        baseline_strategy: Strategy::default(),
        improver_patch: ImproverPatch::default(),
        caps: HostCapabilities {
            available: BTreeSet::new(),
            granted: BTreeSet::new(),
        },
        revoked: BTreeSet::new(),
        development_request,
        model_context,
    }
}

impl GroupFixture {
    fn spec(&self) -> SkillGroupSpec<'_> {
        SkillGroupSpec {
            group_id: self.group_id.clone(),
            request: self.request(),
        }
    }

    fn request(&self) -> OptimizationStepRequest<'_> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.selection,
            source_bindings: &self.bindings,
            traces: self.traces.clone(),
            model_context: self.model_context.clone(),
            parent_skill: &self.parent,
            edit_context: &self.edit_context,
            edit_batch_template: self.template.clone(),
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
            development_request: self.development_request.clone(),
            allow_rank_call: false,
        }
    }
}

fn job<'a>(groups: impl IntoIterator<Item = &'a GroupFixture>) -> SkillGroupJobRequest<'a> {
    SkillGroupJobRequest {
        job_id: "job-1".into(),
        groups: groups.into_iter().map(GroupFixture::spec).collect(),
    }
}

/// A real store with Host-issued source authority for every group's runs.
struct World {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    host: Context,
    worker: Context,
    admin: Context,
    journal: StoreOptimizationJournal,
    groups: Vec<GroupFixture>,
}

impl World {
    async fn new(specs: &[Spec]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("groups.sqlite3");
        let store = Store::open(&path).await.unwrap();
        let host = Context::new(NS, "host", Role::Host).unwrap();
        let worker = Context::new(NS, "worker", Role::Worker).unwrap();
        let admin = Context::new(NS, "admin", Role::Admin).unwrap();
        let groups: Vec<GroupFixture> = specs.iter().map(build).collect();
        for group in &groups {
            for (record, trace) in group.records.iter().zip(&group.traces) {
                store_trace_authority(
                    &store,
                    &host,
                    &StoredTraceAuthority {
                        schema_version: "rsia.optimization.source.v1".into(),
                        record: record.clone(),
                        trace: trace.clone(),
                        excerpt_start: 0,
                        excerpt_end: record.id.len(),
                    },
                )
                .await
                .unwrap();
            }
            store_source_selection(&store, &host, &group.selection)
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&host, "initial-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let journal =
            StoreOptimizationJournal::new(store.clone(), worker.clone(), "worker").unwrap();
        Self {
            _dir: dir,
            path,
            store,
            host,
            worker,
            admin,
            journal,
            groups,
        }
    }

    /// The same database through fresh handles, as after a process restart.
    async fn reopened(self) -> Self {
        let Self {
            _dir,
            path,
            store,
            host,
            worker,
            admin,
            journal,
            groups,
        } = self;
        drop(journal);
        drop(store);
        let store = Store::open(&path).await.unwrap();
        let journal =
            StoreOptimizationJournal::new(store.clone(), worker.clone(), "worker").unwrap();
        Self {
            _dir,
            path,
            store,
            host,
            worker,
            admin,
            journal,
            groups,
        }
    }

    async fn artifacts(&self) -> Vec<Value> {
        let mut session = self.store.session().await.unwrap();
        let artifacts: Vec<Value> = session.list(&self.worker, "artifact").await.unwrap();
        session.commit().await.unwrap();
        artifacts
    }

    async fn artifact_ids(&self) -> BTreeSet<String> {
        self.artifacts()
            .await
            .iter()
            .filter_map(|value| value["artifact_id"].as_str().map(String::from))
            .collect()
    }

    async fn stage_facts(&self) -> Vec<Value> {
        self.artifacts()
            .await
            .into_iter()
            .filter(|value| value["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA)
            .collect()
    }

    async fn raw(&self, id: &str) -> Value {
        let mut session = self.store.session().await.unwrap();
        let value = session
            .need::<Value>(&self.worker, "artifact", id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        value
    }

    async fn run(
        &self,
        model: &ScriptedModel,
        runner: &ScriptedRunner,
        groups: &[&GroupFixture],
    ) -> Result<SkillGroupJobOutcome> {
        run_skill_group_job(
            Some(model),
            Some(runner),
            Some(&self.journal),
            job(groups.iter().copied()),
        )
        .await
    }
}

fn facts_of(facts: &[Value], episode: &str) -> BTreeMap<String, Value> {
    facts
        .iter()
        .filter(|fact| fact["episode_id"] == episode)
        .map(|fact| {
            (
                fact["artifact_id"].as_str().unwrap().to_string(),
                fact.clone(),
            )
        })
        .collect()
}

fn fact_kinds(facts: &BTreeMap<String, Value>) -> BTreeSet<String> {
    facts
        .values()
        .map(|fact| fact["kind"].as_str().unwrap().to_string())
        .collect()
}

fn group<'a>(outcome: &'a SkillGroupJobOutcome, id: &str) -> &'a GroupOutcome {
    outcome
        .groups
        .iter()
        .find(|group| group.group_id == id)
        .unwrap_or_else(|| panic!("job outcome has no group {id}"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// ---------------------------------------------------------------------------
// Ports with counters
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelScript {
    Edit,
    NoSuggestions,
    Reject,
    Uncertain,
    TransportError,
}

fn input_part<T: serde::de::DeserializeOwned>(request: &ModelRequest, label: &str) -> Result<T> {
    let part = request
        .input
        .iter()
        .find(|part| part.label == label)
        .ok_or_else(|| Error::Invalid(format!("fixture input {label} missing")))?;
    serde_json::from_str(&part.content)
        .map_err(|_| Error::Invalid(format!("fixture input {label} invalid")))
}

/// One bounded insertion per reflection batch, exactly as in `tests/optimization.rs`.
fn editing_output(request: &ModelRequest) -> Result<(&'static str, String)> {
    let parent: SkillSnapshot = input_part(request, "parent-skill")?;
    let strategy: Strategy = input_part(request, "generation-strategy")?;
    let source = request.source_closure[0].clone();
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
    Ok((id, serde_json::to_string(&vec![suggestion]).unwrap()))
}

fn completed(request: ModelRequest, output: String, suffix: &str) -> ModelResponse {
    ModelResponse::Completed {
        request_id: request.request_id,
        response_id: format!("fixture-response-{suffix}"),
        actual_model_digest: request.model_digest,
        input_digest: request.input_digest,
        output_digest: hash(output.as_bytes()),
        output,
        execution_receipt: ModelExecutionReceipt {
            call_id: format!("fixture-call-{suffix}"),
            dispatch_id: format!("fixture-dispatch-{suffix}"),
            root_budget_id: "fixture-budget".into(),
            provider_request_id: format!("fixture-provider-{suffix}"),
            usage_record_id: format!("fixture-usage-{suffix}"),
            provenance: ModelExecutionProvenance::Fixture,
        },
    }
}

/// Behaviour is chosen per episode, so one group can fail while the other works.
#[derive(Default)]
struct ScriptedModel {
    scripts: BTreeMap<String, ModelScript>,
    total: AtomicUsize,
    per_episode: Mutex<BTreeMap<String, usize>>,
}

impl ScriptedModel {
    fn script(mut self, episode: &str, script: ModelScript) -> Self {
        self.scripts.insert(episode.into(), script);
        self
    }

    fn calls(&self) -> usize {
        self.total.load(SeqCst)
    }

    fn calls_for(&self, episode: &str) -> usize {
        self.per_episode
            .lock()
            .unwrap()
            .get(episode)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl ModelPort for ScriptedModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.total.fetch_add(1, SeqCst);
        *self
            .per_episode
            .lock()
            .unwrap()
            .entry(request.episode_id.clone())
            .or_default() += 1;
        let script = self
            .scripts
            .get(&request.episode_id)
            .copied()
            .unwrap_or(ModelScript::Edit);
        match script {
            ModelScript::Edit => {
                let (id, output) = editing_output(&request)?;
                let suffix = format!("{}-{id}", request.episode_id);
                Ok(completed(request, output, &suffix))
            }
            ModelScript::NoSuggestions => {
                let suffix = format!("{}-none", request.episode_id);
                Ok(completed(request, "[]".into(), &suffix))
            }
            ModelScript::Reject => Ok(ModelResponse::Rejected {
                request_id: request.request_id,
                kind: ModelRejectionKind::ProviderRejected,
                reason: "fixture provider refused the request".into(),
                dispatch: RejectedDispatch::NotDispatched,
            }),
            ModelScript::Uncertain => Ok(ModelResponse::Uncertain {
                request_id: request.request_id,
                dispatch_id: "unknown-dispatch".into(),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: None,
                usage_record_id: None,
            }),
            ModelScript::TransportError => Err(Error::Internal),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunnerScript {
    Improve,
    /// Candidate scores equal the parent's: development selection keeps the incumbent.
    Regress,
    TransportError,
}

fn development_report(request: &DevelopmentRunRequest, improve: bool) -> DevelopmentRunReport {
    DevelopmentRunReport {
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
                candidate_score_micros: if improve { 700_000 } else { 500_000 },
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: format!("parent-{}-{}", request.episode_id, task.id),
                candidate_execution_id: format!("candidate-{}-{}", request.episode_id, task.id),
                grader_receipt_digest: hash(
                    format!("grade-{}-{}", request.episode_id, task.id).as_bytes(),
                ),
            })
            .collect(),
        execution_receipt_id: format!("fixture-execution-{}", request.episode_id),
        usage_record_ids: vec![format!("fixture-usage-{}", request.episode_id)],
        provenance: DevelopmentExecutionProvenance::Fixture,
    }
}

#[derive(Default)]
struct ScriptedRunner {
    scripts: BTreeMap<String, RunnerScript>,
    total: AtomicUsize,
    per_episode: Mutex<BTreeMap<String, usize>>,
}

impl ScriptedRunner {
    fn script(mut self, episode: &str, script: RunnerScript) -> Self {
        self.scripts.insert(episode.into(), script);
        self
    }

    fn calls(&self) -> usize {
        self.total.load(SeqCst)
    }

    fn calls_for(&self, episode: &str) -> usize {
        self.per_episode
            .lock()
            .unwrap()
            .get(episode)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl DevRunner for ScriptedRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.total.fetch_add(1, SeqCst);
        *self
            .per_episode
            .lock()
            .unwrap()
            .entry(request.episode_id.clone())
            .or_default() += 1;
        match self
            .scripts
            .get(&request.episode_id)
            .copied()
            .unwrap_or(RunnerScript::Improve)
        {
            RunnerScript::Improve => Ok(development_report(&request, true)),
            RunnerScript::Regress => Ok(development_report(&request, false)),
            RunnerScript::TransportError => Err(Error::Internal),
        }
    }
}

/// The same suggestions as `ScriptedModel`, but dispatched through the persistent
/// broker so that a real root budget is reserved and settled.
#[derive(Clone)]
struct EditingTransport {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelTransport for EditingTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, SeqCst);
        let (_, output) = editing_output(request)?;
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            output,
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
    }
}

fn assert_send<T: Send>(_: &T) {}

/// Where the candidate of `group` ends up when its two fixture suggestions are applied.
fn expected_candidate_digest(group: &GroupFixture) -> String {
    let instruction = Strategy::default().instruction;
    skill_snapshot_digest(&SkillSnapshot {
        content: format!("{} [{instruction}]", group.parent.content),
        applicability: format!("{} [{instruction}]", group.parent.applicability),
        counterexample: group.parent.counterexample.clone(),
        required_capabilities: vec![],
        dependencies: vec![],
    })
    .unwrap()
}

fn two_specs() -> Vec<Spec> {
    vec![Spec::new("a", "a-failure"), Spec::new("b", "b-failure")]
}

/// Keys, at any depth, that would make an outcome look like an approval,
/// publication or activation.
fn authority_like_keys(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let key_lower = key.to_lowercase();
                if ["approv", "publish", "activat", "release", "active"]
                    .iter()
                    .any(|word| key_lower.contains(word))
                {
                    found.push(key.clone());
                }
                authority_like_keys(inner, found);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| authority_like_keys(item, found)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 1. Two groups, each with its own candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_groups_each_produce_a_candidate_with_disjoint_facts_and_an_order_independent_marker() {
    let world = World::new(&two_specs()).await;
    let (a, b) = (&world.groups[0], &world.groups[1]);
    let model = ScriptedModel::default();
    let runner = ScriptedRunner::default();
    let ids_before = world.artifact_ids().await;

    let both = [a, b];
    let future = world.run(&model, &runner, &both);
    assert_send(&future);
    let outcome = future.await.unwrap();

    assert_eq!(outcome.job_id, "job-1");
    assert_eq!(outcome.groups.len(), 2);
    for (fixture, skill) in [(a, "skill-a"), (b, "skill-b")] {
        let got = group(&outcome, &fixture.group_id);
        assert_eq!(got.skill_id, skill);
        assert_eq!(got.episode_id, fixture.episode_id);
        assert_eq!(got.kind, GroupOutcomeKind::Candidate);
        assert_eq!(got.terminal, None);
        assert_eq!(
            got.candidate_skill_digest.as_deref(),
            Some(expected_candidate_digest(fixture).as_str())
        );
        assert!(is_sha256(got.candidate_bundle_digest.as_deref().unwrap()));
    }
    assert_ne!(
        group(&outcome, "group-a").candidate_bundle_digest,
        group(&outcome, "group-b").candidate_bundle_digest
    );
    // Two reflection batches and one development run per group, nothing more.
    assert_eq!((model.calls(), runner.calls()), (4, 2));

    // The facts are the ordinary stage facts, one disjoint set per episode, and
    // the job persisted nothing else.
    let facts = world.stage_facts().await;
    let a_facts = facts_of(&facts, &a.episode_id);
    let b_facts = facts_of(&facts, &b.episode_id);
    assert!(!a_facts.is_empty() && a_facts.len() == b_facts.len());
    assert!(a_facts.keys().all(|id| !b_facts.contains_key(id)));
    assert_eq!(facts.len(), a_facts.len() + b_facts.len());
    let ids_after = world.artifact_ids().await;
    assert_eq!(
        ids_after
            .difference(&ids_before)
            .cloned()
            .collect::<BTreeSet<_>>(),
        a_facts
            .keys()
            .chain(b_facts.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
    );
    for (id, fact) in a_facts.iter().chain(&b_facts) {
        assert_eq!(
            world.journal.reload(id).await.unwrap().episode_id,
            fact["episode_id"]
        );
    }
    // Each outcome is the durable record, not a recomputation.
    for fixture in [a, b] {
        let compiled = facts_of(&facts, &fixture.episode_id)
            .into_values()
            .find(|fact| fact["kind"] == "edit_compiled")
            .unwrap();
        assert_eq!(
            Some(compiled["payload"]["bundle"]["digest"].as_str().unwrap()),
            group(&outcome, &fixture.group_id)
                .candidate_bundle_digest
                .as_deref()
        );
    }

    // The marker lists both groups, in group order, and claims nothing else.
    let combined = outcome.combined.as_ref().expect("two candidates combine");
    assert_eq!(combined.schema_version, "rsia.combined_skill_candidate.v1");
    assert_eq!(
        combined
            .groups
            .iter()
            .map(|group| (group.group_id.as_str(), group.candidate_digest.as_str()))
            .collect::<Vec<_>>(),
        ["group-a", "group-b"].map(|id| (
            id,
            group(&outcome, id)
                .candidate_bundle_digest
                .as_deref()
                .unwrap()
        ))
    );
    assert!(combined.requires_full_development_rerun);
    assert!(combined.requires_full_formal_evaluation);
    assert_eq!(
        combined.combined_digest,
        fingerprint(&combined.groups).unwrap()
    );
    let mut keys = Vec::new();
    authority_like_keys(&serde_json::to_value(&outcome).unwrap(), &mut keys);
    assert!(
        keys.is_empty(),
        "outcome carries authority-like fields: {keys:?}"
    );

    // Swapping the request order changes neither the outcome nor the marker.
    let swapped = world.run(&model, &runner, &[b, a]).await.unwrap();
    assert_eq!(
        serde_json::to_value(&swapped).unwrap(),
        serde_json::to_value(&outcome).unwrap()
    );
    assert_eq!((model.calls(), runner.calls()), (4, 2));
    // The core digest itself is order sensitive, which is why the job sorts.
    let reversed: Vec<SkillGroupCandidate> = combined.groups.iter().rev().cloned().collect();
    assert_ne!(
        build_combined_skill_candidate(reversed)
            .unwrap()
            .combined_digest,
        combined.combined_digest
    );
    // A fresh execution with the swapped order reaches the same marker.
    let fresh = World::new(&[two_specs()[1].clone(), two_specs()[0].clone()]).await;
    let fresh_outcome = fresh
        .run(
            &ScriptedModel::default(),
            &ScriptedRunner::default(),
            &[&fresh.groups[0], &fresh.groups[1]],
        )
        .await
        .unwrap();
    assert_eq!(
        fresh_outcome.combined.unwrap().combined_digest,
        combined.combined_digest
    );
}

async fn clean_baseline() -> SkillGroupJobOutcome {
    let world = World::new(&two_specs()).await;
    let both = [&world.groups[0], &world.groups[1]];
    world
        .run(&ScriptedModel::default(), &ScriptedRunner::default(), &both)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// 2. A group that does not produce a candidate never erases the other group
// ---------------------------------------------------------------------------

struct Isolation {
    name: &'static str,
    /// The group made to fail: "a" runs first, "b" runs second.
    failing: &'static str,
    model: Option<ModelScript>,
    runner: Option<RunnerScript>,
    /// Applied to the failing group's fixture after the store was seeded.
    mutate: Option<fn(&mut GroupFixture)>,
    kind: GroupOutcomeKind,
    /// The fixed code of the terminal class the group's outcome carries (AG-048).
    class_code: &'static str,
    /// Calls the failing group is expected to make: (model, development runner).
    calls: (usize, usize),
    /// Durable fact kinds the failing group must have left behind; empty means none at all.
    facts: &'static [&'static str],
    /// The status its `step_completed` fact records.
    status: Option<&'static str>,
}

#[tokio::test]
async fn a_group_that_fails_keeps_its_own_record_and_never_erases_the_other_groups_result() {
    let baseline = clean_baseline().await;
    let cases = [
        Isolation {
            name: "the model rejects the request",
            failing: "a",
            model: Some(ModelScript::Reject),
            runner: None,
            mutate: None,
            kind: GroupOutcomeKind::Rejected,
            class_code: "model_rejected_provider_rejected",
            calls: (1, 0),
            facts: &["response_observed", "terminal_rejected", "step_completed"],
            status: Some("rejected"),
        },
        Isolation {
            name: "the model reports unknown usage",
            failing: "a",
            model: Some(ModelScript::Uncertain),
            runner: None,
            mutate: None,
            kind: GroupOutcomeKind::Uncertain,
            class_code: "model_usage_unknown",
            calls: (1, 0),
            facts: &["dispatch_observed", "terminal_uncertain", "step_completed"],
            status: Some("uncertain"),
        },
        Isolation {
            name: "the model transport errors",
            failing: "a",
            model: Some(ModelScript::TransportError),
            runner: None,
            mutate: None,
            kind: GroupOutcomeKind::Uncertain,
            class_code: "model_transport_outcome_unknown",
            calls: (1, 0),
            facts: &["dispatch_prepared", "step_completed"],
            status: Some("uncertain"),
        },
        Isolation {
            name: "the development runner errors",
            failing: "a",
            model: None,
            runner: Some(RunnerScript::TransportError),
            mutate: None,
            kind: GroupOutcomeKind::Uncertain,
            class_code: "development_execution_outcome_unknown",
            calls: (2, 1),
            facts: &[
                "edit_compiled",
                "development_request_prepared",
                "dispatch_prepared",
                "step_completed",
            ],
            status: Some("uncertain"),
        },
        Isolation {
            name: "the candidate does not improve the development manifest",
            failing: "a",
            model: None,
            runner: Some(RunnerScript::Regress),
            mutate: None,
            kind: GroupOutcomeKind::Rejected,
            class_code: "keep_incumbent_not_improved",
            calls: (2, 1),
            facts: &["edit_compiled", "development_observed", "terminal_rejected"],
            status: Some("rejected"),
        },
        Isolation {
            name: "the model proposes nothing",
            failing: "a",
            model: Some(ModelScript::NoSuggestions),
            runner: None,
            mutate: None,
            kind: GroupOutcomeKind::NoChange,
            class_code: "no_edit_suggestions",
            calls: (2, 0),
            facts: &["response_observed", "terminal_no_change", "step_completed"],
            status: Some("no_change"),
        },
        Isolation {
            name: "the group's source grant does not allow model excerpts",
            failing: "a",
            model: None,
            runner: None,
            mutate: Some(|group| group.selection.allow_model_excerpts = false),
            kind: GroupOutcomeKind::Rejected,
            class_code: "grant_unavailable",
            calls: (0, 0),
            facts: &["terminal_rejected", "step_completed"],
            status: Some("rejected"),
        },
        Isolation {
            name: "the group's trace is not backed by the stored source",
            failing: "a",
            model: None,
            runner: None,
            mutate: Some(|group| group.traces[0].excerpt = "unbacked bytes".into()),
            kind: GroupOutcomeKind::Failed,
            class_code: "step_error_forbidden",
            calls: (0, 0),
            facts: &[],
            status: None,
        },
        Isolation {
            name: "the group that runs second fails after the first group completed",
            failing: "b",
            model: None,
            runner: Some(RunnerScript::TransportError),
            mutate: None,
            kind: GroupOutcomeKind::Uncertain,
            class_code: "development_execution_outcome_unknown",
            calls: (2, 1),
            facts: &["edit_compiled", "dispatch_prepared", "step_completed"],
            status: Some("uncertain"),
        },
    ];
    for case in cases {
        let mut world = World::new(&two_specs()).await;
        let (bad, good, bad_index, bad_episode, good_episode) = if case.failing == "a" {
            ("group-a", "group-b", 0, "episode-a", "episode-b")
        } else {
            ("group-b", "group-a", 1, "episode-b", "episode-a")
        };
        let mut model = ScriptedModel::default();
        let mut runner = ScriptedRunner::default();
        if let Some(script) = case.model {
            model = model.script(bad_episode, script);
        }
        if let Some(script) = case.runner {
            runner = runner.script(bad_episode, script);
        }
        if let Some(mutate) = case.mutate {
            mutate(&mut world.groups[bad_index]);
        }
        let both = [&world.groups[0], &world.groups[1]];
        let outcome = world.run(&model, &runner, &both).await.unwrap();
        let name = case.name;

        let failed = group(&outcome, bad);
        assert_eq!(failed.kind, case.kind, "{name}");
        assert_eq!(
            failed.terminal.map(|class| class.code()),
            Some(case.class_code),
            "{name}: {:?}",
            failed.terminal
        );
        assert_eq!(
            (
                &failed.candidate_skill_digest,
                &failed.candidate_bundle_digest
            ),
            (&None, &None),
            "{name}"
        );
        // The healthy group is exactly what it is when nothing fails beside it.
        assert_eq!(group(&outcome, good), group(&baseline, good), "{name}");
        let combined = outcome.combined.as_ref().expect(name);
        assert_eq!(combined.groups.len(), 1, "{name}");
        assert_eq!(combined.groups[0].group_id, good, "{name}");
        assert_eq!(
            Some(&combined.groups[0].candidate_digest),
            group(&baseline, good).candidate_bundle_digest.as_ref(),
            "{name}"
        );
        assert!(
            combined.requires_full_development_rerun && combined.requires_full_formal_evaluation,
            "{name}"
        );
        assert_eq!(
            (model.calls_for(bad_episode), runner.calls_for(bad_episode)),
            case.calls,
            "{name}"
        );
        assert_eq!(
            (
                model.calls_for(good_episode),
                runner.calls_for(good_episode)
            ),
            (2, 1),
            "{name}"
        );

        // Both groups keep their own durable record, reloadable as it was written.
        let facts = world.stage_facts().await;
        let bad_facts = facts_of(&facts, bad_episode);
        let good_facts = facts_of(&facts, good_episode);
        assert!(!good_facts.is_empty(), "{name}");
        assert!(
            good_facts.keys().all(|id| !bad_facts.contains_key(id)),
            "{name}"
        );
        assert_eq!(facts.len(), bad_facts.len() + good_facts.len(), "{name}");
        for id in good_facts.keys().chain(bad_facts.keys()) {
            world.journal.reload(id).await.expect(name);
        }
        let kinds = fact_kinds(&bad_facts);
        assert_eq!(kinds.is_empty(), case.facts.is_empty(), "{name}: {kinds:?}");
        for kind in case.facts {
            assert!(kinds.contains(*kind), "{name}: missing {kind} in {kinds:?}");
        }
        let step_status = bad_facts
            .values()
            .find(|fact| fact["kind"] == "step_completed")
            .map(|fact| fact["payload"]["status"].as_str().unwrap().to_string());
        assert_eq!(step_status.as_deref(), case.status, "{name}");
    }
}

// ---------------------------------------------------------------------------
// 3. Everything that needs no model or runner is checked before the first call
// ---------------------------------------------------------------------------

type TemplateChange = fn(&mut SkillEditBatch);
type DevelopmentChange = fn(&mut DevelopmentRunRequest);

fn invalid(part: &'static str) -> impl Fn(&Error) -> bool {
    move |error| matches!(error, Error::Invalid(message) if message.contains(part))
}

fn conflict(part: &'static str) -> impl Fn(&Error) -> bool {
    move |error| matches!(error, Error::Conflict(message) if message.contains(part))
}

/// The job is refused as a whole: the expected error, no model call, no runner
/// call, and (checked by the caller) no fact written.
async fn refused(
    world: &World,
    what: &str,
    request: SkillGroupJobRequest<'_>,
    expected: impl Fn(&Error) -> bool,
) {
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let error = run_skill_group_job(Some(&model), Some(&runner), Some(&world.journal), request)
        .await
        .expect_err(what);
    assert!(expected(&error), "{what}: unexpected error {error:?}");
    assert_eq!((model.calls(), runner.calls()), (0, 0), "{what}");
}

#[tokio::test]
async fn an_invalid_job_is_refused_whole_before_any_model_or_runner_call() {
    let world = World::new(&two_specs()).await;
    let (a, b) = (&world.groups[0], &world.groups[1]);
    let c = build(&Spec::new("c", "c-failure"));
    let ids_before = world.artifact_ids().await;
    let variant = |change: fn(&mut Spec)| {
        let mut spec = Spec::new("b", "b-failure");
        change(&mut spec);
        build(&spec)
    };

    // Shape of the job.
    refused(
        &world,
        "no group",
        SkillGroupJobRequest {
            job_id: "job-1".into(),
            groups: vec![],
        },
        invalid("one or two groups"),
    )
    .await;
    refused(
        &world,
        "three groups",
        SkillGroupJobRequest {
            job_id: "job-1".into(),
            groups: vec![a.spec(), b.spec(), c.spec()],
        },
        invalid("one or two groups"),
    )
    .await;
    refused(
        &world,
        "empty job id",
        SkillGroupJobRequest {
            job_id: String::new(),
            ..job([a, b])
        },
        invalid("identifier"),
    )
    .await;
    for bad_id in ["", "group a"] {
        let mut request = job([a, b]);
        request.groups[1].group_id = bad_id.into();
        refused(&world, "unusable group id", request, invalid("identifier")).await;
    }
    let mut request = job([a, b]);
    request.groups[1].group_id = "group-a".into();
    refused(
        &world,
        "duplicate group id",
        request,
        conflict("duplicate skill group id"),
    )
    .await;
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let missing_journal = run_skill_group_job(Some(&model), Some(&runner), None, job([a, b])).await;
    assert!(matches!(missing_journal, Err(Error::NotFound)));
    assert_eq!((model.calls(), runner.calls()), (0, 0));

    // One skill, two groups.
    let same_skill = variant(|spec| {
        spec.skill_id = "skill-a".into();
        spec.content = "old-a".into();
    });
    refused(
        &world,
        "same skill, same parent",
        job([a, &same_skill]),
        conflict("target the same skill"),
    )
    .await;
    let other_body = variant(|spec| {
        spec.skill_id = "skill-a".into();
        spec.content = "a different body".into();
    });
    refused(
        &world,
        "same name, different body",
        job([a, &other_body]),
        conflict("different parent versions"),
    )
    .await;
    let other_version = variant(|spec| {
        spec.skill_id = "skill-a".into();
        spec.content = "old-a".into();
        spec.skill_version = "v2".into();
    });
    refused(
        &world,
        "same name, different version",
        job([a, &other_version]),
        conflict("different parent versions"),
    )
    .await;

    // Groups that would write the same records, or live in different namespaces.
    let mut same_episode = build(&Spec::new("b", "b-failure"));
    same_episode.model_context.episode_id = "episode-a".into();
    same_episode.development_request.episode_id = "episode-a".into();
    refused(
        &world,
        "same episode",
        job([a, &same_episode]),
        conflict("distinct episodes"),
    )
    .await;
    let mut same_request = build(&Spec::new("b", "b-failure"));
    same_request.model_context.request_id = "request-a".into();
    refused(
        &world,
        "same request id",
        job([a, &same_request]),
        conflict("distinct request ids"),
    )
    .await;
    let other_namespace = variant(|spec| spec.namespace = "other".into());
    refused(
        &world,
        "different namespace",
        job([a, &other_namespace]),
        conflict("share one namespace"),
    )
    .await;

    // Each group's request must agree with its own trusted edit context.
    let template_fields: [(&str, TemplateChange); 7] = [
        ("namespace", |template| template.namespace = "other".into()),
        ("profile_id", |template| {
            template.profile_id = "other-profile".into()
        }),
        // The edit template aims at the skill of the other group.
        ("skill_id", |template| template.skill_id = "skill-b".into()),
        ("skill_version", |template| {
            template.skill_version = "v9".into()
        }),
        ("input_digest", |template| {
            template.input_digest = hash(b"other-input")
        }),
        ("approved_parent_digest", |template| {
            template.approved_parent_digest = hash(b"other-parent")
        }),
        ("safe_baseline_digest", |template| {
            template.safe_baseline_digest = hash(b"other-baseline")
        }),
    ];
    for (field, change) in template_fields {
        let mut request = job([a, b]);
        change(&mut request.groups[0].request.edit_batch_template);
        let expected = format!("edit_scope_mismatch:{field}");
        refused(&world, field, request, |error| {
            matches!(error, Error::Conflict(message) if message.contains("group-a") && message.contains(&expected))
        })
        .await;
    }
    let mut request = job([a, b]);
    request.groups[0].request.edit_batch_template.schema_version = "rsia.skill_edit.v0".into();
    refused(
        &world,
        "template schema",
        request,
        invalid("unsupported_skill_edit_schema"),
    )
    .await;
    let mut request = job([a, b]);
    request.groups[0]
        .request
        .edit_batch_template
        .compiler_version = "other".into();
    refused(
        &world,
        "template compiler",
        request,
        conflict("compiler_version_mismatch"),
    )
    .await;
    let mut request = job([a, b]);
    request.groups[0].request.edit_context = &b.edit_context;
    refused(
        &world,
        "context of the other group",
        request,
        conflict("edit_scope_mismatch"),
    )
    .await;
    let tampered = SkillSnapshot {
        content: "tampered".into(),
        ..a.parent.clone()
    };
    let mut request = job([a, b]);
    request.groups[0].request.parent_skill = &tampered;
    refused(
        &world,
        "parent differs from the context",
        request,
        conflict("trusted_input_digest_mismatch"),
    )
    .await;
    let oversized = SkillSnapshot {
        content: "x".repeat(16 * 1024 + 1),
        ..a.parent.clone()
    };
    let mut request = job([a, b]);
    request.groups[0].request.parent_skill = &oversized;
    refused(
        &world,
        "parent is not a valid skill snapshot",
        request,
        invalid("content"),
    )
    .await;
    let scope_changes: [(&str, DevelopmentChange); 5] = [
        ("namespace", |request| request.namespace = "other".into()),
        ("episode", |request| request.episode_id = "episode-x".into()),
        ("attempt", |request| request.attempt = 2),
        ("step", |request| request.step = 2),
        ("watermark", |request| request.revoke_watermark = 2),
    ];
    for (what, change) in scope_changes {
        let mut request = job([a, b]);
        change(&mut request.groups[1].request.development_request);
        refused(
            &world,
            what,
            request,
            conflict("step/development scope differs"),
        )
        .await;
    }

    let mut request = job([a, b]);
    request.groups[0].request.model_context.namespace = "other".into();
    request.groups[0].request.development_request.namespace = "other".into();
    refused(
        &world,
        "edit scope in another namespace than the model context",
        request,
        conflict("edit scope namespace differs"),
    )
    .await;

    // Nothing was written by any refused job.
    assert!(world.stage_facts().await.is_empty());
    assert_eq!(world.artifact_ids().await, ids_before);

    // The same store accepts the valid job, even when the template still carries
    // evidence and edits: only its scope is checked before the first call.
    let mut request = job([a, b]);
    let template = &mut request.groups[0].request.edit_batch_template;
    template.evidence = EvidenceClosure {
        support: vec![],
        counterexamples: vec![],
        dependencies: vec![EvidenceRef {
            id: "a-failure".into(),
            digest: hash(b"a-failure"),
        }],
    };
    template.edits = vec![SkillTextEdit {
        field: SkillTextField::Content,
        start: 0,
        end: 0,
        expected_text_digest: hash(b""),
        exact_anchor: None,
        operation: TextEditOperation::Insert {
            text: "stale".into(),
        },
    }];
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let outcome = run_skill_group_job(Some(&model), Some(&runner), Some(&world.journal), request)
        .await
        .unwrap();
    assert!(
        outcome
            .groups
            .iter()
            .all(|group| group.kind == GroupOutcomeKind::Candidate)
    );
    assert_eq!((model.calls(), runner.calls()), (4, 2));
}

// ---------------------------------------------------------------------------
// 4. Repeating a job converges through the journals
// ---------------------------------------------------------------------------

fn as_json(outcome: &SkillGroupJobOutcome) -> Value {
    serde_json::to_value(outcome).unwrap()
}

#[tokio::test]
async fn repeating_a_job_converges_through_the_journals_without_new_dispatch_or_spend() {
    let world = World::new(&two_specs()).await;
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let both = [&world.groups[0], &world.groups[1]];
    let first = as_json(&world.run(&model, &runner, &both).await.unwrap());
    let ids = world.artifact_ids().await;
    assert_eq!((model.calls(), runner.calls()), (4, 2));

    // The same request again: same outcome, nothing dispatched, nothing written.
    let again = world.run(&model, &runner, &both).await.unwrap();
    assert_eq!(as_json(&again), first);
    assert_eq!((model.calls(), runner.calls()), (4, 2));
    assert_eq!(world.artifact_ids().await, ids);
    // The journal alone is enough to answer it.
    let replay = run_skill_group_job(None, None, Some(&world.journal), job(both))
        .await
        .unwrap();
    assert_eq!(as_json(&replay), first);

    // So is a restarted process, with or without ports.
    let world = world.reopened().await;
    let both = [&world.groups[0], &world.groups[1]];
    let restarted = run_skill_group_job(None, None, Some(&world.journal), job(both))
        .await
        .unwrap();
    assert_eq!(as_json(&restarted), first);
    let restarted = world.run(&model, &runner, &both).await.unwrap();
    assert_eq!(as_json(&restarted), first);
    assert_eq!((model.calls(), runner.calls()), (4, 2));
    assert_eq!(world.artifact_ids().await, ids);
}

#[tokio::test]
async fn an_uncertain_group_is_never_retried_by_repeating_the_job() {
    let world = World::new(&two_specs()).await;
    let model = ScriptedModel::default().script("episode-a", ModelScript::Uncertain);
    let runner = ScriptedRunner::default().script("episode-b", RunnerScript::TransportError);
    let both = [&world.groups[0], &world.groups[1]];
    let first = world.run(&model, &runner, &both).await.unwrap();
    assert_eq!(
        first
            .groups
            .iter()
            .map(|group| (group.group_id.as_str(), group.kind))
            .collect::<Vec<_>>(),
        [
            ("group-a", GroupOutcomeKind::Uncertain),
            ("group-b", GroupOutcomeKind::Uncertain)
        ]
    );
    assert!(first.combined.is_none(), "no candidate, no marker");
    assert_eq!((model.calls(), runner.calls()), (3, 1));
    for _ in 0..2 {
        let again = world.run(&model, &runner, &both).await.unwrap();
        assert_eq!(as_json(&again), as_json(&first));
        assert_eq!((model.calls(), runner.calls()), (3, 1));
    }
}

// ---------------------------------------------------------------------------
// 5. Revocation reaches exactly the groups that read the revoked source
// ---------------------------------------------------------------------------

async fn revoke_and_clean(world: &World, source: &str, now: i64) {
    let mut status = LifecycleCoordinator::revoke_source(
        &world.admin,
        &world.store,
        source,
        "privacy_delete",
        now,
    )
    .await
    .unwrap();
    for step in 1..=400 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(
            &world.admin,
            &world.store,
            &status.job_id,
            8,
            now + step,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "{:?}",
        status.last_error
    );
}

fn grant_id(group: &GroupFixture) -> String {
    format!("optgrant-{}", fingerprint(&group.selection).unwrap())
}

async fn is_redacted(world: &World, id: &str) -> bool {
    world.raw(id).await["schema_version"] == "rsia.redacted.v1"
}

async fn run_exists(world: &World, id: &str) -> bool {
    let mut session = world.store.session().await.unwrap();
    let found = session
        .get::<Value>(&world.worker, "run", id)
        .await
        .unwrap()
        .is_some();
    session.commit().await.unwrap();
    found
}

#[tokio::test]
async fn revoking_a_source_only_one_group_read_redacts_that_groups_facts_and_keeps_the_others() {
    let world = World::new(&two_specs()).await;
    let (a, b) = (&world.groups[0], &world.groups[1]);
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let both = [a, b];
    let outcome = world.run(&model, &runner, &both).await.unwrap();
    assert!(
        outcome
            .groups
            .iter()
            .all(|group| group.kind == GroupOutcomeKind::Candidate)
    );
    let facts = world.stage_facts().await;
    let a_facts = facts_of(&facts, &a.episode_id);
    let b_facts = facts_of(&facts, &b.episode_id);
    let b_grant = world.raw(&grant_id(b)).await;
    for id in a_facts.keys().chain(b_facts.keys()) {
        assert!(!is_redacted(&world, id).await);
    }

    revoke_and_clean(&world, "a-failure", 100).await;

    for id in a_facts.keys() {
        assert!(
            is_redacted(&world, id).await,
            "{id} still carries group a content"
        );
    }
    assert!(is_redacted(&world, &grant_id(a)).await);
    for (id, before) in &b_facts {
        assert_eq!(&world.raw(id).await, before, "{id} changed");
    }
    assert_eq!(world.raw(&grant_id(b)).await, b_grant);
    assert!(!run_exists(&world, "a-failure").await);
    assert!(run_exists(&world, "b-failure").await);
    assert!(run_exists(&world, "shared-success").await);
    // Group b's record still ends in its accepted candidate.
    let terminal = b_facts
        .values()
        .find(|fact| fact["kind"] == "step_completed")
        .unwrap();
    assert_eq!(terminal["payload"]["status"], "candidate");
}

#[tokio::test]
async fn revoking_a_source_both_groups_read_invalidates_both_groups() {
    let world = World::new(&two_specs()).await;
    let (a, b) = (&world.groups[0], &world.groups[1]);
    let (model, runner) = (ScriptedModel::default(), ScriptedRunner::default());
    let both = [a, b];
    world.run(&model, &runner, &both).await.unwrap();
    let facts = world.stage_facts().await;
    let mut ids: Vec<String> = facts
        .iter()
        .map(|fact| fact["artifact_id"].as_str().unwrap().to_string())
        .collect();
    assert!(!ids.is_empty());
    ids.extend([grant_id(a), grant_id(b)]);

    revoke_and_clean(&world, "shared-success", 100).await;

    for id in &ids {
        assert!(
            is_redacted(&world, id).await,
            "{id} survived the shared revocation"
        );
    }
    assert!(!run_exists(&world, "shared-success").await);
    // The sources only one group read are not revoked by it.
    assert!(run_exists(&world, "a-failure").await);
    assert!(run_exists(&world, "b-failure").await);
}

// ---------------------------------------------------------------------------
// 6. One root budget, shared by both groups
// ---------------------------------------------------------------------------

async fn budget_world(
    total_limit_micros: i64,
) -> (
    World,
    PersistentModelBroker<EditingTransport>,
    Arc<AtomicUsize>,
) {
    let world = World::new(&two_specs()).await;
    world
        .store
        .authorize_root_budget(
            &world.admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec![NS.into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"admin-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        world.store.clone(),
        EditingTransport {
            calls: calls.clone(),
        },
        BrokerConfig {
            billing_scope: "scope-1".into(),
            actor: "trusted-broker".into(),
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(|| 10),
    )
    .unwrap();
    (world, broker, calls)
}

#[tokio::test]
async fn an_exhausted_root_budget_ends_the_affected_groups_in_a_terminal_state_and_keeps_finished_ones()
 {
    use GroupOutcomeKind::{Candidate, Rejected};
    // Each model call reserves 20 micros against the root limit and settles at 10;
    // a group needs two calls, so both groups need a limit of at least 50.
    for (limit, expected, transport_calls, spent, billed) in [
        (100, [Candidate, Candidate], 4, 40, [true, true]),
        (45, [Candidate, Rejected], 3, 30, [true, true]),
        (30, [Candidate, Rejected], 2, 20, [true, false]),
        (20, [Rejected, Rejected], 1, 10, [true, false]),
    ] {
        let (world, broker, transport) = budget_world(limit).await;
        let runner = ScriptedRunner::default();
        let both = [&world.groups[0], &world.groups[1]];
        let run = || {
            run_skill_group_job(
                Some(&broker),
                Some(&runner),
                Some(&world.journal),
                job(both),
            )
        };
        let outcome = run().await.unwrap();

        // Neither group is dropped, and the later one is told why it stopped.
        assert_eq!(
            outcome
                .groups
                .iter()
                .map(|group| (group.group_id.as_str(), group.kind))
                .collect::<Vec<_>>(),
            [("group-a", expected[0]), ("group-b", expected[1])],
            "limit {limit}"
        );
        for stopped in outcome.groups.iter().filter(|group| group.kind == Rejected) {
            // The group carries the closed class of its step: a model rejected for want
            // of budget, as a typed kind and not as the words of the refusal (AG-048).
            assert_eq!(
                stopped.terminal.map(|class| class.code()),
                Some("model_rejected_budget_unavailable"),
                "limit {limit}: {:?}",
                stopped.terminal
            );
        }
        // The marker lists exactly the groups that finished.
        let finished: Vec<&str> = outcome
            .groups
            .iter()
            .filter(|group| group.kind == Candidate)
            .map(|group| group.group_id.as_str())
            .collect();
        match &outcome.combined {
            Some(combined) => assert_eq!(
                combined
                    .groups
                    .iter()
                    .map(|group| group.group_id.as_str())
                    .collect::<Vec<_>>(),
                finished,
                "limit {limit}"
            ),
            None => assert!(finished.is_empty(), "limit {limit}"),
        }
        // Both groups drew on the one root budget, and every group has a terminal record.
        let root = world
            .store
            .root_budget(&world.admin, "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (root.spent_micros, root.reserved_micros),
            (spent, 0),
            "limit {limit}"
        );
        assert_eq!(transport.load(SeqCst), transport_calls, "limit {limit}");
        let facts = world.stage_facts().await;
        for (episode, billed) in ["episode-a", "episode-b"].into_iter().zip(billed) {
            let group_facts = facts_of(&facts, episode);
            assert!(
                fact_kinds(&group_facts).contains("step_completed"),
                "{episode}"
            );
            let drew_on_the_root = group_facts.values().any(|fact| {
                fact["kind"] == "response_observed"
                    && fact["dependencies"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|dependency| dependency["id"] == "root-1")
            });
            assert_eq!(drew_on_the_root, billed, "limit {limit} {episode}");
        }

        // Repeating the job neither dispatches nor bills again.
        let again = run().await.unwrap();
        assert_eq!(as_json(&again), as_json(&outcome), "limit {limit}");
        assert_eq!(transport.load(SeqCst), transport_calls, "limit {limit}");
        let root = world
            .store
            .root_budget(&world.admin, "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.spent_micros, spent, "limit {limit}");
    }
}

// ---------------------------------------------------------------------------
// 7. A marker for one group is still only a marker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_marker_for_a_single_candidate_group_still_demands_the_full_rerun_and_acceptance() {
    let baseline = clean_baseline().await;
    let world = World::new(&two_specs()).await;
    let model = ScriptedModel::default();
    let runner = ScriptedRunner::default().script("episode-a", RunnerScript::Regress);
    let both = [&world.groups[0], &world.groups[1]];
    let outcome = world.run(&model, &runner, &both).await.unwrap();
    assert_eq!(group(&outcome, "group-a").kind, GroupOutcomeKind::Rejected);
    assert_eq!(group(&outcome, "group-b").kind, GroupOutcomeKind::Candidate);

    let combined = outcome.combined.as_ref().unwrap();
    let only_b = vec![SkillGroupCandidate {
        group_id: "group-b".into(),
        candidate_digest: group(&outcome, "group-b")
            .candidate_bundle_digest
            .clone()
            .unwrap(),
    }];
    assert_eq!(combined.groups.len(), 1);
    assert_eq!(combined.combined_digest, fingerprint(&only_b).unwrap());
    assert!(combined.requires_full_development_rerun);
    assert!(combined.requires_full_formal_evaluation);
    // It is a different combination than the one that also adopts group a.
    assert_ne!(
        combined.combined_digest,
        baseline.combined.as_ref().unwrap().combined_digest
    );

    // A job that never contained group a names the same combination.
    let alone = world
        .run(&model, &runner, &[&world.groups[1]])
        .await
        .unwrap();
    assert_eq!(alone.groups.len(), 1);
    assert_eq!(
        serde_json::to_value(alone.combined.as_ref().unwrap()).unwrap(),
        serde_json::to_value(combined).unwrap()
    );

    // No group with a candidate, no marker.
    let world = World::new(&two_specs()).await;
    let runner = ScriptedRunner::default()
        .script("episode-a", RunnerScript::Regress)
        .script("episode-b", RunnerScript::Regress);
    let both = [&world.groups[0], &world.groups[1]];
    let none = world
        .run(&ScriptedModel::default(), &runner, &both)
        .await
        .unwrap();
    assert!(
        none.groups
            .iter()
            .all(|group| group.kind == GroupOutcomeKind::Rejected)
    );
    assert!(none.combined.is_none());
}

/// Why the job also refuses a request id shared by two groups: the broker's
/// ledger keys every model call by the request id alone, so the second group's
/// call would be read as a reuse of the first group's call id.
#[tokio::test]
async fn groups_that_share_a_request_id_would_reuse_the_brokers_call_id() {
    let (world, broker, transport) = budget_world(100).await;
    let a = &world.groups[0];
    let mut b = build(&Spec::new("b", "b-failure"));
    b.model_context.request_id = a.model_context.request_id.clone();
    let runner = ScriptedRunner::default();
    let first = run_optimization_step(
        Some(&broker),
        Some(&runner),
        Some(&world.journal),
        a.request(),
    )
    .await
    .unwrap();
    assert!(matches!(first, OptimizationStepOutcome::Candidate { .. }));
    let calls_after_first = transport.load(SeqCst);
    let second = run_optimization_step(
        Some(&broker),
        Some(&runner),
        Some(&world.journal),
        b.request(),
    )
    .await
    .unwrap();
    // The broker refuses the reused call id with a conflict that reaches the step as an
    // error of the model port: the second group ends uncertain, and its outcome carries
    // the fixed class of a transport outcome that is unknown, not the port's own words
    // (AG-048).
    match second {
        OptimizationStepOutcome::Uncertain { class } => {
            assert_eq!(class, StepTerminalClass::ModelTransportOutcomeUnknown)
        }
        other => panic!("the second group must not proceed: {other:?}"),
    }
    assert_eq!(transport.load(SeqCst), calls_after_first);
}
