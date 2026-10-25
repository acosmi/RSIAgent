//! Learner-conditioned curriculum control. V2 remains offline and zero-budget.
use crate::curriculum_profiles::{
    OfflineProfileCheck, RegisteredPureFunctionProfileV1, ValidityReportV1,
    registered_offline_check,
};
use crate::development::{
    CONTROL_KIND as DEVELOPMENT_CONTROL_KIND, DevelopmentControlV1, DevelopmentEvidenceScope,
    DevelopmentExecutionReceiptV1, EXECUTION_RECEIPT_KIND as DEVELOPMENT_EXECUTION_RECEIPT_KIND,
    registered_runner_digest,
};
use crate::evidence::StoredTraceAuthority;
use crate::executor::RootBudget;
use crate::optimization::verified_development_observation_in_session;
use crate::release_store::{
    HostApplicationRecord, PERSISTENT_RELEASE_SCHEMA, PersistentRelease, PersistentReleaseState,
    RELEASE_CANDIDATE_SCHEMA, ReleaseCandidateRecord, ReleaseStore, TrustedHostExecutionReceipt,
    TypedSourceRef,
};
use evo_core::curriculum::{
    AppliedLearnerAssetRef, CurriculumControlProfileV1, CurriculumRuntimeStatus,
    DevelopmentCycleObservation, FailureCycleOutcome, FailureRecordStatus, FailureRecordType,
    LEARNER_STATE_V2_SCHEMA, LEARNER_STATE_V3_SCHEMA, LearnerState, LearnerStateV2, LearnerStateV3,
    PlateauSignalV1, ProbeJobV1, ProbeTerminal, ProposalAttemptOutcome, StructuredTestProposalV1,
    TaskProposal, apply_cycle_to_failure_records, detect_plateau_signal_v3, next_task,
};
use evo_core::evaluation::DataUse;
use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;

/// Legacy v1 behavior is retained for compatibility and is not v2 evidence.
///
/// Selection is a pure computation and runs first. A selection that cannot be made
/// returns its own typed error and reserves nothing, so a failed step leaves the
/// budget as it was. Only a feasible selection reserves `cost`; if that reservation
/// fails the budget's error is returned and the selected task is not handed out.
/// Nothing can fail after the reservation, so there is no reservation to release.
pub fn step(
    budget: &mut RootBudget,
    state: &LearnerState,
    pool: &[TaskProposal],
    cost: i64,
) -> Result<String> {
    let task = next_task(state, pool)?;
    budget.reserve(cost)?;
    Ok(task)
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
) -> Result<LearnerStateV3> {
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
    let mut state: LearnerStateV3 = load_state_v3(&mut session, context, state_id).await?;
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
    let asset = AppliedLearnerAssetRef {
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
    // Plan §8.1.1/§8.4.1: a trusted application of an approved asset changes
    // the learner Skill snapshot, moves the active failure records of the
    // development families the release is bound to `retest_due` (naming the
    // release), and grants every bound family one one-shot retest mark. The
    // bound families come only from the release candidate's development
    // evidence sources: its trusted runs, read back from the store
    // (`release_candidate_development_families`). Nothing is written when they
    // cannot be resolved. Applying an asset never clears a record.
    let release: PersistentRelease = session.need(context, "release", &release_id).await?;
    if release.schema_version != PERSISTENT_RELEASE_SCHEMA || release.id != release_id {
        return Err(Error::Conflict(
            "applied release differs from its record".into(),
        ));
    }
    let candidate: ReleaseCandidateRecord = session
        .need(context, "artifact", &release.candidate_id)
        .await?;
    if candidate.schema_version != RELEASE_CANDIDATE_SCHEMA
        || candidate.id != release.candidate_id
        || candidate.bundle_digest != release.bundle_digest
    {
        return Err(Error::Conflict(
            "applied release and its candidate record differ".into(),
        ));
    }
    let bound_families =
        release_candidate_development_families(context, &mut session, &candidate.sources).await?;
    state.apply_learner_asset(asset, &bound_families)?;
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
    // The bound families were derived from these runs, so the state depends
    // on them directly (plan §11.4).
    for source in &candidate.sources {
        session
            .put_edge(
                context,
                "artifact",
                &state_storage_id,
                &source.kind,
                &source.id,
            )
            .await?;
    }
    session.commit().await?;
    Ok(state)
}

/// Development families an applied release is bound to (plan §8.4.1:
/// 资产绑定的开发家族只取自该Release候选的development证据来源).
///
/// Every source of the release candidate must be a trusted `run` whose stored
/// authority is development evidence; the bound families are the union of
/// their `parent_family`, sorted and deduplicated. A source of another kind, a
/// run without a stored record, a record of an unknown or invalid shape, a run
/// that is not development evidence, a run whose content changed since staging,
/// or an empty source list cannot bind a family: the call fails closed with a
/// `Conflict` naming the release and the family binding, and the caller writes
/// nothing. A revoked (tombstoned) run is `Forbidden`, as in every other source
/// check. Read-only and Admin-only; `record_applied_learning_asset` uses it
/// inside its own transaction.
pub async fn release_candidate_development_families(
    context: &Context,
    session: &mut Session,
    sources: &[TypedSourceRef],
) -> Result<Vec<String>> {
    context.require(&[Role::Admin])?;
    if sources.is_empty() {
        return Err(Error::Conflict(
            "release candidate has no development evidence source to bind a family".into(),
        ));
    }
    let mut families = BTreeSet::new();
    for source in sources {
        if source.kind != "run" {
            return Err(Error::Conflict(format!(
                "release source {}:{} is not a trusted run and cannot bind a development family",
                source.kind, source.id
            )));
        }
        identifier(&source.id)?;
        let unbindable = |why: &str| {
            Error::Conflict(format!(
                "release source run {} {why} and cannot bind a development family",
                source.id
            ))
        };
        if session
            .get::<serde_json::Value>(context, "tombstone", &source.id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let Some(body) = session
            .get::<serde_json::Value>(context, "run", &source.id)
            .await?
        else {
            return Err(unbindable("has no stored record"));
        };
        if body
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            != Some("rsia.optimization.source.v1")
        {
            return Err(unbindable("is not a typed trusted-run authority"));
        }
        let authority: StoredTraceAuthority = serde_json::from_value(body)
            .map_err(|_| unbindable("does not decode as a trusted-run authority"))?;
        if authority.record.purpose != Purpose::Development
            || authority.trace.purpose != Purpose::Development
        {
            return Err(unbindable("is not development evidence"));
        }
        authority
            .validate()
            .map_err(|_| unbindable("is not a valid trusted run"))?;
        if authority.record.id != source.id
            || authority.trace.source_digest != source.content_digest
        {
            return Err(unbindable("changed since the candidate was staged"));
        }
        families.insert(authority.record.parent_family);
    }
    Ok(families.into_iter().collect())
}

const PROFILE_KIND: &str = "curriculum_profile_v1";
/// Record kind of the persisted learner state. It keeps its v2 name so every
/// stored learner state stays at one storage address (and inside the storage
/// revocation bookkeeping, which keys on the record kind); the payload wire is
/// `rsia.learner_state.v3` on every new write and the read side explains both.
const STATE_KIND: &str = "learner_state_v2";
const CYCLE_KIND: &str = "development_cycle_receipt_v1";
const JOB_KIND: &str = "coverage_probe_job_v1";
const ATTEMPT_KIND: &str = "curriculum_proposal_attempt_v1";
const PROPOSAL_KIND: &str = "structured_test_proposal_v1";
const VALIDITY_KIND: &str = "curriculum_validity_report_v1";
/// Keeps the v1 record kind for the same reason as `STATE_KIND`: the storage
/// address and the revocation bookkeeping stay put while the payload wire
/// moves to `rsia.curriculum_selection.v2`.
const SELECTION_KIND: &str = "curriculum_selection_v1";
const RECEIPT_KIND: &str = "probe_schedule_receipt_v1";
const ENVELOPE_SCHEMA: &str = "rsia.curriculum_artifact_envelope.v1";
const SELECTION_V2_SCHEMA: &str = "rsia.curriculum_selection.v2";
/// Schema of the tombstone the revocation cleanup leaves in place of a record
/// whose source was revoked (plan §11.5, E08).
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// Storage id of a registered curriculum control profile artifact.
pub fn curriculum_profile_storage_id(profile_id: &str) -> Result<String> {
    storage_id(PROFILE_KIND, profile_id)
}

/// Storage id of a persisted learner state artifact.
pub fn curriculum_state_storage_id(state_id: &str) -> Result<String> {
    storage_id(STATE_KIND, state_id)
}

/// Storage id of a persisted coverage probe job artifact.
pub fn curriculum_probe_job_storage_id(probe_job_id: &str) -> Result<String> {
    storage_id(JOB_KIND, probe_job_id)
}

/// Receipt id derived from a management idempotency key.
pub fn probe_schedule_receipt_id(idempotency_key: &str) -> Result<String> {
    identifier(idempotency_key)?;
    Ok(format!(
        "probe-schedule-{}",
        &fingerprint(&(RECEIPT_KIND, idempotency_key))?[..24]
    ))
}

/// Persisted stage fact binding one management idempotency key to the probe
/// job it scheduled. A crash re-run reloads this instead of re-triggering the
/// curriculum consumer, so the learner state is never mutated twice for one
/// management request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeScheduleReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub idempotency_key: String,
    pub profile_id: String,
    pub state_id: String,
    pub root_budget_limit_micros: u64,
    pub probe_job_id: String,
    pub trigger_digest: String,
    pub revoke_watermark: u64,
}

impl ProbeScheduleReceiptV1 {
    pub const SCHEMA: &'static str = "rsia.probe_schedule_receipt.v1";
}

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

/// Priority category of one selection (plan §8.4.1): a family holding an
/// unconsumed one-shot retest mark of a live applied release outranks every
/// active failure gap (an `active` absolute-failure record of a registered
/// task bucket), which outranks a coverage gap (the candidate's registered
/// bucket is still short of its required clusters), which outranks the
/// remaining candidates. Same category ties break by the stable choice
/// sequence, then by candidate id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionPriorityCategory {
    RetestDue,
    ActiveFailureGap,
    CoverageGap,
    Other,
}

/// Influence type of a consumed asset (plan §8.4.1: 首版仅`retest_after_apply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetInfluenceKindV1 {
    RetestAfterApply,
}

/// The consumed asset influence of one selection (plan §8.4.1): the applied
/// release whose one-shot retest mark this selection consumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumedAssetInfluenceV1 {
    pub release_id: String,
    pub influence: AssetInfluenceKindV1,
}

/// v2 selection record (plan §8.4.1, V109): the digest of the state the
/// choice was computed from, an input-order independent digest of every
/// offered candidate with its eligibility, the priority category, the failure
/// records and coverage buckets the choice consumed, the applied asset it
/// answers to, and the two counterfactual arms over the same eligible set —
/// the choice the frozen (as-registered) learner state would have made, and
/// the choice of the pre-registered fixed random curriculum (`None` when the
/// profile registered no seed commitment). The selection basis and the
/// budget status are derived from the live state and profile, never
/// constants. The record holds candidate, record, bucket and release ids and
/// digests only. Selection consumes development facts and the applied asset's
/// identity and release state; it never uses formal evaluation detail, a
/// holdout manifest, the exposure ledger or a broker seed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionDecisionV2 {
    pub schema_version: String,
    pub id: String,
    pub state_id: String,
    pub state_digest: String,
    pub candidate_set_digest: String,
    pub chosen_task_id: String,
    pub proposal_id: String,
    pub stable_choice_seq: u32,
    pub priority_category: SelectionPriorityCategory,
    pub consumed_failure_record_ids: Vec<String>,
    pub consumed_coverage_bucket_ids: Vec<String>,
    pub consumed_asset: Option<ConsumedAssetInfluenceV1>,
    pub frozen_state_choice_task_id: String,
    pub fixed_random_choice_task_id: Option<String>,
    pub fixed_random_seed_commitment: Option<String>,
    pub selection_basis: String,
    pub budget_status: String,
}

/// The fixed random curriculum arm (plan §8.4.1): a reproducible pick from
/// the eligible set, the candidate whose `sha256("{commitment}:{candidate_id}")`
/// is smallest. It depends only on the registered commitment and the set of
/// eligible ids, never on the input order or the learner state.
fn fixed_random_arm_choice(
    eligible: &[&CurriculumTaskCandidateV1],
    seed_commitment: &str,
) -> Option<String> {
    eligible
        .iter()
        .map(|candidate| candidate.id.as_str())
        .min_by_key(|id| hash(format!("{seed_commitment}:{id}").as_bytes()))
        .map(str::to_string)
}

/// One ranking entry of a priority arm.
struct PriorityRanking<'a> {
    candidate: &'a CurriculumTaskCandidateV1,
    category: SelectionPriorityCategory,
    failure_record_ids: Vec<String>,
    coverage_bucket_id: Option<String>,
    release_id: Option<String>,
}

/// Ranks the eligible candidates by the §8.4.1 priority order against one
/// learner state: retest_due (an unconsumed mark of the family whose release
/// is in `live_releases`) > active failure gap (an `active` absolute-failure
/// record of the family, and the family is a registered task bucket) >
/// coverage gap (the candidate's registered bucket is still short of its
/// required independent clusters) > other. Ties break by the stable choice
/// sequence, then by candidate id, so the order is fully deterministic. The
/// adaptive arm ranks the live state; the frozen arm ranks
/// [`LearnerStateV3::frozen_selection_view`] with no live release.
fn rank_by_priority<'a>(
    eligible: &[&'a CurriculumTaskCandidateV1],
    state: &LearnerStateV3,
    live_releases: &BTreeSet<String>,
) -> Vec<PriorityRanking<'a>> {
    let registered: BTreeSet<&str> = state
        .coverage_buckets
        .iter()
        .map(|bucket| bucket.bucket_id.as_str())
        .collect();
    let uncovered: BTreeSet<&str> = state
        .coverage_buckets
        .iter()
        .filter(|bucket| {
            bucket.observed_independent_clusters < bucket.required_independent_clusters
        })
        .map(|bucket| bucket.bucket_id.as_str())
        .collect();
    let mut ranked: Vec<PriorityRanking> = eligible
        .iter()
        .map(|candidate| {
            let candidate = *candidate;
            let family = candidate.parent_family.as_str();
            let mark = state.retest_priority_marks.iter().find(|mark| {
                !mark.consumed && mark.family == family && live_releases.contains(&mark.release_id)
            });
            let open_record_ids: Vec<String> = state
                .failure_records
                .iter()
                .filter(|record| {
                    record.cluster_id == family
                        && (record.status == FailureRecordStatus::RetestDue
                            || (record.status == FailureRecordStatus::Active
                                && record.record_type == Some(FailureRecordType::AbsoluteFailure)))
                })
                .map(|record| record.record_id.clone())
                .collect();
            let active_gap = registered.contains(family)
                && state.failure_records.iter().any(|record| {
                    record.cluster_id == family
                        && record.status == FailureRecordStatus::Active
                        && record.record_type == Some(FailureRecordType::AbsoluteFailure)
                });
            let coverage_bucket_id = candidate
                .coverage_bucket_id
                .as_deref()
                .filter(|bucket| uncovered.contains(bucket))
                .map(str::to_string);
            let (category, release_id) = if let Some(mark) = mark {
                (
                    SelectionPriorityCategory::RetestDue,
                    Some(mark.release_id.clone()),
                )
            } else if active_gap {
                (SelectionPriorityCategory::ActiveFailureGap, None)
            } else if coverage_bucket_id.is_some() {
                (SelectionPriorityCategory::CoverageGap, None)
            } else {
                (SelectionPriorityCategory::Other, None)
            };
            let failure_record_ids = match category {
                SelectionPriorityCategory::RetestDue
                | SelectionPriorityCategory::ActiveFailureGap => open_record_ids,
                SelectionPriorityCategory::CoverageGap | SelectionPriorityCategory::Other => {
                    Vec::new()
                }
            };
            PriorityRanking {
                candidate,
                category,
                failure_record_ids,
                coverage_bucket_id,
                release_id,
            }
        })
        .collect();
    ranked.sort_by(|left, right| {
        category_rank(left.category)
            .cmp(&category_rank(right.category))
            .then_with(|| {
                left.candidate
                    .stable_choice_seq
                    .cmp(&right.candidate.stable_choice_seq)
            })
            .then_with(|| left.candidate.id.cmp(&right.candidate.id))
    });
    ranked
}

fn category_rank(category: SelectionPriorityCategory) -> u8 {
    match category {
        SelectionPriorityCategory::RetestDue => 0,
        SelectionPriorityCategory::ActiveFailureGap => 1,
        SelectionPriorityCategory::CoverageGap => 2,
        SelectionPriorityCategory::Other => 3,
    }
}

/// Wire (snake_case) name of a priority category, as the selection basis
/// spells it.
fn category_name(category: SelectionPriorityCategory) -> &'static str {
    match category {
        SelectionPriorityCategory::RetestDue => "retest_due",
        SelectionPriorityCategory::ActiveFailureGap => "active_failure_gap",
        SelectionPriorityCategory::CoverageGap => "coverage_gap",
        SelectionPriorityCategory::Other => "other",
    }
}

/// Budget status of a selection, derived from the registered profile's
/// runtime status and monetary limit (it is never a fixed literal).
fn budget_status(profile: &CurriculumControlProfileV1) -> String {
    let runtime = match profile.runtime_status {
        CurriculumRuntimeStatus::Disabled => "disabled",
    };
    format!(
        "runtime_status={runtime};monetary_limit_micros={}",
        profile.monetary_limit_micros
    )
}

/// Digest of every offered candidate with its development eligibility,
/// sorted by candidate id, so the same offered set has one digest whatever
/// its input order, and two different offered sets never share one.
fn candidate_set_digest(offered: &[(&CurriculumTaskCandidateV1, bool)]) -> Result<String> {
    #[derive(Serialize)]
    struct OfferedCandidate<'a> {
        candidate: &'a CurriculumTaskCandidateV1,
        development_eligible: bool,
    }
    let mut entries: Vec<OfferedCandidate> = offered
        .iter()
        .map(|(candidate, eligible)| OfferedCandidate {
            candidate,
            development_eligible: *eligible,
        })
        .collect();
    entries.sort_by(|left, right| left.candidate.id.cmp(&right.candidate.id));
    fingerprint(&entries)
}

/// Storage id of a validity report: bound to the learner state it was
/// recorded for, the proposal and the report content.
fn validity_report_id(
    state_id: &str,
    proposal_id: &str,
    report: &ValidityReportV1,
) -> Result<String> {
    Ok(format!(
        "validity-{}",
        &fingerprint(&(state_id, proposal_id, report))?[..24]
    ))
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
        // Every registration persists the v3 payload wire (failure records
        // start empty: a prefilled v2 cluster list would project to legacy
        // diagnostic records and is refused below) at the fixed v2 storage
        // address.
        let state = LearnerStateV3::from_v2(state);
        state.validate()?;
        if !state.completed_cycles.is_empty()
            || !state.failure_clusters.is_empty()
            || !state.failure_records.is_empty()
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
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        if get_record::<CurriculumControlProfileV1>(
            &mut session,
            &self.context,
            PROFILE_KIND,
            &profile.profile_id,
        )
        .await?
        .is_some()
            || session
                .get::<serde_json::Value>(&self.context, "artifact", &state_storage_id)
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

    pub async fn record_cycle(&self, receipt: DevelopmentCycleReceiptV1) -> Result<LearnerStateV3> {
        if receipt.schema_version != "rsia.development_cycle_receipt.v1" {
            return Err(Error::Invalid("unsupported cycle receipt schema".into()));
        }
        identifier(&receipt.id)?;
        identifier(&receipt.state_id)?;
        identifier(&receipt.development_request_fact_id)?;
        identifier(&receipt.development_observed_fact_id)?;
        let mut session = self.store.session().await?;
        let mut state: LearnerStateV3 =
            load_state_v3(&mut session, &self.context, &receipt.state_id).await?;
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
        // Plan §8.1.1 (v4.5 Q-10): the cycle first reduces into typed failure
        // records, each bound to its verified task, that task's candidate
        // execution receipt id and its grader receipt digest — an absolute
        // failure (`!passed`) opens a new failure-gap record, a regression
        // (`passed` below its parent) opens a record that never counts as a
        // gap, a verified pass resolves the cluster's open records and stores
        // the pass next to the untouched failure evidence, and drift moves
        // stale records to diagnostic. The v3 cluster list then mirrors
        // exactly the active absolute-failure records.
        let failure_outcomes: Vec<FailureCycleOutcome> = verified
            .outcomes
            .iter()
            .map(|outcome| FailureCycleOutcome {
                cluster_id: outcome.parent_family.clone(),
                task_id: outcome.task_id.clone(),
                passed: outcome.candidate_passed,
                paired_gain_micros: i64::from(outcome.candidate_score_micros)
                    - i64::from(outcome.parent_score_micros),
                candidate_execution_id: outcome.candidate_execution_id.clone(),
                grader_receipt_digest: outcome.grader_receipt_digest.clone(),
            })
            .collect();
        apply_cycle_to_failure_records(
            &mut state.failure_records,
            &observation.cycle_id,
            &state.environment_digest,
            &state.grader_digest,
            &state.skill_snapshot_digest,
            &failure_outcomes,
        )?;
        for outcome in &verified.outcomes {
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
        state.failure_clusters = state.active_absolute_failure_clusters();
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
        let job = self
            .schedule_probe_in_session(&mut session, profile_id, state_id, root_budget_limit_micros)
            .await?;
        session.commit().await?;
        Ok(job)
    }

    /// Schedules at most one probe for `idempotency_key`. The first run
    /// persists the probe job, the updated learner state, their edges and a
    /// `ProbeScheduleReceiptV1` in one transaction; every later run with the
    /// same key and inputs reloads the recorded probe job without mutating the
    /// learner state again (plan §6.7.4: recovery continues from persisted
    /// stage facts, never from "a file exists").
    ///
    /// The replay is not exempt from the revocation gate (plan §11.4): before it
    /// hands the recorded job out it re-reads the learner state and re-verifies
    /// its source watermark and source closure, the same rule a new execution
    /// applies, so a revoked source is a `Conflict` (or `Forbidden`) for a
    /// replayed key too, and a redacted learner state is a `Conflict` that names
    /// the redaction.
    pub async fn schedule_probe_idempotent(
        &self,
        idempotency_key: &str,
        profile_id: &str,
        state_id: &str,
        root_budget_limit_micros: u64,
    ) -> Result<ProbeJobV1> {
        identifier(profile_id)?;
        identifier(state_id)?;
        let receipt_id = probe_schedule_receipt_id(idempotency_key)?;
        let mut session = self.store.session().await?;
        if let Some(receipt) = get_record::<ProbeScheduleReceiptV1>(
            &mut session,
            &self.context,
            RECEIPT_KIND,
            &receipt_id,
        )
        .await?
        {
            if receipt.schema_version != ProbeScheduleReceiptV1::SCHEMA
                || receipt.id != receipt_id
                || receipt.idempotency_key != idempotency_key
                || receipt.profile_id != profile_id
                || receipt.state_id != state_id
                || receipt.root_budget_limit_micros != root_budget_limit_micros
            {
                return Err(Error::Conflict(
                    "probe schedule receipt differs from the request".into(),
                ));
            }
            let job: ProbeJobV1 =
                get_record(&mut session, &self.context, JOB_KIND, &receipt.probe_job_id)
                    .await?
                    .ok_or_else(|| {
                        Error::Conflict(
                            "probe schedule receipt points at a missing probe job".into(),
                        )
                    })?;
            if job.id != receipt.probe_job_id
                || job.profile_id != receipt.profile_id
                || job.state_id != receipt.state_id
                || job.trigger_digest != receipt.trigger_digest
                || job.root_budget_limit_micros != receipt.root_budget_limit_micros
            {
                return Err(Error::Conflict(
                    "recorded probe job differs from its schedule receipt".into(),
                ));
            }
            // Fail closed like every other entry point: the recorded job is only
            // returned while the learner state it was scheduled from is live.
            let state: LearnerStateV3 =
                load_state_v3(&mut session, &self.context, state_id).await?;
            verify_watermark(&mut session, &self.context, &state).await?;
            verify_state_sources(&mut session, &self.context, &state).await?;
            session.commit().await?;
            return Ok(job);
        }
        let job = self
            .schedule_probe_in_session(&mut session, profile_id, state_id, root_budget_limit_micros)
            .await?;
        let revoke_watermark = session
            .watermark(&self.context)
            .await?
            .and_then(|value| u64::try_from(value.0).ok())
            .ok_or_else(|| Error::Conflict("missing curriculum source watermark".into()))?;
        let receipt = ProbeScheduleReceiptV1 {
            schema_version: ProbeScheduleReceiptV1::SCHEMA.into(),
            id: receipt_id.clone(),
            idempotency_key: idempotency_key.into(),
            profile_id: profile_id.into(),
            state_id: state_id.into(),
            root_budget_limit_micros,
            probe_job_id: job.id.clone(),
            trigger_digest: job.trigger_digest.clone(),
            revoke_watermark,
        };
        put_record(
            &mut session,
            &self.context,
            RECEIPT_KIND,
            &receipt_id,
            &self.owner,
            &receipt,
        )
        .await?;
        let receipt_storage_id = storage_id(RECEIPT_KIND, &receipt_id)?;
        for dependency in [
            storage_id(JOB_KIND, &job.id)?,
            storage_id(STATE_KIND, state_id)?,
            storage_id(PROFILE_KIND, profile_id)?,
        ] {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &receipt_storage_id,
                    "artifact",
                    &dependency,
                )
                .await?;
        }
        session.commit().await?;
        Ok(job)
    }

    async fn schedule_probe_in_session(
        &self,
        session: &mut Session,
        profile_id: &str,
        state_id: &str,
        root_budget_limit_micros: u64,
    ) -> Result<ProbeJobV1> {
        let profile: CurriculumControlProfileV1 =
            need_record(session, &self.context, PROFILE_KIND, profile_id).await?;
        let mut state: LearnerStateV3 = load_state_v3(session, &self.context, state_id).await?;
        verify_watermark(session, &self.context, &state).await?;
        verify_state_sources(session, &self.context, &state).await?;
        // Plan §8.1.1: the v3 trigger keeps only the active failure clusters
        // that carry an executable improvement direction (a registered task
        // bucket). When none does, it answers `NotTriggered` with the
        // `failure_cluster_outside_registered_task_space` reason, which ends
        // below in the queryable `unsupported_scope` terminal job rather than a
        // FailureGap probe or an error.
        let trigger = detect_plateau_signal_v3(&profile, &state)?;
        let trigger_digest = fingerprint(&trigger)?;
        let job_id = format!(
            "probe-{}",
            &fingerprint(&(profile_id, &trigger_digest))?[..24]
        );
        if let Some(existing) =
            get_record::<ProbeJobV1>(session, &self.context, JOB_KIND, &job_id).await?
        {
            if existing.root_budget_limit_micros != root_budget_limit_micros {
                return Err(Error::Conflict(
                    "probe trigger cannot change its frozen root budget".into(),
                ));
            }
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
        put_record(session, &self.context, JOB_KIND, &job.id, &self.owner, &job).await?;
        put_record(
            session,
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
        let mut state: LearnerStateV3 =
            load_state_v3(&mut session, &self.context, &job.state_id).await?;
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
        let state: LearnerStateV3 = load_state_v3(&mut session, &self.context, state_id).await?;
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

    /// Persists one validity report of a stored proposal of this state's
    /// probe job. Quarantined and sandbox-unavailable reports keep their
    /// previous checks. A runner-verified report is re-verified in this
    /// transaction before anything is written (see
    /// [`ValidityReportV1::RunnerVerified`] and `verify_runner_verified_binding`):
    /// quarantine wins, the registered offline check must pass and match
    /// `offline_output_digest`, and the bound verified development observation
    /// must cover the proposal family through a control that binds the
    /// registered target, oracle and runner. The report then depends on the
    /// proposal, its probe job and, for a runner-verified report, both stage
    /// facts it binds.
    pub async fn record_validity_report(
        &self,
        state_id: &str,
        report: ValidityReportV1,
    ) -> Result<String> {
        identifier(state_id)?;
        let proposal_id = report.proposal_id().to_string();
        identifier(&proposal_id)?;
        let report_id = validity_report_id(state_id, &proposal_id, &report)?;
        let mut session = self.store.session().await?;
        let state: LearnerStateV3 = load_state_v3(&mut session, &self.context, state_id).await?;
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
        let mut bound_facts: Vec<&str> = Vec::new();
        if let ValidityReportV1::RunnerVerified {
            development_request_fact_id,
            development_observed_fact_id,
            offline_output_digest,
            ..
        } = &report
        {
            verify_runner_verified_binding(
                &self.context,
                &mut session,
                &state,
                &proposal,
                &registered,
                development_request_fact_id,
                development_observed_fact_id,
                offline_output_digest.as_deref(),
            )
            .await?;
            bound_facts = vec![
                development_request_fact_id.as_str(),
                development_observed_fact_id.as_str(),
            ];
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
        for dependency in [proposal_storage_id.as_str(), job_storage_id.as_str()]
            .into_iter()
            .chain(bound_facts)
        {
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

    /// Adaptive selection over the runner-verified candidates of one state
    /// (plan §8.4.1, V109).
    ///
    /// Every offered candidate must name a validity report recorded for this
    /// learner state and a stored proposal of the same family that belongs to
    /// one of this state's probe jobs; anything else is a `Conflict`. A
    /// candidate is eligible when its report is runner-verified and its
    /// proposal has no quarantined report. Before ranking, every release a
    /// retest influence names (an unconsumed mark or a `retest_due` record) is
    /// read back: an unreadable release record is a `Conflict`; a release that
    /// is no longer active or superseded (rolled back, revoked) is void — its
    /// marks are ignored and its `retest_due` records return to `active`.
    ///
    /// The chosen arm ranks retest_due > active failure gap > coverage gap >
    /// other (ties by stable sequence). The record also persists the frozen
    /// arm (the same ranking over the as-registered state) and the fixed
    /// random arm (`None` without a registered seed commitment), the failure
    /// records and coverage bucket the choice consumed and the asset it
    /// answers to. A consumed retest mark is flipped to consumed and the
    /// family's `retest_due` records return to `active` in the same
    /// transaction, so the priority really is one-shot.
    pub async fn select_next_task(
        &self,
        state_id: &str,
        candidates: &[CurriculumTaskCandidateV1],
    ) -> Result<TaskSelectionDecisionV2> {
        identifier(state_id)?;
        let mut session = self.store.session().await?;
        let mut state: LearnerStateV3 =
            load_state_v3(&mut session, &self.context, state_id).await?;
        verify_watermark(&mut session, &self.context, &state).await?;
        verify_state_sources(&mut session, &self.context, &state).await?;
        let profile: CurriculumControlProfileV1 =
            need_record(&mut session, &self.context, PROFILE_KIND, &state.profile_id).await?;
        let mut offered: Vec<(&CurriculumTaskCandidateV1, bool)> =
            Vec::with_capacity(candidates.len());
        let mut candidate_ids = BTreeSet::new();
        for candidate in candidates {
            identifier(&candidate.id)?;
            identifier(&candidate.parent_family)?;
            identifier(&candidate.proposal_id)?;
            identifier(&candidate.validity_report_id)?;
            if let Some(bucket) = &candidate.coverage_bucket_id {
                identifier(bucket)?;
            }
            if !candidate_ids.insert(candidate.id.as_str()) {
                return Err(Error::Invalid("duplicate task candidate id".into()));
            }
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
            if candidate.validity_report_id
                != validity_report_id(&state.id, &candidate.proposal_id, &report)?
            {
                return Err(Error::Conflict(
                    "task candidate's validity report was not recorded for this learner state"
                        .into(),
                ));
            }
            let proposal: StructuredTestProposalV1 = need_record(
                &mut session,
                &self.context,
                PROPOSAL_KIND,
                &candidate.proposal_id,
            )
            .await?;
            if proposal.parent_family != candidate.parent_family {
                return Err(Error::Conflict(
                    "selection candidate and proposal family differ".into(),
                ));
            }
            let job: ProbeJobV1 = need_record(
                &mut session,
                &self.context,
                JOB_KIND,
                &proposal.probe_job_id,
            )
            .await?;
            if job.state_id != state.id
                || job.profile_id != state.profile_id
                || report.profile_id() != job.profile_id
            {
                return Err(Error::Conflict(
                    "task candidate is not bound to a probe job of this learner state".into(),
                ));
            }
            let eligible = report.is_development_eligible()
                && !proposal_has_quarantine(&mut session, &self.context, &proposal.id).await?;
            offered.push((candidate, eligible));
        }
        let eligible: Vec<&CurriculumTaskCandidateV1> = offered
            .iter()
            .filter(|(_, eligible)| *eligible)
            .map(|(candidate, _)| *candidate)
            .collect();
        if eligible.is_empty() {
            return Err(Error::Conflict(
                "no runner-verified development task; sandbox unavailable".into(),
            ));
        }
        // Plan §8.4.1/V109.c: an asset influences selection only while its
        // release is live. The void influence of a rolled-back or revoked
        // release is cleared before the state is ranked and digested.
        let live_releases = live_asset_releases(&mut session, &self.context, &state).await?;
        let void_releases: BTreeSet<String> = state
            .failure_records
            .iter()
            .filter_map(|record| record.retest_release_id.clone())
            .filter(|release| !live_releases.contains(release))
            .collect();
        for release in &void_releases {
            state.void_retest_due_of_release(release)?;
        }
        let state_digest = fingerprint(&state)?;
        let candidate_set_digest = candidate_set_digest(&offered)?;
        let ranked = rank_by_priority(&eligible, &state, &live_releases);
        let top = ranked.first().ok_or(Error::NotFound)?;
        let chosen = top.candidate;
        let frozen_state_choice_task_id =
            rank_by_priority(&eligible, &state.frozen_selection_view(), &BTreeSet::new())
                .first()
                .map(|ranking| ranking.candidate.id.clone())
                .ok_or(Error::NotFound)?;
        let fixed_random_choice_task_id = profile
            .fixed_random_seed_commitment
            .as_deref()
            .and_then(|commitment| fixed_random_arm_choice(&eligible, commitment));
        let consumed_asset = top
            .release_id
            .clone()
            .map(|release_id| ConsumedAssetInfluenceV1 {
                release_id,
                influence: AssetInfluenceKindV1::RetestAfterApply,
            });
        let live_marks = state
            .retest_priority_marks
            .iter()
            .filter(|mark| !mark.consumed && live_releases.contains(&mark.release_id))
            .count();
        let uncovered_buckets = state
            .coverage_buckets
            .iter()
            .filter(|bucket| {
                bucket.observed_independent_clusters < bucket.required_independent_clusters
            })
            .count();
        let decision = TaskSelectionDecisionV2 {
            schema_version: SELECTION_V2_SCHEMA.into(),
            id: format!(
                "selection-{}",
                &fingerprint(&(
                    state_id,
                    chosen.id.as_str(),
                    &state_digest,
                    &candidate_set_digest
                ))?[..24]
            ),
            state_id: state.id.clone(),
            state_digest,
            candidate_set_digest,
            chosen_task_id: chosen.id.clone(),
            proposal_id: chosen.proposal_id.clone(),
            stable_choice_seq: chosen.stable_choice_seq,
            priority_category: top.category,
            consumed_failure_record_ids: top.failure_record_ids.clone(),
            consumed_coverage_bucket_ids: top.coverage_bucket_id.clone().into_iter().collect(),
            frozen_state_choice_task_id,
            fixed_random_choice_task_id,
            fixed_random_seed_commitment: profile.fixed_random_seed_commitment.clone(),
            // Derived from the state that was ranked: its category, the asset
            // it answers to and the counts that drove it.
            selection_basis: format!(
                "priority={};asset_release={};active_failure_clusters={};live_retest_marks={};uncovered_buckets={};eligible={};offered={}",
                category_name(top.category),
                consumed_asset
                    .as_ref()
                    .map_or("none", |asset| asset.release_id.as_str()),
                state.active_absolute_failure_clusters().len(),
                live_marks,
                uncovered_buckets,
                eligible.len(),
                offered.len(),
            ),
            budget_status: budget_status(&profile),
            consumed_asset,
        };
        if let Some(asset) = &decision.consumed_asset {
            state.consume_retest_priority_mark(&chosen.parent_family, &asset.release_id)?;
        }
        state.validate()?;
        put_record(
            &mut session,
            &self.context,
            SELECTION_KIND,
            &decision.id,
            &self.owner,
            &decision,
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
        let selection_storage_id = storage_id(SELECTION_KIND, &decision.id)?;
        let state_storage_id = storage_id(STATE_KIND, &state.id)?;
        let proposal_storage_id = storage_id(PROPOSAL_KIND, &chosen.proposal_id)?;
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
    ) -> Result<LearnerStateV3> {
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

/// Read-side gate for a persisted probe job (plan §11.4). Reloads the probe
/// job and its learner state, then re-verifies the live source closure: any
/// watermark bump is `Conflict`, any tombstoned source is `Forbidden`, and a
/// missing or malformed source fails closed. The stored job is never rewritten.
pub async fn verified_probe_job_view(
    ctx: &Context,
    store: &Store,
    probe_job_id: &str,
) -> Result<ProbeJobV1> {
    ctx.require(&[Role::Admin, Role::Evaluator])?;
    identifier(probe_job_id)?;
    let mut session = store.session().await?;
    let job: ProbeJobV1 = need_record(&mut session, ctx, JOB_KIND, probe_job_id).await?;
    if job.id != probe_job_id || job.schema_version != "rsia.coverage_probe_job.v1" {
        return Err(Error::Conflict(
            "stored probe job differs from its storage identity".into(),
        ));
    }
    let state: LearnerStateV3 = load_state_v3(&mut session, ctx, &job.state_id).await?;
    if state.id != job.state_id || state.profile_id != job.profile_id {
        return Err(Error::Conflict(
            "probe job and learner state identities differ".into(),
        ));
    }
    verify_watermark(&mut session, ctx, &state).await?;
    verify_state_sources(&mut session, ctx, &state).await?;
    session.commit().await?;
    Ok(job)
}

/// Schema of the typed envelope the E03 development records are stored in.
const DEVELOPMENT_ENVELOPE_SCHEMA: &str = "rsia.typed_artifact_envelope.v1";

/// Re-verifies a runner-verified report inside the caller's transaction
/// (plan §8.2 L1320, §8.3 L1324–L1326, §8.4 L1334):
///
/// 1. quarantine wins: a recorded quarantined report of the proposal, or a
///    registered offline check (in-process target twice + independent oracle)
///    that quarantines it, refuses the report (`Conflict`, "quarantined");
/// 2. the development stage-fact pair reloads and verifies in the learner
///    state's environment and grader version, and one verified outcome
///    belongs to the proposal family — another family's observation is a
///    `Conflict` naming the runner-verified report and the family;
/// 3. that outcome's development control binds the registered target, oracle
///    and in-process runner, the task is of the proposal family and target;
/// 4. `offline_output_digest` is the fingerprint of the recomputed passed
///    offline conclusion; absent or different, the report is refused.
#[allow(clippy::too_many_arguments)]
async fn verify_runner_verified_binding(
    context: &Context,
    session: &mut Session,
    state: &LearnerStateV3,
    proposal: &StructuredTestProposalV1,
    registered: &RegisteredPureFunctionProfileV1,
    request_fact_id: &str,
    observed_fact_id: &str,
    offline_output_digest: Option<&str>,
) -> Result<()> {
    if proposal_has_quarantine(session, context, &proposal.id).await? {
        return Err(Error::Conflict(format!(
            "runner-verified report refused: proposal {} is quarantined by a recorded validity report",
            proposal.id
        )));
    }
    let offline = registered_offline_check(registered, proposal)?;
    if let OfflineProfileCheck::Quarantined { reason } = &offline {
        return Err(Error::Conflict(format!(
            "runner-verified report refused: the registered offline check quarantined proposal {} ({reason})",
            proposal.id
        )));
    }
    let verified = verified_development_observation_in_session(
        context,
        session,
        request_fact_id,
        observed_fact_id,
    )
    .await?;
    if verified.environment_digest != state.environment_digest
        || verified.grader_digest != state.grader_digest
    {
        return Err(Error::Conflict(
            "runner-verified report context differs from learner state".into(),
        ));
    }
    let outcome = verified
        .outcomes
        .iter()
        .find(|outcome| outcome.parent_family == proposal.parent_family)
        .ok_or_else(|| {
            Error::Conflict(format!(
                "runner-verified report binds a development observation that does not cover the proposal family {}",
                proposal.parent_family
            ))
        })?;
    let control =
        development_control_of_execution(session, context, &outcome.candidate_execution_id).await?;
    let task = control.task(&outcome.task_id)?;
    if control.evidence_scope != DevelopmentEvidenceScope::RegisteredPureFunctionExecution
        || control.target_digest != registered.target_digest
        || control.oracle_digest != registered.oracle_digest
        || control.runner_digest != registered_runner_digest()?
        || control.environment_digest != state.environment_digest
        || control.grader_digest != state.grader_digest
        || task.parent_family != proposal.parent_family
        || task.target_id != proposal.target_id
        || proposal.target_id != registered.target_id
    {
        return Err(Error::Conflict(
            "runner-verified report: the observation's development control does not bind the registered target, oracle and runner of the proposal family".into(),
        ));
    }
    let expected = fingerprint(&offline)?;
    match offline_output_digest {
        None => Err(Error::Conflict(
            "runner-verified report lacks the offline_output_digest of its passed offline conclusion"
                .into(),
        )),
        Some(digest) if digest != expected => Err(Error::Conflict(
            "runner-verified report offline_output_digest differs from the registered offline conclusion"
                .into(),
        )),
        Some(_) => Ok(()),
    }
}

/// Whether a quarantined validity report of `proposal_id` is stored: the
/// proposal's dependents are scanned for curriculum validity reports. A
/// redacted validity report of the proposal fails closed (its verdict is no
/// longer readable); a validity envelope that does not decode is corruption.
async fn proposal_has_quarantine(
    session: &mut Session,
    context: &Context,
    proposal_id: &str,
) -> Result<bool> {
    let proposal_storage_id = storage_id(PROPOSAL_KIND, proposal_id)?;
    for (kind, id) in session
        .dependents(context, "artifact", &proposal_storage_id)
        .await?
    {
        if kind != "artifact" {
            continue;
        }
        let Some(body) = session
            .get::<serde_json::Value>(context, "artifact", &id)
            .await?
        else {
            continue;
        };
        let schema = body
            .get("schema_version")
            .and_then(serde_json::Value::as_str);
        if schema == Some(REDACTED_SCHEMA) {
            if body
                .pointer("/metadata/record_kind")
                .and_then(serde_json::Value::as_str)
                == Some(VALIDITY_KIND)
            {
                return Err(Error::Conflict(format!(
                    "a validity report of proposal {proposal_id} was redacted; its quarantine verdict is unreadable"
                )));
            }
            continue;
        }
        if schema != Some(ENVELOPE_SCHEMA)
            || body.get("record_kind").and_then(serde_json::Value::as_str) != Some(VALIDITY_KIND)
        {
            continue;
        }
        let envelope: ArtifactEnvelope<ValidityReportV1> =
            serde_json::from_value(body).map_err(|error| {
                tracing::error!(%error, record_kind = VALIDITY_KIND, id = id.as_str(), "stored validity report does not decode");
                Error::Internal
            })?;
        if envelope.id != id {
            return Err(Error::Conflict(
                "curriculum artifact envelope mismatch".into(),
            ));
        }
        if matches!(
            &envelope.payload,
            ValidityReportV1::Quarantined { proposal_id: quarantined, .. } if quarantined == proposal_id
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads back every release a retest influence of `state` names (an
/// unconsumed mark or a `retest_due` record) and returns the live ones: a
/// release in state `active` or `superseded`. A release record that is
/// missing or does not decode as a persistent release is a `Conflict`; a
/// rolled-back, revoked or never-activated release is simply not live. Only
/// the release's identity and state are used, never its formal evaluation.
async fn live_asset_releases(
    session: &mut Session,
    context: &Context,
    state: &LearnerStateV3,
) -> Result<BTreeSet<String>> {
    let mut referenced = BTreeSet::new();
    for mark in &state.retest_priority_marks {
        if !mark.consumed {
            referenced.insert(mark.release_id.clone());
        }
    }
    for record in &state.failure_records {
        if let Some(release) = &record.retest_release_id {
            referenced.insert(release.clone());
        }
    }
    let mut live = BTreeSet::new();
    for release_id in referenced {
        let unreadable = || {
            Error::Conflict(format!(
                "retest influence names release {release_id} whose release record is unreadable"
            ))
        };
        let body = session
            .get::<serde_json::Value>(context, "release", &release_id)
            .await?
            .ok_or_else(unreadable)?;
        let release: PersistentRelease = serde_json::from_value(body).map_err(|_| unreadable())?;
        if release.schema_version != PERSISTENT_RELEASE_SCHEMA || release.id != release_id {
            return Err(unreadable());
        }
        if matches!(
            release.state,
            PersistentReleaseState::Active | PersistentReleaseState::Superseded
        ) {
            live.insert(release_id);
        }
    }
    Ok(live)
}

/// Reads one E03 development record (typed envelope) inside the caller's
/// transaction.
async fn development_record<T: DeserializeOwned>(
    session: &mut Session,
    context: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    let storage = crate::development::storage_id(record_kind, id)?;
    let body: serde_json::Value = session.need(context, "artifact", &storage).await?;
    let mismatch = || Error::Conflict("development record envelope mismatch or redacted".into());
    let envelope: ArtifactEnvelope<T> = serde_json::from_value(body).map_err(|_| mismatch())?;
    if envelope.schema_version != DEVELOPMENT_ENVELOPE_SCHEMA
        || envelope.id != storage
        || envelope.record_kind != record_kind
    {
        return Err(mismatch());
    }
    Ok(envelope.payload)
}

/// The registered development control behind one verified candidate
/// execution receipt.
async fn development_control_of_execution(
    session: &mut Session,
    context: &Context,
    execution_id: &str,
) -> Result<DevelopmentControlV1> {
    let receipt: DevelopmentExecutionReceiptV1 = development_record(
        session,
        context,
        DEVELOPMENT_EXECUTION_RECEIPT_KIND,
        execution_id,
    )
    .await?;
    if receipt.receipt_id != execution_id || receipt.namespace != context.namespace() {
        return Err(Error::Conflict(
            "development execution receipt identity mismatch".into(),
        ));
    }
    let control: DevelopmentControlV1 = development_record(
        session,
        context,
        DEVELOPMENT_CONTROL_KIND,
        &receipt.control_id,
    )
    .await?;
    control.validate()?;
    if control.id != receipt.control_id || control.namespace != context.namespace() {
        return Err(Error::Conflict(
            "development control identity mismatch".into(),
        ));
    }
    Ok(control)
}

async fn verify_watermark(
    session: &mut Session,
    context: &Context,
    state: &LearnerStateV3,
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
    // Same logical-revocation semantics as `evidence::validate_stored_sources`:
    // a tombstoned source member fails the whole closure closed.
    if session
        .get::<serde_json::Value>(context, "tombstone", id)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    let source: CurriculumSourceArtifactV1 = session.need(context, "artifact", id).await?;
    source.validate()?;
    Ok(source)
}

async fn verify_state_sources(
    session: &mut Session,
    context: &Context,
    state: &LearnerStateV3,
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

/// Loads the learner state at its fixed `STATE_KIND` storage address and
/// explains both payload wires: a `rsia.learner_state.v3` body decodes
/// directly, a `rsia.learner_state.v2` body is projected through
/// [`LearnerStateV3::from_v2`] (its bare cluster list survives as untyped
/// `legacy_v2_untyped` diagnostic records), and anything else fails closed. A
/// redacted body names the revocation the same way `get_record` does.
async fn load_state_v3(
    session: &mut Session,
    context: &Context,
    state_id: &str,
) -> Result<LearnerStateV3> {
    let id = storage_id(STATE_KIND, state_id)?;
    let Some(body) = session
        .get::<serde_json::Value>(context, "artifact", &id)
        .await?
    else {
        return Err(Error::NotFound);
    };
    if body
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(REDACTED_SCHEMA)
    {
        return Err(Error::Conflict(format!(
            "curriculum record {STATE_KIND} {state_id} was redacted because its source was revoked"
        )));
    }
    let envelope: ArtifactEnvelope<serde_json::Value> = serde_json::from_value(body).map_err(
        |error| {
            tracing::error!(%error, record_kind = STATE_KIND, state_id, "stored curriculum record does not decode");
            Error::Internal
        },
    )?;
    if envelope.schema_version != ENVELOPE_SCHEMA
        || envelope.id != id
        || envelope.record_kind != STATE_KIND
    {
        return Err(Error::Conflict(
            "curriculum artifact envelope mismatch".into(),
        ));
    }
    let payload = envelope.payload;
    match payload
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
    {
        Some(LEARNER_STATE_V3_SCHEMA) => {
            let state: LearnerStateV3 = serde_json::from_value(payload).map_err(|error| {
                tracing::error!(%error, record_kind = STATE_KIND, state_id, "stored v3 learner state does not decode");
                Error::Internal
            })?;
            state.validate()?;
            Ok(state)
        }
        Some(LEARNER_STATE_V2_SCHEMA) => {
            let legacy: LearnerStateV2 = serde_json::from_value(payload).map_err(|error| {
                tracing::error!(%error, record_kind = STATE_KIND, state_id, "stored v2 learner state does not decode");
                Error::Internal
            })?;
            legacy.validate()?;
            Ok(LearnerStateV3::from_v2(legacy))
        }
        // Any other payload body is corruption, exactly like a `get_record`
        // decode failure: an unknown future wire is not a readable state and
        // fails closed as Internal (the pinned test
        // `a_curriculum_body_that_is_not_a_tombstone_and_does_not_decode_stays_internal`).
        other => {
            tracing::error!(
                payload_schema = other.unwrap_or("<none>"),
                record_kind = STATE_KIND,
                state_id,
                "stored learner state payload has no readable schema"
            );
            Err(Error::Internal)
        }
    }
}

/// Reads the stored body of one curriculum record as an envelope.
///
/// After a source revocation the cleanup replaces the body of a learner state,
/// cycle receipt, proposal, validity report, attempt or selection with an
/// `rsia.redacted.v1` tombstone, which is not an envelope. That is the expected
/// state of a revoked curriculum, not corruption, so the read names it: a
/// `Conflict` (HTTP 409) carrying the record kind and id, the same verdict
/// `exploration.rs` (`read_envelope`) gives a redacted exploration record,
/// instead of a decode failure that would surface as `Internal`. The body is
/// inspected before it is decoded, so the expected state does not raise the
/// storage layer's "database operation failed" error log either. Any other body
/// that does not decode is corruption and stays `Internal`.
async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    context: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let Some(body) = session
        .get::<serde_json::Value>(context, "artifact", &storage_id)
        .await?
    else {
        return Ok(None);
    };
    if body
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(REDACTED_SCHEMA)
    {
        return Err(Error::Conflict(format!(
            "curriculum record {record_kind} {id} was redacted because its source was revoked"
        )));
    }
    let envelope: ArtifactEnvelope<T> = match serde_json::from_value(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            tracing::error!(%error, record_kind, id, "stored curriculum record does not decode");
            return Err(Error::Internal);
        }
    };
    if envelope.schema_version == ENVELOPE_SCHEMA
        && envelope.id == storage_id
        && envelope.record_kind == record_kind
    {
        Ok(Some(envelope.payload))
    } else {
        Err(Error::Conflict(
            "curriculum artifact envelope mismatch".into(),
        ))
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
