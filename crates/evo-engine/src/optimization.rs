//! Development-only optimizer orchestration ports and deterministic selection.

use async_trait::async_trait;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, ResolvedBundle, SkillSnapshot,
    compile_bundle,
};
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    EditSuggestion, MAX_FINAL_EDITS, MAX_SUGGESTIONS, ModelInputPart, ModelInputRole, ModelRequest,
    ModelRequestContext, ModelStage, OptimizationTrace, ReflectionBatch, ReflectionBatchKind,
    TrustedSourceBinding, build_reflection_batches, merge_suggestions, validate_suggestion_pool,
};
use evo_core::skill_edit::{
    CompiledSkillEdit, ProtectedTextRange, SkillEditBatch, TrustedEditContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, identifier};
use evo_storage::lifecycle::{RevokeTombstone, redacted_object_body};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::compiler::compile_skill_edits;
use crate::model::{ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse};

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!("{name} must be a lowercase sha256")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentExecutionProvenance {
    Fixture,
    /// Reserved for a future sandboxed runner; nothing issues it today.
    IsolatedRunner,
    /// Registered pure-function adapters executed in-process by the fixed
    /// trusted runner (`crate::development`). Not isolated, not a sandbox.
    RegisteredPureFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentTask {
    pub id: String,
    pub parent_family: String,
    pub input_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentManifest {
    pub id: String,
    pub tasks: Vec<DevelopmentTask>,
    pub digest: String,
}

impl DevelopmentManifest {
    pub fn build(id: impl Into<String>, tasks: Vec<DevelopmentTask>) -> Result<Self> {
        let id = id.into();
        identifier(&id)?;
        if tasks.is_empty() {
            return Err(Error::Invalid("development manifest is empty".into()));
        }
        let mut ids = BTreeSet::new();
        for task in &tasks {
            identifier(&task.id)?;
            identifier(&task.parent_family)?;
            validate_digest(&task.input_digest, "development task input digest")?;
            if !ids.insert(task.id.as_str()) {
                return Err(Error::Conflict("duplicate development task".into()));
            }
        }
        let digest = fingerprint(&(id.as_str(), tasks.as_slice()))?;
        Ok(Self { id, tasks, digest })
    }

    pub fn validate(&self) -> Result<()> {
        let rebuilt = Self::build(self.id.clone(), self.tasks.clone())?;
        if rebuilt.digest != self.digest {
            return Err(Error::Conflict(
                "development manifest digest mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentRunRequest {
    pub request_id: String,
    pub namespace: String,
    pub purpose: Purpose,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub manifest: DevelopmentManifest,
    pub parent_bundle_digest: String,
    pub candidate_bundle_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub rules_digest: String,
    pub tools_digest: String,
    pub revoke_watermark: u64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTaskResult {
    pub task_id: String,
    pub parent_score_micros: u32,
    pub candidate_score_micros: u32,
    pub parent_passed: bool,
    pub candidate_passed: bool,
    pub parent_execution_id: String,
    pub candidate_execution_id: String,
    pub grader_receipt_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentRunReport {
    pub request_id: String,
    pub manifest_digest: String,
    pub parent_bundle_digest: String,
    pub candidate_bundle_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub results: Vec<PairedTaskResult>,
    pub execution_receipt_id: String,
    pub usage_record_ids: Vec<String>,
    pub provenance: DevelopmentExecutionProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentSelectionDecision {
    AcceptCandidate,
    KeepIncumbent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentSelection {
    pub request_id: String,
    pub manifest_digest: String,
    pub parent_bundle_digest: String,
    pub candidate_bundle_digest: String,
    pub decision: DevelopmentSelectionDecision,
    pub parent_total_micros: u64,
    pub candidate_total_micros: u64,
    pub reason: String,
}

pub fn select_development(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
) -> Result<DevelopmentSelection> {
    select_development_detailed(request, report).map(|(selection, _)| selection)
}

/// [`select_development`], and whether the candidate kept every task the incumbent
/// passed. The step needs the second to tell the two ways a selection keeps the
/// incumbent apart without reading the sentence of the selection.
fn select_development_detailed(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
) -> Result<(DevelopmentSelection, bool)> {
    validate_development_binding(request, report)?;
    let result_by_id: BTreeMap<&str, &PairedTaskResult> = report
        .results
        .iter()
        .map(|result| (result.task_id.as_str(), result))
        .collect();
    if result_by_id.len() != request.manifest.tasks.len() {
        return Err(Error::Invalid(
            "development report task list is incomplete".into(),
        ));
    }
    let mut parent_total_micros = 0u64;
    let mut candidate_total_micros = 0u64;
    let mut preserves_passes = true;
    for task in &request.manifest.tasks {
        let result = result_by_id.get(task.id.as_str()).ok_or_else(|| {
            Error::Invalid("development report does not match the full manifest".into())
        })?;
        if result.parent_score_micros > 1_000_000 || result.candidate_score_micros > 1_000_000 {
            return Err(Error::Invalid(
                "development score micros must be within 0..=1000000".into(),
            ));
        }
        identifier(&result.parent_execution_id)?;
        identifier(&result.candidate_execution_id)?;
        validate_digest(&result.grader_receipt_digest, "grader receipt digest")?;
        parent_total_micros = parent_total_micros
            .checked_add(u64::from(result.parent_score_micros))
            .ok_or_else(|| Error::Invalid("parent development total overflow".into()))?;
        candidate_total_micros = candidate_total_micros
            .checked_add(u64::from(result.candidate_score_micros))
            .ok_or_else(|| Error::Invalid("candidate development total overflow".into()))?;
        preserves_passes &= !result.parent_passed || result.candidate_passed;
    }
    let improved = candidate_total_micros > parent_total_micros && preserves_passes;
    Ok((
        DevelopmentSelection {
            request_id: request.request_id.clone(),
            manifest_digest: request.manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            decision: if improved {
                DevelopmentSelectionDecision::AcceptCandidate
            } else {
                DevelopmentSelectionDecision::KeepIncumbent
            },
            parent_total_micros,
            candidate_total_micros,
            reason: if improved {
                "strictly improved the same full development manifest and preserved passes".into()
            } else {
                "candidate did not strictly improve while preserving all incumbent passes".into()
            },
        },
        preserves_passes,
    ))
}

fn validate_development_binding(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
) -> Result<()> {
    validate_development_request(request)?;
    if request.purpose != Purpose::Development {
        return Err(Error::Forbidden);
    }
    for value in [
        &request.request_id,
        &request.namespace,
        &request.episode_id,
        &request.idempotency_key,
        &report.execution_receipt_id,
    ] {
        identifier(value)?;
    }
    for (value, name) in [
        (&request.parent_bundle_digest, "parent bundle digest"),
        (&request.candidate_bundle_digest, "candidate bundle digest"),
        (&request.environment_digest, "environment digest"),
        (&request.grader_digest, "grader digest"),
        (&request.rules_digest, "rules digest"),
        (&request.tools_digest, "tools digest"),
    ] {
        validate_digest(value, name)?;
    }
    if request.request_id != report.request_id
        || request.manifest.digest != report.manifest_digest
        || request.parent_bundle_digest != report.parent_bundle_digest
        || request.candidate_bundle_digest != report.candidate_bundle_digest
        || request.environment_digest != report.environment_digest
        || request.grader_digest != report.grader_digest
    {
        return Err(Error::Conflict(
            "development report binding mismatch".into(),
        ));
    }
    let expected_order: Vec<&str> = request
        .manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect();
    let observed_order: Vec<&str> = report
        .results
        .iter()
        .map(|result| result.task_id.as_str())
        .collect();
    if observed_order != expected_order {
        return Err(Error::Conflict(
            "development report task order differs from frozen manifest".into(),
        ));
    }
    for usage in &report.usage_record_ids {
        identifier(usage)?;
    }
    Ok(())
}

pub(crate) fn validate_development_request(request: &DevelopmentRunRequest) -> Result<()> {
    if request.purpose != Purpose::Development {
        return Err(Error::Forbidden);
    }
    request.manifest.validate()?;
    for value in [
        &request.request_id,
        &request.namespace,
        &request.episode_id,
        &request.idempotency_key,
    ] {
        identifier(value)?;
    }
    for (value, name) in [
        (&request.parent_bundle_digest, "parent bundle digest"),
        (&request.candidate_bundle_digest, "candidate bundle digest"),
        (&request.environment_digest, "environment digest"),
        (&request.grader_digest, "grader digest"),
        (&request.rules_digest, "rules digest"),
        (&request.tools_digest, "tools digest"),
    ] {
        validate_digest(value, name)?;
    }
    Ok(())
}

#[async_trait]
pub trait DevRunner: Send + Sync {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport>;
}

#[derive(Debug, Clone, Serialize)]
pub enum SuggestionOutcome {
    NoChange {
        response_id: String,
        output: String,
        output_digest: String,
        receipt: ModelExecutionReceipt,
    },
    Suggestions {
        items: Vec<EditSuggestion>,
        response_id: String,
        output: String,
        output_digest: String,
        receipt: ModelExecutionReceipt,
    },
    /// The provider rejected the request. Only the typed kind of the rejection is kept:
    /// the words that came with it are the provider's, not the engine's.
    Rejected {
        kind: ModelRejectionKind,
        dependency_ids: Vec<String>,
    },
    Uncertain {
        dependency_ids: Vec<String>,
    },
}

pub async fn request_suggestions(
    port: Option<&dyn ModelPort>,
    request: ModelRequest,
    batches: &[ReflectionBatch],
) -> Result<SuggestionOutcome> {
    suggestion_outcome(port, request, batches)
        .await?
        // The text of the parser's error is not kept: it echoes the answer.
        .ok_or_else(|| {
            Error::Invalid("optimizer suggestions are not a valid suggestion list".into())
        })
}

/// [`request_suggestions`], with the one verdict that is no outcome and no error: `None`
/// when the answer is complete and is not a suggestion list (it does not parse as one,
/// or it holds more suggestions than the request allowed). The step records that exit
/// as a class of its own, and writes no fact for it, as it never did.
async fn suggestion_outcome(
    port: Option<&dyn ModelPort>,
    request: ModelRequest,
    batches: &[ReflectionBatch],
) -> Result<Option<SuggestionOutcome>> {
    request.validate()?;
    validate_model_source_scope(&request, batches)?;
    let port = port.ok_or(Error::NotFound)?;
    let response = port.dispatch(request.clone()).await?;
    response.validate_against(&request)?;
    let (response_id, output, output_digest, execution_receipt) = match response {
        ModelResponse::Completed {
            response_id,
            output,
            output_digest,
            execution_receipt,
            ..
        } => (response_id, output, output_digest, execution_receipt),
        ModelResponse::Rejected { kind, dispatch, .. } => {
            let dependency_ids = match dispatch {
                crate::model::RejectedDispatch::NotDispatched => vec![],
                crate::model::RejectedDispatch::Dispatched { receipt } => vec![
                    receipt.call_id,
                    receipt.dispatch_id,
                    receipt.root_budget_id,
                    receipt.provider_request_id,
                    receipt.usage_record_id,
                ],
            };
            return Ok(Some(SuggestionOutcome::Rejected {
                kind,
                dependency_ids,
            }));
        }
        ModelResponse::Uncertain {
            dispatch_id,
            root_budget_id,
            provider_request_id,
            usage_record_id,
            ..
        } => {
            let mut dependency_ids = vec![dispatch_id, root_budget_id];
            dependency_ids.extend(provider_request_id);
            dependency_ids.extend(usage_record_id);
            return Ok(Some(SuggestionOutcome::Uncertain { dependency_ids }));
        }
    };
    let Ok(suggestions) = serde_json::from_str::<Vec<EditSuggestion>>(&output) else {
        return Ok(None);
    };
    if suggestions.is_empty() {
        return Ok(Some(SuggestionOutcome::NoChange {
            response_id,
            output,
            output_digest,
            receipt: execution_receipt,
        }));
    }
    if suggestions.len() > request.max_suggestions || suggestions.len() > MAX_SUGGESTIONS {
        return Ok(None);
    }
    validate_suggestion_pool(batches, &suggestions)?;
    Ok(Some(SuggestionOutcome::Suggestions {
        items: suggestions,
        response_id,
        output,
        output_digest,
        receipt: execution_receipt,
    }))
}

fn validate_model_source_scope(request: &ModelRequest, batches: &[ReflectionBatch]) -> Result<()> {
    if batches.is_empty() {
        return Err(Error::Invalid(
            "model request has no reflection batch".into(),
        ));
    }
    let request_sources: BTreeSet<_> = request.source_closure.iter().cloned().collect();
    let batch_sources: BTreeSet<_> = batches
        .iter()
        .flat_map(|batch| batch.source_closure.iter().cloned())
        .collect();
    if request_sources != batch_sources {
        return Err(Error::Conflict(
            "model request source closure differs from reflection batches".into(),
        ));
    }
    Ok(())
}

pub async fn run_development(
    runner: Option<&dyn DevRunner>,
    request: DevelopmentRunRequest,
) -> Result<DevelopmentSelection> {
    validate_development_request(&request)?;
    let runner = runner.ok_or(Error::NotFound)?;
    let report = runner.run(request.clone()).await?;
    select_development(&request, &report)
}

pub const OPTIMIZATION_STAGE_FACT_SCHEMA: &str = "rsia.optimization.stage_fact.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageFactKind {
    RequestPrepared,
    ResponseObserved,
    EditCompiled,
    DevelopmentRequestPrepared,
    DevelopmentObserved,
    TerminalNoChange,
    TerminalRejected,
    TerminalCandidate,
    StepPrepared,
    StepCompleted,
    DispatchPrepared,
    DispatchObserved,
    TerminalUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationJournalStage {
    ReflectFailure,
    ReflectSuccess,
    Merge,
    Rank,
    EditCompile,
    Development,
    /// The model-call facts of a consolidation step (`ModelStage::Consolidate`).
    /// The step's own `StepPrepared`/`StepCompleted` facts stay under `Merge`.
    Consolidate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageFact {
    pub schema_version: String,
    pub artifact_id: String,
    pub namespace: String,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub stage: OptimizationJournalStage,
    pub kind: StageFactKind,
    pub request_id: String,
    pub input_digest: String,
    pub output_digest: Option<String>,
    pub dependencies: Vec<StageDependency>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageDependency {
    pub kind: String,
    pub id: String,
}

fn dependency(kind: &str, id: String) -> StageDependency {
    StageDependency {
        kind: kind.into(),
        id,
    }
}

impl StageFact {
    pub fn seal(mut self) -> Result<Self> {
        self.artifact_id = canonical_stage_artifact_id(&self)?;
        self.output_digest = Some(fingerprint(&self.payload)?);
        self.validate()?;
        Ok(self)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != OPTIMIZATION_STAGE_FACT_SCHEMA {
            return Err(Error::Invalid(
                "unsupported optimization stage fact schema".into(),
            ));
        }
        for value in [
            &self.artifact_id,
            &self.namespace,
            &self.episode_id,
            &self.request_id,
        ] {
            identifier(value)?;
        }
        validate_digest(&self.input_digest, "stage input digest")?;
        if let Some(output) = &self.output_digest {
            validate_digest(output, "stage output digest")?;
        }
        for dependency in &self.dependencies {
            identifier(&dependency.kind)?;
            identifier(&dependency.id)?;
        }
        if self.output_digest.as_deref() != Some(fingerprint(&self.payload)?.as_str()) {
            return Err(Error::Conflict("stage payload digest mismatch".into()));
        }
        if self.payload.is_null() {
            return Err(Error::Invalid(
                "stage fact requires its typed payload".into(),
            ));
        }
        if self.artifact_id != canonical_stage_artifact_id(self)? {
            return Err(Error::Conflict("non-canonical stage artifact id".into()));
        }
        Ok(())
    }
}

fn canonical_stage_artifact_id(fact: &StageFact) -> Result<String> {
    Ok(format!(
        "optstage-{}",
        fingerprint(&(
            &fact.namespace,
            &fact.episode_id,
            fact.step,
            fact.attempt,
            fact.stage,
            fact.kind,
            &fact.request_id,
        ))?
    ))
}

/// Canonical id of the `DevelopmentRequestPrepared` fact that pairs with a
/// `DevelopmentObserved` fact. E03 stores the two as one Development stage, so
/// namespace, episode, step, attempt and request id are shared and only the kind
/// differs. A consumer handed just the observation (E13's cycle close) names the
/// request half to the observation gate from this, never from the caller.
pub(crate) fn development_request_fact_id(observed: &StageFact) -> Result<String> {
    if observed.stage != OptimizationJournalStage::Development
        || observed.kind != StageFactKind::DevelopmentObserved
    {
        return Err(Error::Invalid(
            "a development request pairs with a DevelopmentObserved fact".into(),
        ));
    }
    canonical_stage_artifact_id(&StageFact {
        kind: StageFactKind::DevelopmentRequestPrepared,
        ..observed.clone()
    })
}

/// Canonical ids `(request_fact_id, observed_fact_id)` of the
/// `DevelopmentRequestPrepared` / `DevelopmentObserved` pair that
/// `run_optimization_step` journals for the development stage of `context` and
/// `development_request` (the request as the caller built it, before the step
/// rewrites its identity). A consumer that holds only the step's request, such as
/// the exploration coordinator, names the two facts to the E03 observation gate
/// from this, never from the runner's report.
///
/// It mirrors, without sharing code with it, the request-id rewrite in
/// `run_optimization_step_inner` (`optdev-` and a fingerprint of the scope, the
/// step's request id and the caller's development request id) and the canonical id
/// of `StageFact`. If the step ever derives either differently, the gate finds no
/// fact and refuses the evidence: it fails closed, and the trusted-path tests of
/// the coordinator fail with it.
pub(crate) fn development_stage_fact_ids(
    context: &ModelRequestContext,
    development_request: &DevelopmentRunRequest,
) -> Result<(String, String)> {
    let request_id = format!(
        "optdev-{}",
        fingerprint(&(
            &context.namespace,
            &context.episode_id,
            context.step,
            context.attempt,
            &context.request_id,
            &development_request.request_id
        ))?
    );
    let observed = StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: context.namespace.clone(),
        episode_id: context.episode_id.clone(),
        step: context.step,
        attempt: context.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id,
        input_digest: String::new(),
        output_digest: None,
        dependencies: vec![],
        payload: serde_json::Value::Null,
    };
    Ok((
        development_request_fact_id(&observed)?,
        canonical_stage_artifact_id(&observed)?,
    ))
}

#[async_trait]
pub trait OptimizationJournal: Send + Sync {
    /// Must commit one typed artifact and all dependency edges atomically.
    /// The implementation must return only after the transaction commits.
    async fn commit(&self, fact: StageFact) -> Result<()>;
    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>>;
    async fn claim(&self, fact: StageFact) -> Result<bool>;
    async fn verify_sources(
        &self,
        selection: &evo_core::evidence::SourceSelection,
        evidence: &evo_core::evidence::EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> Result<()>;
    async fn check_live(&self, source_ids: &[String], watermark: u64) -> Result<()>;
}

#[derive(Clone)]
pub struct StoreOptimizationJournal {
    store: Store,
    context: Context,
    owner: String,
}

pub use crate::development::VerifiedDevelopmentTaskOutcome;

/// A DevelopmentObserved report whose receipts, budget rows, scores, and
/// source closure were reloaded and verified. Never built from caller JSON.
#[derive(Debug, Clone)]
pub struct VerifiedDevelopmentObservationView {
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub request_id: String,
    pub manifest_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub outcomes: Vec<VerifiedDevelopmentTaskOutcome>,
}

/// E03 observation gate shared by E12 (`record_cycle`) and other consumers.
///
/// The two stage facts must form one Development stage; the report must pass
/// `select_development`; the fact's `execution`/`grader` dependencies must
/// equal the report's receipt ids (other dependency kinds such as `run`,
/// `revoke_watermark`, and `artifact` are added by the recovery journal and
/// are checked separately, not counted here); then every receipt is reloaded
/// as a typed envelope and verified against the Admin-registered control, its
/// persisted budget rows, server-recomputed scores, and the live source
/// closure. `report.provenance` is non-authoritative: Fixture is rejected
/// outright, and any other value grants nothing by itself.
pub(crate) async fn verified_development_observation_in_session(
    ctx: &Context,
    session: &mut Session,
    request_fact_id: &str,
    observed_fact_id: &str,
) -> Result<VerifiedDevelopmentObservationView> {
    ctx.require(&[Role::Admin, Role::Worker])?;
    for id in [request_fact_id, observed_fact_id] {
        identifier(id)?;
    }
    let request_fact = load_stage_fact(session, ctx, request_fact_id).await?;
    let observed_fact = load_stage_fact(session, ctx, observed_fact_id).await?;
    request_fact.validate()?;
    observed_fact.validate()?;
    if request_fact.stage != OptimizationJournalStage::Development
        || request_fact.kind != StageFactKind::DevelopmentRequestPrepared
        || observed_fact.stage != OptimizationJournalStage::Development
        || observed_fact.kind != StageFactKind::DevelopmentObserved
        || request_fact.namespace != ctx.namespace()
        || observed_fact.namespace != ctx.namespace()
        || request_fact.episode_id != observed_fact.episode_id
        || request_fact.step != observed_fact.step
        || request_fact.attempt != observed_fact.attempt
    {
        return Err(Error::Conflict(
            "development request/observation facts do not form one stage".into(),
        ));
    }
    let request: DevelopmentRunRequest = serde_json::from_value(request_fact.payload.clone())
        .map_err(|_| Error::Invalid("development request fact payload is invalid".into()))?;
    let report: DevelopmentRunReport = serde_json::from_value(observed_fact.payload.clone())
        .map_err(|_| Error::Invalid("development report fact payload is invalid".into()))?;
    let _selection = select_development(&request, &report)?;
    if request_fact.request_id != request.request_id
        || observed_fact.request_id != request.request_id
        || request_fact.input_digest != request.manifest.digest
        || observed_fact.input_digest != request.manifest.digest
    {
        return Err(Error::Conflict(
            "development facts differ from request/report identity".into(),
        ));
    }
    let expected_dependencies = report
        .results
        .iter()
        .flat_map(|result| {
            [
                ("execution", result.parent_execution_id.as_str()),
                ("execution", result.candidate_execution_id.as_str()),
                ("grader", result.grader_receipt_digest.as_str()),
            ]
        })
        .collect::<std::collections::BTreeSet<_>>();
    let actual_dependencies = observed_fact
        .dependencies
        .iter()
        .filter(|dependency| matches!(dependency.kind.as_str(), "execution" | "grader"))
        .map(|dependency| (dependency.kind.as_str(), dependency.id.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    if actual_dependencies != expected_dependencies {
        return Err(Error::Conflict(
            "development observation dependency closure differs from report".into(),
        ));
    }
    if observed_fact.dependencies.iter().any(|dependency| {
        matches!(
            dependency.kind.as_str(),
            "formal" | "holdout" | "oracle" | "anchor" | "evaluation"
        )
    }) {
        return Err(Error::Forbidden);
    }
    // A self-declared fixture is honest and can never become cycle evidence.
    // Any other declared provenance is checked, not trusted: the receipts,
    // budget rows, scores, and sources are reloaded below.
    if report.provenance == DevelopmentExecutionProvenance::Fixture {
        return Err(Error::Forbidden);
    }
    let outcomes = crate::development::verify_observed_report_in_session(
        ctx,
        session,
        &observed_fact,
        &request,
        &report,
    )
    .await?;
    if outcomes.len() != report.results.len() {
        return Err(Error::Internal);
    }
    Ok(VerifiedDevelopmentObservationView {
        episode_id: observed_fact.episode_id,
        step: observed_fact.step,
        attempt: observed_fact.attempt,
        request_id: request.request_id,
        manifest_digest: request.manifest.digest,
        environment_digest: request.environment_digest,
        grader_digest: request.grader_digest,
        outcomes,
    })
}

/// A redacted or untyped artifact is a clean verdict, not a storage failure.
async fn load_stage_fact(session: &mut Session, ctx: &Context, id: &str) -> Result<StageFact> {
    let value: serde_json::Value = session.need(ctx, "artifact", id).await?;
    if value.get("schema_version").and_then(|value| value.as_str())
        != Some(OPTIMIZATION_STAGE_FACT_SCHEMA)
    {
        return Err(Error::Conflict(
            "development stage fact is redacted or untyped".into(),
        ));
    }
    serde_json::from_value(value)
        .map_err(|_| Error::Invalid("development stage fact is not a strict stage fact".into()))
}

/// The schema of the tombstone the revocation cleanup leaves in place of an object.
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// Reads the stored body of the stage fact `id`.
///
/// After a source revocation the cleanup replaces the body with an
/// `rsia.redacted.v1` tombstone, which is not a stage fact, and a fact written
/// after the revocation is stored as one (`StoreOptimizationJournal::commit_redacted`).
/// That is the expected state of a revoked step, not corruption, so the read names
/// it: a `Conflict` with a fixed message, the verdict the exploration, curriculum and
/// replay stores give a redacted record of theirs (`read_envelope`, `get_record`),
/// instead of a decode failure that would surface as `Internal`. The body is
/// inspected before it is decoded, so the expected state does not raise the storage
/// layer's "database operation failed" error log either. Any other body that does
/// not decode is corruption and stays `Internal`.
async fn read_stage_fact(
    session: &mut Session,
    context: &Context,
    id: &str,
) -> Result<Option<StageFact>> {
    let Some(body) = session
        .get::<serde_json::Value>(context, "artifact", id)
        .await?
    else {
        return Ok(None);
    };
    if body.get("schema_version").and_then(|value| value.as_str()) == Some(REDACTED_SCHEMA) {
        return Err(Error::Conflict(format!(
            "optimization stage fact {id} was redacted because its source was revoked"
        )));
    }
    match serde_json::from_value(body) {
        Ok(fact) => Ok(Some(fact)),
        Err(error) => {
            tracing::error!(%error, id, "stored optimization stage fact does not decode");
            Err(Error::Internal)
        }
    }
}

/// Public entry to the E03 observation gate for consumers that do not hold a
/// session (E13 and operators). Same rules as the in-session gate.
pub async fn verify_development_observation(
    store: &Store,
    ctx: &Context,
    request_fact_id: &str,
    observed_fact_id: &str,
) -> Result<VerifiedDevelopmentObservationView> {
    let mut session = store.session().await?;
    let view = verified_development_observation_in_session(
        ctx,
        &mut session,
        request_fact_id,
        observed_fact_id,
    )
    .await?;
    session.commit().await?;
    Ok(view)
}

#[cfg(test)]
mod verified_development_tests {
    use super::*;
    use evo_core::hash;

    #[tokio::test]
    async fn fixture_development_facts_cannot_become_verified_cycle_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("verified-development.sqlite3"))
            .await
            .unwrap();
        let context = Context::new("n", "admin", Role::Admin).unwrap();
        let manifest = DevelopmentManifest::build(
            "manifest",
            vec![DevelopmentTask {
                id: "task".into(),
                parent_family: "family".into(),
                input_digest: hash(b"task"),
            }],
        )
        .unwrap();
        let request = DevelopmentRunRequest {
            request_id: "development-request".into(),
            namespace: "n".into(),
            purpose: Purpose::Development,
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            manifest: manifest.clone(),
            parent_bundle_digest: hash(b"parent"),
            candidate_bundle_digest: hash(b"candidate"),
            environment_digest: hash(b"environment"),
            grader_digest: hash(b"grader"),
            rules_digest: hash(b"rules"),
            tools_digest: hash(b"tools"),
            revoke_watermark: 1,
            idempotency_key: "development-idempotency".into(),
        };
        let report = DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results: vec![PairedTaskResult {
                task_id: "task".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: 600_000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: "parent-execution".into(),
                candidate_execution_id: "candidate-execution".into(),
                grader_receipt_digest: hash(b"grade"),
            }],
            execution_receipt_id: "fixture-execution".into(),
            usage_record_ids: vec!["fixture-usage".into()],
            provenance: DevelopmentExecutionProvenance::Fixture,
        };
        let request_fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: "n".into(),
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentRequestPrepared,
            request_id: request.request_id.clone(),
            input_digest: manifest.digest.clone(),
            output_digest: None,
            dependencies: vec![],
            payload: serde_json::to_value(&request).unwrap(),
        }
        .seal()
        .unwrap();
        let observed_fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: "n".into(),
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id: request.request_id.clone(),
            input_digest: manifest.digest,
            output_digest: None,
            dependencies: vec![
                StageDependency {
                    kind: "execution".into(),
                    id: "parent-execution".into(),
                },
                StageDependency {
                    kind: "execution".into(),
                    id: "candidate-execution".into(),
                },
                StageDependency {
                    kind: "grader".into(),
                    id: hash(b"grade"),
                },
            ],
            payload: serde_json::to_value(report).unwrap(),
        }
        .seal()
        .unwrap();
        let mut session = store.session().await.unwrap();
        for fact in [&request_fact, &observed_fact] {
            session
                .put(&context, "artifact", &fact.artifact_id, "admin", fact)
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let mut session = store.session().await.unwrap();
        assert!(matches!(
            verified_development_observation_in_session(
                &context,
                &mut session,
                &request_fact.artifact_id,
                &observed_fact.artifact_id,
            )
            .await,
            Err(Error::Forbidden)
        ));
    }
}

impl StoreOptimizationJournal {
    pub fn new(store: Store, context: Context, owner: impl Into<String>) -> Result<Self> {
        context.require(&[Role::Worker, Role::Admin])?;
        let owner = owner.into();
        identifier(&owner)?;
        Ok(Self {
            store,
            context,
            owner,
        })
    }

    pub async fn reload(&self, artifact_id: &str) -> Result<StageFact> {
        self.lookup(artifact_id).await?.ok_or(Error::NotFound)
    }

    /// Stores `fact` as the `rsia.redacted.v1` object the revocation cleanup would
    /// have made of it (`redacted_object_body`, the one construction), and nothing
    /// of its payload. The dependency edges are kept, so the fact stays on the
    /// closure of the runs it depends on. The idempotency row is written for the
    /// same key and the same digest and is redacted at once: the same fact is
    /// refused if it is committed again (`cached` refuses a redacted subject), and
    /// no stored response holds its content. `session` is the transaction the
    /// decision was taken in.
    async fn commit_redacted(&self, mut session: Session, fact: &StageFact) -> Result<()> {
        let value = serde_json::to_value(fact).map_err(|_| Error::Internal)?;
        let body = serde_json::to_string(fact).map_err(|_| Error::Internal)?;
        let redacted = redacted_object_body("artifact", &fact.artifact_id, &value, &body);
        session
            .put(
                &self.context,
                "artifact",
                &fact.artifact_id,
                &self.owner,
                &redacted,
            )
            .await?;
        for dependency in &fact.dependencies {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &fact.artifact_id,
                    &dependency.kind,
                    &dependency.id,
                )
                .await?;
        }
        session
            .cache(
                &self.context,
                "optimization.stage",
                &fact.artifact_id,
                fact,
                &fact.artifact_id,
                &serde_json::Value::Null,
            )
            .await?;
        session
            .redact_cache(&self.context, &fact.artifact_id)
            .await?;
        session
            .audit(
                &self.context,
                "optimization.stage.commit_redacted",
                &fact.artifact_id,
            )
            .await?;
        session.commit().await
    }
}

#[async_trait]
impl OptimizationJournal for StoreOptimizationJournal {
    async fn claim(&self, fact: StageFact) -> Result<bool> {
        fact.validate()?;
        if fact.namespace != self.context.namespace() {
            return Err(Error::Forbidden);
        }
        let mut session = self.store.session().await?;
        validate_live_fact(&mut session, &self.context, &fact).await?;
        if let Some(old) = session
            .get::<StageFact>(&self.context, "artifact", &fact.artifact_id)
            .await?
        {
            old.validate()?;
            if fingerprint(&old)? != fingerprint(&fact)? {
                return Err(Error::Conflict("prepared claim differs".into()));
            }
            session.commit().await?;
            return Ok(false);
        }
        // A redacted/deleted idempotency subject must never become a fresh dispatch.
        if session
            .cached::<StageFact, _>(
                &self.context,
                "optimization.stage",
                &fact.artifact_id,
                &fact,
            )
            .await?
            .is_some()
        {
            return Err(Error::Conflict("prepared subject missing".into()));
        }
        session
            .put(
                &self.context,
                "artifact",
                &fact.artifact_id,
                &self.owner,
                &fact,
            )
            .await?;
        for d in &fact.dependencies {
            session
                .put_edge(&self.context, "artifact", &fact.artifact_id, &d.kind, &d.id)
                .await?;
        }
        session
            .cache(
                &self.context,
                "optimization.stage",
                &fact.artifact_id,
                &fact,
                &fact.artifact_id,
                &fact,
            )
            .await?;
        session
            .audit(&self.context, "optimization.stage.claim", &fact.artifact_id)
            .await?;
        session.commit().await?;
        Ok(true)
    }
    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>> {
        let mut session = self.store.session().await?;
        let fact = read_stage_fact(&mut session, &self.context, artifact_id).await?;
        session.commit().await?;
        if let Some(fact) = &fact {
            fact.validate()?;
            if fact.namespace != self.context.namespace() {
                return Err(Error::Forbidden);
            }
            let sources = fact
                .dependencies
                .iter()
                .filter(|d| d.kind == "run")
                .map(|d| d.id.clone())
                .collect::<Vec<_>>();
            if let Some(w) = fact
                .dependencies
                .iter()
                .find(|d| d.kind == "revoke_watermark")
            {
                self.check_live(&sources, w.id.parse().map_err(|_| Error::Internal)?)
                    .await?;
            }
        }
        Ok(fact)
    }
    async fn check_live(&self, source_ids: &[String], watermark: u64) -> Result<()> {
        let mut session = self.store.session().await?;
        crate::evidence::validate_stored_sources(
            &mut session,
            &self.context,
            source_ids,
            watermark,
        )
        .await?;
        session.commit().await
    }
    async fn verify_sources(
        &self,
        selection: &evo_core::evidence::SourceSelection,
        evidence: &evo_core::evidence::EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> Result<()> {
        validate_outbound_grant(selection)?;
        self.check_live(&selection.run_ids, watermark).await?;
        let mut session = self.store.session().await?;
        let grant: evo_core::evidence::SourceSelection = session
            .need(
                &self.context,
                "artifact",
                &format!("optgrant-{}", fingerprint(selection)?),
            )
            .await?;
        if fingerprint(&grant)? != fingerprint(selection)? {
            return Err(Error::Forbidden);
        }
        let mut records = Vec::new();
        for id in &selection.run_ids {
            let stored =
                crate::evidence::load_stored_source(&mut session, &self.context, id).await?;
            let binding = bindings
                .iter()
                .find(|b| &b.source_id == id)
                .ok_or(Error::Forbidden)?;
            if binding.source_digest != stored.trace.source_digest
                || binding.parent_family != stored.record.parent_family
            {
                return Err(Error::Forbidden);
            }
            for trace in traces.iter().filter(|t| &t.run_id == id) {
                if fingerprint(trace)? != fingerprint(&stored.trace)? {
                    return Err(Error::Forbidden);
                }
            }
            records.push(stored.record);
        }
        if traces
            .iter()
            .any(|t| !selection.run_ids.contains(&t.run_id))
        {
            return Err(Error::Forbidden);
        }
        let actual = crate::evidence::ingest_trusted_run_records(selection, &records)?;
        if fingerprint(&actual.members)? != fingerprint(&evidence.members)?
            || actual.independent_clusters != evidence.independent_clusters
            || fingerprint(&actual.coverage)? != fingerprint(&evidence.coverage)?
        {
            return Err(Error::Forbidden);
        }
        session.commit().await
    }
    async fn commit(&self, fact: StageFact) -> Result<()> {
        fact.validate()?;
        self.context.require(&[Role::Worker, Role::Admin])?;
        if fact.namespace != self.context.namespace() {
            return Err(Error::Forbidden);
        }
        let key = fact.artifact_id.clone();
        let mut session = self.store.session().await?;
        validate_live_fact(&mut session, &self.context, &fact).await?;
        if session
            .cached::<StageFact, _>(&self.context, "optimization.stage", &key, &fact)
            .await?
            .is_some()
        {
            let old: StageFact = session
                .get(&self.context, "artifact", &fact.artifact_id)
                .await?
                .ok_or_else(|| Error::Conflict("cached stage subject was deleted".into()))?;
            old.validate()?;
            if fingerprint(&old)? != fingerprint(&fact)? {
                return Err(Error::Conflict("cached stage payload differs".into()));
            }
            session.commit().await?;
            return Ok(());
        }
        if let Some(old) = session
            .get::<StageFact>(&self.context, "artifact", &fact.artifact_id)
            .await?
        {
            old.validate()?;
            if fingerprint(&old)? != fingerprint(&fact)? {
                return Err(Error::Conflict("immutable stage differs".into()));
            }
            session.commit().await?;
            return Ok(());
        }
        // A fact that holds a model's answer and lands after a run it depends on was
        // revoked is accepted, because the receipt it carries is accounting truth,
        // but its content is not kept: the revocation cleanup only reaches what
        // exists when it looks, so an answer stored in plaintext now could stay in
        // the database for good. The decision is taken here, in the transaction that
        // would write the fact, so that no revocation can fall between the check and
        // the write.
        if holds_model_output(&fact)
            && run_dependency_revoked(&mut session, &self.context, &fact).await?
        {
            return self.commit_redacted(session, &fact).await;
        }
        session
            .put(
                &self.context,
                "artifact",
                &fact.artifact_id,
                &self.owner,
                &fact,
            )
            .await?;
        for dependency in &fact.dependencies {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &fact.artifact_id,
                    &dependency.kind,
                    &dependency.id,
                )
                .await?;
        }
        session
            .cache(
                &self.context,
                "optimization.stage",
                &key,
                &fact,
                &fact.artifact_id,
                &fact,
            )
            .await?;
        session
            .audit(
                &self.context,
                "optimization.stage.commit",
                &fact.artifact_id,
            )
            .await?;
        session.commit().await
    }
}

pub struct BundleCompileContext<'a> {
    pub profile: &'a Profile,
    pub baseline: &'a SkillSnapshot,
    pub parent_strategy: &'a Strategy,
    pub baseline_strategy: &'a Strategy,
    pub improver_patch: &'a ImproverPatch,
    pub caps: &'a HostCapabilities,
    pub revoked: &'a BTreeSet<String>,
}

pub struct OptimizationStepRequest<'a> {
    pub evidence: &'a evo_core::evidence::EvidenceSet,
    pub source_selection: &'a evo_core::evidence::SourceSelection,
    pub source_bindings: &'a [TrustedSourceBinding],
    pub traces: Vec<OptimizationTrace>,
    pub model_context: ModelRequestContext,
    pub parent_skill: &'a SkillSnapshot,
    pub edit_context: &'a TrustedEditContext,
    pub edit_batch_template: SkillEditBatch,
    pub protected_ranges: &'a [ProtectedTextRange],
    pub bundle_context: BundleCompileContext<'a>,
    pub development_request: DevelopmentRunRequest,
    pub allow_rank_call: bool,
}

/// The kind of the error that ended a step for a reason no named [`StepTerminalClass`]
/// describes. The error itself is dropped: its message can echo a source, a model
/// answer or what a store said, and what a record keeps of it is this closed kind and
/// nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepErrorKind {
    Invalid,
    Forbidden,
    NotFound,
    Budget,
    Conflict,
    Cancelled,
    Internal,
}

impl StepErrorKind {
    /// The fixed code of the kind, the suffix of the code of a [`StepTerminalClass::StepError`].
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Budget => "budget",
            Self::Conflict => "conflict",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

impl From<&Error> for StepErrorKind {
    /// Every error is named, so an error added to `evo_core` has to be decided here.
    fn from(error: &Error) -> Self {
        match error {
            Error::Invalid(_) => Self::Invalid,
            Error::Forbidden => Self::Forbidden,
            Error::NotFound => Self::NotFound,
            Error::Budget => Self::Budget,
            Error::Conflict(_) => Self::Conflict,
            Error::Cancelled => Self::Cancelled,
            Error::Internal => Self::Internal,
        }
    }
}

/// The closed set of ways an optimization step, or the dispatch that paid for one,
/// ends without a candidate (plan §5.3.1: no change is an explicit end point; §5.8 and
/// §6.7.3: the accepted, the rejected and the unchanged end points, the unmatched edits
/// and the failure modes are recorded; §7.2: a legal direction that failed, a compile,
/// shape or implementation error, an external failure, a lack of resources and a safety
/// rejection are recorded apart, and an unknown one as unknown).
///
/// Every exit is one variant. A variant serializes as a fixed snake_case `class` tag
/// plus the typed fields it carries (a number, a model's rejection kind, an error
/// kind), and has one fixed [`code`](Self::code), the only text any record keeps of the
/// outcome: the stage facts and the `StepCompleted` payload of the journal, the
/// reason of an exploration dispatch fact and the category of a consolidation run. The
/// text of an error, of a model's rejection or of a source never becomes part of a
/// class, so nothing derived from them reaches a record that outlives the source.
///
/// The variants are grouped by the outcome that carries them
/// ([`OptimizationStepOutcome::NoChange`], `Rejected`, `Uncertain`); the last group is
/// what the exploration coordinator settles a dispatch as when the step is not the one
/// that ended it, and what it records of a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum StepTerminalClass {
    // ----- no change: the step ended on purpose, with nothing to propose (V088.b, V089.c)
    /// No reflection batch could be built from the traces of the step.
    NoEligibleReflectionBatch,
    /// Every reflection call answered with no suggestion.
    NoEditSuggestions,
    /// The atomic edit compiled and changed nothing.
    EditProducedNoChange,

    // ----- the incumbent is kept: the candidate was run and not accepted (V090.a)
    /// The development selection kept the incumbent: the candidate did not strictly
    /// improve the total of the same full manifest, and no incumbent pass was lost.
    KeepIncumbentNotImproved {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },
    /// The development selection kept the incumbent because the candidate lost a task
    /// the incumbent passed, whatever its total (a higher total included).
    KeepIncumbentRetentionBroken {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },

    // ----- rejected: the step ended on a refusal
    /// The source grant does not allow a model excerpt, or is not the one of the step.
    GrantUnavailable,
    /// The model's provider rejected a request. The kind is the typed answer; the
    /// words that came with it are never kept.
    ModelRejected { kind: ModelRejectionKind },
    /// The model's answer is not a suggestion list: it does not parse as one, or it is
    /// longer than the request allowed.
    SuggestionShapeInvalid,
    /// The pool holds more suggestions than the final edit limit and no ranking call
    /// was preregistered.
    SuggestionPoolUnranked,
    /// The atomic skill edit did not compile.
    EditCompileFailed,
    /// The complete bundle did not compile.
    BundleCompileFailed,
    /// The development selection refused the report of the development runner.
    DevelopmentReportRejected,
    /// Any other refusal of the step, by the kind of the error that ended it.
    StepError { error: StepErrorKind },

    // ----- uncertain: the dispatch was paid for and its outcome cannot be recorded
    /// The usage of a model call is unknown.
    ModelUsageUnknown,
    /// A model dispatch was prepared and has no durable response: its usage is unknown.
    ModelDispatchUnrecorded,
    /// Another worker holds the model dispatch.
    ModelDispatchConcurrent,
    /// The model transport failed after the dispatch.
    ModelTransportOutcomeUnknown,
    /// A development execution was prepared and has no durable result.
    DevelopmentExecutionUnrecorded,
    /// Another worker holds the development execution.
    DevelopmentExecutionConcurrent,
    /// The development runner failed after the dispatch.
    DevelopmentExecutionOutcomeUnknown,
    /// The dispatch was cancelled, or its outcome cannot be told.
    CancelledOrUncertain,
    /// The optimization step ended in an error the coordinator cannot classify.
    OptimizationDispatchOutcomeUncertain,
    /// The source closure of the world changed while the step ran.
    ExplorationSourceClosureChangedAfterDispatch,
    /// The prefix the step was decided on is not the one the world holds when it ends.
    ExplorationPrefixChangedAfterDispatch,
    /// A claim was resumed on a world that no longer carries the inputs it was claimed with.
    ClaimInputsChangedAfterDispatch,
    /// The node a `Recover` repairs is not where the world lists it.
    RecoverTargetMissing,
    /// The observation gate could not answer for the evidence of a candidate.
    DevelopmentEvidenceUnverified,
    /// The digests of a candidate that compiled and was selected cannot be computed.
    CandidateMaterialUnavailable,

    // ----- a candidate, as the exploration coordinator records the verdict on it
    /// The observation gate verified the candidate's evidence.
    CandidateObserved,
    /// The report declared Fixture provenance, which can never be trusted evidence.
    FixtureEvidenceNotAccepted,
    /// The observation gate refused the report.
    DevelopmentEvidenceRejected,

    // ----- a record written before the classes existed
    /// The terminal of a step journaled as free text. The text is not carried forward.
    LegacyUnclassified,
}

impl StepTerminalClass {
    /// The fixed code of the class: lowercase ASCII and underscores, the same for the
    /// same class on every record. A class that carries a typed field adds the field's
    /// own code where the field decides the outcome (the kind of a model's rejection,
    /// the kind of an error); a number is never part of a code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoEligibleReflectionBatch => "no_eligible_reflection_batch",
            Self::NoEditSuggestions => "no_edit_suggestions",
            Self::EditProducedNoChange => "edit_produced_no_change",
            Self::KeepIncumbentNotImproved { .. } => "keep_incumbent_not_improved",
            Self::KeepIncumbentRetentionBroken { .. } => "keep_incumbent_retention_broken",
            Self::GrantUnavailable => "grant_unavailable",
            Self::ModelRejected { kind } => match kind {
                ModelRejectionKind::Unauthorized => "model_rejected_unauthorized",
                ModelRejectionKind::BudgetUnavailable => "model_rejected_budget_unavailable",
                ModelRejectionKind::InvalidRequest => "model_rejected_invalid_request",
                ModelRejectionKind::ProviderRejected => "model_rejected_provider_rejected",
                ModelRejectionKind::CancelledBeforeDispatch => {
                    "model_rejected_cancelled_before_dispatch"
                }
                ModelRejectionKind::CancelledAfterDispatch => {
                    "model_rejected_cancelled_after_dispatch"
                }
            },
            Self::SuggestionShapeInvalid => "suggestion_shape_invalid",
            Self::SuggestionPoolUnranked => "suggestion_pool_unranked",
            Self::EditCompileFailed => "edit_compile_failed",
            Self::BundleCompileFailed => "bundle_compile_failed",
            Self::DevelopmentReportRejected => "development_report_rejected",
            Self::StepError { error } => match error {
                StepErrorKind::Invalid => "step_error_invalid",
                StepErrorKind::Forbidden => "step_error_forbidden",
                StepErrorKind::NotFound => "step_error_not_found",
                StepErrorKind::Budget => "step_error_budget",
                StepErrorKind::Conflict => "step_error_conflict",
                StepErrorKind::Cancelled => "step_error_cancelled",
                StepErrorKind::Internal => "step_error_internal",
            },
            Self::ModelUsageUnknown => "model_usage_unknown",
            Self::ModelDispatchUnrecorded => "model_dispatch_unrecorded",
            Self::ModelDispatchConcurrent => "model_dispatch_concurrent",
            Self::ModelTransportOutcomeUnknown => "model_transport_outcome_unknown",
            Self::DevelopmentExecutionUnrecorded => "development_execution_unrecorded",
            Self::DevelopmentExecutionConcurrent => "development_execution_concurrent",
            Self::DevelopmentExecutionOutcomeUnknown => "development_execution_outcome_unknown",
            Self::CancelledOrUncertain => "cancelled_or_uncertain",
            Self::OptimizationDispatchOutcomeUncertain => "optimization_dispatch_outcome_uncertain",
            Self::ExplorationSourceClosureChangedAfterDispatch => {
                "exploration_source_closure_changed_after_dispatch"
            }
            Self::ExplorationPrefixChangedAfterDispatch => {
                "exploration_prefix_changed_after_dispatch"
            }
            Self::ClaimInputsChangedAfterDispatch => "claim_inputs_changed_after_dispatch",
            Self::RecoverTargetMissing => "recover_target_missing",
            Self::DevelopmentEvidenceUnverified => "development_evidence_unverified",
            Self::CandidateMaterialUnavailable => "candidate_material_unavailable",
            Self::CandidateObserved => "candidate_observed",
            Self::FixtureEvidenceNotAccepted => "fixture_evidence_not_accepted",
            Self::DevelopmentEvidenceRejected => "development_evidence_rejected",
            Self::LegacyUnclassified => "legacy_unclassified",
        }
    }
}

/// The code of a rejection kind: what a journaled rejection keeps where the provider's
/// own words were (`journaled_response`).
fn rejection_kind_code(kind: ModelRejectionKind) -> &'static str {
    match kind {
        ModelRejectionKind::Unauthorized => "unauthorized",
        ModelRejectionKind::BudgetUnavailable => "budget_unavailable",
        ModelRejectionKind::InvalidRequest => "invalid_request",
        ModelRejectionKind::ProviderRejected => "provider_rejected",
        ModelRejectionKind::CancelledBeforeDispatch => "cancelled_before_dispatch",
        ModelRejectionKind::CancelledAfterDispatch => "cancelled_after_dispatch",
    }
}

/// How a step ended. An outcome that is no candidate carries the closed class of its
/// exit, never the text of an error, of a model or of a source.
#[derive(Debug)]
pub enum OptimizationStepOutcome {
    NoChange {
        class: StepTerminalClass,
    },
    /// The candidate, the model or the step was refused: a rejection proper, or the
    /// incumbent kept (`KeepIncumbent*`, which is what a development selection that
    /// does not accept the candidate ends in).
    Rejected {
        class: StepTerminalClass,
    },
    Uncertain {
        class: StepTerminalClass,
    },
    Candidate {
        edit: Box<CompiledSkillEdit>,
        bundle: Box<ResolvedBundle>,
        selection: DevelopmentSelection,
    },
}

async fn run_optimization_step_inner(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: OptimizationStepRequest<'_>,
) -> Result<OptimizationStepOutcome> {
    let model = model.ok_or(Error::NotFound)?;
    let runner = runner.ok_or(Error::NotFound)?;
    let journal = journal.ok_or(Error::NotFound)?;
    if validate_outbound_grant(request.source_selection).is_err() {
        let class = StepTerminalClass::GrantUnavailable;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            &class,
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::Rejected { class });
    }
    let reflection =
        build_reflection_batches(request.evidence, request.source_bindings, request.traces)?;
    if reflection.batches.is_empty() {
        let class = StepTerminalClass::NoEligibleReflectionBatch;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            &class,
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange { class });
    }

    let per_batch_limit = MAX_SUGGESTIONS / reflection.batches.len();
    let mut suggestions = Vec::new();
    for (index, batch) in reflection.batches.iter().enumerate() {
        let stage = call_stage(
            request.model_context.stage,
            match batch.kind {
                ReflectionBatchKind::Failure => ModelStage::ReflectFailure,
                ReflectionBatchKind::Success => ModelStage::ReflectSuccess,
            },
        );
        let context =
            model_context_for_batch(&request.model_context, batch, stage, index, per_batch_limit);
        let payload = serde_json::to_string(batch).map_err(|_| Error::Internal)?;
        validate_batch_grant(request.source_selection, batch)?;
        let model_request = ModelRequest::build(
            context,
            vec![
                ModelInputPart {
                    role: ModelInputRole::System,
                    label: "generation-strategy".into(),
                    content: serde_json::to_string(request.bundle_context.parent_strategy)
                        .map_err(|_| Error::Internal)?,
                },
                ModelInputPart {
                    role: ModelInputRole::Evidence,
                    label: batch.id.clone(),
                    content: payload,
                },
                ModelInputPart {
                    role: ModelInputRole::User,
                    label: "parent-skill".into(),
                    content: serde_json::to_string(request.parent_skill)
                        .map_err(|_| Error::Internal)?,
                },
                ModelInputPart {
                    role: ModelInputRole::System,
                    label: "fixed-controls".into(),
                    content: format!(
                        "max_suggestions={per_batch_limit};max_final_edits={MAX_FINAL_EDITS};preserve_all_sources_and_counterexamples=true"
                    ),
                },
            ],
        )?;
        commit_model_fact(
            journal,
            &model_request,
            StageFactKind::RequestPrepared,
            None,
            vec![],
            serde_json::to_value(&model_request).map_err(|_| Error::Internal)?,
        )
        .await?;
        let outcome = match suggestion_outcome(
            Some(model),
            model_request.clone(),
            std::slice::from_ref(batch),
        )
        .await
        {
            Ok(Some(outcome)) => outcome,
            // The exit that ends the step without a fact of its own, as it always did:
            // only the terminal fact of the step (`StepCompleted`) records it.
            Ok(None) => {
                return Ok(OptimizationStepOutcome::Rejected {
                    class: StepTerminalClass::SuggestionShapeInvalid,
                });
            }
            Err(error) => return Err(error),
        };
        let outcome_payload = serde_json::to_value(&outcome).map_err(|_| Error::Internal)?;
        let (mut items, response_id, output_digest, receipt) = match outcome {
            SuggestionOutcome::NoChange {
                response_id,
                output: _,
                output_digest,
                receipt,
            } => (Vec::new(), response_id, output_digest, receipt),
            SuggestionOutcome::Suggestions {
                items,
                response_id,
                output: _,
                output_digest,
                receipt,
            } => (items, response_id, output_digest, receipt),
            SuggestionOutcome::Rejected {
                kind,
                dependency_ids,
            } => {
                let class = StepTerminalClass::ModelRejected { kind };
                commit_model_fact(
                    journal,
                    &model_request,
                    StageFactKind::ResponseObserved,
                    None,
                    dependency_ids.clone(),
                    outcome_payload,
                )
                .await?;
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &class,
                    dependency_ids,
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected { class });
            }
            SuggestionOutcome::Uncertain { dependency_ids } => {
                let class = StepTerminalClass::ModelUsageUnknown;
                commit_model_fact(
                    journal,
                    &model_request,
                    StageFactKind::ResponseObserved,
                    None,
                    dependency_ids.clone(),
                    outcome_payload,
                )
                .await?;
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalUncertain,
                    &class,
                    dependency_ids,
                )
                .await?;
                return Ok(OptimizationStepOutcome::Uncertain { class });
            }
        };
        commit_model_fact(
            journal,
            &model_request,
            StageFactKind::ResponseObserved,
            Some(output_digest),
            vec![
                response_id,
                receipt.call_id,
                receipt.dispatch_id,
                receipt.root_budget_id,
                receipt.provider_request_id,
                receipt.usage_record_id,
            ],
            outcome_payload,
        )
        .await?;
        suggestions.append(&mut items);
    }
    if suggestions.is_empty() {
        let class = StepTerminalClass::NoEditSuggestions;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            &class,
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange { class });
    }
    if suggestions.len() > MAX_FINAL_EDITS && !request.allow_rank_call {
        let class = StepTerminalClass::SuggestionPoolUnranked;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            &class,
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::Rejected { class });
    }
    let selected_ids: Vec<String> = if suggestions.len() > MAX_FINAL_EDITS {
        match rank_suggestions(
            model,
            journal,
            &request.model_context,
            &reflection.batches,
            &suggestions,
            request.parent_skill,
            request.bundle_context.parent_strategy,
        )
        .await
        {
            Ok(selected) => selected,
            Err(error) => return Err(error),
        }
    } else {
        suggestions.iter().map(|item| item.id.clone()).collect()
    };
    let merged = merge_suggestions(&reflection.batches, &suggestions, &selected_ids)?;
    let merge_record = serde_json::to_value(&merged).map_err(|_| Error::Internal)?;
    let mut edit_batch = request.edit_batch_template;
    edit_batch.evidence = merged.evidence;
    edit_batch.edits = merged.edits;
    let compile_key = recovery_fact(
        &request.model_context,
        OptimizationJournalStage::EditCompile,
        StageFactKind::EditCompiled,
        request.model_context.request_id.clone(),
        fingerprint(&edit_batch)?,
        serde_json::json!({}),
    )?;
    let saved_compile = journal.lookup(&compile_key.artifact_id).await?;
    if let Some(saved) = &saved_compile
        && saved.input_digest != compile_key.input_digest
    {
        return Err(Error::Conflict("compile input differs".into()));
    }
    let compiled = if let Some(saved) = &saved_compile {
        CompiledSkillEdit {
            output: serde_json::from_value(saved.payload["output"].clone())
                .map_err(|_| Error::Internal)?,
            patch: serde_json::from_value(saved.payload["patch"].clone())
                .map_err(|_| Error::Internal)?,
            report: serde_json::from_value(saved.payload["report"].clone())
                .map_err(|_| Error::Internal)?,
        }
    } else {
        match compile_skill_edits(
            request.parent_skill,
            request.edit_context,
            &edit_batch,
            request.protected_ranges,
        ) {
            Ok(compiled) => compiled,
            Err(_) => {
                let class = StepTerminalClass::EditCompileFailed;
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &class,
                    vec![],
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected { class });
            }
        }
    };
    if compiled.report.status == evo_core::skill_edit::EditApplyStatus::NoChange {
        let class = StepTerminalClass::EditProducedNoChange;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            &class,
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange { class });
    }
    let bundle = if let Some(saved) = &saved_compile {
        serde_json::from_value(saved.payload["bundle"].clone()).map_err(|_| Error::Internal)?
    } else {
        match compile_bundle(CompileParts {
            profile: request.bundle_context.profile,
            parent: request.parent_skill,
            baseline: request.bundle_context.baseline,
            parent_strategy: request.bundle_context.parent_strategy,
            baseline_strategy: request.bundle_context.baseline_strategy,
            skill_patch: &compiled.patch,
            improver_patch: request.bundle_context.improver_patch,
            caps: request.bundle_context.caps,
            revoked: request.bundle_context.revoked,
        }) {
            Ok(bundle) => bundle,
            Err(_) => {
                let class = StepTerminalClass::BundleCompileFailed;
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &class,
                    vec![],
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected { class });
            }
        }
    };
    let compile_fact = StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: format!(
            "{}-{}-{}-edit",
            request.model_context.episode_id,
            request.model_context.step,
            request.model_context.attempt
        ),
        namespace: request.model_context.namespace.clone(),
        episode_id: request.model_context.episode_id.clone(),
        step: request.model_context.step,
        attempt: request.model_context.attempt,
        stage: OptimizationJournalStage::EditCompile,
        kind: StageFactKind::EditCompiled,
        request_id: request.model_context.request_id.clone(),
        input_digest: compile_key.input_digest,
        output_digest: Some(compiled.report.output_digest.clone()),
        dependencies: merged
            .read_dependencies
            .iter()
            .map(|reference| dependency("run", reference.id.clone()))
            .collect(),
        payload: serde_json::json!({"output":compiled.output,"patch":compiled.patch,"report":compiled.report,"bundle":bundle,"merge":merge_record}),
    };
    commit_stage(journal, compile_fact).await?;

    let mut development_request = request.development_request;
    development_request.request_id = format!(
        "optdev-{}",
        fingerprint(&(
            &request.model_context.namespace,
            &request.model_context.episode_id,
            request.model_context.step,
            request.model_context.attempt,
            &request.model_context.request_id,
            &development_request.request_id
        ))?
    );
    development_request.idempotency_key = format!(
        "optdevidem-{}",
        fingerprint(&(
            &development_request.request_id,
            &development_request.idempotency_key
        ))?
    );
    development_request.candidate_bundle_digest = bundle.digest.clone();
    validate_development_request(&development_request)?;
    commit_stage(
        journal,
        StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: format!(
                "{}-{}-{}-development-request",
                request.model_context.episode_id,
                request.model_context.step,
                request.model_context.attempt
            ),
            namespace: request.model_context.namespace.clone(),
            episode_id: request.model_context.episode_id.clone(),
            step: request.model_context.step,
            attempt: request.model_context.attempt,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentRequestPrepared,
            request_id: development_request.request_id.clone(),
            input_digest: development_request.manifest.digest.clone(),
            output_digest: None,
            dependencies: vec![
                dependency("bundle", development_request.parent_bundle_digest.clone()),
                dependency(
                    "bundle",
                    development_request.candidate_bundle_digest.clone(),
                ),
                dependency(
                    "environment",
                    development_request.environment_digest.clone(),
                ),
                dependency("grader", development_request.grader_digest.clone()),
            ],
            payload: serde_json::to_value(&development_request).map_err(|_| Error::Internal)?,
        },
    )
    .await?;
    let report = match runner.run(development_request.clone()).await {
        Ok(report) => report,
        Err(error) => return Err(error),
    };
    let (selection, preserves_passes) =
        match select_development_detailed(&development_request, &report) {
            Ok(selected) => selected,
            Err(_) => {
                let class = StepTerminalClass::DevelopmentReportRejected;
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &class,
                    vec![bundle.digest.clone()],
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected { class });
            }
        };
    commit_stage(
        journal,
        StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: format!(
                "{}-{}-{}-development",
                request.model_context.episode_id,
                request.model_context.step,
                request.model_context.attempt
            ),
            namespace: request.model_context.namespace.clone(),
            episode_id: request.model_context.episode_id.clone(),
            step: request.model_context.step,
            attempt: request.model_context.attempt,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id: development_request.request_id.clone(),
            input_digest: development_request.manifest.digest,
            output_digest: Some(fingerprint(&selection)?),
            dependencies: {
                let mut dependencies: Vec<StageDependency> = report
                    .results
                    .iter()
                    .flat_map(|result| {
                        [
                            dependency("execution", result.parent_execution_id.clone()),
                            dependency("execution", result.candidate_execution_id.clone()),
                            dependency("grader", result.grader_receipt_digest.clone()),
                        ]
                    })
                    .collect();
                // A non-fixture report names typed receipts; declare them so
                // revocation traverses fact -> receipts and the gate can
                // require the closure. Fixture ids are opaque and untyped.
                if report.provenance != DevelopmentExecutionProvenance::Fixture {
                    dependencies.extend(crate::development::typed_receipt_closure(&report)?);
                }
                dependencies
            },
            payload: serde_json::to_value(&report).map_err(|_| Error::Internal)?,
        },
    )
    .await?;
    if selection.decision == DevelopmentSelectionDecision::AcceptCandidate {
        let outcome = OptimizationStepOutcome::Candidate {
            edit: Box::new(compiled),
            bundle: Box::new(bundle),
            selection,
        };
        let mut fact = recovery_fact(
            &request.model_context,
            OptimizationJournalStage::Development,
            StageFactKind::TerminalCandidate,
            request.model_context.request_id.clone(),
            fingerprint(&request.model_context)?,
            encode_outcome(&outcome)?,
        )?;
        fact.dependencies
            .push(dependency("artifact", compile_key.artifact_id));
        let dev_fact = recovery_fact(
            &request.model_context,
            OptimizationJournalStage::Development,
            StageFactKind::DevelopmentObserved,
            development_request.request_id.clone(),
            fingerprint(&request.model_context)?,
            serde_json::json!({}),
        )?;
        fact.dependencies
            .push(dependency("artifact", dev_fact.artifact_id));
        commit_stage(journal, fact).await?;
        Ok(outcome)
    } else {
        let class = keep_incumbent_class(&selection, preserves_passes);
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            &class,
            vec![bundle.digest],
        )
        .await?;
        Ok(OptimizationStepOutcome::Rejected { class })
    }
}

/// The class of a development selection that kept the incumbent. Which way it was kept
/// is read from the pass-retention verdict of the selection, never from its sentence:
/// a candidate that lost a task the incumbent passed is `retention_broken` whatever its
/// total was (a higher one included), and every other candidate the selection did not
/// accept is `not_improved`. The totals are the selection's, and the only numbers the
/// class carries.
fn keep_incumbent_class(
    selection: &DevelopmentSelection,
    preserves_passes: bool,
) -> StepTerminalClass {
    let (parent_total_micros, candidate_total_micros) = (
        selection.parent_total_micros,
        selection.candidate_total_micros,
    );
    if preserves_passes {
        StepTerminalClass::KeepIncumbentNotImproved {
            parent_total_micros,
            candidate_total_micros,
        }
    } else {
        StepTerminalClass::KeepIncumbentRetentionBroken {
            parent_total_micros,
            candidate_total_micros,
        }
    }
}

fn model_context_for_batch(
    base: &ModelRequestContext,
    batch: &ReflectionBatch,
    stage: ModelStage,
    index: usize,
    max_suggestions: usize,
) -> ModelRequestContext {
    ModelRequestContext {
        request_id: format!(
            "optrequest-{}",
            fingerprint(&(&base.request_id, index)).expect("serializable request identity")
        ),
        namespace: base.namespace.clone(),
        purpose: base.purpose,
        stage,
        episode_id: base.episode_id.clone(),
        step: base.step,
        attempt: base.attempt,
        parent_skill_digest: base.parent_skill_digest.clone(),
        bundle_digest: base.bundle_digest.clone(),
        source_closure: batch.source_closure.clone(),
        model_digest: base.model_digest.clone(),
        tools_digest: base.tools_digest.clone(),
        rules_digest: base.rules_digest.clone(),
        sampling_digest: base.sampling_digest.clone(),
        revoke_watermark: base.revoke_watermark,
        max_suggestions,
    }
}

fn validate_outbound_grant(selection: &evo_core::evidence::SourceSelection) -> Result<()> {
    selection.validate()?;
    if selection.purpose != Purpose::Development || !selection.allow_model_excerpts {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn validate_batch_grant(
    selection: &evo_core::evidence::SourceSelection,
    batch: &ReflectionBatch,
) -> Result<()> {
    let selected: BTreeSet<&str> = selection.run_ids.iter().map(String::as_str).collect();
    if !batch
        .source_closure
        .iter()
        .all(|reference| selected.contains(reference.id.as_str()))
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

async fn rank_suggestions(
    model: &dyn ModelPort,
    journal: &dyn OptimizationJournal,
    base: &ModelRequestContext,
    batches: &[ReflectionBatch],
    suggestions: &[EditSuggestion],
    parent: &SkillSnapshot,
    strategy: &Strategy,
) -> Result<Vec<String>> {
    let source_closure: Vec<_> = batches
        .iter()
        .flat_map(|batch| batch.source_closure.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: format!("optrank-{}", fingerprint(&base.request_id)?),
            namespace: base.namespace.clone(),
            purpose: base.purpose,
            stage: call_stage(base.stage, ModelStage::Rank),
            episode_id: base.episode_id.clone(),
            step: base.step,
            attempt: base.attempt,
            parent_skill_digest: base.parent_skill_digest.clone(),
            bundle_digest: base.bundle_digest.clone(),
            source_closure,
            model_digest: base.model_digest.clone(),
            tools_digest: base.tools_digest.clone(),
            rules_digest: base.rules_digest.clone(),
            sampling_digest: base.sampling_digest.clone(),
            revoke_watermark: base.revoke_watermark,
            max_suggestions: MAX_FINAL_EDITS,
        },
        vec![
            ModelInputPart {
                role: ModelInputRole::System,
                label: "generation-strategy".into(),
                content: serde_json::to_string(strategy).map_err(|_| Error::Internal)?,
            },
            ModelInputPart {
                role: ModelInputRole::User,
                label: "parent-skill".into(),
                content: serde_json::to_string(parent).map_err(|_| Error::Internal)?,
            },
            ModelInputPart {
                role: ModelInputRole::Evidence,
                label: "suggestion-pool".into(),
                content: serde_json::to_string(suggestions).map_err(|_| Error::Internal)?,
            },
            ModelInputPart {
                role: ModelInputRole::System,
                label: "rank-controls".into(),
                content: "return 1..=4 suggestion ids; preserve provenance and counterexamples"
                    .into(),
            },
        ],
    )?;
    commit_model_fact(
        journal,
        &request,
        StageFactKind::RequestPrepared,
        None,
        vec![],
        serde_json::to_value(&request).map_err(|_| Error::Internal)?,
    )
    .await?;
    let response = model.dispatch(request.clone()).await?;
    response.validate_against(&request)?;
    let response_payload = serde_json::to_value(&response).map_err(|_| Error::Internal)?;
    let ModelResponse::Completed {
        output,
        output_digest,
        execution_receipt,
        response_id,
        ..
    } = response
    else {
        commit_model_fact(
            journal,
            &request,
            StageFactKind::ResponseObserved,
            None,
            vec![],
            response_payload,
        )
        .await?;
        return Err(Error::Invalid("ranking model did not complete".into()));
    };
    commit_model_fact(
        journal,
        &request,
        StageFactKind::ResponseObserved,
        Some(output_digest),
        vec![
            response_id,
            execution_receipt.call_id,
            execution_receipt.dispatch_id,
            execution_receipt.root_budget_id,
            execution_receipt.provider_request_id,
            execution_receipt.usage_record_id,
        ],
        response_payload,
    )
    .await?;
    let selected: Vec<String> = serde_json::from_str(&output)
        .map_err(|error| Error::Invalid(format!("invalid ranking response: {error}")))?;
    let available: BTreeSet<&str> = suggestions.iter().map(|item| item.id.as_str()).collect();
    let unique: BTreeSet<&str> = selected.iter().map(String::as_str).collect();
    if selected.is_empty()
        || selected.len() > MAX_FINAL_EDITS
        || unique.len() != selected.len()
        || !unique.iter().all(|id| available.contains(id))
    {
        return Err(Error::Invalid(
            "ranking selected invalid suggestion ids".into(),
        ));
    }
    Ok(selected)
}

async fn commit_model_fact(
    journal: &dyn OptimizationJournal,
    request: &ModelRequest,
    kind: StageFactKind,
    output_digest: Option<String>,
    dependencies: Vec<String>,
    payload: serde_json::Value,
) -> Result<()> {
    commit_stage(
        journal,
        StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: format!("{}-{:?}", request.request_id, kind).to_lowercase(),
            namespace: request.namespace.clone(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            stage: journal_stage(request.stage),
            kind,
            request_id: request.request_id.clone(),
            input_digest: request.input_digest.clone(),
            output_digest,
            dependencies: dependencies
                .into_iter()
                .map(|id| dependency("model_receipt", id))
                .chain(
                    request
                        .source_closure
                        .iter()
                        .map(|reference| dependency("run", reference.id.clone())),
                )
                .collect(),
            payload,
        },
    )
    .await
}

async fn commit_stage(journal: &dyn OptimizationJournal, fact: StageFact) -> Result<()> {
    journal.commit(fact.seal()?).await
}

/// Commits the terminal stage fact of a step that ended without a candidate. Its payload
/// is the fixed code of the class and the class itself (typed fields included): the
/// fact never holds the text of an error, of a model or of a source.
async fn commit_terminal(
    journal: &dyn OptimizationJournal,
    context: &ModelRequestContext,
    kind: StageFactKind,
    class: &StepTerminalClass,
    dependencies: Vec<String>,
) -> Result<()> {
    let payload = serde_json::json!({"reason": class.code(), "class": class});
    commit_stage(
        journal,
        StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: format!(
                "{}-{}-{}-{:?}",
                context.episode_id, context.step, context.attempt, kind
            )
            .to_lowercase(),
            namespace: context.namespace.clone(),
            episode_id: context.episode_id.clone(),
            step: context.step,
            attempt: context.attempt,
            stage: OptimizationJournalStage::Development,
            kind,
            request_id: context.request_id.clone(),
            input_digest: fingerprint(&payload)?,
            output_digest: None,
            dependencies: dependencies
                .into_iter()
                .map(|id| dependency("artifact", id))
                .collect(),
            payload,
        },
    )
    .await
}

fn journal_stage(stage: ModelStage) -> OptimizationJournalStage {
    match stage {
        ModelStage::ReflectFailure => OptimizationJournalStage::ReflectFailure,
        ModelStage::ReflectSuccess => OptimizationJournalStage::ReflectSuccess,
        ModelStage::Merge => OptimizationJournalStage::Merge,
        ModelStage::Rank => OptimizationJournalStage::Rank,
        ModelStage::Consolidate => OptimizationJournalStage::Consolidate,
    }
}

/// The stage one model call of a step is made under. A consolidation step (base
/// stage `Consolidate`) makes all of its calls under that one stage, so the
/// ledger meters them as `Consolidation` (plan §3.6, §11.5) inside the claim's
/// root budget. For every other step the base stage is ignored and `otherwise`
/// (the batch kind, or `Rank`) decides, exactly as before.
fn call_stage(base: ModelStage, otherwise: ModelStage) -> ModelStage {
    if base == ModelStage::Consolidate {
        ModelStage::Consolidate
    } else {
        otherwise
    }
}

// Recovery records are ordinary typed artifacts, committed by the same Store journal.
fn recovery_fact(
    context: &ModelRequestContext,
    stage: OptimizationJournalStage,
    kind: StageFactKind,
    request_id: String,
    input_digest: String,
    payload: serde_json::Value,
) -> Result<StageFact> {
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: context.namespace.clone(),
        episode_id: context.episode_id.clone(),
        step: context.step,
        attempt: context.attempt,
        stage,
        kind,
        request_id,
        input_digest,
        output_digest: None,
        dependencies: vec![],
        payload,
    }
    .seal()
}

struct RecoveryJournal<'a> {
    inner: &'a dyn OptimizationJournal,
    sources: Vec<String>,
    watermark: u64,
    step_id: String,
    grant_id: String,
    stage_ids: std::sync::Mutex<BTreeSet<String>>,
}
#[async_trait]
impl OptimizationJournal for RecoveryJournal<'_> {
    async fn claim(&self, mut fact: StageFact) -> Result<bool> {
        self.check_live(&self.sources, self.watermark).await?;
        fact.dependencies
            .extend(self.sources.iter().map(|id| dependency("run", id.clone())));
        fact.dependencies
            .push(dependency("revoke_watermark", self.watermark.to_string()));
        fact.dependencies
            .push(dependency("artifact", self.grant_id.clone()));
        fact.dependencies
            .push(dependency("artifact", self.step_id.clone()));
        fact.dependencies
            .sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        fact.dependencies
            .dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
        let id = fact.artifact_id.clone();
        let claimed = self.inner.claim(fact).await?;
        self.stage_ids
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(id);
        Ok(claimed)
    }
    async fn lookup(&self, id: &str) -> Result<Option<StageFact>> {
        self.check_live(&self.sources, self.watermark).await?;
        let found = self.inner.lookup(id).await?;
        if found.is_some() {
            self.stage_ids
                .lock()
                .map_err(|_| Error::Internal)?
                .insert(id.into());
        }
        Ok(found)
    }
    async fn check_live(&self, ids: &[String], watermark: u64) -> Result<()> {
        self.inner.check_live(ids, watermark).await
    }
    async fn verify_sources(
        &self,
        s: &evo_core::evidence::SourceSelection,
        e: &evo_core::evidence::EvidenceSet,
        b: &[TrustedSourceBinding],
        t: &[OptimizationTrace],
        w: u64,
    ) -> Result<()> {
        self.inner.verify_sources(s, e, b, t, w).await
    }
    async fn commit(&self, mut fact: StageFact) -> Result<()> {
        self.check_live(&self.sources, self.watermark).await?;
        fact.dependencies
            .extend(self.sources.iter().map(|id| dependency("run", id.clone())));
        fact.dependencies
            .push(dependency("revoke_watermark", self.watermark.to_string()));
        fact.dependencies
            .push(dependency("artifact", self.grant_id.clone()));
        if fact.artifact_id != self.step_id {
            fact.dependencies
                .push(dependency("artifact", self.step_id.clone()));
        }
        fact.dependencies
            .sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        fact.dependencies
            .dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
        self.inner.commit(fact.clone()).await?;
        self.stage_ids
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(fact.artifact_id);
        Ok(())
    }
}

struct RecoveryModel<'a> {
    port: &'a dyn ModelPort,
    journal: &'a RecoveryJournal<'a>,
    context: &'a ModelRequestContext,
    uncertain: &'a std::sync::Mutex<Option<StepTerminalClass>>,
}

/// What the journal keeps of a model's answer: all of it, except the words a rejection
/// came with. They are the provider's, they do not decide anything (the typed kind
/// does), and a rejected call has nothing else to recover from them, so the journal
/// keeps the code of the kind where they were. The answer a completed call returned is
/// the model's work product and is kept whole, as the recovery of the step needs it.
fn journaled_response(response: &ModelResponse) -> Result<serde_json::Value> {
    let kept = match response {
        ModelResponse::Rejected {
            request_id,
            kind,
            dispatch,
            ..
        } => ModelResponse::Rejected {
            request_id: request_id.clone(),
            kind: *kind,
            reason: rejection_kind_code(*kind).into(),
            dispatch: dispatch.clone(),
        },
        other => other.clone(),
    };
    serde_json::to_value(&kept).map_err(|_| Error::Internal)
}
#[async_trait]
impl ModelPort for RecoveryModel<'_> {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut prepared = recovery_fact(
            self.context,
            journal_stage(request.stage),
            StageFactKind::DispatchPrepared,
            request.request_id.clone(),
            fingerprint(&request)?,
            serde_json::to_value(&request).map_err(|_| Error::Internal)?,
        )?;
        let observed = recovery_fact(
            self.context,
            journal_stage(request.stage),
            StageFactKind::DispatchObserved,
            request.request_id.clone(),
            prepared.input_digest.clone(),
            serde_json::json!({}),
        )?;
        if let Some(old) = self.journal.lookup(&observed.artifact_id).await? {
            if old.input_digest != prepared.input_digest {
                return Err(Error::Conflict("model recovery input differs".into()));
            }
            let response: ModelResponse =
                serde_json::from_value(old.payload).map_err(|_| Error::Internal)?;
            response.validate_against(&request)?;
            if matches!(response, ModelResponse::Uncertain { .. }) {
                *self.uncertain.lock().map_err(|_| Error::Internal)? =
                    Some(StepTerminalClass::ModelUsageUnknown);
            }
            return Ok(response);
        }
        if let Some(old) = self.journal.lookup(&prepared.artifact_id).await? {
            if old.input_digest != prepared.input_digest {
                return Err(Error::Conflict("model recovery input differs".into()));
            }
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some(StepTerminalClass::ModelDispatchUnrecorded);
            return Err(Error::Conflict(
                "model dispatch uncertain; automatic retry forbidden".into(),
            ));
        }
        if !self.journal.claim(prepared.clone()).await? {
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some(StepTerminalClass::ModelDispatchConcurrent);
            return Err(Error::Conflict("model dispatch already claimed".into()));
        }
        let response = match self.port.dispatch(request.clone()).await {
            Ok(r) => r,
            Err(e) => {
                // The error is the port's own and is not kept: the outcome is unknown.
                *self.uncertain.lock().map_err(|_| Error::Internal)? =
                    Some(StepTerminalClass::ModelTransportOutcomeUnknown);
                return Err(e);
            }
        };
        prepared.kind = StageFactKind::DispatchObserved;
        prepared.payload = journaled_response(&response)?;
        // Preserve the receipt even if revocation raced with dispatch. It cannot be reused.
        prepared.dependencies.extend(
            self.journal
                .sources
                .iter()
                .map(|id| dependency("run", id.clone())),
        );
        prepared.dependencies.push(dependency(
            "revoke_watermark",
            self.journal.watermark.to_string(),
        ));
        prepared
            .dependencies
            .push(dependency("artifact", self.journal.step_id.clone()));
        let prepared = prepared.seal()?;
        self.journal.inner.commit(prepared.clone()).await?;
        self.journal
            .stage_ids
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(prepared.artifact_id);
        self.journal
            .check_live(&self.journal.sources, self.journal.watermark)
            .await?;
        if matches!(response, ModelResponse::Uncertain { .. }) {
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some(StepTerminalClass::ModelUsageUnknown);
        }
        response.validate_against(&request)?;
        Ok(response)
    }
}

struct RecoveryRunner<'a> {
    runner: &'a dyn DevRunner,
    journal: &'a RecoveryJournal<'a>,
    context: &'a ModelRequestContext,
    uncertain: &'a std::sync::Mutex<Option<StepTerminalClass>>,
}
#[async_trait]
impl DevRunner for RecoveryRunner<'_> {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        let prepared = recovery_fact(
            self.context,
            OptimizationJournalStage::Development,
            StageFactKind::DispatchPrepared,
            request.request_id.clone(),
            fingerprint(&request)?,
            serde_json::to_value(&request).map_err(|_| Error::Internal)?,
        )?;
        let mut observed = recovery_fact(
            self.context,
            OptimizationJournalStage::Development,
            StageFactKind::DispatchObserved,
            request.request_id.clone(),
            prepared.input_digest.clone(),
            serde_json::json!({}),
        )?;
        if let Some(old) = self.journal.lookup(&observed.artifact_id).await? {
            if old.input_digest != prepared.input_digest {
                return Err(Error::Conflict("development recovery input differs".into()));
            }
            let report = serde_json::from_value(old.payload).map_err(|_| Error::Internal)?;
            validate_development_binding(&request, &report)?;
            return Ok(report);
        }
        if let Some(old) = self.journal.lookup(&prepared.artifact_id).await? {
            if old.input_digest != prepared.input_digest {
                return Err(Error::Conflict("development recovery input differs".into()));
            }
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some(StepTerminalClass::DevelopmentExecutionUnrecorded);
            return Err(Error::Conflict(
                "development execution uncertain; automatic retry forbidden".into(),
            ));
        }
        if !self.journal.claim(prepared).await? {
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some(StepTerminalClass::DevelopmentExecutionConcurrent);
            return Err(Error::Conflict(
                "development execution already claimed".into(),
            ));
        }
        let report = match self.runner.run(request.clone()).await {
            Ok(r) => r,
            Err(e) => {
                // The error is the runner's own and is not kept: the outcome is unknown.
                *self.uncertain.lock().map_err(|_| Error::Internal)? =
                    Some(StepTerminalClass::DevelopmentExecutionOutcomeUnknown);
                return Err(e);
            }
        };
        observed.payload = serde_json::to_value(&report).map_err(|_| Error::Internal)?;
        observed.dependencies.extend(
            self.journal
                .sources
                .iter()
                .map(|id| dependency("run", id.clone())),
        );
        observed.dependencies.push(dependency(
            "revoke_watermark",
            self.journal.watermark.to_string(),
        ));
        observed
            .dependencies
            .push(dependency("artifact", self.journal.step_id.clone()));
        let observed = observed.seal()?;
        self.journal.inner.commit(observed.clone()).await?;
        self.journal
            .stage_ids
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(observed.artifact_id);
        self.journal
            .check_live(&self.journal.sources, self.journal.watermark)
            .await?;
        validate_development_binding(&request, &report)?;
        Ok(report)
    }
}

fn encode_outcome(outcome: &OptimizationStepOutcome) -> Result<serde_json::Value> {
    Ok(match outcome {
        OptimizationStepOutcome::Candidate {
            edit,
            bundle,
            selection,
        } => {
            serde_json::json!({"status":"candidate", "output": edit.output, "patch":edit.patch, "report":edit.report, "bundle":bundle, "selection":selection})
        }
        OptimizationStepOutcome::Rejected { class } => terminal_payload("rejected", class),
        OptimizationStepOutcome::NoChange { class } => terminal_payload("no_change", class),
        OptimizationStepOutcome::Uncertain { class } => terminal_payload("uncertain", class),
    })
}

/// The `StepCompleted` payload of an outcome that is no candidate: its status, the
/// fixed code of its class and the class. No text of an error, a model or a source.
fn terminal_payload(status: &str, class: &StepTerminalClass) -> serde_json::Value {
    serde_json::json!({"status": status, "reason": class.code(), "class": class})
}

/// The class a stored terminal names. A terminal journaled before the classes existed
/// carries free text only, which is never carried forward: it reads as
/// [`StepTerminalClass::LegacyUnclassified`].
fn stored_class(value: &serde_json::Value) -> Result<StepTerminalClass> {
    match value.get("class") {
        Some(class) => serde_json::from_value(class.clone()).map_err(|_| Error::Internal),
        None => Ok(StepTerminalClass::LegacyUnclassified),
    }
}
fn decode_outcome(value: serde_json::Value) -> Result<OptimizationStepOutcome> {
    fn field<T: serde::de::DeserializeOwned>(v: &serde_json::Value, name: &str) -> Result<T> {
        serde_json::from_value(v.get(name).ok_or(Error::Internal)?.clone())
            .map_err(|_| Error::Internal)
    }
    match value.get("status").and_then(|s| s.as_str()) {
        Some("candidate") => Ok(OptimizationStepOutcome::Candidate {
            edit: Box::new(CompiledSkillEdit {
                output: field(&value, "output")?,
                patch: field(&value, "patch")?,
                report: field(&value, "report")?,
            }),
            bundle: Box::new(field(&value, "bundle")?),
            selection: field(&value, "selection")?,
        }),
        Some("rejected") => Ok(OptimizationStepOutcome::Rejected {
            class: stored_class(&value)?,
        }),
        Some("no_change") => Ok(OptimizationStepOutcome::NoChange {
            class: stored_class(&value)?,
        }),
        Some("uncertain") => Ok(OptimizationStepOutcome::Uncertain {
            class: stored_class(&value)?,
        }),
        _ => Err(Error::Invalid(
            "invalid durable optimization outcome".into(),
        )),
    }
}

fn optimization_request_input(request: &OptimizationStepRequest<'_>) -> serde_json::Value {
    serde_json::json!({"evidence":request.evidence, "selection":request.source_selection,
        "bindings":request.source_bindings.iter().map(|b| (&b.source_id,&b.source_digest,&b.parent_family)).collect::<Vec<_>>(),
        "traces":request.traces, "context":request.model_context, "parent":request.parent_skill,
        "trusted_edit_context":format!("{:?}",request.edit_context), "edit_template":request.edit_batch_template,
        "protected_ranges":format!("{:?}",request.protected_ranges), "profile":request.bundle_context.profile,
        "baseline":request.bundle_context.baseline,"parent_strategy":request.bundle_context.parent_strategy,
        "baseline_strategy":request.bundle_context.baseline_strategy,"improver":request.bundle_context.improver_patch,
        "caps":request.bundle_context.caps,"revoked":request.bundle_context.revoked,
        "development":request.development_request,"allow_rank":request.allow_rank_call})
}

pub(crate) fn optimization_request_digest(request: &OptimizationStepRequest<'_>) -> Result<String> {
    fingerprint(&optimization_request_input(request))
}

/// Runs one ordinary optimization step. The `Consolidate` base stage belongs to
/// the consolidation run alone (`MonitoringCoordinator::run_consolidation`): any
/// other caller presenting it is refused before anything is journaled or
/// dispatched.
pub async fn run_optimization_step(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: OptimizationStepRequest<'_>,
) -> Result<OptimizationStepOutcome> {
    if request.model_context.stage == ModelStage::Consolidate {
        return Err(Error::Forbidden);
    }
    run_step(model, runner, journal, request).await
}

/// Entry of `MonitoringCoordinator::run_consolidation`. The step must carry the
/// `Consolidate` base stage, which meters every model call of the step under the
/// `Consolidation` budget stage; any other base stage is refused before anything
/// is journaled or dispatched.
pub(crate) async fn run_consolidation_step(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: OptimizationStepRequest<'_>,
) -> Result<OptimizationStepOutcome> {
    if request.model_context.stage != ModelStage::Consolidate {
        return Err(Error::Forbidden);
    }
    run_step(model, runner, journal, request).await
}

async fn run_step(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: OptimizationStepRequest<'_>,
) -> Result<OptimizationStepOutcome> {
    let journal = journal.ok_or(Error::NotFound)?;
    let input = optimization_request_input(&request);
    let context = request.model_context.clone();
    let prepared = recovery_fact(
        &context,
        OptimizationJournalStage::Merge,
        StageFactKind::StepPrepared,
        context.request_id.clone(),
        optimization_request_digest(&request)?,
        input,
    )?;
    let mut terminal = recovery_fact(
        &context,
        OptimizationJournalStage::Merge,
        StageFactKind::StepCompleted,
        context.request_id.clone(),
        prepared.input_digest.clone(),
        serde_json::json!({}),
    )?;
    // No outbound action, including a cached continuation, is allowed without the current grant.
    if validate_outbound_grant(request.source_selection).is_err() {
        if let Some(old) = journal.lookup(&terminal.artifact_id).await? {
            if old.input_digest != terminal.input_digest {
                return Err(Error::Conflict("terminal input differs".into()));
            }
            return decode_outcome(old.payload);
        }
        let class = StepTerminalClass::GrantUnavailable;
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            &class,
            vec![],
        )
        .await?;
        terminal.payload = encode_outcome(&OptimizationStepOutcome::Rejected { class })?;
        journal.commit(terminal.seal()?).await?;
        return Ok(OptimizationStepOutcome::Rejected { class });
    }
    journal
        .verify_sources(
            request.source_selection,
            request.evidence,
            request.source_bindings,
            &request.traces,
            request.model_context.revoke_watermark,
        )
        .await?;
    if request.model_context.namespace != request.development_request.namespace
        || request.model_context.episode_id != request.development_request.episode_id
        || request.model_context.step != request.development_request.step
        || request.model_context.attempt != request.development_request.attempt
        || request.model_context.revoke_watermark != request.development_request.revoke_watermark
    {
        return Err(Error::Conflict("step/development scope differs".into()));
    }
    let recovery = RecoveryJournal {
        inner: journal,
        sources: request.source_selection.run_ids.clone(),
        watermark: context.revoke_watermark,
        step_id: prepared.artifact_id.clone(),
        grant_id: format!("optgrant-{}", fingerprint(request.source_selection)?),
        stage_ids: Default::default(),
    };
    if let Some(old) = recovery.lookup(&prepared.artifact_id).await?
        && old.input_digest != prepared.input_digest
    {
        return Err(Error::Conflict(
            "same optimization key has different full input".into(),
        ));
    }
    if let Some(old) = recovery.lookup(&terminal.artifact_id).await? {
        if old.input_digest != prepared.input_digest {
            return Err(Error::Conflict("terminal input differs".into()));
        }
        return decode_outcome(old.payload);
    }
    recovery.commit(prepared).await?;
    let uncertain = std::sync::Mutex::new(None);
    let model = RecoveryModel {
        port: model.ok_or(Error::NotFound)?,
        journal: &recovery,
        context: &context,
        uncertain: &uncertain,
    };
    let runner = RecoveryRunner {
        runner: runner.ok_or(Error::NotFound)?,
        journal: &recovery,
        context: &context,
        uncertain: &uncertain,
    };
    let result =
        run_optimization_step_inner(Some(&model), Some(&runner), Some(&recovery), request).await;
    let outcome = if let Some(class) = uncertain.into_inner().map_err(|_| Error::Internal)? {
        OptimizationStepOutcome::Uncertain { class }
    } else {
        match result {
            Ok(v) => v,
            // Any other refusal is recorded by the kind of the error, never by its
            // message: it can echo a source, a model answer or a store.
            Err(e) => OptimizationStepOutcome::Rejected {
                class: StepTerminalClass::StepError {
                    error: StepErrorKind::from(&e),
                },
            },
        }
    };
    terminal.payload = encode_outcome(&outcome)?;
    terminal.dependencies.extend(
        recovery
            .stage_ids
            .lock()
            .map_err(|_| Error::Internal)?
            .iter()
            .map(|id| dependency("artifact", id.clone())),
    );
    recovery.commit(terminal.seal()?).await?;
    Ok(outcome)
}

async fn validate_live_fact(
    session: &mut evo_storage::Session,
    context: &Context,
    fact: &StageFact,
) -> Result<()> {
    // Raw late receipts preserve billing/audit truth; every consumer still checks their watermark.
    // What such a receipt holds of a revoked source's answer is not kept: `commit` stores
    // it redacted (`run_dependency_revoked`).
    if fact.kind == StageFactKind::DispatchObserved {
        return Ok(());
    }
    if let Some(w) = fact
        .dependencies
        .iter()
        .find(|d| d.kind == "revoke_watermark")
    {
        let expected = w.id.parse::<u64>().map_err(|_| Error::Internal)?;
        let ids = fact
            .dependencies
            .iter()
            .filter(|d| d.kind == "run")
            .map(|d| d.id.clone())
            .collect::<Vec<_>>();
        crate::evidence::validate_stored_sources(session, context, &ids, expected).await?;
    }
    Ok(())
}

/// Whether `fact` holds the answer of a model: the observation of a model call (the
/// whole `ModelResponse`, or the outcome built from it) of a model stage. The
/// scores-only observation of the development runner holds none, and is not handled.
fn holds_model_output(fact: &StageFact) -> bool {
    matches!(
        fact.kind,
        StageFactKind::DispatchObserved | StageFactKind::ResponseObserved
    ) && fact.stage != OptimizationJournalStage::Development
}

/// Whether a run `fact` depends on was revoked as a run, inside the caller's
/// transaction. The tombstone of the run decides, and nothing else:
///
/// * not the watermark, which any revocation moves (another source's included): a
///   fact over live runs must not be redacted because something unrelated was
///   revoked, and the consumers of such a fact still check the watermark on read;
/// * the kind is compared exactly: a tombstone is keyed by the id of its source
///   alone and records the kind it was written for, so the tombstone of an artifact
///   that shares the id of a run revokes the artifact, not the run, and spares the
///   fact. Every other tombstone revokes: one written for a run, and one that cannot
///   be read as a tombstone `begin_revoke` writes (a kind it does not write, or a
///   body that does not decode) cannot be matched to anything and fails closed, the
///   rule of the other tombstone gates (the management submit check, the budget
///   reservation, dispatch and settlement gates). Production never writes such a
///   tombstone; the rule only decides what a hand-written one means. (The readers of
///   the fact go further: `validate_stored_sources` takes any tombstone for a
///   revocation.)
async fn run_dependency_revoked(
    session: &mut Session,
    context: &Context,
    fact: &StageFact,
) -> Result<bool> {
    for dependency in fact.dependencies.iter().filter(|d| d.kind == "run") {
        let Some(body) = session
            .get::<serde_json::Value>(context, "tombstone", &dependency.id)
            .await?
        else {
            continue;
        };
        let source_kind = serde_json::from_value::<RevokeTombstone>(body)
            .ok()
            .map(|tombstone| tombstone.source_kind);
        if source_kind.as_deref() != Some("artifact") {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod step_terminal_class_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// One value of every variant (and of every typed field that decides a code). A
    /// variant added to the enum has to be named here: [`ordinal`] matches without a
    /// wildcard, so it does not compile until it is.
    fn every_class() -> Vec<StepTerminalClass> {
        use StepTerminalClass as C;
        let mut classes = vec![
            C::NoEligibleReflectionBatch,
            C::NoEditSuggestions,
            C::EditProducedNoChange,
            C::KeepIncumbentNotImproved {
                parent_total_micros: 500_000,
                candidate_total_micros: 400_000,
            },
            C::KeepIncumbentRetentionBroken {
                parent_total_micros: 500_000,
                candidate_total_micros: 900_000,
            },
            C::GrantUnavailable,
            C::SuggestionShapeInvalid,
            C::SuggestionPoolUnranked,
            C::EditCompileFailed,
            C::BundleCompileFailed,
            C::DevelopmentReportRejected,
            C::ModelUsageUnknown,
            C::ModelDispatchUnrecorded,
            C::ModelDispatchConcurrent,
            C::ModelTransportOutcomeUnknown,
            C::DevelopmentExecutionUnrecorded,
            C::DevelopmentExecutionConcurrent,
            C::DevelopmentExecutionOutcomeUnknown,
            C::CancelledOrUncertain,
            C::OptimizationDispatchOutcomeUncertain,
            C::ExplorationSourceClosureChangedAfterDispatch,
            C::ExplorationPrefixChangedAfterDispatch,
            C::ClaimInputsChangedAfterDispatch,
            C::RecoverTargetMissing,
            C::DevelopmentEvidenceUnverified,
            C::CandidateMaterialUnavailable,
            C::CandidateObserved,
            C::FixtureEvidenceNotAccepted,
            C::DevelopmentEvidenceRejected,
            C::LegacyUnclassified,
        ];
        classes.extend(
            [
                ModelRejectionKind::Unauthorized,
                ModelRejectionKind::BudgetUnavailable,
                ModelRejectionKind::InvalidRequest,
                ModelRejectionKind::ProviderRejected,
                ModelRejectionKind::CancelledBeforeDispatch,
                ModelRejectionKind::CancelledAfterDispatch,
            ]
            .map(|kind| C::ModelRejected { kind }),
        );
        classes.extend(
            [
                StepErrorKind::Invalid,
                StepErrorKind::Forbidden,
                StepErrorKind::NotFound,
                StepErrorKind::Budget,
                StepErrorKind::Conflict,
                StepErrorKind::Cancelled,
                StepErrorKind::Internal,
            ]
            .map(|error| C::StepError { error }),
        );
        classes
    }

    /// The position of a variant in the declaration, with no wildcard: it exists to
    /// stop compiling when a variant is added without being listed above.
    fn ordinal(class: &StepTerminalClass) -> usize {
        use StepTerminalClass as C;
        match class {
            C::NoEligibleReflectionBatch => 0,
            C::NoEditSuggestions => 1,
            C::EditProducedNoChange => 2,
            C::KeepIncumbentNotImproved { .. } => 3,
            C::KeepIncumbentRetentionBroken { .. } => 4,
            C::GrantUnavailable => 5,
            C::ModelRejected { .. } => 6,
            C::SuggestionShapeInvalid => 7,
            C::SuggestionPoolUnranked => 8,
            C::EditCompileFailed => 9,
            C::BundleCompileFailed => 10,
            C::DevelopmentReportRejected => 11,
            C::StepError { .. } => 12,
            C::ModelUsageUnknown => 13,
            C::ModelDispatchUnrecorded => 14,
            C::ModelDispatchConcurrent => 15,
            C::ModelTransportOutcomeUnknown => 16,
            C::DevelopmentExecutionUnrecorded => 17,
            C::DevelopmentExecutionConcurrent => 18,
            C::DevelopmentExecutionOutcomeUnknown => 19,
            C::CancelledOrUncertain => 20,
            C::OptimizationDispatchOutcomeUncertain => 21,
            C::ExplorationSourceClosureChangedAfterDispatch => 22,
            C::ExplorationPrefixChangedAfterDispatch => 23,
            C::ClaimInputsChangedAfterDispatch => 24,
            C::RecoverTargetMissing => 25,
            C::DevelopmentEvidenceUnverified => 26,
            C::CandidateMaterialUnavailable => 27,
            C::CandidateObserved => 28,
            C::FixtureEvidenceNotAccepted => 29,
            C::DevelopmentEvidenceRejected => 30,
            C::LegacyUnclassified => 31,
        }
    }

    #[test]
    fn every_variant_is_listed_and_has_its_own_fixed_snake_case_code() {
        let classes = every_class();
        let variants: BTreeSet<usize> = classes.iter().map(ordinal).collect();
        assert_eq!(variants, (0..=31).collect::<BTreeSet<_>>());
        let codes: BTreeSet<&str> = classes.iter().map(StepTerminalClass::code).collect();
        assert_eq!(codes.len(), classes.len(), "two classes share a code");
        for code in &codes {
            assert!(
                !code.is_empty()
                    && code.len() <= 64
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{code}"
            );
        }
    }

    #[test]
    fn the_serialized_tag_of_a_class_is_its_code_unless_a_typed_field_decides_the_code() {
        for class in every_class() {
            let value = serde_json::to_value(class).unwrap();
            let tag = value["class"].as_str().unwrap();
            match class {
                StepTerminalClass::ModelRejected { kind } => {
                    assert_eq!(tag, "model_rejected");
                    assert_eq!(
                        class.code(),
                        format!("model_rejected_{}", rejection_kind_code(kind))
                    );
                    assert_eq!(value["kind"], json!(rejection_kind_code(kind)));
                }
                StepTerminalClass::StepError { error } => {
                    assert_eq!(tag, "step_error");
                    assert_eq!(class.code(), format!("step_error_{}", error.code()));
                    assert_eq!(value["error"], json!(error.code()));
                }
                _ => assert_eq!(tag, class.code(), "{class:?}"),
            }
        }
    }

    #[test]
    fn a_class_round_trips_through_json_and_is_read_strictly() {
        for class in every_class() {
            let value = serde_json::to_value(class).unwrap();
            assert_eq!(
                serde_json::from_value::<StepTerminalClass>(value).unwrap(),
                class
            );
        }
        // The totals of a kept incumbent are numbers and nothing else.
        let kept = StepTerminalClass::KeepIncumbentNotImproved {
            parent_total_micros: 7,
            candidate_total_micros: 3,
        };
        assert_eq!(
            serde_json::to_value(kept).unwrap(),
            json!({
                "class": "keep_incumbent_not_improved",
                "parent_total_micros": 7,
                "candidate_total_micros": 3,
            })
        );
        // A class that is not one of the closed set, a typed field outside its set or of
        // the wrong type, a typed field that is missing and a value that is no object
        // are refused. (A field the class does not have is not read, so no text under
        // another name is carried by it.)
        for forged in [
            json!({"class": "something_else"}),
            json!({"class": "model_rejected", "kind": "politely_declined"}),
            json!({"class": "model_rejected"}),
            json!({"class": "step_error", "error": "boom"}),
            json!({"class": "keep_incumbent_not_improved", "parent_total_micros": "7"}),
            json!({"class": "keep_incumbent_retention_broken", "parent_total_micros": 7}),
            json!("no_edit_suggestions"),
            json!(null),
        ] {
            assert!(
                serde_json::from_value::<StepTerminalClass>(forged.clone()).is_err(),
                "{forged}"
            );
        }
    }

    #[test]
    fn an_error_is_kept_as_its_kind_alone() {
        for (error, kind, code) in [
            (
                Error::Invalid("echoes a source".into()),
                StepErrorKind::Invalid,
                "invalid",
            ),
            (Error::Forbidden, StepErrorKind::Forbidden, "forbidden"),
            (Error::NotFound, StepErrorKind::NotFound, "not_found"),
            (Error::Budget, StepErrorKind::Budget, "budget"),
            (
                Error::Conflict("echoes a store".into()),
                StepErrorKind::Conflict,
                "conflict",
            ),
            (Error::Cancelled, StepErrorKind::Cancelled, "cancelled"),
            (Error::Internal, StepErrorKind::Internal, "internal"),
        ] {
            assert_eq!(StepErrorKind::from(&error), kind);
            assert_eq!(kind.code(), code);
        }
    }

    #[test]
    fn a_terminal_payload_is_the_status_the_code_and_the_class_and_reads_back() {
        for class in every_class() {
            for (status, outcome) in [
                ("no_change", OptimizationStepOutcome::NoChange { class }),
                ("rejected", OptimizationStepOutcome::Rejected { class }),
                ("uncertain", OptimizationStepOutcome::Uncertain { class }),
            ] {
                let payload = encode_outcome(&outcome).unwrap();
                assert_eq!(payload["status"], status);
                assert_eq!(payload["reason"], class.code());
                assert_eq!(payload["class"], serde_json::to_value(class).unwrap());
                let read = match (status, decode_outcome(payload).unwrap()) {
                    ("no_change", OptimizationStepOutcome::NoChange { class }) => class,
                    ("rejected", OptimizationStepOutcome::Rejected { class }) => class,
                    ("uncertain", OptimizationStepOutcome::Uncertain { class }) => class,
                    (status, other) => panic!("{status} read back as {other:?}"),
                };
                assert_eq!(read, class);
            }
        }
    }

    #[test]
    fn a_terminal_journaled_as_free_text_reads_as_unclassified_and_carries_no_text() {
        for (status, free_text) in [
            ("no_change", "model returned no edit suggestions"),
            (
                "rejected",
                "invalid input: invalid optimizer suggestions: unknown field `x`",
            ),
            (
                "uncertain",
                "model transport outcome unknown: state conflict: words",
            ),
        ] {
            let read = decode_outcome(json!({"status": status, "reason": free_text})).unwrap();
            let class = match read {
                OptimizationStepOutcome::NoChange { class }
                | OptimizationStepOutcome::Rejected { class }
                | OptimizationStepOutcome::Uncertain { class } => class,
                OptimizationStepOutcome::Candidate { .. } => panic!("{status}"),
            };
            assert_eq!(class, StepTerminalClass::LegacyUnclassified);
            assert_eq!(class.code(), "legacy_unclassified");
        }
        // A class that is not one of the closed set is a corrupt terminal, not a text.
        assert!(matches!(
            decode_outcome(json!({"status": "rejected", "reason": "x", "class": {"class": "x"}})),
            Err(Error::Internal)
        ));
        // And an unknown status is refused as it always was.
        assert!(decode_outcome(json!({"status": "approved", "reason": "x"})).is_err());
    }

    /// A development request and its report over `results.len()` tasks:
    /// `(parent score, candidate score, parent passed, candidate passed)` per task.
    fn development_pair(
        results: &[(u32, u32, bool, bool)],
    ) -> (DevelopmentRunRequest, DevelopmentRunReport) {
        let tasks: Vec<DevelopmentTask> = (0..results.len())
            .map(|index| DevelopmentTask {
                id: format!("task-{index}"),
                parent_family: "family".into(),
                input_digest: evo_core::hash(format!("task-{index}").as_bytes()),
            })
            .collect();
        let manifest = DevelopmentManifest::build("manifest", tasks).unwrap();
        let request = DevelopmentRunRequest {
            request_id: "development-request".into(),
            namespace: "n".into(),
            purpose: Purpose::Development,
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            manifest: manifest.clone(),
            parent_bundle_digest: evo_core::hash(b"parent"),
            candidate_bundle_digest: evo_core::hash(b"candidate"),
            environment_digest: evo_core::hash(b"environment"),
            grader_digest: evo_core::hash(b"grader"),
            rules_digest: evo_core::hash(b"rules"),
            tools_digest: evo_core::hash(b"tools"),
            revoke_watermark: 1,
            idempotency_key: "idempotency".into(),
        };
        let report = DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results: results
                .iter()
                .enumerate()
                .map(
                    |(index, (parent, candidate, parent_passed, candidate_passed))| {
                        PairedTaskResult {
                            task_id: format!("task-{index}"),
                            parent_score_micros: *parent,
                            candidate_score_micros: *candidate,
                            parent_passed: *parent_passed,
                            candidate_passed: *candidate_passed,
                            parent_execution_id: format!("parent-{index}"),
                            candidate_execution_id: format!("candidate-{index}"),
                            grader_receipt_digest: evo_core::hash(
                                format!("grade-{index}").as_bytes(),
                            ),
                        }
                    },
                )
                .collect(),
            execution_receipt_id: "execution".into(),
            usage_record_ids: vec![],
            provenance: DevelopmentExecutionProvenance::Fixture,
        };
        (request, report)
    }

    #[test]
    fn a_kept_incumbent_is_told_apart_by_the_retention_of_passes_and_carries_only_the_totals() {
        use DevelopmentSelectionDecision::{AcceptCandidate, KeepIncumbent};
        // `(results, decision, preserves passes, class)`.
        type Case = (
            Vec<(u32, u32, bool, bool)>,
            DevelopmentSelectionDecision,
            bool,
            Option<StepTerminalClass>,
        );
        let cases: Vec<Case> = vec![
            // Strictly better with every pass kept: the candidate is accepted.
            (
                vec![(500_000, 900_000, true, true)],
                AcceptCandidate,
                true,
                None,
            ),
            // Equal totals, every pass kept: not improved.
            (
                vec![(500_000, 500_000, true, true)],
                KeepIncumbent,
                true,
                Some(StepTerminalClass::KeepIncumbentNotImproved {
                    parent_total_micros: 500_000,
                    candidate_total_micros: 500_000,
                }),
            ),
            // A lower total, every pass kept: not improved.
            (
                vec![(500_000, 100_000, false, false)],
                KeepIncumbent,
                true,
                Some(StepTerminalClass::KeepIncumbentNotImproved {
                    parent_total_micros: 500_000,
                    candidate_total_micros: 100_000,
                }),
            ),
            // A higher total that loses a task the incumbent passed: retention broken.
            (
                vec![(400_000, 900_000, true, false)],
                KeepIncumbent,
                false,
                Some(StepTerminalClass::KeepIncumbentRetentionBroken {
                    parent_total_micros: 400_000,
                    candidate_total_micros: 900_000,
                }),
            ),
            // The same over two tasks, one lost and one gained.
            (
                vec![
                    (400_000, 100_000, true, false),
                    (100_000, 900_000, false, true),
                ],
                KeepIncumbent,
                false,
                Some(StepTerminalClass::KeepIncumbentRetentionBroken {
                    parent_total_micros: 500_000,
                    candidate_total_micros: 1_000_000,
                }),
            ),
            // A lower total that also loses a pass: retention is what broke.
            (
                vec![(500_000, 100_000, true, false)],
                KeepIncumbent,
                false,
                Some(StepTerminalClass::KeepIncumbentRetentionBroken {
                    parent_total_micros: 500_000,
                    candidate_total_micros: 100_000,
                }),
            ),
        ];
        for (results, decision, preserves, class) in cases {
            let (request, report) = development_pair(&results);
            let (selection, preserved) = select_development_detailed(&request, &report).unwrap();
            assert_eq!(selection.decision, decision, "{results:?}");
            assert_eq!(preserved, preserves, "{results:?}");
            // The public selection is the same one.
            let public = select_development(&request, &report).unwrap();
            assert_eq!(
                serde_json::to_value(&public).unwrap(),
                serde_json::to_value(&selection).unwrap(),
                "{results:?}"
            );
            if decision == KeepIncumbent {
                assert_eq!(
                    Some(keep_incumbent_class(&selection, preserved)),
                    class,
                    "{results:?}"
                );
            }
        }
    }

    #[test]
    fn a_journaled_rejection_keeps_its_kind_and_receipt_and_never_the_providers_words() {
        let receipt = ModelExecutionReceipt {
            call_id: "call".into(),
            dispatch_id: "dispatch".into(),
            root_budget_id: "root".into(),
            provider_request_id: "provider".into(),
            usage_record_id: "usage".into(),
            provenance: crate::model::ModelExecutionProvenance::Fixture,
        };
        for kind in [
            ModelRejectionKind::Unauthorized,
            ModelRejectionKind::BudgetUnavailable,
            ModelRejectionKind::InvalidRequest,
            ModelRejectionKind::ProviderRejected,
            ModelRejectionKind::CancelledBeforeDispatch,
            ModelRejectionKind::CancelledAfterDispatch,
        ] {
            let dispatch = match kind {
                ModelRejectionKind::ProviderRejected
                | ModelRejectionKind::CancelledAfterDispatch => {
                    crate::model::RejectedDispatch::Dispatched {
                        receipt: receipt.clone(),
                    }
                }
                _ => crate::model::RejectedDispatch::NotDispatched,
            };
            let rejected = ModelResponse::Rejected {
                request_id: "request".into(),
                kind,
                reason: "the provider's own words".into(),
                dispatch: dispatch.clone(),
            };
            let kept = journaled_response(&rejected).unwrap();
            assert!(!kept.to_string().contains("own words"), "{kept}");
            // It is still a rejection that reads back, with the same kind and dispatch,
            // and its reason is a non-empty text (the model response contract).
            let read: ModelResponse = serde_json::from_value(kept).unwrap();
            let ModelResponse::Rejected {
                kind: read_kind,
                reason,
                dispatch: read_dispatch,
                ..
            } = read
            else {
                panic!("a rejection reads back as something else");
            };
            assert_eq!((read_kind, read_dispatch), (kind, dispatch));
            assert_eq!(reason, rejection_kind_code(kind));
        }
        // An answer that completed is the model's work product and is kept whole.
        let completed = ModelResponse::Completed {
            request_id: "request".into(),
            response_id: "response".into(),
            actual_model_digest: evo_core::hash(b"model"),
            input_digest: evo_core::hash(b"input"),
            output: "[]".into(),
            output_digest: evo_core::hash(b"[]"),
            execution_receipt: receipt,
        };
        assert_eq!(
            journaled_response(&completed).unwrap(),
            serde_json::to_value(&completed).unwrap()
        );
    }
}
