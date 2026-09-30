//! Persistent deployment monitoring and bounded cross-cycle consolidation.
//!
//! Facts are derived from stored Host and E03 evidence. Monitoring never
//! consumes hidden formal details, publishes a release, or mutates Active.
//!
//! Revocation: every object written here is `kind = "artifact"` and is classified
//! by schema in the cleanup matching of `evo-storage` (`lifecycle.rs`). Content
//! derived from sources (environment and observation identities, cycles and their
//! report bindings, claims, proposals) is redacted; the scope index, terminal run
//! records (outcome and the budget calls they reconcile) and environment-drift
//! facts are preserved. A new monitoring schema must be classified there too, or a
//! revoked run that reaches it fails its cleanup job closed.

use crate::broker::{BudgetPortBinding, ModelTransport, PersistentModelBroker};
use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::model::{ModelPort, ModelResponse, RejectedDispatch};
use crate::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentRunReport, DevelopmentSelection,
    DevelopmentSelectionDecision, OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournal,
    OptimizationJournalStage, OptimizationStepOutcome, OptimizationStepRequest, StageFact,
    StageFactKind, optimization_request_digest, run_optimization_step,
};
use crate::release_store::{
    HostApplicationRecord, ReleaseCandidateRecord, ReleaseStore, RunApplicationSnapshot,
    StageBundleRequest, TrustedHostExecutionReceipt, TypedSourceRef,
};
use crate::releases::validate_resolved_bundle_identity;
use async_trait::async_trait;
use evo_core::contract::ResolvedBundle;
use evo_core::evidence::Purpose;
use evo_core::optimization::{ModelRequest, ModelRequestContext, TraceOutcome};
use evo_core::skill_edit::skill_snapshot_digest;
use evo_core::strategy::{ConsolidationClass, classify_consolidation_pair};
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const ENVIRONMENT_IDENTITY_SCHEMA: &str = "rsia.monitoring.environment.v1";
pub const DEPLOYMENT_OBSERVATION_SCHEMA: &str = "rsia.monitoring.observation.v1";
/// Current cycle schema. `v1` kept a score-biased `pair_classes` list without task
/// ids; it is refused when read (there is no production writer to migrate).
pub const DEVELOPMENT_CYCLE_SCHEMA: &str = "rsia.monitoring.development_cycle.v2";
pub const DEVELOPMENT_REPORT_BINDING_SCHEMA: &str = "rsia.monitoring.development_report_binding.v1";
pub const CONSOLIDATION_SCOPE_SCHEMA: &str = "rsia.monitoring.consolidation_scope.v1";
pub const CONSOLIDATION_CLAIM_SCHEMA: &str = "rsia.monitoring.consolidation_claim.v1";
pub const CONSOLIDATION_PROPOSAL_SCHEMA: &str = "rsia.monitoring.consolidation_proposal.v1";
pub const CONSOLIDATION_RUN_SCHEMA: &str = "rsia.monitoring.consolidation_run.v1";
pub const ENVIRONMENT_DRIFT_SCHEMA: &str = "rsia.monitoring.environment_drift.v1";

const LEGACY_DEVELOPMENT_CYCLE_SCHEMA_V1: &str = "rsia.monitoring.development_cycle.v1";
/// Body schema that source-revocation cleanup leaves behind for redacted content.
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
const ARTIFACT_KIND: &str = "artifact";
const RECEIPT_KIND: &str = "receipt";

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

fn canonical_strings(mut values: Vec<String>, name: &str) -> Result<Vec<String>> {
    for value in &values {
        identifier(value)?;
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::Conflict(format!("duplicate {name}")));
    }
    Ok(values)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentIdentityRecord {
    pub id: String,
    pub schema_version: String,
    pub identity_digest: String,
    pub namespace: String,
    pub evidence_scope: EnvironmentEvidenceScope,
    pub profile_id: String,
    pub release_id: Option<String>,
    pub bundle_digest: Option<String>,
    pub environment_digest: String,
    pub model_identity: String,
    pub ordered_tools: Vec<String>,
    pub host_id: String,
    pub host_version: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    pub mandatory_context_digest: String,
    pub development_manifest_digest: String,
    pub grader_digest: String,
    pub generation_strategy_digest: String,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentEvidenceScope {
    TrustedExecution,
    ProgramFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    NotSelected,
    OfferedNotAttached,
    AttachedNotUsed,
    Used,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityStatus {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentObservation {
    pub id: String,
    pub schema_version: String,
    pub run_id: String,
    pub environment_id: String,
    pub snapshot_id: String,
    pub application_id: String,
    pub request_digest: String,
    pub execution_receipt_id: Option<String>,
    pub application_status: ApplicationStatus,
    pub quality_status: QualityStatus,
}

#[derive(Debug, Clone)]
pub struct RecordEnvironmentRequest {
    pub run_id: String,
    pub development_manifest_digest: String,
    pub grader_digest: String,
    pub generation_strategy_digest: String,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEnvironment {
    pub environment: EnvironmentIdentityRecord,
    pub observation: DeploymentObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDependency {
    pub id: String,
    pub content_digest: String,
}

/// One manifest task before and after the candidate within a completed cycle.
///
/// `class` is the pure pass/fail transition from `classify_consolidation_pair`
/// (plan §11.5, V091.c). The scores are recorded evidence only: a rise in score
/// never turns a failure into an improvement or a pass into a non-regression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPairV1 {
    pub task_id: String,
    pub parent_passed: bool,
    pub candidate_passed: bool,
    pub class: ConsolidationClass,
    pub parent_score_micros: u32,
    pub candidate_score_micros: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationScope {
    pub namespace: String,
    pub skill_id: String,
    pub profile_id: String,
    pub development_manifest_digest: String,
    pub environment_id: String,
    pub environment_identity_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub generation_strategy_digest: String,
    pub parent_skill_digest: String,
    pub parent_bundle_digest: String,
    pub billing_scope: String,
    pub root_budget_id: String,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentCycleRecord {
    pub id: String,
    pub schema_version: String,
    pub scope_id: String,
    pub ordinal: u64,
    pub report_fact_id: String,
    pub report_digest: String,
    pub request_id: String,
    pub candidate_bundle_digest: String,
    pub execution_receipt_id: String,
    pub usage_record_ids: Vec<String>,
    pub provenance: DevelopmentExecutionProvenance,
    /// One entry per reported task, in report order (`v1` kept a score-biased
    /// `pair_classes` list without task ids instead).
    pub pairs: Vec<TaskPairV1>,
    pub has_contrast: bool,
    pub sources: Vec<SourceDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentReportBinding {
    schema_version: String,
    id: String,
    report_fact_id: String,
    scope_id: String,
    cycle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationScopeIndex {
    pub id: String,
    pub schema_version: String,
    pub scope: ConsolidationScope,
    pub cycle_ids: Vec<String>,
    pub report_fact_ids: Vec<String>,
    pub claim_ids: BTreeMap<u64, String>,
    pub invalidated_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompleteDevelopmentCycleRequest {
    pub skill_id: String,
    pub profile_id: String,
    pub environment_id: String,
    pub development_manifest_digest: String,
    pub grader_digest: String,
    pub generation_strategy_digest: String,
    pub parent_skill_digest: String,
    pub parent_bundle_digest: String,
    pub billing_scope: String,
    pub root_budget_id: String,
    pub report_fact_id: String,
    pub source_ids: Vec<String>,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationClaimState {
    Claimed,
    NoContrast,
    Running,
    CompletedNoChange,
    CompletedCandidate,
    CompletedRejected,
    CompletedUncertain,
    CompletedRevoked,
    BlockedBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationClaim {
    pub id: String,
    pub schema_version: String,
    pub scope_id: String,
    pub generation: u64,
    pub cycle_ids: Vec<String>,
    pub sources: Vec<SourceDependency>,
    pub revoke_watermark: u64,
    pub state: ConsolidationClaimState,
    pub execution_input_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    NotEligible { completed_cycles: usize },
    Claimed(ConsolidationClaim),
    AlreadyClaimed(ConsolidationClaim),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationRunOutcome {
    NoChange,
    Candidate,
    Rejected,
    Uncertain,
    Revoked,
    BlockedBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationRunRecord {
    pub id: String,
    pub schema_version: String,
    pub claim_id: String,
    pub input_digest: Option<String>,
    pub outcome: ConsolidationRunOutcome,
    pub reason: String,
    pub budget_call_ids: Vec<String>,
    /// Set exactly when `outcome` is `Candidate`: the durable proposal that was
    /// written before this terminal record. Records written before proposals
    /// existed deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
}

/// The durable form of a `Candidate` consolidation outcome (plan §11.5). It
/// freezes the candidate bundle and the evidence it came from so a later
/// release candidate is staged from stored facts, never from a caller's memory.
/// A proposal is not an approval: the complete candidate chain (development
/// selection, formal evaluation, human approval) still applies and Active is
/// never rewritten here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationProposal {
    pub id: String,
    pub schema_version: String,
    pub claim_id: String,
    pub scope_id: String,
    pub cycle_ids: Vec<String>,
    /// Fingerprint of the per-task pairings of the claim's two cycles.
    pub pairs_digest: String,
    pub parent_skill_digest: String,
    pub parent_bundle_digest: String,
    pub candidate_skill_digest: String,
    pub candidate_bundle: ResolvedBundle,
    pub candidate_bundle_digest: String,
    pub selection_digest: String,
    /// The optimization journal's terminal `StepCompleted` fact this candidate
    /// was read from; it also holds the edit, patch and report.
    pub terminal_fact_id: String,
    pub sources: Vec<SourceDependency>,
    /// `ProgramFixture` when the environment or any backing cycle is fixture
    /// evidence. Fixture proposals may be staged; formal approval still refuses
    /// fixture evidence (`ReleaseStore::approve_verified`).
    pub provenance: EnvironmentEvidenceScope,
    pub environment_digest: String,
    pub revoke_watermark: u64,
}

/// Result of staging a proposal as a release candidate. No release, pointer or
/// Active state exists after staging; `provenance` is the proposal's.
#[derive(Debug, Clone)]
pub struct StagedConsolidationCandidate {
    pub candidate: ReleaseCandidateRecord,
    pub proposal_id: String,
    pub provenance: EnvironmentEvidenceScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDriftRecord {
    pub id: String,
    pub schema_version: String,
    pub previous_environment_id: String,
    pub current_environment_id: String,
    pub previous_identity_digest: String,
    pub current_identity_digest: String,
    pub invalidated_scope_id: String,
    pub reason: String,
}

pub struct MonitoringCoordinator;

#[async_trait]
pub trait ConsolidationModelPort: ModelPort {
    async fn trusted_budget_binding(&self, namespace: &str) -> Result<BudgetPortBinding>;
}

#[async_trait]
impl<T: ModelTransport> ConsolidationModelPort for PersistentModelBroker<T> {
    async fn trusted_budget_binding(&self, namespace: &str) -> Result<BudgetPortBinding> {
        self.budget_binding(namespace).await
    }
}

#[async_trait]
pub trait ConsolidationDevRunner: DevRunner {
    async fn trusted_budget_binding(&self, namespace: &str) -> Result<BudgetPortBinding>;
}

impl MonitoringCoordinator {
    pub async fn record_environment_from_run(
        ctx: &Context,
        store: &Store,
        request: RecordEnvironmentRequest,
    ) -> Result<RecordedEnvironment> {
        ctx.require(&[Role::Host, Role::Admin])?;
        identifier(&request.run_id)?;
        for (value, name) in [
            (
                &request.development_manifest_digest,
                "development manifest digest",
            ),
            (&request.grader_digest, "grader digest"),
            (
                &request.generation_strategy_digest,
                "generation strategy digest",
            ),
        ] {
            validate_digest(value, name)?;
        }
        let mut session = store.session().await?;
        let snapshot =
            ReleaseStore::validate_run_snapshot_live_in_session(ctx, &mut session, &request.run_id)
                .await?;
        let application_id = format!("host-application-{}", request.run_id);
        let application: HostApplicationRecord =
            session.need(ctx, RECEIPT_KIND, &application_id).await?;
        validate_host_application(&mut session, ctx, &snapshot, &application).await?;
        require_watermark(&mut session, ctx, request.revoke_watermark).await?;

        let evidence_scope = if application.execution_receipt_id.is_some() {
            EnvironmentEvidenceScope::TrustedExecution
        } else {
            EnvironmentEvidenceScope::ProgramFixture
        };
        let uncertain_run_scope = (evidence_scope == EnvironmentEvidenceScope::ProgramFixture)
            .then_some(request.run_id.as_str());
        let identity_digest = fingerprint(&(
            ctx.namespace(),
            evidence_scope,
            uncertain_run_scope,
            &snapshot.profile_id,
            &snapshot.release_id,
            &snapshot.bundle_digest,
            &snapshot.environment_digest,
            &snapshot.system_snapshot,
            &snapshot.host_surface_digest,
            &snapshot.host_capabilities_digest,
            &request.development_manifest_digest,
            &request.grader_digest,
            &request.generation_strategy_digest,
            request.revoke_watermark,
        ))?;
        let environment_id = format!("monitor-env-{}", &identity_digest[..32]);
        let environment = EnvironmentIdentityRecord {
            id: environment_id.clone(),
            schema_version: ENVIRONMENT_IDENTITY_SCHEMA.into(),
            identity_digest,
            namespace: ctx.namespace().into(),
            evidence_scope,
            profile_id: snapshot.profile_id.clone(),
            release_id: snapshot.release_id.clone(),
            bundle_digest: snapshot.bundle_digest.clone(),
            environment_digest: snapshot.environment_digest.clone(),
            model_identity: snapshot.system_snapshot.model_id.clone(),
            ordered_tools: snapshot.system_snapshot.tools.clone(),
            host_id: snapshot.system_snapshot.host_id.clone(),
            host_version: snapshot.system_snapshot.host_version.clone(),
            host_surface_digest: snapshot.host_surface_digest.clone(),
            host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
            mandatory_context_digest: snapshot.system_snapshot.mandatory_context_digest.clone(),
            development_manifest_digest: request.development_manifest_digest,
            grader_digest: request.grader_digest,
            generation_strategy_digest: request.generation_strategy_digest,
            revoke_watermark: request.revoke_watermark,
        };
        put_immutable(&mut session, ctx, &environment.id, &environment).await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &environment.id,
                ARTIFACT_KIND,
                &snapshot.id,
            )
            .await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &environment.id,
                RECEIPT_KIND,
                &application.id,
            )
            .await?;

        let application_status = application_status(&snapshot, &application);
        let observation = DeploymentObservation {
            id: format!("monitor-observation-{}", request.run_id),
            schema_version: DEPLOYMENT_OBSERVATION_SCHEMA.into(),
            run_id: request.run_id,
            environment_id,
            snapshot_id: snapshot.id,
            application_id: application.id,
            request_digest: application.actual_request_digest,
            execution_receipt_id: application.execution_receipt_id,
            application_status,
            quality_status: QualityStatus::Unknown,
        };
        put_immutable(&mut session, ctx, &observation.id, &observation).await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &observation.id,
                ARTIFACT_KIND,
                &environment.id,
            )
            .await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &observation.id,
                RECEIPT_KIND,
                &observation.application_id,
            )
            .await?;
        if let Some(execution_id) = &observation.execution_receipt_id {
            session
                .put_edge(
                    ctx,
                    ARTIFACT_KIND,
                    &observation.id,
                    ARTIFACT_KIND,
                    execution_id,
                )
                .await?;
        }
        session
            .audit(ctx, "monitoring.environment.record", &environment.id)
            .await?;
        session.commit().await?;
        Ok(RecordedEnvironment {
            environment,
            observation,
        })
    }

    pub async fn close_development_cycle(
        ctx: &Context,
        store: &Store,
        request: CompleteDevelopmentCycleRequest,
    ) -> Result<DevelopmentCycleRecord> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        validate_cycle_request(&request)?;
        let root = store
            .root_budget(ctx, &request.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != request.root_budget_id {
            return Err(Error::Conflict(
                "development cycle root budget mismatch".into(),
            ));
        }
        let mut session = store.session().await?;
        require_watermark(&mut session, ctx, request.revoke_watermark).await?;
        let source_ids = canonical_strings(request.source_ids.clone(), "cycle source")?;
        if source_ids.is_empty() {
            return Err(Error::Invalid("completed cycle requires sources".into()));
        }
        validate_stored_sources(&mut session, ctx, &source_ids, request.revoke_watermark).await?;
        let sources = load_source_dependencies(&mut session, ctx, &source_ids).await?;
        let environment: EnvironmentIdentityRecord = session
            .need(ctx, ARTIFACT_KIND, &request.environment_id)
            .await?;
        validate_environment_record(&environment)?;
        if environment.namespace != ctx.namespace()
            || environment.profile_id != request.profile_id
            || environment.development_manifest_digest != request.development_manifest_digest
            || environment.grader_digest != request.grader_digest
            || environment.generation_strategy_digest != request.generation_strategy_digest
            || environment.revoke_watermark != request.revoke_watermark
        {
            return Err(Error::Conflict(
                "cycle differs from frozen environment".into(),
            ));
        }
        let fact: StageFact = session
            .need(ctx, ARTIFACT_KIND, &request.report_fact_id)
            .await?;
        fact.validate()?;
        validate_development_fact(
            &mut session,
            ctx,
            &fact,
            &source_ids,
            request.revoke_watermark,
        )
        .await?;
        let report: DevelopmentRunReport = serde_json::from_value(fact.payload.clone())
            .map_err(|_| Error::Invalid("development fact is not a strict report".into()))?;
        validate_report_scope(&report, &request, &environment)?;
        validate_report_receipts(&mut session, ctx, &fact, &report).await?;
        validate_cycle_cost_evidence(
            &mut session,
            ctx,
            &request.billing_scope,
            &fact.episode_id,
            &report,
        )
        .await?;
        let scope = ConsolidationScope {
            namespace: ctx.namespace().into(),
            skill_id: request.skill_id,
            profile_id: request.profile_id,
            development_manifest_digest: request.development_manifest_digest,
            environment_id: environment.id.clone(),
            environment_identity_digest: environment.identity_digest,
            environment_digest: environment.environment_digest,
            grader_digest: request.grader_digest,
            generation_strategy_digest: request.generation_strategy_digest,
            parent_skill_digest: request.parent_skill_digest,
            parent_bundle_digest: request.parent_bundle_digest,
            billing_scope: request.billing_scope,
            root_budget_id: request.root_budget_id,
            revoke_watermark: request.revoke_watermark,
        };
        let scope_id = scope_id(&scope)?;
        let mut index = session
            .get::<ConsolidationScopeIndex>(ctx, ARTIFACT_KIND, &scope_id)
            .await?
            .unwrap_or(ConsolidationScopeIndex {
                id: scope_id.clone(),
                schema_version: CONSOLIDATION_SCOPE_SCHEMA.into(),
                scope: scope.clone(),
                cycle_ids: Vec::new(),
                report_fact_ids: Vec::new(),
                claim_ids: BTreeMap::new(),
                invalidated_by: None,
            });
        if index.scope != scope || index.schema_version != CONSOLIDATION_SCOPE_SCHEMA {
            return Err(Error::Conflict(
                "consolidation scope identity changed".into(),
            ));
        }
        if index.invalidated_by.is_some() {
            return Err(Error::Conflict("consolidation scope is invalidated".into()));
        }
        if let Some(position) = index
            .report_fact_ids
            .iter()
            .position(|id| id == &request.report_fact_id)
        {
            let cycle = load_cycle(&mut session, ctx, &index.cycle_ids[position]).await?;
            session.commit().await?;
            return Ok(cycle);
        }
        let ordinal = u64::try_from(index.cycle_ids.len())
            .map_err(|_| Error::Invalid("cycle ordinal overflow".into()))?
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("cycle ordinal overflow".into()))?;
        let cycle_id = format!(
            "development-cycle-{}",
            &fingerprint(&(&scope_id, &request.report_fact_id))?[..32]
        );
        let (pairs, has_contrast) = pair_tasks(&report)?;
        let cycle = DevelopmentCycleRecord {
            id: cycle_id.clone(),
            schema_version: DEVELOPMENT_CYCLE_SCHEMA.into(),
            scope_id: scope_id.clone(),
            ordinal,
            report_fact_id: request.report_fact_id.clone(),
            report_digest: fingerprint(&report)?,
            request_id: report.request_id,
            candidate_bundle_digest: report.candidate_bundle_digest,
            execution_receipt_id: report.execution_receipt_id,
            usage_record_ids: canonical_strings(report.usage_record_ids, "usage record")?,
            provenance: report.provenance,
            pairs,
            has_contrast,
            sources,
        };
        let binding = DevelopmentReportBinding {
            schema_version: DEVELOPMENT_REPORT_BINDING_SCHEMA.into(),
            id: format!(
                "development-report-binding-{}",
                &fingerprint(&cycle.report_fact_id)?[..32]
            ),
            report_fact_id: cycle.report_fact_id.clone(),
            scope_id: scope_id.clone(),
            cycle_id: cycle.id.clone(),
        };
        put_immutable(&mut session, ctx, &binding.id, &binding).await?;
        put_immutable(&mut session, ctx, &cycle.id, &cycle).await?;
        // The binding names the report fact and cycle of a completed cycle; without
        // an edge revocation cleanup could never reach it.
        session
            .put_edge(ctx, ARTIFACT_KIND, &binding.id, ARTIFACT_KIND, &cycle.id)
            .await?;
        session
            .put_edge(ctx, ARTIFACT_KIND, &cycle.id, ARTIFACT_KIND, &scope_id)
            .await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &cycle.id,
                ARTIFACT_KIND,
                &cycle.report_fact_id,
            )
            .await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &cycle.id,
                ARTIFACT_KIND,
                &cycle.execution_receipt_id,
            )
            .await?;
        for source in &cycle.sources {
            session
                .put_edge(ctx, ARTIFACT_KIND, &cycle.id, "run", &source.id)
                .await?;
        }
        index.cycle_ids.push(cycle_id);
        index.report_fact_ids.push(request.report_fact_id);
        session
            .put(ctx, ARTIFACT_KIND, &scope_id, ctx.actor(), &index)
            .await?;
        session
            .audit(ctx, "monitoring.development_cycle.complete", &cycle.id)
            .await?;
        session.commit().await?;
        Ok(cycle)
    }

    pub async fn claim_consolidation(
        ctx: &Context,
        store: &Store,
        scope_id: &str,
    ) -> Result<ClaimOutcome> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        identifier(scope_id)?;
        let mut session = store.session().await?;
        let mut index: ConsolidationScopeIndex = session.need(ctx, ARTIFACT_KIND, scope_id).await?;
        validate_scope_index(&index)?;
        if index.invalidated_by.is_some() {
            return Err(Error::Conflict("consolidation scope is invalidated".into()));
        }
        require_watermark(&mut session, ctx, index.scope.revoke_watermark).await?;
        let generation = u64::try_from(index.cycle_ids.len() / 2)
            .map_err(|_| Error::Invalid("generation overflow".into()))?;
        if generation == 0 {
            session.commit().await?;
            return Ok(ClaimOutcome::NotEligible {
                completed_cycles: index.cycle_ids.len(),
            });
        }
        if let Some(claim_id) = index.claim_ids.get(&generation) {
            let claim = load_live_claim(&mut session, ctx, claim_id).await?;
            session.commit().await?;
            return Ok(ClaimOutcome::AlreadyClaimed(claim));
        }
        let start = usize::try_from((generation - 1) * 2)
            .map_err(|_| Error::Invalid("generation overflow".into()))?;
        let cycle_ids = index.cycle_ids[start..start + 2].to_vec();
        let mut sources = BTreeSet::new();
        let mut has_contrast = false;
        for cycle_id in &cycle_ids {
            let cycle = load_cycle(&mut session, ctx, cycle_id).await?;
            if cycle.scope_id != scope_id {
                return Err(Error::Conflict("cycle escaped consolidation scope".into()));
            }
            has_contrast |= cycle.has_contrast;
            sources.extend(cycle.sources);
        }
        let sources: Vec<_> = sources.into_iter().collect();
        validate_source_dependency_identities(&sources)?;
        let source_ids = sources
            .iter()
            .map(|source| source.id.clone())
            .collect::<Vec<_>>();
        validate_stored_sources(&mut session, ctx, &source_ids, index.scope.revoke_watermark)
            .await?;
        let claim_id = format!(
            "consolidation-claim-{}",
            &fingerprint(&(scope_id, generation))?[..32]
        );
        let claim = ConsolidationClaim {
            id: claim_id.clone(),
            schema_version: CONSOLIDATION_CLAIM_SCHEMA.into(),
            scope_id: scope_id.into(),
            generation,
            cycle_ids,
            sources,
            revoke_watermark: index.scope.revoke_watermark,
            state: if has_contrast {
                ConsolidationClaimState::Claimed
            } else {
                ConsolidationClaimState::NoContrast
            },
            execution_input_digest: None,
        };
        put_immutable(&mut session, ctx, &claim.id, &claim).await?;
        for cycle_id in &claim.cycle_ids {
            session
                .put_edge(ctx, ARTIFACT_KIND, &claim.id, ARTIFACT_KIND, cycle_id)
                .await?;
        }
        for source in &claim.sources {
            session
                .put_edge(ctx, ARTIFACT_KIND, &claim.id, "run", &source.id)
                .await?;
        }
        index.claim_ids.insert(generation, claim_id);
        session
            .put(ctx, ARTIFACT_KIND, scope_id, ctx.actor(), &index)
            .await?;
        session
            .audit(ctx, "monitoring.consolidation.claim", &claim.id)
            .await?;
        session.commit().await?;
        Ok(ClaimOutcome::Claimed(claim))
    }

    pub async fn complete_no_contrast(
        ctx: &Context,
        store: &Store,
        claim_id: &str,
    ) -> Result<ConsolidationRunRecord> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        let mut session = store.session().await?;
        let mut claim = load_live_claim(&mut session, ctx, claim_id).await?;
        let run_id = format!("consolidation-run-{}", claim.id);
        if let Some(existing) = session
            .get::<ConsolidationRunRecord>(ctx, ARTIFACT_KIND, &run_id)
            .await?
        {
            session.commit().await?;
            return Ok(existing);
        }
        if claim.state != ConsolidationClaimState::NoContrast {
            return Err(Error::Conflict(
                "only a no-contrast claim can finish without dispatch".into(),
            ));
        }
        claim.state = ConsolidationClaimState::CompletedNoChange;
        session
            .put(ctx, ARTIFACT_KIND, &claim.id, ctx.actor(), &claim)
            .await?;
        let record = ConsolidationRunRecord {
            id: run_id,
            schema_version: CONSOLIDATION_RUN_SCHEMA.into(),
            claim_id: claim.id.clone(),
            input_digest: None,
            outcome: ConsolidationRunOutcome::NoChange,
            reason: "two completed development cycles contained no before/after contrast".into(),
            budget_call_ids: vec![],
            proposal_id: None,
        };
        put_immutable(&mut session, ctx, &record.id, &record).await?;
        session
            .put_edge(ctx, ARTIFACT_KIND, &record.id, ARTIFACT_KIND, &claim.id)
            .await?;
        session
            .audit(ctx, "monitoring.consolidation.no_change", &record.id)
            .await?;
        session.commit().await?;
        Ok(record)
    }

    /// Runs one claimed generation through the optimization step.
    ///
    /// A `Candidate` outcome first persists its [`ConsolidationProposal`] and only
    /// then the terminal run record that names it, so a crash in between re-enters
    /// with the journal replaying the same candidate and finds the same proposal.
    /// No other outcome (no change, rejected, uncertain, revoked, blocked budget)
    /// leaves a proposal, and none of them can touch Active.
    pub async fn run_consolidation(
        ctx: &Context,
        store: &Store,
        claim_id: &str,
        model: Option<&dyn ConsolidationModelPort>,
        runner: Option<&dyn ConsolidationDevRunner>,
        journal: Option<&dyn OptimizationJournal>,
        request: OptimizationStepRequest<'_>,
    ) -> Result<ConsolidationRunRecord> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        let input_digest = optimization_request_digest(&request)?;
        let (claim, scope) = {
            let mut session = store.session().await?;
            let claim = load_claim_record(&mut session, ctx, claim_id).await?;
            let revoked_id = format!("consolidation-revoked-{}", claim.id);
            if let Some(existing) = session
                .get::<ConsolidationRunRecord>(ctx, ARTIFACT_KIND, &revoked_id)
                .await?
            {
                if existing.input_digest.as_deref() != Some(&input_digest) {
                    return Err(Error::Conflict(
                        "revoked consolidation input differs".into(),
                    ));
                }
                session.commit().await?;
                return Ok(existing);
            }
            let run_id = format!("consolidation-run-{}", claim.id);
            if let Some(existing) = session
                .get::<ConsolidationRunRecord>(ctx, ARTIFACT_KIND, &run_id)
                .await?
            {
                if existing.input_digest.as_deref() != Some(&input_digest) {
                    return Err(Error::Conflict(
                        "completed consolidation input differs".into(),
                    ));
                }
                match validate_claim_sources(&mut session, ctx, &claim).await {
                    Ok(()) => {
                        session.commit().await?;
                        return Ok(existing);
                    }
                    Err(_) => {
                        drop(session);
                        return persist_terminal_run_with_id(
                            ctx,
                            store,
                            claim,
                            revoked_id,
                            Some(input_digest),
                            ConsolidationRunOutcome::Revoked,
                            ConsolidationClaimState::CompletedRevoked,
                            "source_revoked_after_consolidation",
                        )
                        .await;
                    }
                }
            }
            let index: ConsolidationScopeIndex =
                session.need(ctx, ARTIFACT_KIND, &claim.scope_id).await?;
            validate_execution_binding(ctx, &claim, &index, &request)?;
            if let Err(error) = validate_claim_sources(&mut session, ctx, &claim).await {
                if claim.state == ConsolidationClaimState::Running {
                    drop(session);
                    let revoked_id = format!("consolidation-revoked-{}", claim.id);
                    return persist_terminal_run_with_id(
                        ctx,
                        store,
                        claim,
                        revoked_id,
                        Some(input_digest),
                        ConsolidationRunOutcome::Revoked,
                        ConsolidationClaimState::CompletedRevoked,
                        "source_revoked_while_running",
                    )
                    .await;
                }
                return Err(error);
            }
            session.commit().await?;
            (claim, index.scope)
        };
        let root = store
            .root_budget(ctx, &scope.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        let available = root
            .total_limit_micros
            .checked_sub(root.spent_micros)
            .and_then(|remaining| remaining.checked_sub(root.reserved_micros))
            .ok_or_else(|| Error::Conflict("root budget accounting overflow".into()))?;
        if root.root_budget_id != scope.root_budget_id || root.stopped || available <= 0 {
            return persist_terminal_run_with_state(
                ctx,
                store,
                claim,
                Some(input_digest),
                ConsolidationRunOutcome::BlockedBudget,
                ConsolidationClaimState::BlockedBudget,
                "root budget is stopped or has no dispatchable balance",
            )
            .await;
        }
        let model = model.ok_or(Error::NotFound)?;
        let runner = runner.ok_or(Error::NotFound)?;
        let journal = journal.ok_or(Error::NotFound)?;
        let model_binding = model.trusted_budget_binding(ctx.namespace()).await?;
        let runner_binding = runner.trusted_budget_binding(ctx.namespace()).await?;
        for binding in [&model_binding, &runner_binding] {
            if binding.billing_scope != scope.billing_scope
                || binding.root_budget_id != scope.root_budget_id
            {
                return Err(Error::Conflict(
                    "consolidation port budget binding differs from the claim".into(),
                ));
            }
        }
        {
            let mut session = store.session().await?;
            let mut current = load_live_claim(&mut session, ctx, &claim.id).await?;
            match current.state {
                ConsolidationClaimState::Claimed => {
                    current.state = ConsolidationClaimState::Running;
                    current.execution_input_digest = Some(input_digest.clone());
                    session
                        .put(ctx, ARTIFACT_KIND, &current.id, ctx.actor(), &current)
                        .await?;
                }
                ConsolidationClaimState::Running
                    if current.execution_input_digest.as_deref() == Some(&input_digest) => {}
                _ => {
                    return Err(Error::Conflict(
                        "consolidation claim is not dispatchable".into(),
                    ));
                }
            }
            session.commit().await?;
        }
        let root_bound_model = RootBoundModelPort {
            inner: model,
            root_budget_id: &scope.root_budget_id,
        };
        let root_bound_runner = RootBoundDevRunner {
            inner: runner,
            store,
            context: ctx,
            billing_scope: &scope.billing_scope,
        };
        // The step consumes its request; a Candidate's terminal journal fact is
        // located from the identity of the request afterwards.
        let step_context = request.model_context.clone();
        let outcome = run_optimization_step(
            Some(&root_bound_model),
            Some(&root_bound_runner),
            Some(journal),
            request,
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let revoked = {
                    let mut session = store.session().await?;
                    let revoked = validate_claim_sources(&mut session, ctx, &claim)
                        .await
                        .is_err();
                    session.commit().await?;
                    revoked
                };
                let (outcome, state, reason) = if revoked {
                    (
                        ConsolidationRunOutcome::Revoked,
                        ConsolidationClaimState::CompletedRevoked,
                        format!("source_revoked_after_{}", error_category(&error)),
                    )
                } else {
                    (
                        ConsolidationRunOutcome::Uncertain,
                        ConsolidationClaimState::CompletedUncertain,
                        format!("optimization_step_{}", error_category(&error)),
                    )
                };
                return if outcome == ConsolidationRunOutcome::Revoked {
                    let revoked_id = format!("consolidation-revoked-{}", claim.id);
                    persist_terminal_run_with_id(
                        ctx,
                        store,
                        claim,
                        revoked_id,
                        Some(input_digest),
                        outcome,
                        state,
                        &reason,
                    )
                    .await
                } else {
                    persist_terminal_run_with_state(
                        ctx,
                        store,
                        claim,
                        Some(input_digest),
                        outcome,
                        state,
                        &reason,
                    )
                    .await
                };
            }
        };
        {
            let mut session = store.session().await?;
            if validate_claim_sources(&mut session, ctx, &claim)
                .await
                .is_err()
            {
                drop(session);
                let revoked_id = format!("consolidation-revoked-{}", claim.id);
                return persist_terminal_run_with_id(
                    ctx,
                    store,
                    claim,
                    revoked_id,
                    Some(input_digest),
                    ConsolidationRunOutcome::Revoked,
                    ConsolidationClaimState::CompletedRevoked,
                    "source_revoked_after_optimization",
                )
                .await;
            }
            session.commit().await?;
        }
        let mut proposal_id = None;
        let (run_outcome, claim_state, reason) = match outcome {
            OptimizationStepOutcome::NoChange { reason } => (
                ConsolidationRunOutcome::NoChange,
                ConsolidationClaimState::CompletedNoChange,
                reason,
            ),
            OptimizationStepOutcome::Candidate {
                bundle, selection, ..
            } => {
                // The proposal is durable before the terminal record names it. A
                // crash in between re-enters here, the journal replays the same
                // Candidate and the same proposal is found instead of rewritten.
                let terminal_fact_id = optimization_terminal_fact_id(&step_context, &input_digest)?;
                let written = persist_consolidation_proposal(
                    ctx,
                    store,
                    journal,
                    &claim,
                    CandidateMaterial {
                        bundle: &bundle,
                        selection: &selection,
                        terminal_fact_id: &terminal_fact_id,
                        input_digest: &input_digest,
                    },
                )
                .await;
                match written {
                    Ok(proposal) => proposal_id = Some(proposal.id),
                    Err(error) => {
                        let revoked = {
                            let mut session = store.session().await?;
                            let revoked = validate_claim_sources(&mut session, ctx, &claim)
                                .await
                                .is_err();
                            session.commit().await?;
                            revoked
                        };
                        if !revoked {
                            return Err(error);
                        }
                        let revoked_id = format!("consolidation-revoked-{}", claim.id);
                        return persist_terminal_run_with_id(
                            ctx,
                            store,
                            claim,
                            revoked_id,
                            Some(input_digest),
                            ConsolidationRunOutcome::Revoked,
                            ConsolidationClaimState::CompletedRevoked,
                            "source_revoked_after_optimization",
                        )
                        .await;
                    }
                }
                (
                    ConsolidationRunOutcome::Candidate,
                    ConsolidationClaimState::CompletedCandidate,
                    "development candidate produced; Active remains unchanged".into(),
                )
            }
            OptimizationStepOutcome::Rejected { reason } => (
                ConsolidationRunOutcome::Rejected,
                ConsolidationClaimState::CompletedRejected,
                reason,
            ),
            OptimizationStepOutcome::Uncertain { reason } => (
                ConsolidationRunOutcome::Uncertain,
                ConsolidationClaimState::CompletedUncertain,
                reason,
            ),
        };
        let run_id = format!("consolidation-run-{}", claim.id);
        persist_terminal_run(
            ctx,
            store,
            claim,
            run_id,
            Some(input_digest),
            run_outcome,
            claim_state,
            &reason,
            proposal_id,
        )
        .await
    }

    /// Stages a stored consolidation proposal as a release candidate.
    ///
    /// Only a `ReleaseCandidateRecord` is written: no release, no pointer, and
    /// Active is untouched. The candidate still needs its own development
    /// selection, formal evaluation and a different approver (plan §11.5, E13).
    /// A proposal whose sources were revoked, whose content was redacted, or
    /// whose scope drifted is refused. Staging the same candidate id again with
    /// the same content is idempotent; other content under that id is a conflict.
    /// Fixture-backed proposals may be staged; their provenance is carried on the
    /// proposal and on the result, and formal approval refuses fixture evidence.
    pub async fn stage_consolidation_candidate(
        ctx: &Context,
        store: &Store,
        proposal_id: &str,
        candidate_id: &str,
    ) -> Result<StagedConsolidationCandidate> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        identifier(proposal_id)?;
        identifier(candidate_id)?;
        let proposal = {
            let mut session = store.session().await?;
            let proposal = load_stageable_proposal(&mut session, ctx, proposal_id).await?;
            session.commit().await?;
            proposal
        };
        // `stage_bundle` re-checks the source closure in its own transaction, so a
        // revocation after the read above still cannot produce a candidate.
        let candidate = ReleaseStore::stage_bundle(
            ctx,
            store,
            StageBundleRequest {
                candidate_id: candidate_id.into(),
                bundle: proposal.candidate_bundle.clone(),
                environment_digest: proposal.environment_digest.clone(),
                proposer_actor: ctx.actor().into(),
                sources: proposal
                    .sources
                    .iter()
                    .map(|source| TypedSourceRef {
                        kind: "run".into(),
                        id: source.id.clone(),
                        content_digest: source.content_digest.clone(),
                    })
                    .collect(),
                revoke_watermark: proposal.revoke_watermark,
            },
        )
        .await?;
        // The candidate depends on the proposal it was staged from (plan §11.5
        // closure: ConsolidationProposal -> Candidate), idempotently.
        let mut session = store.session().await?;
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &candidate.id,
                ARTIFACT_KIND,
                &proposal.id,
            )
            .await?;
        session.commit().await?;
        Ok(StagedConsolidationCandidate {
            candidate,
            proposal_id: proposal.id,
            provenance: proposal.provenance,
        })
    }

    pub async fn record_environment_drift(
        ctx: &Context,
        store: &Store,
        previous_environment_id: &str,
        current_environment_id: &str,
        scope_id: &str,
        reason: &str,
    ) -> Result<EnvironmentDriftRecord> {
        ctx.require(&[Role::Admin])?;
        for value in [previous_environment_id, current_environment_id, scope_id] {
            identifier(value)?;
        }
        if reason.is_empty() || reason.len() > 512 {
            return Err(Error::Invalid("drift reason must be 1..=512 bytes".into()));
        }
        let mut session = store.session().await?;
        let previous: EnvironmentIdentityRecord = session
            .need(ctx, ARTIFACT_KIND, previous_environment_id)
            .await?;
        let current: EnvironmentIdentityRecord = session
            .need(ctx, ARTIFACT_KIND, current_environment_id)
            .await?;
        validate_environment_record(&previous)?;
        validate_environment_record(&current)?;
        if previous.identity_digest == current.identity_digest {
            return Err(Error::Conflict("environment identity did not drift".into()));
        }
        let mut index: ConsolidationScopeIndex = session.need(ctx, ARTIFACT_KIND, scope_id).await?;
        if index.scope.environment_identity_digest != previous.identity_digest {
            return Err(Error::Conflict(
                "drift does not describe the persisted scope environment".into(),
            ));
        }
        let id = format!(
            "environment-drift-{}",
            &fingerprint(&(previous_environment_id, current_environment_id, scope_id))?[..32]
        );
        let drift = EnvironmentDriftRecord {
            id: id.clone(),
            schema_version: ENVIRONMENT_DRIFT_SCHEMA.into(),
            previous_environment_id: previous_environment_id.into(),
            current_environment_id: current_environment_id.into(),
            previous_identity_digest: previous.identity_digest,
            current_identity_digest: current.identity_digest,
            invalidated_scope_id: scope_id.into(),
            reason: reason.into(),
        };
        if let Some(existing) = session
            .get::<EnvironmentDriftRecord>(ctx, ARTIFACT_KIND, &id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&drift)? {
                return Err(Error::Conflict("environment drift id changed".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        if let Some(old) = &index.invalidated_by
            && old != &id
        {
            return Err(Error::Conflict(
                "scope already invalidated by another drift fact".into(),
            ));
        }
        index.invalidated_by = Some(id.clone());
        session
            .put(ctx, ARTIFACT_KIND, scope_id, ctx.actor(), &index)
            .await?;
        put_immutable(&mut session, ctx, &id, &drift).await?;
        for environment_id in [previous_environment_id, current_environment_id] {
            session
                .put_edge(ctx, ARTIFACT_KIND, &id, ARTIFACT_KIND, environment_id)
                .await?;
        }
        session
            .audit(ctx, "monitoring.environment.drift", &id)
            .await?;
        session.commit().await?;
        Ok(drift)
    }
}

struct RootBoundModelPort<'a> {
    inner: &'a dyn ConsolidationModelPort,
    root_budget_id: &'a str,
}

#[async_trait]
impl ModelPort for RootBoundModelPort<'_> {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let response = self.inner.dispatch(request).await?;
        let actual_root = match &response {
            ModelResponse::Completed {
                execution_receipt, ..
            } => Some(execution_receipt.root_budget_id.as_str()),
            ModelResponse::Rejected {
                dispatch: RejectedDispatch::Dispatched { receipt },
                ..
            } => Some(receipt.root_budget_id.as_str()),
            ModelResponse::Rejected {
                dispatch: RejectedDispatch::NotDispatched,
                ..
            } => None,
            ModelResponse::Uncertain { root_budget_id, .. } => Some(root_budget_id.as_str()),
        };
        if actual_root.is_some_and(|root| root != self.root_budget_id) {
            return Err(Error::Conflict(
                "model dispatch escaped the consolidation root budget".into(),
            ));
        }
        Ok(response)
    }
}

struct RootBoundDevRunner<'a> {
    inner: &'a dyn ConsolidationDevRunner,
    store: &'a Store,
    context: &'a Context,
    billing_scope: &'a str,
}

#[async_trait]
impl DevRunner for RootBoundDevRunner<'_> {
    async fn run(
        &self,
        request: crate::optimization::DevelopmentRunRequest,
    ) -> Result<DevelopmentRunReport> {
        let episode_id = request.episode_id.clone();
        let report = self.inner.run(request).await?;
        let mut session = self.store.session().await?;
        let calls = session
            .budget_calls_for_group(self.context, self.billing_scope, &episode_id)
            .await?;
        session.commit().await?;
        let usage_ids: BTreeSet<_> = calls
            .iter()
            .filter_map(|call| call.usage_record_id.as_deref())
            .collect();
        if report.usage_record_ids.is_empty()
            || !report
                .usage_record_ids
                .iter()
                .all(|usage| usage_ids.contains(usage.as_str()))
        {
            return Err(Error::Conflict(
                "development report usage escaped the consolidation root ledger".into(),
            ));
        }
        Ok(report)
    }
}

async fn validate_host_application(
    session: &mut Session,
    ctx: &Context,
    snapshot: &RunApplicationSnapshot,
    application: &HostApplicationRecord,
) -> Result<()> {
    if application.schema_version != "rsia.host_application.v1"
        || application.run_id != snapshot.run_id
        || application.release_id != snapshot.release_id
        || application.actual_request_digest != snapshot.request_digest
    {
        return Err(Error::Conflict(
            "stored Host application differs from the run snapshot".into(),
        ));
    }
    if let Some(receipt) = &application.receipt {
        receipt.validate()?;
        if receipt.request_digest != snapshot.request_digest
            || Some(receipt.bundle_digest.as_str()) != snapshot.bundle_digest.as_deref()
        {
            return Err(Error::Conflict(
                "stored application receipt differs from frozen projection".into(),
            ));
        }
    }
    if let Some(execution_id) = &application.execution_receipt_id {
        let execution: TrustedHostExecutionReceipt =
            session.need(ctx, ARTIFACT_KIND, execution_id).await?;
        if execution.run_id != snapshot.run_id
            || execution.request_digest != snapshot.request_digest
            || execution.environment_digest != snapshot.environment_digest
            || execution.host_surface_digest != snapshot.host_surface_digest
        {
            return Err(Error::Conflict(
                "Host execution receipt differs from frozen projection".into(),
            ));
        }
    } else if application
        .receipt
        .as_ref()
        .is_some_and(|receipt| !receipt.used.is_empty())
    {
        return Err(Error::Conflict(
            "used application lacks a stored execution receipt".into(),
        ));
    }
    Ok(())
}

fn application_status(
    snapshot: &RunApplicationSnapshot,
    application: &HostApplicationRecord,
) -> ApplicationStatus {
    if snapshot.bundle_digest.is_none() {
        return ApplicationStatus::NotSelected;
    }
    let Some(receipt) = &application.receipt else {
        return ApplicationStatus::Unknown;
    };
    if receipt.truncated {
        ApplicationStatus::Unknown
    } else if !receipt.used.is_empty() {
        ApplicationStatus::Used
    } else if !receipt.attached.is_empty() {
        ApplicationStatus::AttachedNotUsed
    } else if !receipt.offered.is_empty() {
        ApplicationStatus::OfferedNotAttached
    } else {
        ApplicationStatus::Unknown
    }
}

fn validate_cycle_request(request: &CompleteDevelopmentCycleRequest) -> Result<()> {
    for value in [
        &request.skill_id,
        &request.profile_id,
        &request.environment_id,
        &request.billing_scope,
        &request.root_budget_id,
        &request.report_fact_id,
    ] {
        identifier(value)?;
    }
    for (value, name) in [
        (
            &request.development_manifest_digest,
            "development manifest digest",
        ),
        (&request.grader_digest, "grader digest"),
        (
            &request.generation_strategy_digest,
            "generation strategy digest",
        ),
        (&request.parent_skill_digest, "parent skill digest"),
        (&request.parent_bundle_digest, "parent bundle digest"),
    ] {
        validate_digest(value, name)?;
    }
    Ok(())
}

async fn validate_development_fact(
    session: &mut Session,
    ctx: &Context,
    fact: &StageFact,
    expected_source_ids: &[String],
    expected_watermark: u64,
) -> Result<()> {
    if fact.stage != OptimizationJournalStage::Development
        || fact.kind != StageFactKind::DevelopmentObserved
        || fact.namespace != ctx.namespace()
    {
        return Err(Error::Invalid(
            "cycle must consume an E03 DevelopmentObserved fact".into(),
        ));
    }
    let mut executions = 0usize;
    let mut graders = 0usize;
    let mut source_ids = Vec::new();
    let mut watermarks = Vec::new();
    for dependency in &fact.dependencies {
        if matches!(
            dependency.kind.as_str(),
            "formal" | "holdout" | "oracle" | "anchor" | "evaluation"
        ) {
            return Err(Error::Forbidden);
        }
        match dependency.kind.as_str() {
            "execution" => executions += 1,
            "grader" => graders += 1,
            "run" => source_ids.push(dependency.id.clone()),
            "revoke_watermark" => {
                watermarks.push(
                    dependency
                        .id
                        .parse::<u64>()
                        .map_err(|_| Error::Invalid("invalid stage revoke watermark".into()))?,
                );
                continue;
            }
            "artifact" => {}
            _ => {
                return Err(Error::Invalid(
                    "development report has unsupported dependencies".into(),
                ));
            }
        }
        let actual_kind = if dependency.kind == "run" {
            "run"
        } else {
            ARTIFACT_KIND
        };
        let _: serde_json::Value = session.need(ctx, actual_kind, &dependency.id).await?;
    }
    if executions == 0 || graders == 0 {
        return Err(Error::Invalid(
            "development report lacks execution or grader receipts".into(),
        ));
    }
    source_ids.sort();
    if source_ids != expected_source_ids || watermarks != [expected_watermark] {
        return Err(Error::Conflict(
            "development fact source closure differs from completed cycle".into(),
        ));
    }
    Ok(())
}

fn validate_report_scope(
    report: &DevelopmentRunReport,
    request: &CompleteDevelopmentCycleRequest,
    environment: &EnvironmentIdentityRecord,
) -> Result<()> {
    identifier(&report.request_id)?;
    identifier(&report.execution_receipt_id)?;
    if report.manifest_digest != request.development_manifest_digest
        || report.parent_bundle_digest != request.parent_bundle_digest
        || report.environment_digest != environment.environment_digest
        || report.grader_digest != request.grader_digest
    {
        return Err(Error::Conflict(
            "development report differs from cycle scope".into(),
        ));
    }
    for value in [
        &report.manifest_digest,
        &report.parent_bundle_digest,
        &report.candidate_bundle_digest,
        &report.environment_digest,
        &report.grader_digest,
    ] {
        validate_digest(value, "development report digest")?;
    }
    if report.results.is_empty() {
        return Err(Error::Invalid(
            "development report has no task results".into(),
        ));
    }
    Ok(())
}

async fn validate_report_receipts(
    session: &mut Session,
    ctx: &Context,
    fact: &StageFact,
    report: &DevelopmentRunReport,
) -> Result<()> {
    let expected: BTreeSet<_> = report
        .results
        .iter()
        .flat_map(|result| {
            [
                ("execution".to_string(), result.parent_execution_id.clone()),
                (
                    "execution".to_string(),
                    result.candidate_execution_id.clone(),
                ),
                ("grader".to_string(), result.grader_receipt_digest.clone()),
            ]
        })
        .collect();
    let actual: BTreeSet<_> = fact
        .dependencies
        .iter()
        .filter(|dependency| matches!(dependency.kind.as_str(), "execution" | "grader"))
        .map(|dependency| (dependency.kind.clone(), dependency.id.clone()))
        .collect();
    if expected.is_empty() || expected != actual {
        return Err(Error::Conflict(
            "development report receipt closure differs from stage fact".into(),
        ));
    }
    let _: serde_json::Value = session
        .need(ctx, ARTIFACT_KIND, &report.execution_receipt_id)
        .await?;
    Ok(())
}

async fn validate_cycle_cost_evidence(
    session: &mut Session,
    ctx: &Context,
    billing_scope: &str,
    dispatch_group_id: &str,
    report: &DevelopmentRunReport,
) -> Result<()> {
    if report.usage_record_ids.is_empty() {
        if report.provenance == DevelopmentExecutionProvenance::Fixture {
            return Ok(());
        }
        return Err(Error::Invalid(
            "isolated development report lacks usage records".into(),
        ));
    }
    let calls = session
        .budget_calls_for_group(ctx, billing_scope, dispatch_group_id)
        .await?;
    let actual_usage: BTreeSet<_> = calls
        .iter()
        .filter_map(|call| call.usage_record_id.as_deref())
        .collect();
    if !report
        .usage_record_ids
        .iter()
        .all(|usage| actual_usage.contains(usage.as_str()))
    {
        return Err(Error::Conflict(
            "development report usage is absent from the root ledger".into(),
        ));
    }
    Ok(())
}

/// Pairs every reported task with its parent and candidate outcomes.
///
/// The class is the pure pass/fail transition; scores are kept as evidence and
/// never decide it. `has_contrast` keeps its earlier meaning: any task whose
/// score or pass state differs between parent and candidate. A report with no
/// results or a repeated task id is not a pairing and is refused. Tasks that the
/// frozen manifest lists but the report omits are not detectable here; that
/// check belongs to the E03 pre-registered manifest gate.
fn pair_tasks(report: &DevelopmentRunReport) -> Result<(Vec<TaskPairV1>, bool)> {
    if report.results.is_empty() {
        return Err(Error::Invalid(
            "development report has no task results".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::with_capacity(report.results.len());
    let mut contrast = false;
    for result in &report.results {
        identifier(&result.task_id)?;
        if !seen.insert(result.task_id.as_str()) {
            return Err(Error::Invalid(
                "development report repeats a task id".into(),
            ));
        }
        if result.parent_score_micros > 1_000_000 || result.candidate_score_micros > 1_000_000 {
            return Err(Error::Invalid(
                "development score micros must be within 0..=1000000".into(),
            ));
        }
        contrast |= result.parent_score_micros != result.candidate_score_micros
            || result.parent_passed != result.candidate_passed;
        pairs.push(TaskPairV1 {
            task_id: result.task_id.clone(),
            parent_passed: result.parent_passed,
            candidate_passed: result.candidate_passed,
            class: classify_consolidation_pair(result.parent_passed, result.candidate_passed),
            parent_score_micros: result.parent_score_micros,
            candidate_score_micros: result.candidate_score_micros,
        });
    }
    Ok((pairs, contrast))
}

/// Reads a development cycle. Pre-v2 records are refused explicitly, redacted
/// content is reported as redacted, and a stored record must still be exactly
/// what `pair_tasks` would have produced (no score-derived or altered class).
async fn load_cycle(
    session: &mut Session,
    ctx: &Context,
    cycle_id: &str,
) -> Result<DevelopmentCycleRecord> {
    identifier(cycle_id)?;
    let value: serde_json::Value = session.need(ctx, ARTIFACT_KIND, cycle_id).await?;
    match value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
    {
        Some(DEVELOPMENT_CYCLE_SCHEMA) => {}
        Some(LEGACY_DEVELOPMENT_CYCLE_SCHEMA_V1) => {
            return Err(Error::Invalid(
                "pre-v2 development cycle records are not supported".into(),
            ));
        }
        Some(REDACTED_SCHEMA) => {
            return Err(Error::Conflict(
                "development cycle was redacted after source revocation".into(),
            ));
        }
        _ => {
            return Err(Error::Invalid(
                "unsupported development cycle schema".into(),
            ));
        }
    }
    let cycle: DevelopmentCycleRecord =
        serde_json::from_value(value).map_err(|_| Error::Internal)?;
    validate_cycle_record(&cycle)?;
    Ok(cycle)
}

fn validate_cycle_record(cycle: &DevelopmentCycleRecord) -> Result<()> {
    identifier(&cycle.id)?;
    identifier(&cycle.scope_id)?;
    if cycle.pairs.is_empty() {
        return Err(Error::Invalid("development cycle has no task pairs".into()));
    }
    let mut seen = BTreeSet::new();
    let mut contrast = false;
    for pair in &cycle.pairs {
        identifier(&pair.task_id)?;
        if !seen.insert(pair.task_id.as_str()) {
            return Err(Error::Invalid("development cycle repeats a task id".into()));
        }
        if pair.parent_score_micros > 1_000_000 || pair.candidate_score_micros > 1_000_000 {
            return Err(Error::Invalid(
                "development score micros must be within 0..=1000000".into(),
            ));
        }
        if pair.class != classify_consolidation_pair(pair.parent_passed, pair.candidate_passed) {
            return Err(Error::Conflict(
                "stored pair class differs from its pass/fail outcomes".into(),
            ));
        }
        contrast |= pair.parent_score_micros != pair.candidate_score_micros
            || pair.parent_passed != pair.candidate_passed;
    }
    if contrast != cycle.has_contrast {
        return Err(Error::Conflict(
            "stored cycle contrast differs from its pairs".into(),
        ));
    }
    Ok(())
}

fn scope_id(scope: &ConsolidationScope) -> Result<String> {
    Ok(format!(
        "consolidation-scope-{}",
        &fingerprint(scope)?[..32]
    ))
}

fn validate_scope_index(index: &ConsolidationScopeIndex) -> Result<()> {
    if index.schema_version != CONSOLIDATION_SCOPE_SCHEMA
        || index.id != scope_id(&index.scope)?
        || index.cycle_ids.len() != index.report_fact_ids.len()
    {
        return Err(Error::Conflict("invalid consolidation scope".into()));
    }
    Ok(())
}

async fn load_live_claim(
    session: &mut Session,
    ctx: &Context,
    claim_id: &str,
) -> Result<ConsolidationClaim> {
    let claim = load_claim_record(session, ctx, claim_id).await?;
    validate_claim_sources(session, ctx, &claim).await?;
    Ok(claim)
}

async fn load_claim_record(
    session: &mut Session,
    ctx: &Context,
    claim_id: &str,
) -> Result<ConsolidationClaim> {
    identifier(claim_id)?;
    let value: serde_json::Value = session.need(ctx, ARTIFACT_KIND, claim_id).await?;
    if value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
        == Some(REDACTED_SCHEMA)
    {
        return Err(Error::Conflict(
            "consolidation claim was redacted after source revocation".into(),
        ));
    }
    let claim: ConsolidationClaim = serde_json::from_value(value).map_err(|_| Error::Internal)?;
    if claim.schema_version != CONSOLIDATION_CLAIM_SCHEMA {
        return Err(Error::Invalid("unsupported claim schema".into()));
    }
    let index: ConsolidationScopeIndex = session.need(ctx, ARTIFACT_KIND, &claim.scope_id).await?;
    validate_scope_index(&index)?;
    if index.claim_ids.get(&claim.generation) != Some(&claim.id)
        || claim.revoke_watermark != index.scope.revoke_watermark
    {
        return Err(Error::Conflict("consolidation claim is stale".into()));
    }
    Ok(claim)
}

async fn validate_claim_sources(
    session: &mut Session,
    ctx: &Context,
    claim: &ConsolidationClaim,
) -> Result<()> {
    let index: ConsolidationScopeIndex = session.need(ctx, ARTIFACT_KIND, &claim.scope_id).await?;
    if index.invalidated_by.is_some() {
        return Err(Error::Conflict("consolidation claim is stale".into()));
    }
    validate_stored_sources(
        session,
        ctx,
        &claim
            .sources
            .iter()
            .map(|source| source.id.clone())
            .collect::<Vec<_>>(),
        claim.revoke_watermark,
    )
    .await?;
    for source in &claim.sources {
        let stored = load_stored_source(session, ctx, &source.id).await?;
        if stored.trace.source_digest != source.content_digest {
            return Err(Error::Conflict("claim source content changed".into()));
        }
    }
    Ok(())
}

fn validate_source_dependency_identities(sources: &[SourceDependency]) -> Result<()> {
    let mut by_id = BTreeMap::new();
    for source in sources {
        if let Some(previous) = by_id.insert(&source.id, &source.content_digest)
            && previous != &source.content_digest
        {
            return Err(Error::Conflict(
                "same source id has different content digests".into(),
            ));
        }
    }
    Ok(())
}

fn error_category(error: &Error) -> &'static str {
    match error {
        Error::Invalid(_) => "invalid",
        Error::Forbidden => "forbidden",
        Error::NotFound => "not_found",
        Error::Budget => "budget",
        Error::Conflict(_) => "conflict",
        Error::Cancelled => "cancelled",
        Error::Internal => "internal",
    }
}

fn validate_execution_binding(
    ctx: &Context,
    claim: &ConsolidationClaim,
    index: &ConsolidationScopeIndex,
    request: &OptimizationStepRequest<'_>,
) -> Result<()> {
    if claim.state != ConsolidationClaimState::Claimed
        && claim.state != ConsolidationClaimState::Running
    {
        return Err(Error::Conflict("claim is not dispatchable".into()));
    }
    let scope = &index.scope;
    let request_sources: BTreeSet<_> = request
        .model_context
        .source_closure
        .iter()
        .map(|source| (source.id.clone(), source.digest.clone()))
        .collect();
    let claim_sources: BTreeSet<_> = claim
        .sources
        .iter()
        .map(|source| (source.id.clone(), source.content_digest.clone()))
        .collect();
    let selected: BTreeSet<_> = request.source_selection.run_ids.iter().cloned().collect();
    let expected_ids: BTreeSet<_> = claim
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect();
    if request.model_context.namespace != ctx.namespace()
        || request.model_context.purpose != Purpose::Development
        || request.model_context.episode_id != claim.id
        || request.model_context.parent_skill_digest != scope.parent_skill_digest
        || request.model_context.bundle_digest != scope.parent_bundle_digest
        || request.model_context.revoke_watermark != claim.revoke_watermark
        || request_sources != claim_sources
        || selected != expected_ids
        || request.development_request.namespace != ctx.namespace()
        || request.development_request.purpose != Purpose::Development
        || request.development_request.episode_id != claim.id
        || request.development_request.manifest.digest != scope.development_manifest_digest
        || request.development_request.parent_bundle_digest != scope.parent_bundle_digest
        || request.development_request.environment_digest != scope.environment_digest
        || request.development_request.grader_digest != scope.grader_digest
        || request.development_request.revoke_watermark != scope.revoke_watermark
        || request.bundle_context.profile.id != scope.profile_id
        || fingerprint(request.bundle_context.parent_strategy)? != scope.generation_strategy_digest
    {
        return Err(Error::Conflict(
            "optimization request differs from persisted claim".into(),
        ));
    }
    Ok(())
}

async fn require_watermark(session: &mut Session, ctx: &Context, expected: u64) -> Result<()> {
    let (actual, _) = session.watermark(ctx).await?.ok_or_else(|| {
        Error::Conflict("namespace revoke watermark is missing; zero is not inferred".into())
    })?;
    if u64::try_from(actual).map_err(|_| Error::Internal)? != expected {
        return Err(Error::Conflict("namespace revoke watermark changed".into()));
    }
    Ok(())
}

async fn load_source_dependencies(
    session: &mut Session,
    ctx: &Context,
    source_ids: &[String],
) -> Result<Vec<SourceDependency>> {
    let mut sources = Vec::with_capacity(source_ids.len());
    for id in source_ids {
        let stored = load_stored_source(session, ctx, id).await?;
        if stored.trace.purpose != Purpose::Development
            || matches!(
                stored.trace.outcome,
                TraceOutcome::Invalid | TraceOutcome::Incomplete
            )
        {
            return Err(Error::Forbidden);
        }
        sources.push(SourceDependency {
            id: id.clone(),
            content_digest: stored.trace.source_digest,
        });
    }
    Ok(sources)
}

fn validate_environment_record(environment: &EnvironmentIdentityRecord) -> Result<()> {
    if environment.schema_version != ENVIRONMENT_IDENTITY_SCHEMA {
        return Err(Error::Invalid("unsupported environment schema".into()));
    }
    identifier(&environment.id)?;
    validate_digest(&environment.identity_digest, "environment identity digest")?;
    validate_digest(&environment.environment_digest, "environment digest")?;
    Ok(())
}

async fn put_immutable<T>(session: &mut Session, ctx: &Context, id: &str, value: &T) -> Result<()>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    if let Some(existing) = session.get::<T>(ctx, ARTIFACT_KIND, id).await? {
        if fingerprint(&existing)? != fingerprint(value)? {
            return Err(Error::Conflict(
                "immutable monitoring artifact changed".into(),
            ));
        }
        return Ok(());
    }
    session
        .put(ctx, ARTIFACT_KIND, id, ctx.actor(), value)
        .await
}

fn consolidation_proposal_id(claim_id: &str) -> String {
    format!("consolidation-proposal-{claim_id}")
}

/// Id of the optimization journal's terminal `StepCompleted` fact for a step:
/// the same canonical identity `run_optimization_step` seals for it.
fn optimization_terminal_fact_id(
    context: &ModelRequestContext,
    input_digest: &str,
) -> Result<String> {
    Ok(StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: context.namespace.clone(),
        episode_id: context.episode_id.clone(),
        step: context.step,
        attempt: context.attempt,
        stage: OptimizationJournalStage::Merge,
        kind: StageFactKind::StepCompleted,
        request_id: context.request_id.clone(),
        input_digest: input_digest.into(),
        output_digest: None,
        dependencies: vec![],
        payload: serde_json::json!({}),
    }
    .seal()?
    .artifact_id)
}

struct CandidateMaterial<'a> {
    bundle: &'a ResolvedBundle,
    selection: &'a DevelopmentSelection,
    terminal_fact_id: &'a str,
    input_digest: &'a str,
}

/// The journal's terminal fact must be the very step this claim executed and
/// must carry the same candidate bundle and selection the proposal will freeze.
fn verify_terminal_candidate_fact(
    fact: &StageFact,
    claim: &ConsolidationClaim,
    material: &CandidateMaterial<'_>,
) -> Result<()> {
    fact.validate()?;
    if fact.artifact_id != material.terminal_fact_id
        || fact.stage != OptimizationJournalStage::Merge
        || fact.kind != StageFactKind::StepCompleted
        || fact.episode_id != claim.id
        || fact.input_digest != material.input_digest
    {
        return Err(Error::Conflict(
            "optimization terminal fact differs from the consolidation step".into(),
        ));
    }
    let payload_bundle_digest = fact
        .payload
        .get("bundle")
        .and_then(|bundle| bundle.get("digest"))
        .and_then(|digest| digest.as_str());
    let selection = serde_json::to_value(material.selection).map_err(|_| Error::Internal)?;
    if fact
        .payload
        .get("status")
        .and_then(|status| status.as_str())
        != Some("candidate")
        || payload_bundle_digest != Some(material.bundle.digest.as_str())
        || fact.payload.get("selection") != Some(&selection)
    {
        return Err(Error::Conflict(
            "optimization terminal fact does not hold this candidate".into(),
        ));
    }
    Ok(())
}

fn pairs_digest(cycles: &[DevelopmentCycleRecord]) -> Result<String> {
    fingerprint(
        &cycles
            .iter()
            .map(|cycle| (&cycle.id, &cycle.pairs))
            .collect::<Vec<_>>(),
    )
}

/// A proposal is fixture-grade unless both the frozen environment and every
/// backing cycle are free of fixture evidence.
fn proposal_provenance(
    environment: &EnvironmentIdentityRecord,
    cycles: &[DevelopmentCycleRecord],
) -> EnvironmentEvidenceScope {
    if environment.evidence_scope == EnvironmentEvidenceScope::TrustedExecution
        && cycles
            .iter()
            .all(|cycle| cycle.provenance != DevelopmentExecutionProvenance::Fixture)
    {
        EnvironmentEvidenceScope::TrustedExecution
    } else {
        EnvironmentEvidenceScope::ProgramFixture
    }
}

/// Persists the proposal of a `Candidate` outcome exactly once. The identity is
/// derived from the claim; replaying the same content after a crash finds the
/// stored proposal and writes nothing, different content under that identity is
/// a conflict. The source closure is re-checked in the writing transaction so a
/// proposal can never appear after the logical revocation block.
async fn persist_consolidation_proposal(
    ctx: &Context,
    store: &Store,
    journal: &dyn OptimizationJournal,
    claim: &ConsolidationClaim,
    material: CandidateMaterial<'_>,
) -> Result<ConsolidationProposal> {
    if material.selection.decision != DevelopmentSelectionDecision::AcceptCandidate
        || material.selection.candidate_bundle_digest != material.bundle.digest
    {
        return Err(Error::Conflict(
            "development selection does not accept this candidate bundle".into(),
        ));
    }
    validate_resolved_bundle_identity(material.bundle)?;
    // The store has a single connection: journal reads open their own session, so
    // they happen before this function's session does.
    let terminal = journal
        .lookup(material.terminal_fact_id)
        .await?
        .ok_or_else(|| Error::Conflict("optimization terminal fact is missing".into()))?;
    verify_terminal_candidate_fact(&terminal, claim, &material)?;

    let mut session = store.session().await?;
    validate_claim_sources(&mut session, ctx, claim).await?;
    let index: ConsolidationScopeIndex = session.need(ctx, ARTIFACT_KIND, &claim.scope_id).await?;
    validate_scope_index(&index)?;
    if index.claim_ids.get(&claim.generation) != Some(&claim.id)
        || claim.revoke_watermark != index.scope.revoke_watermark
    {
        return Err(Error::Conflict("consolidation claim is stale".into()));
    }
    let scope = &index.scope;
    if material.selection.manifest_digest != scope.development_manifest_digest
        || material.selection.parent_bundle_digest != scope.parent_bundle_digest
    {
        return Err(Error::Conflict(
            "development selection differs from the consolidation scope".into(),
        ));
    }
    let environment: EnvironmentIdentityRecord = session
        .need(ctx, ARTIFACT_KIND, &scope.environment_id)
        .await?;
    validate_environment_record(&environment)?;
    let mut cycles = Vec::with_capacity(claim.cycle_ids.len());
    for cycle_id in &claim.cycle_ids {
        let cycle = load_cycle(&mut session, ctx, cycle_id).await?;
        if cycle.scope_id != claim.scope_id {
            return Err(Error::Conflict("cycle escaped consolidation scope".into()));
        }
        cycles.push(cycle);
    }
    let proposal = ConsolidationProposal {
        id: consolidation_proposal_id(&claim.id),
        schema_version: CONSOLIDATION_PROPOSAL_SCHEMA.into(),
        claim_id: claim.id.clone(),
        scope_id: claim.scope_id.clone(),
        cycle_ids: claim.cycle_ids.clone(),
        pairs_digest: pairs_digest(&cycles)?,
        parent_skill_digest: scope.parent_skill_digest.clone(),
        parent_bundle_digest: scope.parent_bundle_digest.clone(),
        candidate_skill_digest: skill_snapshot_digest(&material.bundle.skill)?,
        candidate_bundle: material.bundle.clone(),
        candidate_bundle_digest: material.bundle.digest.clone(),
        selection_digest: fingerprint(material.selection)?,
        terminal_fact_id: material.terminal_fact_id.into(),
        sources: claim.sources.clone(),
        provenance: proposal_provenance(&environment, &cycles),
        environment_digest: scope.environment_digest.clone(),
        revoke_watermark: claim.revoke_watermark,
    };
    validate_proposal(&proposal, claim)?;
    // Same content: nothing is written again. Other content under this identity is
    // a conflict (`put_immutable`). A transaction either holds the proposal with
    // all its edges or none of it, so an existing proposal already has its edges.
    let already_written = session
        .get::<ConsolidationProposal>(ctx, ARTIFACT_KIND, &proposal.id)
        .await?
        .is_some();
    put_immutable(&mut session, ctx, &proposal.id, &proposal).await?;
    if already_written {
        session.commit().await?;
        return Ok(proposal);
    }
    session
        .put_edge(ctx, ARTIFACT_KIND, &proposal.id, ARTIFACT_KIND, &claim.id)
        .await?;
    session
        .put_edge(
            ctx,
            ARTIFACT_KIND,
            &proposal.id,
            ARTIFACT_KIND,
            &proposal.terminal_fact_id,
        )
        .await?;
    for source in &proposal.sources {
        session
            .put_edge(ctx, ARTIFACT_KIND, &proposal.id, "run", &source.id)
            .await?;
    }
    session
        .audit(ctx, "monitoring.consolidation.proposal", &proposal.id)
        .await?;
    session.commit().await?;
    Ok(proposal)
}

/// What a stored proposal must equal, field for field, given its claim.
fn validate_proposal(proposal: &ConsolidationProposal, claim: &ConsolidationClaim) -> Result<()> {
    if proposal.schema_version != CONSOLIDATION_PROPOSAL_SCHEMA {
        return Err(Error::Invalid(
            "unsupported consolidation proposal schema".into(),
        ));
    }
    if proposal.id != consolidation_proposal_id(&claim.id)
        || proposal.claim_id != claim.id
        || proposal.scope_id != claim.scope_id
        || proposal.cycle_ids != claim.cycle_ids
        || proposal.sources != claim.sources
        || proposal.revoke_watermark != claim.revoke_watermark
    {
        return Err(Error::Conflict(
            "consolidation proposal differs from its claim".into(),
        ));
    }
    for (value, name) in [
        (&proposal.pairs_digest, "proposal pairs digest"),
        (
            &proposal.parent_skill_digest,
            "proposal parent skill digest",
        ),
        (
            &proposal.parent_bundle_digest,
            "proposal parent bundle digest",
        ),
        (
            &proposal.candidate_skill_digest,
            "proposal candidate skill digest",
        ),
        (
            &proposal.candidate_bundle_digest,
            "proposal candidate bundle digest",
        ),
        (&proposal.selection_digest, "proposal selection digest"),
        (&proposal.environment_digest, "proposal environment digest"),
    ] {
        validate_digest(value, name)?;
    }
    identifier(&proposal.terminal_fact_id)?;
    validate_resolved_bundle_identity(&proposal.candidate_bundle)?;
    if proposal.candidate_bundle.digest != proposal.candidate_bundle_digest
        || proposal.candidate_skill_digest
            != skill_snapshot_digest(&proposal.candidate_bundle.skill)?
    {
        return Err(Error::Conflict(
            "consolidation proposal candidate digests differ from its bundle".into(),
        ));
    }
    Ok(())
}

/// Reads a proposal; content that cleanup redacted is reported as such.
async fn load_proposal(
    session: &mut Session,
    ctx: &Context,
    proposal_id: &str,
) -> Result<ConsolidationProposal> {
    identifier(proposal_id)?;
    let value: serde_json::Value = session.need(ctx, ARTIFACT_KIND, proposal_id).await?;
    match value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
    {
        Some(CONSOLIDATION_PROPOSAL_SCHEMA) => {}
        Some(REDACTED_SCHEMA) => {
            return Err(Error::Conflict(
                "consolidation proposal was redacted after source revocation".into(),
            ));
        }
        _ => {
            return Err(Error::Invalid(
                "unsupported consolidation proposal schema".into(),
            ));
        }
    }
    serde_json::from_value(value).map_err(|_| Error::Internal)
}

/// A proposal can be staged only while it is the recorded candidate outcome of a
/// live claim: its sources unrevoked, its scope undrifted and its terminal run
/// record naming it. A proposal written but not yet named (a crash between the
/// two writes) is not stageable until the run completes.
async fn load_stageable_proposal(
    session: &mut Session,
    ctx: &Context,
    proposal_id: &str,
) -> Result<ConsolidationProposal> {
    let proposal = load_proposal(session, ctx, proposal_id).await?;
    let claim = load_live_claim(session, ctx, &proposal.claim_id).await?;
    validate_proposal(&proposal, &claim)?;
    let record = session
        .get::<ConsolidationRunRecord>(
            ctx,
            ARTIFACT_KIND,
            &format!("consolidation-run-{}", claim.id),
        )
        .await?
        .ok_or_else(|| {
            Error::Conflict("consolidation proposal has no terminal run record yet".into())
        })?;
    if claim.state != ConsolidationClaimState::CompletedCandidate
        || record.outcome != ConsolidationRunOutcome::Candidate
        || record.proposal_id.as_deref() != Some(proposal.id.as_str())
    {
        return Err(Error::Conflict(
            "proposal is not the recorded candidate outcome of its claim".into(),
        ));
    }
    Ok(proposal)
}

async fn persist_terminal_run_with_state(
    ctx: &Context,
    store: &Store,
    claim: ConsolidationClaim,
    input_digest: Option<String>,
    outcome: ConsolidationRunOutcome,
    state: ConsolidationClaimState,
    reason: &str,
) -> Result<ConsolidationRunRecord> {
    let id = format!("consolidation-run-{}", claim.id);
    persist_terminal_run_with_id(ctx, store, claim, id, input_digest, outcome, state, reason).await
}

#[allow(clippy::too_many_arguments)]
async fn persist_terminal_run_with_id(
    ctx: &Context,
    store: &Store,
    claim: ConsolidationClaim,
    id: String,
    input_digest: Option<String>,
    outcome: ConsolidationRunOutcome,
    state: ConsolidationClaimState,
    reason: &str,
) -> Result<ConsolidationRunRecord> {
    persist_terminal_run(
        ctx,
        store,
        claim,
        id,
        input_digest,
        outcome,
        state,
        reason,
        None,
    )
    .await
}

/// Writes the terminal run record and the claim's terminal state in one
/// transaction. Only a `Candidate` outcome names a proposal, and it must: every
/// other outcome (no change, rejected, uncertain, revoked, blocked budget)
/// leaves no proposal behind.
#[allow(clippy::too_many_arguments)]
async fn persist_terminal_run(
    ctx: &Context,
    store: &Store,
    claim: ConsolidationClaim,
    id: String,
    input_digest: Option<String>,
    outcome: ConsolidationRunOutcome,
    state: ConsolidationClaimState,
    reason: &str,
    proposal_id: Option<String>,
) -> Result<ConsolidationRunRecord> {
    if reason.is_empty() || reason.len() > 128 {
        return Err(Error::Invalid(
            "consolidation terminal category must be 1..=128 bytes".into(),
        ));
    }
    if (outcome == ConsolidationRunOutcome::Candidate) != proposal_id.is_some() {
        return Err(Error::Invalid(
            "only a candidate consolidation outcome names a proposal".into(),
        ));
    }
    let mut session = store.session().await?;
    let mut current = load_claim_record(&mut session, ctx, &claim.id).await?;
    if let Some(existing) = session
        .get::<ConsolidationRunRecord>(ctx, ARTIFACT_KIND, &id)
        .await?
    {
        if existing.input_digest != input_digest {
            return Err(Error::Conflict(
                "terminal consolidation input differs".into(),
            ));
        }
        if existing.proposal_id != proposal_id {
            return Err(Error::Conflict(
                "terminal consolidation proposal differs".into(),
            ));
        }
        session.commit().await?;
        return Ok(existing);
    }
    let index: ConsolidationScopeIndex =
        session.need(ctx, ARTIFACT_KIND, &current.scope_id).await?;
    let mut budget_call_ids = session
        .budget_calls_for_group(ctx, &index.scope.billing_scope, &current.id)
        .await?
        .into_iter()
        .map(|call| call.call_id)
        .collect::<Vec<_>>();
    budget_call_ids.sort();
    current.state = state;
    current.execution_input_digest = input_digest.clone();
    session
        .put(ctx, ARTIFACT_KIND, &current.id, ctx.actor(), &current)
        .await?;
    let record = ConsolidationRunRecord {
        id,
        schema_version: CONSOLIDATION_RUN_SCHEMA.into(),
        claim_id: current.id.clone(),
        input_digest,
        outcome,
        reason: reason.into(),
        budget_call_ids,
        proposal_id,
    };
    put_immutable(&mut session, ctx, &record.id, &record).await?;
    session
        .put_edge(ctx, ARTIFACT_KIND, &record.id, ARTIFACT_KIND, &current.id)
        .await?;
    session
        .audit(ctx, "monitoring.consolidation.complete", &record.id)
        .await?;
    session.commit().await?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimization::PairedTaskResult;

    fn result(
        task_id: &str,
        parent_score_micros: u32,
        candidate_score_micros: u32,
        parent_passed: bool,
        candidate_passed: bool,
    ) -> PairedTaskResult {
        PairedTaskResult {
            task_id: task_id.into(),
            parent_score_micros,
            candidate_score_micros,
            parent_passed,
            candidate_passed,
            parent_execution_id: format!("parent-{task_id}"),
            candidate_execution_id: format!("candidate-{task_id}"),
            grader_receipt_digest: "0".repeat(64),
        }
    }

    fn report(results: Vec<PairedTaskResult>) -> DevelopmentRunReport {
        DevelopmentRunReport {
            request_id: "request".into(),
            manifest_digest: "0".repeat(64),
            parent_bundle_digest: "0".repeat(64),
            candidate_bundle_digest: "0".repeat(64),
            environment_digest: "0".repeat(64),
            grader_digest: "0".repeat(64),
            results,
            execution_receipt_id: "execution".into(),
            usage_record_ids: vec![],
            provenance: DevelopmentExecutionProvenance::Fixture,
        }
    }

    #[test]
    fn a_pair_class_is_the_pass_fail_transition_and_the_score_is_only_recorded() {
        let (pairs, contrast) = pair_tasks(&report(vec![
            result("fail-fail-up", 100, 900, false, false),
            result("pass-fail-up", 500, 600, true, false),
            result("fail-pass-down", 500, 100, false, true),
            result("pass-pass-down", 900, 800, true, true),
        ]))
        .unwrap();
        let classes: Vec<_> = pairs
            .iter()
            .map(|pair| (pair.task_id.as_str(), pair.class))
            .collect();
        assert_eq!(
            classes,
            vec![
                ("fail-fail-up", ConsolidationClass::PersistentFail),
                ("pass-fail-up", ConsolidationClass::Regressed),
                ("fail-pass-down", ConsolidationClass::Improved),
                ("pass-pass-down", ConsolidationClass::StableSuccess),
            ]
        );
        assert!(contrast);
        assert_eq!(pairs[0].parent_score_micros, 100);
        assert_eq!(pairs[0].candidate_score_micros, 900);
    }

    #[test]
    fn contrast_is_any_score_or_pass_difference_and_nothing_else() {
        assert!(
            !pair_tasks(&report(vec![result("same", 5, 5, true, true)]))
                .unwrap()
                .1
        );
        assert!(
            pair_tasks(&report(vec![result("score-only", 5, 6, true, true)]))
                .unwrap()
                .1
        );
        assert!(
            pair_tasks(&report(vec![result("pass-only", 5, 5, false, true)]))
                .unwrap()
                .1
        );
    }

    #[test]
    fn a_repeated_task_an_empty_report_or_an_out_of_range_score_is_not_a_pairing() {
        assert!(matches!(
            pair_tasks(&report(vec![])),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            pair_tasks(&report(vec![
                result("task", 1, 2, true, true),
                result("task", 3, 4, true, true),
            ])),
            Err(Error::Invalid(message)) if message.contains("repeats a task id")
        ));
        assert!(matches!(
            pair_tasks(&report(vec![result("task", 1, 1_000_001, true, true)])),
            Err(Error::Invalid(_))
        ));
    }

    fn environment(scope: EnvironmentEvidenceScope) -> EnvironmentIdentityRecord {
        EnvironmentIdentityRecord {
            id: "environment".into(),
            schema_version: ENVIRONMENT_IDENTITY_SCHEMA.into(),
            identity_digest: "1".repeat(64),
            namespace: "tenant".into(),
            evidence_scope: scope,
            profile_id: "profile".into(),
            release_id: None,
            bundle_digest: None,
            environment_digest: "2".repeat(64),
            model_identity: "model".into(),
            ordered_tools: vec![],
            host_id: "host".into(),
            host_version: "1".into(),
            host_surface_digest: "3".repeat(64),
            host_capabilities_digest: "4".repeat(64),
            mandatory_context_digest: "5".repeat(64),
            development_manifest_digest: "6".repeat(64),
            grader_digest: "7".repeat(64),
            generation_strategy_digest: "8".repeat(64),
            revoke_watermark: 1,
        }
    }

    fn cycle(provenance: DevelopmentExecutionProvenance) -> DevelopmentCycleRecord {
        DevelopmentCycleRecord {
            id: "cycle".into(),
            schema_version: DEVELOPMENT_CYCLE_SCHEMA.into(),
            scope_id: "scope".into(),
            ordinal: 1,
            report_fact_id: "fact".into(),
            report_digest: "9".repeat(64),
            request_id: "request".into(),
            candidate_bundle_digest: "a".repeat(64),
            execution_receipt_id: "execution".into(),
            usage_record_ids: vec![],
            provenance,
            pairs: vec![],
            has_contrast: false,
            sources: vec![],
        }
    }

    #[test]
    fn a_proposal_is_trusted_only_when_neither_the_environment_nor_any_cycle_is_fixture() {
        use DevelopmentExecutionProvenance::{Fixture, IsolatedRunner, RegisteredPureFunction};
        use EnvironmentEvidenceScope::{ProgramFixture, TrustedExecution};
        let trusted = environment(TrustedExecution);
        let fixture = environment(ProgramFixture);
        assert_eq!(
            proposal_provenance(
                &trusted,
                &[cycle(RegisteredPureFunction), cycle(IsolatedRunner)]
            ),
            TrustedExecution
        );
        // One fixture cycle taints the proposal; so does a fixture environment.
        assert_eq!(
            proposal_provenance(&trusted, &[cycle(RegisteredPureFunction), cycle(Fixture)]),
            ProgramFixture
        );
        assert_eq!(
            proposal_provenance(&fixture, &[cycle(RegisteredPureFunction)]),
            ProgramFixture
        );
        assert_eq!(
            proposal_provenance(&fixture, &[cycle(Fixture)]),
            ProgramFixture
        );
    }

    #[test]
    fn a_stored_cycle_must_equal_what_pairing_would_have_written() {
        let pair = |class| TaskPairV1 {
            task_id: "task".into(),
            parent_passed: true,
            candidate_passed: true,
            class,
            parent_score_micros: 1,
            candidate_score_micros: 2,
        };
        let mut valid = cycle(DevelopmentExecutionProvenance::Fixture);
        valid.pairs = vec![pair(ConsolidationClass::StableSuccess)];
        valid.has_contrast = true;
        assert!(validate_cycle_record(&valid).is_ok());
        let mut score_biased = valid.clone();
        score_biased.pairs = vec![pair(ConsolidationClass::Improved)];
        assert!(matches!(
            validate_cycle_record(&score_biased),
            Err(Error::Conflict(_))
        ));
        let mut wrong_contrast = valid.clone();
        wrong_contrast.has_contrast = false;
        assert!(matches!(
            validate_cycle_record(&wrong_contrast),
            Err(Error::Conflict(_))
        ));
        let mut empty = valid;
        empty.pairs.clear();
        assert!(matches!(
            validate_cycle_record(&empty),
            Err(Error::Invalid(_))
        ));
    }
}
