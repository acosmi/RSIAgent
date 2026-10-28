//! Learner-conditioned task proposals. Generated items stay in development.
use crate::evaluation::DataUse;
use crate::{Error, Result, hash, identifier, text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnerState {
    pub checkpoint: String,
    pub failure_clusters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskProposal {
    pub id: String,
    pub parent_family: String,
    pub data_use: DataUse,
    pub difficulty: f64,
    pub learning_value: f64,
    pub correct: bool,
    pub oracle_ok: bool,
}

impl TaskProposal {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        identifier(&self.parent_family)?;
        if !self.difficulty.is_finite() || !self.learning_value.is_finite() {
            return Err(Error::Invalid("non-finite curriculum scores".into()));
        }
        if self.data_use.is_protected_holdout() {
            return Err(Error::Conflict(
                "curriculum items cannot enter holdout".into(),
            ));
        }
        Ok(())
    }

    pub fn quarantine_reason(&self) -> Option<&'static str> {
        if !self.oracle_ok {
            return Some("no_oracle");
        }
        if self.correct && self.learning_value <= 0.0 {
            return Some("correct_but_no_learning_value");
        }
        None
    }
}

pub fn next_task(state: &LearnerState, pool: &[TaskProposal]) -> Result<String> {
    identifier(&state.checkpoint)?;
    if state.failure_clusters.is_empty() {
        return Err(Error::Invalid(
            "empty learner state cannot drive selection".into(),
        ));
    }
    pool.iter()
        .filter(|p| p.validate().is_ok() && p.quarantine_reason().is_none())
        .find(|p| state.failure_clusters.contains(&p.parent_family))
        .map(|p| p.id.clone())
        .ok_or(Error::NotFound)
}

/// Builds a development-only proposal from model text.
///
/// The text is checked for shape only (nonempty, bounded, no NUL). That check says
/// nothing about whether the task has a sound oracle, so the proposal is born
/// unverified (`oracle_ok: false`): `quarantine_reason()` reports `no_oracle` and
/// `next_task` never selects it. Only an independent ValidityReport together with
/// real check references can make a proposal verified; neither the model text nor
/// this constructor may self-report it.
pub fn proposal_from_text(id: &str, family: &str, body: &str) -> Result<TaskProposal> {
    text(body, "body", 4096)?;
    Ok(TaskProposal {
        id: id.into(),
        parent_family: family.into(),
        data_use: DataUse::Development,
        difficulty: 0.5,
        learning_value: 0.5,
        correct: false,
        oracle_ok: false,
    })
}

pub const CURRICULUM_PROFILE_SCHEMA: &str = "rsia.curriculum_control_profile.v1";
pub const LEARNER_STATE_V2_SCHEMA: &str = "rsia.learner_state.v2";
pub const LEARNER_STATE_V3_SCHEMA: &str = "rsia.learner_state.v3";
pub const PLATEAU_SIGNAL_SCHEMA: &str = "rsia.coverage_probe_trigger.v1";

fn digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!(
            "{name} must be a lowercase sha256 digest"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurriculumRuntimeStatus {
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumControlProfileV1 {
    pub schema_version: String,
    pub profile_id: String,
    pub task_space_digest: String,
    pub oracle_digest: String,
    pub runner_digest: String,
    pub max_total_proposals: u8,
    pub plateau_window_cycles: u8,
    pub min_independent_clusters_per_cycle: u8,
    pub min_unique_window_clusters: u8,
    pub min_success_rate_micros: u32,
    pub max_abs_mean_gain_micros: i32,
    pub max_gain_variance_micros_squared: u64,
    pub cooldown_completed_cycles: u8,
    pub curriculum_budget_share_bps: u16,
    pub max_concurrent_probe_jobs: u8,
    pub monetary_limit_micros: u64,
    pub runtime_status: CurriculumRuntimeStatus,
    /// Pre-registered seed commitment of the fixed random curriculum arm
    /// (plan §8.4.1, §3.2.2): a public lowercase sha256 digest fixed when the
    /// profile is registered. The arm's reproducible order is keyed by this
    /// registered value itself — it is the arm's whole seed material, public
    /// from registration on, never revealed later — and it is separate from
    /// every broker private seed. A profile without it has no fixed random
    /// arm: its selection records carry `fixed_random_choice_task_id: null`.
    /// The key is omitted from the wire when absent, so a profile without it
    /// serializes to the exact pre-field v1 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_random_seed_commitment: Option<String>,
}

impl CurriculumControlProfileV1 {
    pub fn offline_default(
        profile_id: impl Into<String>,
        task_space_digest: impl Into<String>,
        oracle_digest: impl Into<String>,
        runner_digest: impl Into<String>,
    ) -> Result<Self> {
        let profile = Self {
            schema_version: CURRICULUM_PROFILE_SCHEMA.into(),
            profile_id: profile_id.into(),
            task_space_digest: task_space_digest.into(),
            oracle_digest: oracle_digest.into(),
            runner_digest: runner_digest.into(),
            max_total_proposals: 4,
            plateau_window_cycles: 3,
            min_independent_clusters_per_cycle: 10,
            min_unique_window_clusters: 30,
            min_success_rate_micros: 950_000,
            max_abs_mean_gain_micros: 5_000,
            max_gain_variance_micros_squared: 1_000_000_000,
            cooldown_completed_cycles: 3,
            curriculum_budget_share_bps: 2_000,
            max_concurrent_probe_jobs: 1,
            monetary_limit_micros: 0,
            runtime_status: CurriculumRuntimeStatus::Disabled,
            fixed_random_seed_commitment: None,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CURRICULUM_PROFILE_SCHEMA
            || self.max_total_proposals != 4
            || self.plateau_window_cycles != 3
            || self.min_independent_clusters_per_cycle != 10
            || self.min_unique_window_clusters != 30
            || self.min_success_rate_micros != 950_000
            || self.max_abs_mean_gain_micros != 5_000
            || self.max_gain_variance_micros_squared != 1_000_000_000
            || self.cooldown_completed_cycles != 3
            || self.curriculum_budget_share_bps != 2_000
            || self.max_concurrent_probe_jobs != 1
            || self.monetary_limit_micros != 0
            || self.runtime_status != CurriculumRuntimeStatus::Disabled
        {
            return Err(Error::Invalid(
                "unsupported offline curriculum profile".into(),
            ));
        }
        identifier(&self.profile_id)?;
        digest(&self.task_space_digest, "task_space_digest")?;
        digest(&self.oracle_digest, "oracle_digest")?;
        digest(&self.runner_digest, "runner_digest")?;
        if let Some(commitment) = &self.fixed_random_seed_commitment {
            // The fixed-random arm's seed commitment is a digest, and it must
            // not be the empty string or a bare readable name (plan §5.1.2:
            // names never stand in for digests).
            digest(commitment, "fixed_random_seed_commitment")?;
        }
        Ok(())
    }

    pub fn curriculum_limit(&self, root_limit_micros: u64) -> Result<u64> {
        root_limit_micros
            .checked_mul(u64::from(self.curriculum_budget_share_bps))
            .map(|value| value / 10_000)
            .ok_or_else(|| Error::Invalid("curriculum budget arithmetic overflow".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentCycleObservation {
    pub cycle_id: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub cluster_ids: Vec<String>,
    pub successful_clusters: u32,
    pub paired_gain_micros: Vec<i32>,
    pub source_artifact_ids: Vec<String>,
}

impl DevelopmentCycleObservation {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.cycle_id)?;
        digest(&self.environment_digest, "cycle environment_digest")?;
        digest(&self.grader_digest, "cycle grader_digest")?;
        if self.cluster_ids.is_empty()
            || self.cluster_ids.len() > 10_000
            || self.cluster_ids.len() != self.paired_gain_micros.len()
            || self.successful_clusters as usize > self.cluster_ids.len()
        {
            return Err(Error::Invalid(
                "invalid development cycle observation".into(),
            ));
        }
        if self
            .paired_gain_micros
            .iter()
            .any(|gain| !(-1_000_000..=1_000_000).contains(gain))
        {
            return Err(Error::Invalid(
                "paired gain micros must be within [-1000000,1000000]".into(),
            ));
        }
        let mut clusters = std::collections::BTreeSet::new();
        for cluster in &self.cluster_ids {
            identifier(cluster)?;
            if !clusters.insert(cluster) {
                return Err(Error::Invalid(
                    "duplicate cluster within development cycle".into(),
                ));
            }
        }
        if self.source_artifact_ids.is_empty() {
            return Err(Error::Invalid(
                "development cycle lacks typed source artifacts".into(),
            ));
        }
        let mut sources = std::collections::BTreeSet::new();
        for source in &self.source_artifact_ids {
            identifier(source)?;
            if !sources.insert(source.as_str()) {
                return Err(Error::Invalid("duplicate cycle source artifact".into()));
            }
        }
        Ok(())
    }

    pub fn mean_gain_micros(&self) -> Result<i64> {
        self.validate()?;
        Ok(self
            .paired_gain_micros
            .iter()
            .map(|gain| i64::from(*gain))
            .sum::<i64>()
            / self.paired_gain_micros.len() as i64)
    }

    fn gain_sum_and_count(&self) -> Result<(i128, i128)> {
        self.validate()?;
        Ok((
            self.paired_gain_micros
                .iter()
                .map(|gain| i128::from(*gain))
                .sum(),
            self.paired_gain_micros.len() as i128,
        ))
    }

    pub fn success_rate_micros(&self) -> Result<u32> {
        self.validate()?;
        Ok(
            ((u64::from(self.successful_clusters) * 1_000_000) / self.cluster_ids.len() as u64)
                as u32,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageBucketState {
    pub bucket_id: String,
    pub task_space_source_id: String,
    pub observed_independent_clusters: u32,
    pub required_independent_clusters: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedLearnerAssetRef {
    pub release_id: String,
    pub bundle_digest: String,
    pub run_application_id: String,
    pub host_execution_receipt_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnerStateV2 {
    pub schema_version: String,
    pub id: String,
    pub profile_id: String,
    pub skill_snapshot_digest: String,
    pub improver_snapshot_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub model_tools_digest: String,
    pub runner_digest: String,
    pub rules_digest: String,
    pub source_watermark: u64,
    pub source_artifact_ids: Vec<String>,
    pub development_fact_ids: Vec<String>,
    pub completed_cycles: Vec<DevelopmentCycleObservation>,
    pub failure_clusters: Vec<String>,
    pub coverage_buckets: Vec<CoverageBucketState>,
    pub applied_assets: Vec<AppliedLearnerAssetRef>,
    pub active_probe_job_id: Option<String>,
    pub last_trigger_window_digest: Option<String>,
    pub cooldown_remaining_cycles: u8,
}

impl LearnerStateV2 {
    pub const SCHEMA: &'static str = LEARNER_STATE_V2_SCHEMA;

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LEARNER_STATE_V2_SCHEMA {
            return Err(Error::Invalid("unsupported learner state schema".into()));
        }
        identifier(&self.id)?;
        identifier(&self.profile_id)?;
        for (value, name) in [
            (&self.skill_snapshot_digest, "skill_snapshot_digest"),
            (&self.improver_snapshot_digest, "improver_snapshot_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.model_tools_digest, "model_tools_digest"),
            (&self.runner_digest, "runner_digest"),
            (&self.rules_digest, "rules_digest"),
        ] {
            digest(value, name)?;
        }
        let mut cycles = std::collections::BTreeSet::new();
        for cycle in &self.completed_cycles {
            cycle.validate()?;
            if cycle.environment_digest != self.environment_digest
                || cycle.grader_digest != self.grader_digest
            {
                return Err(Error::Conflict(
                    "development cycle context differs from learner state".into(),
                ));
            }
            if !cycles.insert(cycle.cycle_id.as_str()) {
                return Err(Error::Invalid("duplicate development cycle".into()));
            }
        }
        let mut sources = std::collections::BTreeSet::new();
        if self.source_artifact_ids.is_empty() {
            return Err(Error::Invalid(
                "learner state requires a typed source closure".into(),
            ));
        }
        for source in &self.source_artifact_ids {
            identifier(source)?;
            if !sources.insert(source.as_str()) {
                return Err(Error::Invalid("duplicate learner source artifact".into()));
            }
        }
        let mut development_facts = std::collections::BTreeSet::new();
        for fact in &self.development_fact_ids {
            identifier(fact)?;
            if !development_facts.insert(fact.as_str()) {
                return Err(Error::Invalid("duplicate development fact artifact".into()));
            }
        }
        if self.completed_cycles.iter().any(|cycle| {
            cycle
                .source_artifact_ids
                .iter()
                .any(|source| !development_facts.contains(source.as_str()))
        }) {
            return Err(Error::Conflict(
                "development cycle source is outside learner closure".into(),
            ));
        }
        let mut failures = std::collections::BTreeSet::new();
        for cluster in &self.failure_clusters {
            identifier(cluster)?;
            if !failures.insert(cluster.as_str()) {
                return Err(Error::Invalid("duplicate failure cluster".into()));
            }
        }
        let mut buckets = std::collections::BTreeSet::new();
        for bucket in &self.coverage_buckets {
            identifier(&bucket.bucket_id)?;
            identifier(&bucket.task_space_source_id)?;
            if !sources.contains(bucket.task_space_source_id.as_str())
                || !buckets.insert(bucket.bucket_id.as_str())
                || bucket.required_independent_clusters == 0
            {
                return Err(Error::Invalid(
                    "coverage bucket lacks a unique typed task-space source".into(),
                ));
            }
        }
        let mut assets = std::collections::BTreeSet::new();
        for asset in &self.applied_assets {
            for value in [
                &asset.release_id,
                &asset.run_application_id,
                &asset.host_execution_receipt_id,
            ] {
                identifier(value)?;
            }
            digest(&asset.bundle_digest, "applied bundle_digest")?;
            if !assets.insert(asset.release_id.as_str()) {
                return Err(Error::Invalid("duplicate applied learner asset".into()));
            }
        }
        if let Some(job) = &self.active_probe_job_id {
            identifier(job)?;
        }
        if let Some(window) = &self.last_trigger_window_digest {
            digest(window, "last_trigger_window_digest")?;
        }
        Ok(())
    }
}

/// Type of a curriculum failure record (plan §8.1.1, v4.3).
///
/// An absolute failure (`!passed`) constitutes a failure gap. A regression
/// (`passed` but scored below its parent) only enters the diagnostic/retention
/// flow and never constitutes a failure gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureRecordType {
    AbsoluteFailure,
    Regression,
}

/// The four states of a failure record (plan §8.1.1, v4.3): `active` counts as
/// a current gap, `retest_due` awaits the one-shot retest after an applied
/// learner asset, `resolved` keeps the record for history, and `diagnostic`
/// marks a record whose environment or grading drifted (it is no longer a
/// current gap) or a legacy v2 cluster that never carried receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureRecordStatus {
    Active,
    RetestDue,
    Resolved,
    Diagnostic,
}

/// Transition note of a record projected from a stored v2 learner state: the
/// v2 wire kept a bare cluster list without receipts or a type, so such a
/// cluster survives only as an untyped diagnostic and never counts as a gap.
pub const LEGACY_V2_UNTYPED_NOTE: &str = "legacy_v2_untyped";
/// Transition note of a record moved to `diagnostic` by an environment or
/// grading drift (plan §8.1.1: 旧记录转diagnostic并注明漂移).
pub const DRIFT_NOTE: &str = "environment_or_grader_drift";
/// Reason of the v3 trigger when every active absolute failure lies outside
/// the registered task space and no other entry applies (plan §8.1.1).
pub const UNSUPPORTED_SCOPE_REASON: &str = "failure_cluster_outside_registered_task_space";
/// Upper bound of the failure records one learner state carries.
pub const MAX_FAILURE_RECORDS: usize = 10_000;

/// The verified pass that resolved a failure record (plan §8.1.1: 转resolved，
/// 原记录保留). It is stored next to the original failure evidence, which a
/// resolution never rewrites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureResolutionV1 {
    pub resolved_cycle_id: String,
    pub resolved_task_id: String,
    pub resolved_candidate_execution_id: String,
    pub resolved_grader_receipt_digest: String,
    pub resolved_skill_snapshot_digest: String,
}

impl FailureResolutionV1 {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.resolved_cycle_id)?;
        identifier(&self.resolved_task_id)?;
        identifier(&self.resolved_candidate_execution_id)?;
        digest(
            &self.resolved_grader_receipt_digest,
            "resolution grader_receipt_digest",
        )?;
        digest(
            &self.resolved_skill_snapshot_digest,
            "resolution skill_snapshot_digest",
        )?;
        Ok(())
    }
}

/// One failure record (plan §8.1.1, L1305): the unit of a failure gap is an
/// independent development cluster bound to verified execution and grading
/// receipts. The record keeps the task, the per-task candidate execution
/// receipt id and grader receipt digest of the verified outcome that produced
/// it, the learner Skill snapshot digest at that time, and its type.
///
/// The failure evidence (`task_id`, `cycle_id`, `candidate_execution_id`,
/// `grader_receipt_digest`, `skill_snapshot_digest`, `environment_digest`,
/// `grader_version_digest`) is immutable: a later pass is stored in
/// `resolution`, a drift only changes the status and note, and a later failure
/// creates a new record. Only the projection of a legacy v2 cluster lacks
/// receipts and a type; it is always an untyped `diagnostic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecordV1 {
    pub record_id: String,
    pub cluster_id: String,
    pub task_id: Option<String>,
    pub record_type: Option<FailureRecordType>,
    pub status: FailureRecordStatus,
    pub cycle_id: Option<String>,
    pub candidate_execution_id: Option<String>,
    pub grader_receipt_digest: Option<String>,
    pub skill_snapshot_digest: String,
    pub environment_digest: String,
    pub grader_version_digest: String,
    /// Fixed-vocabulary note of the last status transition that has one:
    /// `environment_or_grader_drift`, `retest_after:<release_id>`,
    /// `retest_selected:<release_id>`, `retest_void:<release_id>`,
    /// `retest_failed:<cycle_id>` or `legacy_v2_untyped`.
    pub transition_note: Option<String>,
    /// Release whose trusted application moved the record to `retest_due`;
    /// `None` in every other status.
    pub retest_release_id: Option<String>,
    /// The verified pass that resolved the record; `Some` exactly when the
    /// status is `resolved`.
    pub resolution: Option<FailureResolutionV1>,
}

impl FailureRecordV1 {
    /// Whether this record is the untyped projection of a stored v2 cluster.
    pub fn is_legacy_v2(&self) -> bool {
        self.transition_note.as_deref() == Some(LEGACY_V2_UNTYPED_NOTE)
    }

    pub fn validate(&self) -> Result<()> {
        identifier(&self.record_id)?;
        identifier(&self.cluster_id)?;
        for (value, name) in [
            (&self.skill_snapshot_digest, "failure skill_snapshot_digest"),
            (&self.environment_digest, "failure environment_digest"),
            (&self.grader_version_digest, "failure grader_version_digest"),
        ] {
            digest(value, name)?;
        }
        if let Some(note) = &self.transition_note {
            text(note, "failure transition_note", 256)?;
        }
        if self.is_legacy_v2() {
            if self.record_type.is_some()
                || self.task_id.is_some()
                || self.cycle_id.is_some()
                || self.candidate_execution_id.is_some()
                || self.grader_receipt_digest.is_some()
                || self.status != FailureRecordStatus::Diagnostic
                || self.retest_release_id.is_some()
                || self.resolution.is_some()
                || self.record_id != legacy_failure_record_id(&self.cluster_id)
            {
                return Err(Error::Invalid(
                    "a legacy v2 failure record is an untyped diagnostic without receipts".into(),
                ));
            }
            return Ok(());
        }
        let (Some(record_type), Some(task_id), Some(cycle_id), Some(execution), Some(grader)) = (
            self.record_type,
            self.task_id.as_deref(),
            self.cycle_id.as_deref(),
            self.candidate_execution_id.as_deref(),
            self.grader_receipt_digest.as_deref(),
        ) else {
            return Err(Error::Invalid(
                "a failure record is bound to its verified task, cycle and receipts".into(),
            ));
        };
        identifier(task_id)?;
        identifier(cycle_id)?;
        identifier(execution)?;
        digest(grader, "failure grader_receipt_digest")?;
        if self.record_id != failure_record_id(&self.cluster_id, record_type, cycle_id) {
            return Err(Error::Invalid(
                "failure record id differs from its cluster, type and cycle".into(),
            ));
        }
        match (&self.retest_release_id, self.status) {
            (Some(release), FailureRecordStatus::RetestDue) => identifier(release)?,
            (None, FailureRecordStatus::RetestDue) => {
                return Err(Error::Invalid(
                    "a retest_due record names the release that caused it".into(),
                ));
            }
            (Some(_), _) => {
                return Err(Error::Invalid(
                    "retest_release_id is only valid on a retest_due record".into(),
                ));
            }
            (None, _) => {}
        }
        match (&self.resolution, self.status) {
            (Some(resolution), FailureRecordStatus::Resolved) => resolution.validate()?,
            (None, FailureRecordStatus::Resolved) => {
                return Err(Error::Invalid(
                    "a resolved record keeps the verified pass that resolved it".into(),
                ));
            }
            (Some(_), _) => {
                return Err(Error::Invalid(
                    "only a resolved record carries a resolution".into(),
                ));
            }
            (None, _) => {}
        }
        Ok(())
    }

    fn counts_as_open(&self) -> bool {
        matches!(
            self.status,
            FailureRecordStatus::Active | FailureRecordStatus::RetestDue
        )
    }
}

/// One verified task outcome as the failure-record reduction consumes it: the
/// task, the per-task candidate execution receipt id and the grader receipt
/// digest exactly as the verified development observation carries them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureCycleOutcome {
    pub cluster_id: String,
    pub task_id: String,
    pub passed: bool,
    pub paired_gain_micros: i64,
    pub candidate_execution_id: String,
    pub grader_receipt_digest: String,
}

/// One-shot retest priority granted to a development family an applied,
/// approved asset is bound to (plan §8.4.1). Every bound family gets one mark
/// per release, whatever its failure records look like; one selection consumes
/// it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetestPriorityMarkV1 {
    pub family: String,
    pub release_id: String,
    pub consumed: bool,
}

impl RetestPriorityMarkV1 {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.family)?;
        identifier(&self.release_id)?;
        Ok(())
    }
}

fn record_digest_id(prefix: &str, parts: &[&str]) -> String {
    // Length-prefixed parts, so no two part lists share one preimage.
    let mut material = String::new();
    for part in parts {
        material.push_str(&part.len().to_string());
        material.push(':');
        material.push_str(part);
        material.push(';');
    }
    format!("{prefix}-{}", &hash(material.as_bytes())[..24])
}

/// Stable id of the failure record one verified outcome produced: one record
/// per cluster, type and development cycle, so a later outcome never
/// overwrites the evidence of an earlier one.
pub fn failure_record_id(cluster: &str, record_type: FailureRecordType, cycle_id: &str) -> String {
    match record_type {
        FailureRecordType::AbsoluteFailure => {
            record_digest_id("failure", &["absolute_failure", cluster, cycle_id])
        }
        FailureRecordType::Regression => {
            record_digest_id("regression", &["regression", cluster, cycle_id])
        }
    }
}

/// Stable id of the untyped diagnostic a legacy v2 failure cluster projects to.
pub fn legacy_failure_record_id(cluster: &str) -> String {
    record_digest_id("legacy", &[LEGACY_V2_UNTYPED_NOTE, cluster])
}

/// Digest of the learner Skill snapshot after one approved asset was actually
/// applied (plan §8.1.1: 学习者Skill快照因已批准资产的实际应用而改变). It chains the
/// previous snapshot digest with the applied release and bundle, so every
/// trusted application yields a new snapshot digest and records written later
/// carry it.
pub fn learner_skill_snapshot_after_asset(
    previous: &str,
    asset: &AppliedLearnerAssetRef,
) -> String {
    hash(
        format!(
            "rsia.learner_skill_snapshot.v1:{previous}:{}:{}:{}",
            asset.release_id.len(),
            asset.release_id,
            asset.bundle_digest
        )
        .as_bytes(),
    )
}

/// Reduces one verified development cycle into the failure records (plan
/// §8.1.1). Within one environment and grading version:
///
/// - an absolute failure (`!passed`) creates a new `active` record bound to the
///   outcome's task and receipts; a `retest_due` record of the cluster has
///   failed its retest and returns to `active`;
/// - a verified pass resolves every open absolute-failure record of the
///   cluster whatever its paired gain, and every open regression record when
///   it no longer regresses; the original evidence stays and the pass is
///   stored as the record's `resolution`;
/// - a regression (`passed` with a negative paired gain) creates a new
///   `active` regression record, which never counts as a failure gap;
/// - open records of another environment or grading version move to
///   `diagnostic` with a drift note and stop counting as a current gap.
///
/// Failures never retire by time or a fixed number of rounds here, a record
/// is never deleted, and an applied asset never clears one: only a later
/// verified pass of the same cluster resolves it.
// The reduction appends records, so it really needs the Vec.
#[allow(clippy::ptr_arg)]
pub fn apply_cycle_to_failure_records(
    records: &mut Vec<FailureRecordV1>,
    cycle_id: &str,
    environment_digest: &str,
    grader_digest: &str,
    skill_snapshot_digest: &str,
    outcomes: &[FailureCycleOutcome],
) -> Result<()> {
    identifier(cycle_id)?;
    digest(environment_digest, "cycle environment_digest")?;
    digest(grader_digest, "cycle grader_digest")?;
    digest(skill_snapshot_digest, "cycle skill_snapshot_digest")?;
    let mut clusters = std::collections::BTreeSet::new();
    for outcome in outcomes {
        identifier(&outcome.cluster_id)?;
        identifier(&outcome.task_id)?;
        identifier(&outcome.candidate_execution_id)?;
        digest(&outcome.grader_receipt_digest, "outcome grader receipt")?;
        if !clusters.insert(outcome.cluster_id.as_str()) {
            return Err(Error::Invalid(
                "duplicate cluster in failure cycle outcomes".into(),
            ));
        }
    }
    let mut record_ids = std::collections::BTreeSet::new();
    let mut duplicate_record = false;
    for record in records.iter() {
        record.validate()?;
        if record.cycle_id.as_deref() == Some(cycle_id)
            || record
                .resolution
                .as_ref()
                .is_some_and(|resolution| resolution.resolved_cycle_id == cycle_id)
        {
            return Err(Error::Conflict(
                "development cycle already reduced into failure records".into(),
            ));
        }
        duplicate_record |= !record_ids.insert(record.record_id.as_str());
    }
    if records.len() > MAX_FAILURE_RECORDS {
        return Err(Error::Invalid(
            "failure records exceed the supported bound".into(),
        ));
    }
    if duplicate_record {
        return Err(Error::Invalid("duplicate failure record".into()));
    }
    let used = records.len();
    let additional = outcomes
        .iter()
        .filter(|outcome| !outcome.passed || outcome.paired_gain_micros < 0)
        .count();
    if !matches!(used.checked_add(additional), Some(total) if total <= MAX_FAILURE_RECORDS) {
        return Err(Error::Conflict(format!(
            "learner failure records capacity exceeded: used={used}, additional={additional}, limit={MAX_FAILURE_RECORDS}"
        )));
    }
    for record in records.iter_mut() {
        if record.counts_as_open()
            && (record.environment_digest != environment_digest
                || record.grader_version_digest != grader_digest)
        {
            record.status = FailureRecordStatus::Diagnostic;
            record.transition_note = Some(DRIFT_NOTE.into());
            record.retest_release_id = None;
        }
    }
    let new_record =
        |outcome: &FailureCycleOutcome, record_type: FailureRecordType| FailureRecordV1 {
            record_id: failure_record_id(&outcome.cluster_id, record_type, cycle_id),
            cluster_id: outcome.cluster_id.clone(),
            task_id: Some(outcome.task_id.clone()),
            record_type: Some(record_type),
            status: FailureRecordStatus::Active,
            cycle_id: Some(cycle_id.to_string()),
            candidate_execution_id: Some(outcome.candidate_execution_id.clone()),
            grader_receipt_digest: Some(outcome.grader_receipt_digest.clone()),
            skill_snapshot_digest: skill_snapshot_digest.to_string(),
            environment_digest: environment_digest.to_string(),
            grader_version_digest: grader_digest.to_string(),
            transition_note: None,
            retest_release_id: None,
            resolution: None,
        };
    for outcome in outcomes {
        let regression = outcome.passed && outcome.paired_gain_micros < 0;
        for record in records
            .iter_mut()
            .filter(|record| record.cluster_id == outcome.cluster_id && record.counts_as_open())
        {
            let still_failing = !outcome.passed
                || (regression && record.record_type == Some(FailureRecordType::Regression));
            if still_failing {
                // The outcome is the retest of a retest-due record: it failed
                // again, so the record is a current gap once more.
                if record.status == FailureRecordStatus::RetestDue {
                    record.status = FailureRecordStatus::Active;
                    record.transition_note = Some(format!("retest_failed:{cycle_id}"));
                    record.retest_release_id = None;
                }
                continue;
            }
            record.status = FailureRecordStatus::Resolved;
            record.retest_release_id = None;
            record.resolution = Some(FailureResolutionV1 {
                resolved_cycle_id: cycle_id.to_string(),
                resolved_task_id: outcome.task_id.clone(),
                resolved_candidate_execution_id: outcome.candidate_execution_id.clone(),
                resolved_grader_receipt_digest: outcome.grader_receipt_digest.clone(),
                resolved_skill_snapshot_digest: skill_snapshot_digest.to_string(),
            });
        }
        if !outcome.passed {
            records.push(new_record(outcome, FailureRecordType::AbsoluteFailure));
        } else if regression {
            records.push(new_record(outcome, FailureRecordType::Regression));
        }
    }
    validate_failure_records(records)
}

fn validate_failure_records(records: &[FailureRecordV1]) -> Result<()> {
    if records.len() > MAX_FAILURE_RECORDS {
        return Err(Error::Invalid(
            "failure records exceed the supported bound".into(),
        ));
    }
    let mut record_ids = std::collections::BTreeSet::new();
    for record in records {
        record.validate()?;
        if !record_ids.insert(record.record_id.as_str()) {
            return Err(Error::Invalid("duplicate failure record".into()));
        }
    }
    Ok(())
}

/// Plan §8.1.1/§8.4.1: the trusted application of an approved asset changes
/// the learner Skill snapshot, so the `active` records of every bound family
/// move to `retest_due` (naming the release), and every bound family — with
/// active, resolved, diagnostic or no records at all — gets one unconsumed
/// one-shot retest mark for the release. Applying an asset never clears or
/// resolves a record, and applying the same release again changes nothing.
// The mark list is appended to, so it needs the Vec.
#[allow(clippy::ptr_arg)]
pub fn mark_retest_due_after_asset(
    records: &mut Vec<FailureRecordV1>,
    marks: &mut Vec<RetestPriorityMarkV1>,
    families: &[String],
    release_id: &str,
) -> Result<()> {
    identifier(release_id)?;
    let mut bound = std::collections::BTreeSet::new();
    for family in families {
        identifier(family)?;
        bound.insert(family.as_str());
    }
    for family in bound {
        if !marks
            .iter()
            .any(|mark| mark.family == family && mark.release_id == release_id)
        {
            marks.push(RetestPriorityMarkV1 {
                family: family.to_string(),
                release_id: release_id.to_string(),
                consumed: false,
            });
        }
        for record in records.iter_mut() {
            if record.cluster_id == family && record.status == FailureRecordStatus::Active {
                record.status = FailureRecordStatus::RetestDue;
                record.transition_note = Some(format!("retest_after:{release_id}"));
                record.retest_release_id = Some(release_id.to_string());
            }
        }
    }
    for mark in marks.iter() {
        mark.validate()?;
    }
    validate_failure_records(records)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnerStateV3 {
    pub schema_version: String,
    pub id: String,
    pub profile_id: String,
    pub skill_snapshot_digest: String,
    pub improver_snapshot_digest: String,
    pub environment_digest: String,
    pub grader_digest: String,
    pub model_tools_digest: String,
    pub runner_digest: String,
    pub rules_digest: String,
    pub source_watermark: u64,
    pub source_artifact_ids: Vec<String>,
    pub development_fact_ids: Vec<String>,
    pub completed_cycles: Vec<DevelopmentCycleObservation>,
    /// In v3 this field has exactly one meaning: the sorted clusters of the
    /// `active` absolute-failure records (validation enforces it). A legacy v2
    /// cluster list is not carried here but as untyped diagnostic records.
    pub failure_clusters: Vec<String>,
    pub coverage_buckets: Vec<CoverageBucketState>,
    pub applied_assets: Vec<AppliedLearnerAssetRef>,
    pub failure_records: Vec<FailureRecordV1>,
    pub retest_priority_marks: Vec<RetestPriorityMarkV1>,
    pub active_probe_job_id: Option<String>,
    pub last_trigger_window_digest: Option<String>,
    pub cooldown_remaining_cycles: u8,
}

impl LearnerStateV3 {
    pub const SCHEMA: &'static str = LEARNER_STATE_V3_SCHEMA;

    /// Read-side interpretation of a stored v2 learner state. v2 kept a bare
    /// cluster list without receipts or types, mixing absolute failures and
    /// regressions; each listed cluster therefore survives as an untyped
    /// `diagnostic` record noted `legacy_v2_untyped` (so it is neither lost nor
    /// overwritten by the next cycle), and none of them counts as a current
    /// failure gap.
    pub fn from_v2(state: LearnerStateV2) -> Self {
        let failure_records = state
            .failure_clusters
            .iter()
            .map(|cluster| FailureRecordV1 {
                record_id: legacy_failure_record_id(cluster),
                cluster_id: cluster.clone(),
                task_id: None,
                record_type: None,
                status: FailureRecordStatus::Diagnostic,
                cycle_id: None,
                candidate_execution_id: None,
                grader_receipt_digest: None,
                skill_snapshot_digest: state.skill_snapshot_digest.clone(),
                environment_digest: state.environment_digest.clone(),
                grader_version_digest: state.grader_digest.clone(),
                transition_note: Some(LEGACY_V2_UNTYPED_NOTE.into()),
                retest_release_id: None,
                resolution: None,
            })
            .collect();
        Self {
            schema_version: LEARNER_STATE_V3_SCHEMA.into(),
            id: state.id,
            profile_id: state.profile_id,
            skill_snapshot_digest: state.skill_snapshot_digest,
            improver_snapshot_digest: state.improver_snapshot_digest,
            environment_digest: state.environment_digest,
            grader_digest: state.grader_digest,
            model_tools_digest: state.model_tools_digest,
            runner_digest: state.runner_digest,
            rules_digest: state.rules_digest,
            source_watermark: state.source_watermark,
            source_artifact_ids: state.source_artifact_ids,
            development_fact_ids: state.development_fact_ids,
            completed_cycles: state.completed_cycles,
            failure_clusters: vec![],
            coverage_buckets: state.coverage_buckets,
            applied_assets: state.applied_assets,
            failure_records,
            retest_priority_marks: vec![],
            active_probe_job_id: state.active_probe_job_id,
            last_trigger_window_digest: state.last_trigger_window_digest,
            cooldown_remaining_cycles: state.cooldown_remaining_cycles,
        }
    }

    /// The clusters whose active absolute-failure records constitute the
    /// current failure gap. Regression, retest-due, resolved and diagnostic
    /// records are deliberately absent.
    pub fn active_absolute_failure_clusters(&self) -> Vec<String> {
        let mut clusters: Vec<String> = self
            .failure_records
            .iter()
            .filter(|record| {
                record.status == FailureRecordStatus::Active
                    && record.record_type == Some(FailureRecordType::AbsoluteFailure)
            })
            .map(|record| record.cluster_id.clone())
            .collect();
        clusters.sort();
        clusters.dedup();
        clusters
    }

    /// Applies one approved learner asset whose trusted application the
    /// caller has verified (plan §8.1.1/§8.4.1): records the asset, moves the
    /// learner Skill snapshot digest forward, moves the active records of the
    /// bound development families to `retest_due` and grants each bound family
    /// one unconsumed retest mark. An asset already applied with the same
    /// identity is a no-op (`Ok(false)`); a changed identity is a `Conflict`; an
    /// asset bound to no development family is a `Conflict`.
    pub fn apply_learner_asset(
        &mut self,
        asset: AppliedLearnerAssetRef,
        bound_families: &[String],
    ) -> Result<bool> {
        for value in [
            &asset.release_id,
            &asset.run_application_id,
            &asset.host_execution_receipt_id,
        ] {
            identifier(value)?;
        }
        digest(&asset.bundle_digest, "applied bundle_digest")?;
        if let Some(existing) = self
            .applied_assets
            .iter()
            .find(|existing| existing.release_id == asset.release_id)
        {
            if crate::fingerprint(existing)? != crate::fingerprint(&asset)? {
                return Err(Error::Conflict(
                    "applied learner asset identity changed".into(),
                ));
            }
            return Ok(false);
        }
        if bound_families.is_empty() {
            return Err(Error::Conflict(
                "applied release binds no development family".into(),
            ));
        }
        self.skill_snapshot_digest =
            learner_skill_snapshot_after_asset(&self.skill_snapshot_digest, &asset);
        let release_id = asset.release_id.clone();
        self.applied_assets.push(asset);
        self.applied_assets
            .sort_by(|left, right| left.release_id.cmp(&right.release_id));
        mark_retest_due_after_asset(
            &mut self.failure_records,
            &mut self.retest_priority_marks,
            bound_families,
            &release_id,
        )?;
        self.failure_clusters = self.active_absolute_failure_clusters();
        self.validate()?;
        Ok(true)
    }

    /// One selection consumes the unconsumed retest mark of `family` for
    /// `release_id` (plan §8.4.1: 一次性复验优先). The family's `retest_due`
    /// records go back to `active`: the retest is now scheduled, its result is
    /// booked by §8.1.1, and later selections no longer rank the family as
    /// retest-due for this release.
    pub fn consume_retest_priority_mark(&mut self, family: &str, release_id: &str) -> Result<()> {
        let mark = self
            .retest_priority_marks
            .iter_mut()
            .find(|mark| !mark.consumed && mark.family == family && mark.release_id == release_id)
            .ok_or_else(|| Error::Conflict("no unconsumed retest mark to consume".into()))?;
        mark.consumed = true;
        for record in self.failure_records.iter_mut() {
            if record.cluster_id == family && record.status == FailureRecordStatus::RetestDue {
                record.status = FailureRecordStatus::Active;
                record.transition_note = Some(format!("retest_selected:{release_id}"));
                record.retest_release_id = None;
            }
        }
        self.failure_clusters = self.active_absolute_failure_clusters();
        self.validate()
    }

    /// The influence of a release that is no longer live (rolled back or
    /// revoked) is void (plan §8.4.1/V109.c: 撤销资产后影响失效): its
    /// `retest_due` records return to `active`, so the failure counts as a
    /// current gap again instead of waiting for a retest that no longer
    /// applies. Its marks stay unconsumed and are ignored while it is void.
    pub fn void_retest_due_of_release(&mut self, release_id: &str) -> Result<()> {
        identifier(release_id)?;
        for record in self.failure_records.iter_mut() {
            if record.status == FailureRecordStatus::RetestDue
                && record.retest_release_id.as_deref() == Some(release_id)
            {
                record.status = FailureRecordStatus::Active;
                record.transition_note = Some(format!("retest_void:{release_id}"));
                record.retest_release_id = None;
            }
        }
        self.failure_clusters = self.active_absolute_failure_clusters();
        self.validate()
    }

    /// The frozen learner state the counterfactual selection arm ranks
    /// against (plan §8.4.1): the state as registered, before it absorbed any
    /// failure record, retest mark, applied asset or covered cluster. Only the
    /// selection-relevant fields are reset; the view is never persisted.
    pub fn frozen_selection_view(&self) -> Self {
        let mut frozen = self.clone();
        frozen.failure_records.clear();
        frozen.retest_priority_marks.clear();
        frozen.applied_assets.clear();
        frozen.failure_clusters.clear();
        frozen.completed_cycles.clear();
        frozen.development_fact_ids.clear();
        for bucket in frozen.coverage_buckets.iter_mut() {
            bucket.observed_independent_clusters = 0;
        }
        frozen
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LEARNER_STATE_V3_SCHEMA {
            return Err(Error::Invalid("unsupported learner state schema".into()));
        }
        // Reuse the v2 field validation by projecting the shared fields.
        LearnerStateV2 {
            schema_version: LEARNER_STATE_V2_SCHEMA.into(),
            id: self.id.clone(),
            profile_id: self.profile_id.clone(),
            skill_snapshot_digest: self.skill_snapshot_digest.clone(),
            improver_snapshot_digest: self.improver_snapshot_digest.clone(),
            environment_digest: self.environment_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            model_tools_digest: self.model_tools_digest.clone(),
            runner_digest: self.runner_digest.clone(),
            rules_digest: self.rules_digest.clone(),
            source_watermark: self.source_watermark,
            source_artifact_ids: self.source_artifact_ids.clone(),
            development_fact_ids: self.development_fact_ids.clone(),
            completed_cycles: self.completed_cycles.clone(),
            failure_clusters: self.failure_clusters.clone(),
            coverage_buckets: self.coverage_buckets.clone(),
            applied_assets: self.applied_assets.clone(),
            active_probe_job_id: self.active_probe_job_id.clone(),
            last_trigger_window_digest: self.last_trigger_window_digest.clone(),
            cooldown_remaining_cycles: self.cooldown_remaining_cycles,
        }
        .validate()?;
        validate_failure_records(&self.failure_records)?;
        if self.failure_clusters != self.active_absolute_failure_clusters() {
            return Err(Error::Invalid(
                "v3 failure_clusters must list exactly the active absolute-failure clusters".into(),
            ));
        }
        let applied: std::collections::BTreeSet<&str> = self
            .applied_assets
            .iter()
            .map(|asset| asset.release_id.as_str())
            .collect();
        if self.failure_records.iter().any(|record| {
            record
                .retest_release_id
                .as_deref()
                .is_some_and(|release| !applied.contains(release))
        }) {
            return Err(Error::Invalid(
                "a retest_due record names a release that was never applied".into(),
            ));
        }
        let mut marks = std::collections::BTreeSet::new();
        for mark in &self.retest_priority_marks {
            mark.validate()?;
            if !marks.insert((mark.family.as_str(), mark.release_id.as_str())) {
                return Err(Error::Invalid("duplicate retest priority mark".into()));
            }
            if !applied.contains(mark.release_id.as_str()) {
                return Err(Error::Invalid(
                    "a retest priority mark names a release that was never applied".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "trigger", deny_unknown_fields)]
pub enum PlateauSignalV1 {
    FailureGap {
        failure_cluster_ids: Vec<String>,
    },
    CoverageGap {
        authorized_bucket_ids: Vec<String>,
    },
    PlateauProbe {
        cycle_ids: Vec<String>,
        unique_cluster_ids: Vec<String>,
        mean_gain_micros: Vec<i64>,
        sample_variance_micros_squared: u64,
    },
    ColdStart,
    NotTriggered {
        reasons: Vec<String>,
    },
}

pub fn detect_plateau_signal(
    profile: &CurriculumControlProfileV1,
    state: &LearnerStateV2,
) -> Result<PlateauSignalV1> {
    profile.validate()?;
    state.validate()?;
    if state.cooldown_remaining_cycles > 0 {
        return Ok(PlateauSignalV1::NotTriggered {
            reasons: vec!["cooldown".into()],
        });
    }
    if !state.failure_clusters.is_empty() {
        return Ok(PlateauSignalV1::FailureGap {
            failure_cluster_ids: state.failure_clusters.clone(),
        });
    }
    let gaps: Vec<_> = state
        .coverage_buckets
        .iter()
        .filter(|bucket| {
            bucket.observed_independent_clusters < bucket.required_independent_clusters
        })
        .map(|bucket| bucket.bucket_id.clone())
        .collect();
    if !gaps.is_empty() {
        return Ok(PlateauSignalV1::CoverageGap {
            authorized_bucket_ids: gaps,
        });
    }
    let window = usize::from(profile.plateau_window_cycles);
    if state.completed_cycles.len() < window {
        return Ok(PlateauSignalV1::ColdStart);
    }
    let cycles = &state.completed_cycles[state.completed_cycles.len() - window..];
    let environment = &cycles[0].environment_digest;
    let grader = &cycles[0].grader_digest;
    let mut unique = std::collections::BTreeSet::new();
    let mut means = Vec::with_capacity(window);
    let mut exact_means = Vec::with_capacity(window);
    let mut reasons = Vec::new();
    for cycle in cycles {
        if cycle.environment_digest != *environment || cycle.grader_digest != *grader {
            reasons.push("environment_or_grader_changed".into());
        }
        if cycle.cluster_ids.len() < usize::from(profile.min_independent_clusters_per_cycle) {
            reasons.push("insufficient_cycle_clusters".into());
        }
        if cycle.success_rate_micros()? < profile.min_success_rate_micros {
            reasons.push("success_rate_below_threshold".into());
        }
        let (sum, count) = cycle.gain_sum_and_count()?;
        let threshold = i128::from(profile.max_abs_mean_gain_micros);
        if sum.abs() > threshold * count {
            reasons.push("mean_gain_not_plateau".into());
        }
        means.push(i64::try_from(sum / count).map_err(|_| Error::Internal)?);
        exact_means.push((sum, count));
        for cluster in &cycle.cluster_ids {
            if !unique.insert(cluster.clone()) {
                reasons.push("cluster_reused_across_cycles".into());
            }
        }
    }
    if unique.len() < usize::from(profile.min_unique_window_clusters) {
        reasons.push("insufficient_unique_window_clusters".into());
    }
    let (variance, variance_exceeds) =
        sample_variance_micros_squared(&exact_means, profile.max_gain_variance_micros_squared)?;
    if variance_exceeds {
        reasons.push("gain_variance_above_threshold".into());
    }
    reasons.sort();
    reasons.dedup();
    if !reasons.is_empty() {
        return Ok(PlateauSignalV1::NotTriggered { reasons });
    }
    Ok(PlateauSignalV1::PlateauProbe {
        cycle_ids: cycles.iter().map(|cycle| cycle.cycle_id.clone()).collect(),
        unique_cluster_ids: unique.into_iter().collect(),
        mean_gain_micros: means,
        sample_variance_micros_squared: variance,
    })
}

/// v3 trigger (plan §8.1.1): only the clusters with active absolute-failure
/// records constitute a failure gap, and only while they carry an executable
/// improvement direction — the cluster is a registered task bucket of the
/// authorized task space. Active clusters outside it are dropped from the
/// gap. When every active cluster is outside it, coverage, plateau and legal
/// cold-start entries remain available. Only when none applies does the
/// trigger return `NotTriggered` with `UNSUPPORTED_SCOPE_REASON`, which the
/// probe scheduler ends in `unsupported_scope`. Regression, retest-due,
/// resolved and diagnostic records never trigger `failure_gap`. Everything
/// else matches the v2 window logic.
pub fn detect_plateau_signal_v3(
    profile: &CurriculumControlProfileV1,
    state: &LearnerStateV3,
) -> Result<PlateauSignalV1> {
    profile.validate()?;
    state.validate()?;
    if state.cooldown_remaining_cycles > 0 {
        return Ok(PlateauSignalV1::NotTriggered {
            reasons: vec!["cooldown".into()],
        });
    }
    let active_clusters = state.active_absolute_failure_clusters();
    let has_active_failures = !active_clusters.is_empty();
    if has_active_failures {
        let registered: std::collections::BTreeSet<&str> = state
            .coverage_buckets
            .iter()
            .map(|bucket| bucket.bucket_id.as_str())
            .collect();
        let failure_cluster_ids: Vec<String> = active_clusters
            .into_iter()
            .filter(|cluster| registered.contains(cluster.as_str()))
            .collect();
        if !failure_cluster_ids.is_empty() {
            return Ok(PlateauSignalV1::FailureGap {
                failure_cluster_ids,
            });
        }
    }
    let gaps: Vec<_> = state
        .coverage_buckets
        .iter()
        .filter(|bucket| {
            bucket.observed_independent_clusters < bucket.required_independent_clusters
        })
        .map(|bucket| bucket.bucket_id.clone())
        .collect();
    if !gaps.is_empty() {
        return Ok(PlateauSignalV1::CoverageGap {
            authorized_bucket_ids: gaps,
        });
    }
    let legacy = LearnerStateV2 {
        schema_version: LEARNER_STATE_V2_SCHEMA.into(),
        id: state.id.clone(),
        profile_id: state.profile_id.clone(),
        skill_snapshot_digest: state.skill_snapshot_digest.clone(),
        improver_snapshot_digest: state.improver_snapshot_digest.clone(),
        environment_digest: state.environment_digest.clone(),
        grader_digest: state.grader_digest.clone(),
        model_tools_digest: state.model_tools_digest.clone(),
        runner_digest: state.runner_digest.clone(),
        rules_digest: state.rules_digest.clone(),
        source_watermark: state.source_watermark,
        source_artifact_ids: state.source_artifact_ids.clone(),
        development_fact_ids: state.development_fact_ids.clone(),
        completed_cycles: state.completed_cycles.clone(),
        // The v3 failure gap above is decided by the records; the legacy field
        // is emptied here so it cannot leak into the plateau path.
        failure_clusters: vec![],
        coverage_buckets: state.coverage_buckets.clone(),
        applied_assets: state.applied_assets.clone(),
        active_probe_job_id: state.active_probe_job_id.clone(),
        last_trigger_window_digest: state.last_trigger_window_digest.clone(),
        cooldown_remaining_cycles: state.cooldown_remaining_cycles,
    };
    let signal = detect_plateau_signal(profile, &legacy)?;
    if has_active_failures && matches!(signal, PlateauSignalV1::NotTriggered { .. }) {
        return Ok(PlateauSignalV1::NotTriggered {
            reasons: vec![UNSUPPORTED_SCOPE_REASON.into()],
        });
    }
    Ok(signal)
}

fn sample_variance_micros_squared(values: &[(i128, i128)], threshold: u64) -> Result<(u64, bool)> {
    if values.len() < 2 {
        return Err(Error::Invalid(
            "sample variance requires at least two cycles".into(),
        ));
    }
    let common_denominator = values.iter().try_fold(1i128, |product, (_, count)| {
        product
            .checked_mul(*count)
            .ok_or_else(|| Error::Invalid("gain variance denominator overflow".into()))
    })?;
    let scaled = values
        .iter()
        .map(|(sum, count)| {
            sum.checked_mul(common_denominator / count)
                .ok_or_else(|| Error::Invalid("gain variance scale overflow".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let k = scaled.len() as i128;
    let scaled_sum = scaled.iter().try_fold(0i128, |total, value| {
        total
            .checked_add(*value)
            .ok_or_else(|| Error::Invalid("gain variance sum overflow".into()))
    })?;
    let numerator = scaled.iter().try_fold(0i128, |total, value| {
        let delta = value
            .checked_mul(k)
            .and_then(|value| value.checked_sub(scaled_sum))
            .ok_or_else(|| Error::Invalid("gain variance delta overflow".into()))?;
        total
            .checked_add(
                delta
                    .checked_mul(delta)
                    .ok_or_else(|| Error::Invalid("gain variance square overflow".into()))?,
            )
            .ok_or_else(|| Error::Invalid("gain variance numerator overflow".into()))
    })?;
    let denominator = k
        .checked_mul(k)
        .and_then(|value| value.checked_mul(k - 1))
        .and_then(|value| value.checked_mul(common_denominator))
        .and_then(|value| value.checked_mul(common_denominator))
        .ok_or_else(|| Error::Invalid("gain variance denominator overflow".into()))?;
    let comparison = i128::from(threshold)
        .checked_mul(denominator)
        .ok_or_else(|| Error::Invalid("gain variance threshold overflow".into()))?;
    let display = numerator
        .checked_add(denominator - 1)
        .and_then(|value| value.checked_div(denominator))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| Error::Invalid("gain variance out of range".into()))?;
    Ok((display, numerator > comparison))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredProperty {
    ReferenceEqual,
    DeterministicRepeat,
    BelowMapsToMin,
    AboveMapsToMax,
    InRangeIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredTestProposalV1 {
    pub schema_version: String,
    pub id: String,
    pub probe_job_id: String,
    pub target_id: String,
    pub property: RegisteredProperty,
    pub value: i64,
    pub min: i64,
    pub max: i64,
    pub parent_family: String,
    pub data_use: DataUse,
    pub source_artifact_ids: Vec<String>,
    pub reason: String,
}

impl StructuredTestProposalV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.structured_test_proposal.v1" {
            return Err(Error::Invalid(
                "unsupported structured proposal schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.probe_job_id,
            &self.target_id,
            &self.parent_family,
        ] {
            identifier(value)?;
        }
        if self.data_use != DataUse::Development
            || self.min < -1_000_000
            || self.max > 1_000_000
            || self.value < -1_000_000
            || self.value > 1_000_000
            || self.min > self.max
        {
            return Err(Error::Invalid(
                "proposal outside registered clamp domain".into(),
            ));
        }
        text(&self.reason, "proposal reason", 4096)?;
        if self.source_artifact_ids.is_empty() {
            return Err(Error::Invalid(
                "structured proposal requires typed development sources".into(),
            ));
        }
        let mut sources = std::collections::BTreeSet::new();
        for source in &self.source_artifact_ids {
            identifier(source)?;
            if !sources.insert(source.as_str()) {
                return Err(Error::Invalid("duplicate proposal source".into()));
            }
        }
        if serde_json::to_vec(&serde_json::json!({
            "value": self.value,
            "min": self.min,
            "max": self.max
        }))
        .map_err(|_| Error::Internal)?
        .len()
            > 16_384
        {
            return Err(Error::Invalid(
                "normalized proposal input exceeds 16 KiB".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAttemptOutcome {
    Valid,
    Duplicate,
    InvalidOracle,
    Malformed,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeTerminal {
    GeneratedValid,
    NoNovelTask,
    InvalidOracle,
    BudgetExhausted,
    UnsupportedScope,
    Cooldown,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeJobV1 {
    pub schema_version: String,
    pub id: String,
    pub profile_id: String,
    pub state_id: String,
    pub state_digest: String,
    pub trigger_digest: String,
    pub root_budget_limit_micros: u64,
    pub curriculum_share_limit_micros: u64,
    pub effective_monetary_limit_micros: u64,
    pub provider_dispatch_count: u32,
    pub attempts: Vec<ProposalAttemptOutcome>,
    pub terminal: Option<ProbeTerminal>,
}

impl ProbeJobV1 {
    pub fn record_attempt(
        &mut self,
        profile: &CurriculumControlProfileV1,
        outcome: ProposalAttemptOutcome,
    ) -> Result<()> {
        profile.validate()?;
        for value in [&self.id, &self.profile_id, &self.state_id] {
            identifier(value)?;
        }
        digest(&self.state_digest, "probe state_digest")?;
        digest(&self.trigger_digest, "probe trigger_digest")?;
        if self.schema_version != "rsia.coverage_probe_job.v1"
            || self.profile_id != profile.profile_id
            || self.curriculum_share_limit_micros
                != profile.curriculum_limit(self.root_budget_limit_micros)?
            || self.effective_monetary_limit_micros != 0
            || self.provider_dispatch_count != 0
        {
            return Err(Error::Conflict(
                "probe job differs from the disabled zero-budget control".into(),
            ));
        }
        if self.terminal.is_some() {
            return Err(Error::Conflict("probe job is terminal".into()));
        }
        if self.attempts.len() >= usize::from(profile.max_total_proposals) {
            return Err(Error::Budget);
        }
        self.attempts.push(outcome);
        match outcome {
            ProposalAttemptOutcome::Valid => self.terminal = Some(ProbeTerminal::GeneratedValid),
            ProposalAttemptOutcome::InvalidOracle
                if self.attempts.len() == usize::from(profile.max_total_proposals) =>
            {
                self.terminal = Some(ProbeTerminal::InvalidOracle);
            }
            _ if self.attempts.len() == usize::from(profile.max_total_proposals) => {
                self.terminal = Some(ProbeTerminal::NoNovelTask);
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learner_state_changes_next_task() {
        let a = TaskProposal {
            id: "t1".into(),
            parent_family: "fam-a".into(),
            data_use: DataUse::Development,
            difficulty: 0.2,
            learning_value: 0.8,
            correct: false,
            oracle_ok: true,
        };
        let b = TaskProposal {
            id: "t2".into(),
            parent_family: "fam-b".into(),
            ..a.clone()
        };
        let s1 = LearnerState {
            checkpoint: "c1".into(),
            failure_clusters: vec!["fam-a".into()],
        };
        let s2 = LearnerState {
            checkpoint: "c2".into(),
            failure_clusters: vec!["fam-b".into()],
        };
        let pool = [a, b];
        assert_eq!(next_task(&s1, &pool).unwrap(), "t1");
        assert_eq!(next_task(&s2, &pool).unwrap(), "t2");
    }

    #[test]
    fn holdout_and_no_oracle_rejected() {
        let mut p = TaskProposal {
            id: "t1".into(),
            parent_family: "fam".into(),
            data_use: DataUse::AcceptanceEpoch,
            difficulty: 0.2,
            learning_value: 0.8,
            correct: true,
            oracle_ok: false,
        };
        assert!(p.validate().is_err());
        p.data_use = DataUse::Development;
        p.validate().unwrap();
        assert_eq!(p.quarantine_reason(), Some("no_oracle"));
    }
}
