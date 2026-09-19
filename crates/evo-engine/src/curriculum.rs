//! Learner-conditioned curriculum control. V2 remains offline and zero-budget.
use crate::curriculum_profiles::{RegisteredPureFunctionProfileV1, ValidityReportV1};
use crate::executor::RootBudget;
use crate::optimization::verified_development_observation_in_session;
use crate::release_store::{HostApplicationRecord, ReleaseStore, TrustedHostExecutionReceipt};
use evo_core::curriculum::{
    CurriculumControlProfileV1, CurriculumRuntimeStatus, DevelopmentCycleObservation, LearnerState,
    LearnerStateV2, PlateauSignalV1, ProbeJobV1, ProbeTerminal, ProposalAttemptOutcome,
    StructuredTestProposalV1, TaskProposal, detect_plateau_signal, next_task,
};
use evo_core::evaluation::DataUse;
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Legacy v1 behavior is retained for compatibility and is not v2 evidence.
pub fn step(
    budget: &mut RootBudget,
    state: &LearnerState,
    pool: &[TaskProposal],
    cost: i64,
) -> Result<String> {
    budget.reserve(cost)?;
    next_task(state, pool).map_err(|_| Error::NotFound)
}

#[allow(clippy::too_many_arguments)]
async fn record_applied_asset(
    store: &Store,
    context: &Context,
    owner: &str,
    state_id: &str,
    run_id: &str,
    application_record_id: &str,
    execution_receipt_id: &str,
) -> Result<LearnerStateV2> {
    context.require(&[Role::Admin])?;
    for value in [
        state_id,
        run_id,
        application_record_id,
        execution_receipt_id,
    ] {
        identifier(value)?;
    }
    let mut session = store.session().await?;
    let mut state: LearnerStateV2 =
        need_record(&mut session, context, STATE_KIND, state_id).await?;
    verify_watermark(&mut session, context, &state).await?;
    verify_state_sources(&mut session, context, &state).await?;
    let snapshot =
        ReleaseStore::validate_run_snapshot_live_in_session(context, &mut session, run_id).await?;
    let application: HostApplicationRecord = session
        .need(context, "receipt", application_record_id)
        .await?;
    let execution: TrustedHostExecutionReceipt = session
        .need(context, "artifact", execution_receipt_id)
        .await?;
    let release_id = snapshot.release_id.clone().ok_or_else(|| {
        Error::Conflict("zero-skill or unmanaged run cannot update learner assets".into())
    })?;
    let bundle_digest = snapshot
        .bundle_digest
        .clone()
        .ok_or_else(|| Error::Conflict("managed run lacks its frozen bundle digest".into()))?;
    let applied = application
        .receipt
        .as_ref()
        .ok_or_else(|| Error::Conflict("host application has no trusted applied receipt".into()))?;
    if application.id != application_record_id
        || application.run_id != run_id
        || application.release_id.as_deref() != Some(release_id.as_str())
        || application.actual_request_digest != snapshot.request_digest
        || application.execution_receipt_id.as_deref() != Some(execution_receipt_id)
        || applied.bundle_digest != bundle_digest
        || applied.request_digest != snapshot.request_digest
        || applied.used.is_empty()
        || execution.id != execution_receipt_id
        || execution.run_id != run_id
        || execution.request_digest != snapshot.request_digest
        || execution.environment_digest != snapshot.environment_digest
        || execution.host_surface_digest != snapshot.host_surface_digest
        || !applied
            .used
            .iter()
            .all(|id| execution.used_ids.contains(id))
    {
        return Err(Error::Conflict(
            "applied asset, live snapshot and Host receipt do not form one closure".into(),
        ));
    }
    let asset = evo_core::curriculum::AppliedLearnerAssetRef {
        release_id: release_id.clone(),
        bundle_digest,
        run_application_id: snapshot.id.clone(),
        host_execution_receipt_id: execution.id.clone(),
    };
    if let Some(existing) = state
        .applied_assets
        .iter()
        .find(|existing| existing.release_id == release_id)
    {
        if fingerprint(existing)? != fingerprint(&asset)? {
            return Err(Error::Conflict(
                "applied learner asset identity changed".into(),
            ));
        }
        session.commit().await?;
        return Ok(state);
    }
    state.applied_assets.push(asset);
    state
        .applied_assets
        .sort_by(|left, right| left.release_id.cmp(&right.release_id));
    state.validate()?;
    put_record(&mut session, context, STATE_KIND, &state.id, owner, &state).await?;
    let state_storage_id = storage_id(STATE_KIND, &state.id)?;
    for (kind, id) in [
        ("release", release_id.as_str()),
        ("artifact", snapshot.id.as_str()),
        ("receipt", application.id.as_str()),
        ("artifact", execution.id.as_str()),
    ] {
        session
            .put_edge(context, "artifact", &state_storage_id, kind, id)
            .await?;
    }
    session.commit().await?;
    Ok(state)
}

const PROFILE_KIND: &str = "curriculum_profile_v1";
const STATE_KIND: &str = "learner_state_v2";
const CYCLE_KIND: &str = "development_cycle_receipt_v1";
const JOB_KIND: &str = "coverage_probe_job_v1";
const ATTEMPT_KIND: &str = "curriculum_proposal_attempt_v1";
const PROPOSAL_KIND: &str = "structured_test_proposal_v1";
const VALIDITY_KIND: &str = "curriculum_validity_report_v1";
const SELECTION_KIND: &str = "curriculum_selection_v1";
const ENVELOPE_SCHEMA: &str = "rsia.curriculum_artifact_envelope.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurriculumSourceKindV1 {
    TaskSpace,
    TargetSpec,
    OracleSpec,
    RunnerSpec,
    DevelopmentCycleExecution,
    ProposalDiagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumSourceArtifactV1 {
    pub schema_version: String,
    pub id: String,
    pub source_kind: CurriculumSourceKindV1,
    pub data_use: DataUse,
    pub subject_digest: String,
    pub body_digest: String,
    pub body: serde_json::Value,
    pub dependency_ids: Vec<String>,
}

impl CurriculumSourceArtifactV1 {
    pub const SCHEMA: &'static str = "rsia.curriculum_source.v1";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA
            || self.data_use != DataUse::Development
            || self.body.is_null()
            || self.body_digest != fingerprint(&self.body)?
        {
            return Err(Error::Invalid("invalid typed curriculum source".into()));
        }
        identifier(&self.id)?;
        validate_digest(&self.subject_digest)?;
        validate_digest(&self.body_digest)?;
        let mut dependencies = std::collections::BTreeSet::new();
        for dependency in &self.dependency_ids {
            identifier(dependency)?;
            if dependency == &self.id || !dependencies.insert(dependency.as_str()) {
                return Err(Error::Invalid(
                    "invalid curriculum source dependency closure".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentCycleReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub state_id: String,
    pub development_request_fact_id: String,
    pub development_observed_fact_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalAttemptReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub job_id: String,
    pub outcome: ProposalAttemptOutcome,
    pub source_artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumTaskCandidateV1 {
    pub id: String,
    pub parent_family: String,
    pub coverage_bucket_id: Option<String>,
    pub stable_choice_seq: u32,
    pub proposal_id: String,
    pub validity_report_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionDecisionV1 {
    pub schema_version: String,
    pub id: String,
    pub state_id: String,
    pub state_digest: String,
    pub chosen_task_id: String,
    pub proposal_id: String,
    pub stable_choice_seq: u32,
    pub reason: String,
    pub budget_status: String,
}

#[derive(Clone)]
pub struct PersistentCurriculumCoordinator {
    store: Store,
    context: Context,
    owner: String,
}

impl PersistentCurriculumCoordinator {
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

    pub async fn register_source(&self, source: CurriculumSourceArtifactV1) -> Result<()> {
        self.context.require(&[Role::Admin])?;
        source.validate()?;
        let mut session = self.store.session().await?;
        for dependency in &source.dependency_ids {
            let existing: CurriculumSourceArtifactV1 =
                session.need(&self.context, "artifact", dependency).await?;
            existing.validate()?;
        }
        if let Some(existing) = session
            .get::<CurriculumSourceArtifactV1>(&self.context, "artifact", &source.id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&source)? {
                return Err(Error::Conflict(
                    "immutable curriculum source differs".into(),
                ));
            }
            session.commit().await?;
            return Ok(());
        }
        session
            .put(&self.context, "artifact", &source.id, &self.owner, &source)
            .await?;
        for dependency in &source.dependency_ids {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &source.id,
                    "artifact",
                    dependency,
                )
                .await?;
        }
        session.commit().await
    }

    pub async fn register_profile_and_state(
        &self,
        profile: CurriculumControlProfileV1,
        state: LearnerStateV2,
    ) -> Result<()> {
        self.context.require(&[Role::Admin])?;
        profile.validate()?;
        state.validate()?;
        if !state.completed_cycles.is_empty()
            || !state.failure_clusters.is_empty()
            || !state.applied_assets.is_empty()
            || !state.development_fact_ids.is_empty()
            || state.active_probe_job_id.is_some()
            || state.last_trigger_window_digest.is_some()
            || state.cooldown_remaining_cycles != 0
            || state
                .coverage_buckets
                .iter()
                .any(|bucket| bucket.observed_independent_clusters != 0)
        {
            return Err(Error::Invalid(
                "initial learner state cannot prefill derived completion or cooldown facts".into(),
            ));
        }
        if profile.profile_id != state.profile_id || profile.runner_digest != state.runner_digest {
            return Err(Error::Conflict("profile and learner state differ".into()));
        }
        let mut session = self.store.session().await?;
        if get_record::<CurriculumControlProfileV1>(
            &mut session,
            &self.context,
            PROFILE_KIND,
            &profile.profile_id,
        )
        .await?
        .is_some()
            || get_record::<LearnerStateV2>(&mut session, &self.context, STATE_KIND, &state.id)
                .await?
                .is_some()
        {
            return Err(Error::Conflict(
                "curriculum control already registered".into(),
            ));
        }
        verify_watermark(&mut session, &self.context, &state).await?;
        let sources = verify_state_sources(&mut session, &self.context, &state).await?;
        require_profile_sources(&profile, &sources)?;
        put_record(
            &mut session,
            &self.context,
            PROFILE_KIND,
            &profile.profile_id,
            &self.owner,
            &profile,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            STATE_KIND,
            &state.id,
            &self.owner,
            &state,
        )
        .await?;
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        let profile_storage_id = storage_id(PROFILE_KIND, &profile.profile_id)?;
        for source in &state.source_artifact_ids {
            for target in [&state_storage_id, &profile_storage_id] {
                session
                    .put_edge(&self.context, "artifact", target, "artifact", source)
                    .await?;
            }
        }
        session.commit().await
    }

    pub async fn record_cycle(&self, receipt: DevelopmentCycleReceiptV1) -> Result<LearnerStateV2> {
        if receipt.schema_version != "rsia.development_cycle_receipt.v1" {
            return Err(Error::Invalid("unsupported cycle receipt schema".into()));
        }
        identifier(&receipt.id)?;
        identifier(&receipt.state_id)?;
        identifier(&receipt.development_request_fact_id)?;
        identifier(&receipt.development_observed_fact_id)?;
        let mut session = self.store.session().await?;
        let mut state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, &receipt.state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        if let Some(existing) = get_record::<DevelopmentCycleReceiptV1>(
            &mut session,
            &self.context,
            CYCLE_KIND,
            &receipt.id,
        )
        .await?
        {
            if fingerprint(&existing)? != fingerprint(&receipt)? {
                return Err(Error::Conflict("cycle receipt idempotency conflict".into()));
            }
            session.commit().await?;
            return Ok(state);
        }
        let verified = verified_development_observation_in_session(
            &self.context,
            &mut session,
            &receipt.development_request_fact_id,
            &receipt.development_observed_fact_id,
        )
        .await?;
        if verified.environment_digest != state.environment_digest
            || verified.grader_digest != state.grader_digest
        {
            return Err(Error::Conflict(
                "verified development cycle context differs from learner state".into(),
            ));
        }
        let observation = DevelopmentCycleObservation {
            cycle_id: format!(
                "development-cycle-{}-{}-{}",
                verified.episode_id, verified.step, verified.attempt
            ),
            environment_digest: verified.environment_digest,
            grader_digest: verified.grader_digest,
            cluster_ids: verified
                .outcomes
                .iter()
                .map(|outcome| outcome.parent_family.clone())
                .collect(),
            successful_clusters: verified
                .outcomes
                .iter()
                .filter(|outcome| outcome.candidate_passed)
                .count()
                .try_into()
                .map_err(|_| Error::Invalid("too many development outcomes".into()))?,
            paired_gain_micros: verified
                .outcomes
                .iter()
                .map(|outcome| {
                    i32::try_from(
                        i64::from(outcome.candidate_score_micros)
                            - i64::from(outcome.parent_score_micros),
                    )
                    .map_err(|_| Error::Invalid("paired gain is out of range".into()))
                })
                .collect::<Result<Vec<_>>>()?,
            source_artifact_ids: vec![
                receipt.development_request_fact_id.clone(),
                receipt.development_observed_fact_id.clone(),
            ],
        };
        observation.validate()?;
        if state
            .completed_cycles
            .iter()
            .any(|cycle| cycle.cycle_id == observation.cycle_id)
        {
            return Err(Error::Conflict("development cycle already reduced".into()));
        }
        for source in &observation.source_artifact_ids {
            if !state.development_fact_ids.contains(source) {
                state.development_fact_ids.push(source.clone());
            }
        }
        for outcome in &verified.outcomes {
            if (!outcome.candidate_passed
                || outcome.candidate_score_micros < outcome.parent_score_micros)
                && !state.failure_clusters.contains(&outcome.parent_family)
            {
                state.failure_clusters.push(outcome.parent_family.clone());
            }
            if let Some(bucket) = state
                .coverage_buckets
                .iter_mut()
                .find(|bucket| bucket.bucket_id == outcome.parent_family)
            {
                bucket.observed_independent_clusters = bucket
                    .observed_independent_clusters
                    .checked_add(1)
                    .ok_or(Error::Budget)?;
            }
        }
        state.failure_clusters.sort();
        state.development_fact_ids.sort();
        state.completed_cycles.push(observation);
        state.cooldown_remaining_cycles = state.cooldown_remaining_cycles.saturating_sub(1);
        state.validate()?;
        put_record(
            &mut session,
            &self.context,
            CYCLE_KIND,
            &receipt.id,
            &self.owner,
            &receipt,
        )
        .await?;
        let cycle_storage_id = storage_id(CYCLE_KIND, &receipt.id)?;
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        for source in [
            &receipt.development_request_fact_id,
            &receipt.development_observed_fact_id,
        ] {
            for target in [&cycle_storage_id, &state_storage_id] {
                session
                    .put_edge(&self.context, "artifact", target, "artifact", source)
                    .await?;
            }
        }
        put_record(
            &mut session,
            &self.context,
            STATE_KIND,
            &state.id,
            &self.owner,
            &state,
        )
        .await?;
        session.commit().await?;
        Ok(state)
    }

    pub async fn schedule_probe(
        &self,
        profile_id: &str,
        state_id: &str,
        root_budget_limit_micros: u64,
    ) -> Result<ProbeJobV1> {
        let mut session = self.store.session().await?;
        let profile: CurriculumControlProfileV1 =
            need_record(&mut session, &self.context, PROFILE_KIND, profile_id).await?;
        let mut state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        let trigger = detect_plateau_signal(&profile, &state)?;
        let trigger_digest = fingerprint(&trigger)?;
        let job_id = format!(
            "probe-{}",
            &fingerprint(&(profile_id, &trigger_digest))?[..24]
        );
        if let Some(existing) =
            get_record::<ProbeJobV1>(&mut session, &self.context, JOB_KIND, &job_id).await?
        {
            if existing.root_budget_limit_micros != root_budget_limit_micros {
                return Err(Error::Conflict(
                    "probe trigger cannot change its frozen root budget".into(),
                ));
            }
            session.commit().await?;
            return Ok(existing);
        }
        if let Some(active) = &state.active_probe_job_id {
            return Err(Error::Conflict(format!(
                "active probe job exists: {active}"
            )));
        }
        let terminal = match &trigger {
            PlateauSignalV1::NotTriggered { reasons }
                if reasons.iter().any(|reason| reason == "cooldown") =>
            {
                Some(ProbeTerminal::Cooldown)
            }
            PlateauSignalV1::NotTriggered { .. } => Some(ProbeTerminal::UnsupportedScope),
            _ if profile.monetary_limit_micros == 0
                || profile.runtime_status == CurriculumRuntimeStatus::Disabled =>
            {
                Some(ProbeTerminal::BudgetExhausted)
            }
            _ => None,
        };
        let job = ProbeJobV1 {
            schema_version: "rsia.coverage_probe_job.v1".into(),
            id: job_id.clone(),
            profile_id: profile.profile_id.clone(),
            state_id: state.id.clone(),
            state_digest: fingerprint(&state)?,
            trigger_digest: trigger_digest.clone(),
            root_budget_limit_micros,
            curriculum_share_limit_micros: profile.curriculum_limit(root_budget_limit_micros)?,
            effective_monetary_limit_micros: profile.monetary_limit_micros,
            provider_dispatch_count: 0,
            attempts: vec![],
            terminal,
        };
        let triggered = !matches!(trigger, PlateauSignalV1::NotTriggered { .. });
        if job.terminal.is_none() {
            state.active_probe_job_id = Some(job.id.clone());
        }
        if triggered {
            state.cooldown_remaining_cycles = profile.cooldown_completed_cycles;
        }
        state.last_trigger_window_digest = Some(trigger_digest);
        put_record(
            &mut session,
            &self.context,
            JOB_KIND,
            &job.id,
            &self.owner,
            &job,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            STATE_KIND,
            &state.id,
            &self.owner,
            &state,
        )
        .await?;
        let job_storage_id = storage_id(JOB_KIND, &job.id)?;
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        session
            .put_edge(
                &self.context,
                "artifact",
                &job_storage_id,
                "artifact",
                &state_storage_id,
            )
            .await?;
        for source in &state.source_artifact_ids {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &job_storage_id,
                    "artifact",
                    source,
                )
                .await?;
        }
        session.commit().await?;
        Ok(job)
    }

    pub async fn record_probe_attempt(
        &self,
        receipt: ProposalAttemptReceiptV1,
    ) -> Result<ProbeJobV1> {
        if receipt.schema_version != "rsia.curriculum_proposal_attempt.v1"
            || receipt.source_artifact_ids.is_empty()
        {
            return Err(Error::Invalid("invalid proposal attempt receipt".into()));
        }
        identifier(&receipt.id)?;
        identifier(&receipt.job_id)?;
        if receipt.outcome == ProposalAttemptOutcome::Valid {
            return Err(Error::Forbidden);
        }
        let mut session = self.store.session().await?;
        let mut job: ProbeJobV1 =
            need_record(&mut session, &self.context, JOB_KIND, &receipt.job_id).await?;
        let profile: CurriculumControlProfileV1 =
            need_record(&mut session, &self.context, PROFILE_KIND, &job.profile_id).await?;
        let mut state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, &job.state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        if let Some(existing) = get_record::<ProposalAttemptReceiptV1>(
            &mut session,
            &self.context,
            ATTEMPT_KIND,
            &receipt.id,
        )
        .await?
        {
            if fingerprint(&existing)? != fingerprint(&receipt)? {
                return Err(Error::Conflict(
                    "proposal attempt idempotency conflict".into(),
                ));
            }
            session.commit().await?;
            return Ok(job);
        }
        if state.active_probe_job_id.as_deref() != Some(job.id.as_str()) {
            return Err(Error::Conflict(
                "probe job is not the active state job".into(),
            ));
        }
        for source_id in &receipt.source_artifact_ids {
            if !state.source_artifact_ids.contains(source_id) {
                return Err(Error::Forbidden);
            }
            need_typed_source(&mut session, &self.context, source_id).await?;
        }
        job.record_attempt(&profile, receipt.outcome)?;
        if job.terminal.is_some() {
            state.active_probe_job_id = None;
            state.cooldown_remaining_cycles = profile.cooldown_completed_cycles;
        }
        put_record(
            &mut session,
            &self.context,
            ATTEMPT_KIND,
            &receipt.id,
            &self.owner,
            &receipt,
        )
        .await?;
        let attempt_storage_id = storage_id(ATTEMPT_KIND, &receipt.id)?;
        let job_storage_id = storage_id(JOB_KIND, &job.id)?;
        session
            .put_edge(
                &self.context,
                "artifact",
                &attempt_storage_id,
                "artifact",
                &job_storage_id,
            )
            .await?;
        for source in &receipt.source_artifact_ids {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &attempt_storage_id,
                    "artifact",
                    source,
                )
                .await?;
        }
        put_record(
            &mut session,
            &self.context,
            JOB_KIND,
            &job.id,
            &self.owner,
            &job,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            STATE_KIND,
            &state.id,
            &self.owner,
            &state,
        )
        .await?;
        session.commit().await?;
        Ok(job)
    }

    pub async fn store_proposal(
        &self,
        state_id: &str,
        proposal: StructuredTestProposalV1,
    ) -> Result<()> {
        proposal.validate()?;
        let mut session = self.store.session().await?;
        let state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        let job: ProbeJobV1 = need_record(
            &mut session,
            &self.context,
            JOB_KIND,
            &proposal.probe_job_id,
        )
        .await?;
        let active =
            state.active_probe_job_id.as_deref() == Some(job.id.as_str()) && job.terminal.is_none();
        let offline_blocked = state.active_probe_job_id.is_none()
            && job.terminal == Some(ProbeTerminal::BudgetExhausted);
        if job.state_id != state.id || (!active && !offline_blocked) {
            return Err(Error::Conflict(
                "proposal is not bound to the active probe".into(),
            ));
        }
        for source in &proposal.source_artifact_ids {
            if !state.source_artifact_ids.contains(source) {
                return Err(Error::Forbidden);
            }
            need_typed_source(&mut session, &self.context, source).await?;
        }
        if let Some(existing) = get_record::<StructuredTestProposalV1>(
            &mut session,
            &self.context,
            PROPOSAL_KIND,
            &proposal.id,
        )
        .await?
        {
            if fingerprint(&existing)? != fingerprint(&proposal)? {
                return Err(Error::Conflict("proposal idempotency conflict".into()));
            }
            session.commit().await?;
            return Ok(());
        }
        put_record(
            &mut session,
            &self.context,
            PROPOSAL_KIND,
            &proposal.id,
            &self.owner,
            &proposal,
        )
        .await?;
        let proposal_storage_id = storage_id(PROPOSAL_KIND, &proposal.id)?;
        let job_storage_id = storage_id(JOB_KIND, &proposal.probe_job_id)?;
        session
            .put_edge(
                &self.context,
                "artifact",
                &proposal_storage_id,
                "artifact",
                &job_storage_id,
            )
            .await?;
        for source in &proposal.source_artifact_ids {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &proposal_storage_id,
                    "artifact",
                    source,
                )
                .await?;
        }
        session.commit().await
    }

    pub async fn record_validity_report(
        &self,
        state_id: &str,
        report: ValidityReportV1,
    ) -> Result<String> {
        let proposal_id = report.proposal_id().to_string();
        let report_id = format!(
            "validity-{}",
            &fingerprint(&(state_id, proposal_id.as_str(), &report))?[..24]
        );
        let mut session = self.store.session().await?;
        let state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        let proposal: StructuredTestProposalV1 =
            need_record(&mut session, &self.context, PROPOSAL_KIND, &proposal_id).await?;
        let job: ProbeJobV1 = need_record(
            &mut session,
            &self.context,
            JOB_KIND,
            &proposal.probe_job_id,
        )
        .await?;
        let profile: CurriculumControlProfileV1 =
            need_record(&mut session, &self.context, PROFILE_KIND, &job.profile_id).await?;
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        let (proposal_digest, target_digest, oracle_digest, runner_digest) =
            report.binding_digests();
        if job.state_id != state.id
            || report.profile_id() != job.profile_id
            || proposal_digest != fingerprint(&proposal)?
            || target_digest != registered.target_digest
            || oracle_digest != profile.oracle_digest
            || runner_digest != profile.runner_digest
            || profile.oracle_digest != registered.oracle_digest
            || profile.runner_digest != registered.runner_digest
        {
            return Err(Error::Conflict(
                "validity report differs from its protected probe/profile".into(),
            ));
        }
        put_record(
            &mut session,
            &self.context,
            VALIDITY_KIND,
            &report_id,
            &self.owner,
            &report,
        )
        .await?;
        let validity_storage_id = storage_id(VALIDITY_KIND, &report_id)?;
        let proposal_storage_id = storage_id(PROPOSAL_KIND, &proposal.id)?;
        let job_storage_id = storage_id(JOB_KIND, &job.id)?;
        for dependency in [&proposal_storage_id, &job_storage_id] {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &validity_storage_id,
                    "artifact",
                    dependency,
                )
                .await?;
        }
        session.commit().await?;
        Ok(report_id)
    }

    pub async fn select_next_task(
        &self,
        state_id: &str,
        candidates: &[CurriculumTaskCandidateV1],
    ) -> Result<TaskSelectionDecisionV1> {
        let mut session = self.store.session().await?;
        let state: LearnerStateV2 =
            need_record(&mut session, &self.context, STATE_KIND, state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        let mut eligible: Vec<_> = Vec::new();
        for candidate in candidates {
            identifier(&candidate.id)?;
            identifier(&candidate.parent_family)?;
            identifier(&candidate.proposal_id)?;
            identifier(&candidate.validity_report_id)?;
            let report: ValidityReportV1 = need_record(
                &mut session,
                &self.context,
                VALIDITY_KIND,
                &candidate.validity_report_id,
            )
            .await?;
            if report.proposal_id() != candidate.proposal_id {
                return Err(Error::Conflict(
                    "task candidate and validity report proposal differ".into(),
                ));
            }
            if report.is_development_eligible() {
                eligible.push(candidate);
            }
        }
        if eligible.is_empty() {
            return Err(Error::Conflict(
                "no runner-verified development task; sandbox unavailable".into(),
            ));
        }
        eligible.sort_by_key(|candidate| candidate.stable_choice_seq);
        let chosen = eligible
            .iter()
            .find(|candidate| state.failure_clusters.contains(&candidate.parent_family))
            .copied()
            .or_else(|| eligible.first().copied())
            .ok_or(Error::NotFound)?;
        let proposal: StructuredTestProposalV1 = need_record(
            &mut session,
            &self.context,
            PROPOSAL_KIND,
            &chosen.proposal_id,
        )
        .await?;
        if proposal.parent_family != chosen.parent_family {
            return Err(Error::Conflict(
                "selection candidate and proposal family differ".into(),
            ));
        }
        let state_digest = fingerprint(&state)?;
        let decision = TaskSelectionDecisionV1 {
            schema_version: "rsia.curriculum_selection.v1".into(),
            id: format!(
                "selection-{}",
                &fingerprint(&(state_id, chosen.id.as_str(), &state_digest))?[..24]
            ),
            state_id: state.id.clone(),
            state_digest,
            chosen_task_id: chosen.id.clone(),
            proposal_id: chosen.proposal_id.clone(),
            stable_choice_seq: chosen.stable_choice_seq,
            reason: "runner_verified".into(),
            budget_status: "disabled_zero_budget".into(),
        };
        put_record(
            &mut session,
            &self.context,
            SELECTION_KIND,
            &decision.id,
            &self.owner,
            &decision,
        )
        .await?;
        let selection_storage_id = storage_id(SELECTION_KIND, &decision.id)?;
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        let proposal_storage_id = storage_id(PROPOSAL_KIND, &proposal.id)?;
        let validity_storage_id = storage_id(VALIDITY_KIND, &chosen.validity_report_id)?;
        for dependency in [
            &state_storage_id,
            &proposal_storage_id,
            &validity_storage_id,
        ] {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &selection_storage_id,
                    "artifact",
                    dependency,
                )
                .await?;
        }
        session.commit().await?;
        Ok(decision)
    }

    pub async fn record_applied_learning_asset(
        &self,
        state_id: &str,
        run_id: &str,
        application_record_id: &str,
        execution_receipt_id: &str,
    ) -> Result<LearnerStateV2> {
        record_applied_asset(
            &self.store,
            &self.context,
            &self.owner,
            state_id,
            run_id,
            application_record_id,
            execution_receipt_id,
        )
        .await
    }
}

async fn verify_watermark(
    session: &mut Session,
    context: &Context,
    state: &LearnerStateV2,
) -> Result<()> {
    let current = session
        .watermark(context)
        .await?
        .and_then(|value| u64::try_from(value.0).ok())
        .ok_or_else(|| Error::Conflict("missing curriculum source watermark".into()))?;
    if current != state.source_watermark {
        return Err(Error::Conflict(
            "learner state source watermark changed".into(),
        ));
    }
    Ok(())
}

async fn need_typed_source(
    session: &mut Session,
    context: &Context,
    id: &str,
) -> Result<CurriculumSourceArtifactV1> {
    let source: CurriculumSourceArtifactV1 = session.need(context, "artifact", id).await?;
    source.validate()?;
    Ok(source)
}

async fn verify_state_sources(
    session: &mut Session,
    context: &Context,
    state: &LearnerStateV2,
) -> Result<Vec<CurriculumSourceArtifactV1>> {
    let mut sources = Vec::new();
    let mut pending = state.source_artifact_ids.clone();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(source_id) = pending.pop() {
        if !seen.insert(source_id.clone()) {
            continue;
        }
        if seen.len() > 10_000 {
            return Err(Error::Invalid(
                "curriculum source closure exceeds supported bound".into(),
            ));
        }
        let source = need_typed_source(session, context, &source_id).await?;
        pending.extend(source.dependency_ids.iter().cloned());
        sources.push(source);
    }
    Ok(sources)
}

fn require_profile_sources(
    profile: &CurriculumControlProfileV1,
    sources: &[CurriculumSourceArtifactV1],
) -> Result<()> {
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    for (kind, source_id, digest) in [
        (
            CurriculumSourceKindV1::TargetSpec,
            registered.target_source_id.as_str(),
            registered.target_digest.as_str(),
        ),
        (
            CurriculumSourceKindV1::OracleSpec,
            registered.oracle_source_id.as_str(),
            profile.oracle_digest.as_str(),
        ),
        (
            CurriculumSourceKindV1::RunnerSpec,
            registered.runner_source_id.as_str(),
            profile.runner_digest.as_str(),
        ),
    ] {
        if !sources.iter().any(|source| {
            source.source_kind == kind && source.id == source_id && source.body_digest == digest
        }) {
            return Err(Error::Conflict(
                "learner state lacks a required typed profile source".into(),
            ));
        }
    }
    if !sources.iter().any(|source| {
        source.source_kind == CurriculumSourceKindV1::TaskSpace
            && source.subject_digest == profile.task_space_digest
    }) {
        return Err(Error::Conflict(
            "learner state lacks the protected task-space source".into(),
        ));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid("expected lowercase sha256 digest".into()));
    }
    Ok(())
}

fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e12-{}", fingerprint(&(record_kind, id))?))
}

async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    context: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = session
        .get::<ArtifactEnvelope<T>>(context, "artifact", &storage_id)
        .await?;
    match envelope {
        Some(envelope)
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == record_kind =>
        {
            Ok(Some(envelope.payload))
        }
        Some(_) => Err(Error::Conflict(
            "curriculum artifact envelope mismatch".into(),
        )),
        None => Ok(None),
    }
}

async fn need_record<T: DeserializeOwned>(
    session: &mut Session,
    context: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    get_record(session, context, record_kind, id)
        .await?
        .ok_or(Error::NotFound)
}

async fn put_record<T: Serialize>(
    session: &mut Session,
    context: &Context,
    record_kind: &str,
    id: &str,
    owner: &str,
    payload: &T,
) -> Result<()> {
    let storage_id = storage_id(record_kind, id)?;
    session
        .put(
            context,
            "artifact",
            &storage_id,
            owner,
            &ArtifactEnvelope {
                schema_version: ENVELOPE_SCHEMA.into(),
                id: storage_id.clone(),
                record_kind: record_kind.into(),
                payload,
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curriculum_spend_comes_from_root_budget() {
        let mut b = RootBudget::open("b1", "scope", 5, "lease", 0).unwrap();
        let state = LearnerState {
            checkpoint: "c1".into(),
            failure_clusters: vec!["fam".into()],
        };
        let p = TaskProposal {
            id: "t1".into(),
            parent_family: "fam".into(),
            data_use: DataUse::Development,
            difficulty: 0.5,
            learning_value: 0.5,
            correct: false,
            oracle_ok: true,
        };
        assert_eq!(step(&mut b, &state, &[p], 3).unwrap(), "t1");
        assert!(step(&mut b, &state, &[], 1).is_err());
    }
}
