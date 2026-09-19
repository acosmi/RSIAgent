//! Learner-conditioned task proposals. Generated items stay in development.
use crate::evaluation::DataUse;
use crate::{Error, Result, identifier, text};
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

pub fn proposal_from_text(id: &str, family: &str, body: &str) -> Result<TaskProposal> {
    text(body, "body", 4096)?;
    Ok(TaskProposal {
        id: id.into(),
        parent_family: family.into(),
        data_use: DataUse::Development,
        difficulty: 0.5,
        learning_value: 0.5,
        correct: false,
        oracle_ok: true,
    })
}

pub const CURRICULUM_PROFILE_SCHEMA: &str = "rsia.curriculum_control_profile.v1";
pub const LEARNER_STATE_V2_SCHEMA: &str = "rsia.learner_state.v2";
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
