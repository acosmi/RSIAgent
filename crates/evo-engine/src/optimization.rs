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
use evo_storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::compiler::compile_skill_edits;
use crate::model::{ModelExecutionReceipt, ModelPort, ModelResponse};

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
    IsolatedRunner,
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
    Ok(DevelopmentSelection {
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
    })
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

fn validate_development_request(request: &DevelopmentRunRequest) -> Result<()> {
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
    Rejected {
        reason: String,
        dependency_ids: Vec<String>,
    },
    Uncertain {
        reason: String,
        dependency_ids: Vec<String>,
    },
}

pub async fn request_suggestions(
    port: Option<&dyn ModelPort>,
    request: ModelRequest,
    batches: &[ReflectionBatch],
) -> Result<SuggestionOutcome> {
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
        ModelResponse::Rejected {
            reason, dispatch, ..
        } => {
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
            return Ok(SuggestionOutcome::Rejected {
                reason,
                dependency_ids,
            });
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
            return Ok(SuggestionOutcome::Uncertain {
                reason: "model dispatch or usage remains uncertain".into(),
                dependency_ids,
            });
        }
    };
    let suggestions: Vec<EditSuggestion> = serde_json::from_str(&output)
        .map_err(|error| Error::Invalid(format!("invalid optimizer suggestions: {error}")))?;
    if suggestions.is_empty() {
        return Ok(SuggestionOutcome::NoChange {
            response_id,
            output,
            output_digest,
            receipt: execution_receipt,
        });
    }
    if suggestions.len() > request.max_suggestions || suggestions.len() > MAX_SUGGESTIONS {
        return Err(Error::Invalid(
            "optimizer suggestion pool must contain one to the requested maximum".into(),
        ));
    }
    validate_suggestion_pool(batches, &suggestions)?;
    Ok(SuggestionOutcome::Suggestions {
        items: suggestions,
        response_id,
        output,
        output_digest,
        receipt: execution_receipt,
    })
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
        let fact: Option<StageFact> = session.get(&self.context, "artifact", artifact_id).await?;
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

#[derive(Debug)]
pub enum OptimizationStepOutcome {
    NoChange {
        reason: String,
    },
    Rejected {
        reason: String,
    },
    Uncertain {
        reason: String,
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
    if let Err(error) = validate_outbound_grant(request.source_selection) {
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            "model excerpt grant is absent or incompatible",
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::Rejected {
            reason: error.to_string(),
        });
    }
    let reflection =
        build_reflection_batches(request.evidence, request.source_bindings, request.traces)?;
    if reflection.batches.is_empty() {
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            "no eligible development reflection batch",
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange {
            reason: "no eligible development reflection batch".into(),
        });
    }

    let per_batch_limit = MAX_SUGGESTIONS / reflection.batches.len();
    let mut suggestions = Vec::new();
    for (index, batch) in reflection.batches.iter().enumerate() {
        let stage = match batch.kind {
            ReflectionBatchKind::Failure => ModelStage::ReflectFailure,
            ReflectionBatchKind::Success => ModelStage::ReflectSuccess,
        };
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
        let outcome = match request_suggestions(
            Some(model),
            model_request.clone(),
            std::slice::from_ref(batch),
        )
        .await
        {
            Ok(outcome) => outcome,
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
                reason,
                dependency_ids,
            } => {
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
                    &reason,
                    dependency_ids,
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected { reason });
            }
            SuggestionOutcome::Uncertain {
                reason,
                dependency_ids,
            } => {
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
                    &reason,
                    dependency_ids,
                )
                .await?;
                return Ok(OptimizationStepOutcome::Uncertain { reason });
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
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            "model returned no edit suggestions",
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange {
            reason: "model returned no edit suggestions".into(),
        });
    }
    if suggestions.len() > MAX_FINAL_EDITS && !request.allow_rank_call {
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            "suggestion pool exceeds four and ranking was not preregistered",
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::Rejected {
            reason: "suggestion pool exceeds four and ranking was not preregistered".into(),
        });
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
            Err(error) => {
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &format!("atomic skill edit rejected: {error}"),
                    vec![],
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected {
                    reason: error.to_string(),
                });
            }
        }
    };
    if compiled.report.status == evo_core::skill_edit::EditApplyStatus::NoChange {
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalNoChange,
            "atomic skill edit produced no change",
            vec![],
        )
        .await?;
        return Ok(OptimizationStepOutcome::NoChange {
            reason: "atomic skill edit produced no change".into(),
        });
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
            Err(error) => {
                commit_terminal(
                    journal,
                    &request.model_context,
                    StageFactKind::TerminalRejected,
                    &format!("complete bundle compilation failed: {error}"),
                    vec![],
                )
                .await?;
                return Ok(OptimizationStepOutcome::Rejected {
                    reason: error.to_string(),
                });
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
    let selection = match select_development(&development_request, &report) {
        Ok(selection) => selection,
        Err(error) => {
            commit_terminal(
                journal,
                &request.model_context,
                StageFactKind::TerminalRejected,
                &format!("development report rejected: {error}"),
                vec![bundle.digest.clone()],
            )
            .await?;
            return Ok(OptimizationStepOutcome::Rejected {
                reason: error.to_string(),
            });
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
            dependencies: report
                .results
                .iter()
                .flat_map(|result| {
                    [
                        dependency("execution", result.parent_execution_id.clone()),
                        dependency("execution", result.candidate_execution_id.clone()),
                        dependency("grader", result.grader_receipt_digest.clone()),
                    ]
                })
                .collect(),
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
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            &selection.reason,
            vec![bundle.digest],
        )
        .await?;
        Ok(OptimizationStepOutcome::Rejected {
            reason: selection.reason,
        })
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
            stage: ModelStage::Rank,
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

async fn commit_terminal(
    journal: &dyn OptimizationJournal,
    context: &ModelRequestContext,
    kind: StageFactKind,
    reason: &str,
    dependencies: Vec<String>,
) -> Result<()> {
    let payload = serde_json::json!({"reason": reason});
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
    uncertain: &'a std::sync::Mutex<Option<String>>,
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
                    Some("model usage remains unknown".into());
            }
            return Ok(response);
        }
        if let Some(old) = self.journal.lookup(&prepared.artifact_id).await? {
            if old.input_digest != prepared.input_digest {
                return Err(Error::Conflict("model recovery input differs".into()));
            }
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some("prepared model dispatch has no durable response; usage unknown".into());
            return Err(Error::Conflict(
                "model dispatch uncertain; automatic retry forbidden".into(),
            ));
        }
        if !self.journal.claim(prepared.clone()).await? {
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some("concurrent model dispatch remains uncertain".into());
            return Err(Error::Conflict("model dispatch already claimed".into()));
        }
        let response = match self.port.dispatch(request.clone()).await {
            Ok(r) => r,
            Err(e) => {
                *self.uncertain.lock().map_err(|_| Error::Internal)? =
                    Some(format!("model transport outcome unknown: {e}"));
                return Err(e);
            }
        };
        prepared.kind = StageFactKind::DispatchObserved;
        prepared.payload = serde_json::to_value(&response).map_err(|_| Error::Internal)?;
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
                Some("model usage remains unknown".into());
        }
        response.validate_against(&request)?;
        Ok(response)
    }
}

struct RecoveryRunner<'a> {
    runner: &'a dyn DevRunner,
    journal: &'a RecoveryJournal<'a>,
    context: &'a ModelRequestContext,
    uncertain: &'a std::sync::Mutex<Option<String>>,
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
            *self.uncertain.lock().map_err(|_| Error::Internal)? = Some(
                "prepared development execution has no durable result; execution and usage unknown"
                    .into(),
            );
            return Err(Error::Conflict(
                "development execution uncertain; automatic retry forbidden".into(),
            ));
        }
        if !self.journal.claim(prepared).await? {
            *self.uncertain.lock().map_err(|_| Error::Internal)? =
                Some("concurrent development execution remains uncertain".into());
            return Err(Error::Conflict(
                "development execution already claimed".into(),
            ));
        }
        let report = match self.runner.run(request.clone()).await {
            Ok(r) => r,
            Err(e) => {
                *self.uncertain.lock().map_err(|_| Error::Internal)? =
                    Some(format!("development execution outcome unknown: {e}"));
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
        OptimizationStepOutcome::Rejected { reason } => {
            serde_json::json!({"status":"rejected", "reason":reason})
        }
        OptimizationStepOutcome::NoChange { reason } => {
            serde_json::json!({"status":"no_change", "reason":reason})
        }
        OptimizationStepOutcome::Uncertain { reason } => {
            serde_json::json!({"status":"uncertain", "reason":reason})
        }
    })
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
            reason: field(&value, "reason")?,
        }),
        Some("no_change") => Ok(OptimizationStepOutcome::NoChange {
            reason: field(&value, "reason")?,
        }),
        Some("uncertain") => Ok(OptimizationStepOutcome::Uncertain {
            reason: field(&value, "reason")?,
        }),
        _ => Err(Error::Invalid(
            "invalid durable optimization outcome".into(),
        )),
    }
}

pub async fn run_optimization_step(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: OptimizationStepRequest<'_>,
) -> Result<OptimizationStepOutcome> {
    let journal = journal.ok_or(Error::NotFound)?;
    let input = serde_json::json!({"evidence":request.evidence, "selection":request.source_selection,
        "bindings":request.source_bindings.iter().map(|b| (&b.source_id,&b.source_digest,&b.parent_family)).collect::<Vec<_>>(),
        "traces":request.traces, "context":request.model_context, "parent":request.parent_skill,
        "trusted_edit_context":format!("{:?}",request.edit_context), "edit_template":request.edit_batch_template,
        "protected_ranges":format!("{:?}",request.protected_ranges), "profile":request.bundle_context.profile,
        "baseline":request.bundle_context.baseline,"parent_strategy":request.bundle_context.parent_strategy,
        "baseline_strategy":request.bundle_context.baseline_strategy,"improver":request.bundle_context.improver_patch,
        "caps":request.bundle_context.caps,"revoked":request.bundle_context.revoked,
        "development":request.development_request,"allow_rank":request.allow_rank_call});
    let context = request.model_context.clone();
    let prepared = recovery_fact(
        &context,
        OptimizationJournalStage::Merge,
        StageFactKind::StepPrepared,
        context.request_id.clone(),
        fingerprint(&input)?,
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
        commit_terminal(
            journal,
            &request.model_context,
            StageFactKind::TerminalRejected,
            "model excerpt grant is absent or incompatible",
            vec![],
        )
        .await?;
        terminal.payload = encode_outcome(&OptimizationStepOutcome::Rejected {
            reason: "model excerpt grant is absent or incompatible".into(),
        })?;
        journal.commit(terminal.seal()?).await?;
        return Ok(OptimizationStepOutcome::Rejected {
            reason: "model excerpt grant is absent or incompatible".into(),
        });
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
    let outcome = if let Some(reason) = uncertain.into_inner().map_err(|_| Error::Internal)? {
        OptimizationStepOutcome::Uncertain { reason }
    } else {
        match result {
            Ok(v) => v,
            Err(e) => OptimizationStepOutcome::Rejected {
                reason: e.to_string(),
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
