//! v4.1 dual-timescale holdout governance (§3.2.1–§3.2.3, V084).
//! These are core contracts; persistence and trusted issuance land later.
use crate::sequential::EarlyStopPlan;
use crate::{Error, Result, fingerprint, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ANCHOR_COVERAGE_SCHEMA: &str = "rsia.anchor_coverage_matrix.v1";
pub const CANDIDATE_PAIR_SCHEMA: &str = "rsia.frozen_candidate_pair.v1";
pub const HOLDOUT_SCHEMA: &str = "rsia.dual_timescale_holdout.v1";
pub const EXPOSURE_LEDGER_SCHEMA: &str = "rsia.exposure_ledger.v1";

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriticalCapabilityCoverage {
    pub capability_id: String,
    pub anchor_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorCoverageMatrix {
    pub schema_version: String,
    pub profile_id: String,
    pub version: String,
    pub critical_capabilities: Vec<CriticalCapabilityCoverage>,
}

impl AnchorCoverageMatrix {
    pub fn new(
        profile_id: impl Into<String>,
        version: impl Into<String>,
        critical_capabilities: Vec<CriticalCapabilityCoverage>,
    ) -> Result<Self> {
        let matrix = Self {
            schema_version: ANCHOR_COVERAGE_SCHEMA.into(),
            profile_id: profile_id.into(),
            version: version.into(),
            critical_capabilities,
        };
        matrix.validate()?;
        Ok(matrix)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ANCHOR_COVERAGE_SCHEMA {
            return Err(Error::Invalid("unsupported anchor coverage schema".into()));
        }
        identifier(&self.profile_id)?;
        identifier(&self.version)?;
        if self.critical_capabilities.is_empty() {
            return Err(Error::Invalid(
                "critical capability matrix must not be empty".into(),
            ));
        }
        let mut capabilities = BTreeSet::new();
        let mut anchors = BTreeSet::new();
        for coverage in &self.critical_capabilities {
            identifier(&coverage.capability_id)?;
            if !capabilities.insert(&coverage.capability_id) {
                return Err(Error::Invalid("duplicate critical capability".into()));
            }
            if coverage.anchor_ids.is_empty() {
                return Err(Error::Invalid(
                    "critical capability lacks anchor coverage".into(),
                ));
            }
            for anchor_id in &coverage.anchor_ids {
                identifier(anchor_id)?;
                if !anchors.insert(anchor_id) {
                    return Err(Error::Invalid("anchor assigned more than once".into()));
                }
            }
        }
        Ok(())
    }

    pub fn anchor_ids(&self) -> BTreeSet<&str> {
        self.critical_capabilities
            .iter()
            .flat_map(|coverage| coverage.anchor_ids.iter().map(String::as_str))
            .collect()
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorResult {
    Passed,
    CriticalRegression,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorOutcome {
    pub anchor_id: String,
    pub result: AnchorResult,
    pub grader_receipt_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorGateDecision {
    Passed,
    CriticalRegression,
}

pub fn anchor_gate(
    matrix: &AnchorCoverageMatrix,
    outcomes: &[AnchorOutcome],
) -> Result<AnchorGateDecision> {
    matrix.validate()?;
    let required = matrix.anchor_ids();
    let mut observed = BTreeSet::new();
    let mut regression = false;
    for outcome in outcomes {
        identifier(&outcome.anchor_id)?;
        digest(&outcome.grader_receipt_digest, "grader_receipt_digest")?;
        if !required.contains(outcome.anchor_id.as_str()) {
            return Err(Error::Invalid(
                "outcome is not in the frozen anchor matrix".into(),
            ));
        }
        if !observed.insert(outcome.anchor_id.as_str()) {
            return Err(Error::Invalid("duplicate anchor outcome".into()));
        }
        regression |= outcome.result == AnchorResult::CriticalRegression;
    }
    if observed != required {
        return Err(Error::Invalid(
            "complete critical anchor coverage is required; dynamic tasks cannot substitute".into(),
        ));
    }
    Ok(if regression {
        AnchorGateDecision::CriticalRegression
    } else {
        AnchorGateDecision::Passed
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCandidatePair {
    pub schema_version: String,
    pub v1_plan_snapshot_digest: String,
    pub candidate_digest: String,
    pub baseline_digest: String,
    pub frozen_at_unix_ms: i64,
}

impl FrozenCandidatePair {
    pub fn new(
        v1_plan_snapshot_digest: impl Into<String>,
        candidate_digest: impl Into<String>,
        baseline_digest: impl Into<String>,
        frozen_at_unix_ms: i64,
    ) -> Result<Self> {
        let pair = Self {
            schema_version: CANDIDATE_PAIR_SCHEMA.into(),
            v1_plan_snapshot_digest: v1_plan_snapshot_digest.into(),
            candidate_digest: candidate_digest.into(),
            baseline_digest: baseline_digest.into(),
            frozen_at_unix_ms,
        };
        pair.validate()?;
        Ok(pair)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CANDIDATE_PAIR_SCHEMA {
            return Err(Error::Invalid("unsupported frozen-pair schema".into()));
        }
        digest(&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest")?;
        digest(&self.candidate_digest, "candidate_digest")?;
        digest(&self.baseline_digest, "baseline_digest")?;
        if self.frozen_at_unix_ms < 0 {
            return Err(Error::Invalid("invalid candidate-pair freeze time".into()));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", deny_unknown_fields)]
pub enum SequentialMode {
    Disabled,
    RejectOnly { early_stop_plan_digest: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldoutManifest {
    pub schema_version: String,
    pub research_family_id: String,
    pub alpha_plan_digest: String,
    pub v1_plan_snapshot_digest: String,
    pub sequential_mode: SequentialMode,
    pub candidate_pair_digest: String,
    pub epoch_id: String,
    pub slice_id: String,
    pub ordered_task_cluster_ids: Vec<String>,
    pub source_closure_digests: Vec<String>,
    pub sampler_version: String,
    pub oracle_version: String,
    pub anchor_coverage_digest: String,
    pub grader_digest: String,
    pub sequence_commitment: String,
    pub environment_digest: String,
    pub private_seed_commitment: String,
    pub issued_at_unix_ms: i64,
}

impl HoldoutManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn issue_after_candidates_frozen(
        pair: &FrozenCandidatePair,
        research_family_id: impl Into<String>,
        alpha_plan_digest: impl Into<String>,
        sequential_mode: SequentialMode,
        epoch_id: impl Into<String>,
        slice_id: impl Into<String>,
        ordered_task_cluster_ids: Vec<String>,
        source_closure_digests: Vec<String>,
        sampler_version: impl Into<String>,
        oracle_version: impl Into<String>,
        anchor_coverage_digest: impl Into<String>,
        grader_digest: impl Into<String>,
        environment_digest: impl Into<String>,
        private_seed_commitment: impl Into<String>,
        issued_at_unix_ms: i64,
    ) -> Result<Self> {
        pair.validate()?;
        if issued_at_unix_ms < pair.frozen_at_unix_ms {
            return Err(Error::Invalid(
                "holdout manifest must be issued after candidate and baseline freeze".into(),
            ));
        }
        let sequence_commitment = fingerprint(&ordered_task_cluster_ids)?;
        let manifest = Self {
            schema_version: HOLDOUT_SCHEMA.into(),
            research_family_id: research_family_id.into(),
            alpha_plan_digest: alpha_plan_digest.into(),
            v1_plan_snapshot_digest: pair.v1_plan_snapshot_digest.clone(),
            sequential_mode,
            candidate_pair_digest: pair.digest()?,
            epoch_id: epoch_id.into(),
            slice_id: slice_id.into(),
            ordered_task_cluster_ids,
            source_closure_digests,
            sampler_version: sampler_version.into(),
            oracle_version: oracle_version.into(),
            anchor_coverage_digest: anchor_coverage_digest.into(),
            grader_digest: grader_digest.into(),
            sequence_commitment,
            environment_digest: environment_digest.into(),
            private_seed_commitment: private_seed_commitment.into(),
            issued_at_unix_ms,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != HOLDOUT_SCHEMA {
            return Err(Error::Invalid("unsupported holdout manifest schema".into()));
        }
        identifier(&self.research_family_id)?;
        identifier(&self.epoch_id)?;
        identifier(&self.slice_id)?;
        identifier(&self.sampler_version)?;
        identifier(&self.oracle_version)?;
        for (value, name) in [
            (&self.alpha_plan_digest, "alpha_plan_digest"),
            (&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest"),
            (&self.candidate_pair_digest, "candidate_pair_digest"),
            (&self.anchor_coverage_digest, "anchor_coverage_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.sequence_commitment, "sequence_commitment"),
            (&self.environment_digest, "environment_digest"),
            (&self.private_seed_commitment, "private_seed_commitment"),
        ] {
            digest(value, name)?;
        }
        if let SequentialMode::RejectOnly {
            early_stop_plan_digest,
        } = &self.sequential_mode
        {
            digest(early_stop_plan_digest, "early_stop_plan_digest")?;
        }
        if self.issued_at_unix_ms < 0 || self.ordered_task_cluster_ids.is_empty() {
            return Err(Error::Invalid(
                "manifest requires a valid issue time and clusters".into(),
            ));
        }
        if self.source_closure_digests.len() != self.ordered_task_cluster_ids.len() {
            return Err(Error::Invalid(
                "every independent cluster must bind its source closure".into(),
            ));
        }
        let mut clusters = BTreeSet::new();
        for cluster_id in &self.ordered_task_cluster_ids {
            identifier(cluster_id)?;
            if !clusters.insert(cluster_id) {
                return Err(Error::Invalid(
                    "duplicate cluster in holdout manifest".into(),
                ));
            }
        }
        for source_digest in &self.source_closure_digests {
            digest(source_digest, "source_closure_digest")?;
        }
        if self.sequence_commitment != fingerprint(&self.ordered_task_cluster_ids)? {
            return Err(Error::Invalid(
                "manifest sequence commitment mismatch".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }

    pub fn validate_sequential_binding(&self, plan: &EarlyStopPlan) -> Result<()> {
        self.validate()?;
        plan.validate()?;
        let SequentialMode::RejectOnly {
            early_stop_plan_digest,
        } = &self.sequential_mode
        else {
            return Err(Error::Invalid(
                "disabled manifest has no sequential-plan binding".into(),
            ));
        };
        if early_stop_plan_digest != &plan.digest()?
            || self.v1_plan_snapshot_digest != plan.v1_plan_snapshot_digest
            || self.alpha_plan_digest != plan.alpha_plan_digest
            || self.anchor_coverage_digest != plan.anchor_coverage_digest
            || self.ordered_task_cluster_ids != plan.ordered_cluster_ids
        {
            return Err(Error::Invalid(
                "manifest and early-stop plan bindings do not match".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonetaryReservationState {
    HeldUndispatched,
    ReleasedUndispatched,
    DispatchedCostPending,
    Finalized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum ExposureState {
    Reserved,
    Dispatched { dispatched_at_unix_ms: i64 },
    FeedbackUsed { used_at_unix_ms: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureEntry {
    pub ticket_id: String,
    pub manifest_digest: String,
    pub candidate_pair_digest: String,
    pub epoch_id: String,
    pub slice_id: String,
    pub task_cluster_ids: Vec<String>,
    pub state: ExposureState,
    pub monetary_state: MonetaryReservationState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureLedger {
    pub schema_version: String,
    pub research_family_id: String,
    pub alpha_plan_digest: String,
    pub query_limit: u32,
    pub entries: Vec<ExposureEntry>,
}

impl ExposureLedger {
    pub fn new(
        research_family_id: impl Into<String>,
        alpha_plan_digest: impl Into<String>,
        query_limit: u32,
    ) -> Result<Self> {
        let ledger = Self {
            schema_version: EXPOSURE_LEDGER_SCHEMA.into(),
            research_family_id: research_family_id.into(),
            alpha_plan_digest: alpha_plan_digest.into(),
            query_limit,
            entries: Vec::new(),
        };
        ledger.validate_restore()?;
        Ok(ledger)
    }

    pub fn reserve(
        &mut self,
        ticket_id: impl Into<String>,
        manifest: &HoldoutManifest,
    ) -> Result<()> {
        self.validate_restore()?;
        manifest.validate()?;
        if self.research_family_id != manifest.research_family_id
            || self.alpha_plan_digest != manifest.alpha_plan_digest
        {
            return Err(Error::Conflict(
                "manifest belongs to another research-family alpha plan".into(),
            ));
        }
        if self.entries.len() >= self.query_limit as usize {
            return Err(Error::Budget);
        }
        let ticket_id = ticket_id.into();
        identifier(&ticket_id)?;
        if self
            .entries
            .iter()
            .any(|entry| entry.ticket_id == ticket_id)
        {
            return Err(Error::Conflict("ticket already reserved".into()));
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.slice_id == manifest.slice_id)
        {
            return Err(Error::Conflict("slice already reserved or consumed".into()));
        }
        let occupied = self.occupied_clusters();
        if manifest
            .ordered_task_cluster_ids
            .iter()
            .any(|cluster| occupied.contains(cluster.as_str()))
        {
            return Err(Error::Conflict(
                "cluster already exposed in this research family".into(),
            ));
        }
        self.entries.push(ExposureEntry {
            ticket_id,
            manifest_digest: manifest.digest()?,
            candidate_pair_digest: manifest.candidate_pair_digest.clone(),
            epoch_id: manifest.epoch_id.clone(),
            slice_id: manifest.slice_id.clone(),
            task_cluster_ids: manifest.ordered_task_cluster_ids.clone(),
            state: ExposureState::Reserved,
            monetary_state: MonetaryReservationState::HeldUndispatched,
        });
        Ok(())
    }

    /// Dispatch consumes the complete slice, including members later skipped
    /// by early stopping. Epoch changes never clear this history.
    pub fn mark_dispatched(&mut self, ticket_id: &str, at_unix_ms: i64) -> Result<()> {
        if at_unix_ms < 0 {
            return Err(Error::Invalid("invalid dispatch time".into()));
        }
        let entry = self.entry_mut(ticket_id)?;
        if !matches!(entry.state, ExposureState::Reserved)
            || entry.monetary_state != MonetaryReservationState::HeldUndispatched
        {
            return Err(Error::Conflict(
                "ticket cannot be dispatched from this state".into(),
            ));
        }
        entry.state = ExposureState::Dispatched {
            dispatched_at_unix_ms: at_unix_ms,
        };
        entry.monetary_state = MonetaryReservationState::DispatchedCostPending;
        Ok(())
    }

    pub fn mark_feedback_used(&mut self, ticket_id: &str, at_unix_ms: i64) -> Result<()> {
        if at_unix_ms < 0 {
            return Err(Error::Invalid("invalid feedback-use time".into()));
        }
        let entry = self.entry_mut(ticket_id)?;
        if !matches!(entry.state, ExposureState::Dispatched { .. }) {
            return Err(Error::Conflict(
                "feedback requires a dispatched slice".into(),
            ));
        }
        entry.state = ExposureState::FeedbackUsed {
            used_at_unix_ms: at_unix_ms,
        };
        Ok(())
    }

    pub fn release_undispatched_money(&mut self, ticket_id: &str) -> Result<()> {
        let entry = self.entry_mut(ticket_id)?;
        if !matches!(entry.state, ExposureState::Reserved)
            || entry.monetary_state != MonetaryReservationState::HeldUndispatched
        {
            return Err(Error::Conflict(
                "only a proven undispatched monetary reservation may be released".into(),
            ));
        }
        entry.monetary_state = MonetaryReservationState::ReleasedUndispatched;
        Ok(())
    }

    pub fn finalize_dispatched_cost(&mut self, ticket_id: &str) -> Result<()> {
        let entry = self.entry_mut(ticket_id)?;
        if !matches!(
            entry.state,
            ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. }
        ) || entry.monetary_state != MonetaryReservationState::DispatchedCostPending
        {
            return Err(Error::Conflict(
                "only dispatched pending cost may finalize".into(),
            ));
        }
        entry.monetary_state = MonetaryReservationState::Finalized;
        Ok(())
    }

    pub fn validate_restore(&self) -> Result<()> {
        if self.schema_version != EXPOSURE_LEDGER_SCHEMA {
            return Err(Error::Invalid("unsupported exposure-ledger schema".into()));
        }
        identifier(&self.research_family_id)?;
        digest(&self.alpha_plan_digest, "alpha_plan_digest")?;
        if self.query_limit == 0 || self.entries.len() > self.query_limit as usize {
            return Err(Error::Invalid(
                "exposure ledger exceeds its query limit".into(),
            ));
        }
        let mut tickets = BTreeSet::new();
        let mut slices = BTreeSet::new();
        let mut occupied_clusters = BTreeSet::new();
        for entry in &self.entries {
            identifier(&entry.ticket_id)?;
            identifier(&entry.epoch_id)?;
            identifier(&entry.slice_id)?;
            digest(&entry.manifest_digest, "manifest_digest")?;
            digest(&entry.candidate_pair_digest, "candidate_pair_digest")?;
            if !tickets.insert(&entry.ticket_id) || !slices.insert(&entry.slice_id) {
                return Err(Error::Invalid(
                    "duplicate ticket or slice in restored ledger".into(),
                ));
            }
            if entry.task_cluster_ids.is_empty() {
                return Err(Error::Invalid("exposure entry lacks task clusters".into()));
            }
            for cluster_id in &entry.task_cluster_ids {
                identifier(cluster_id)?;
                if !occupied_clusters.insert(cluster_id) {
                    return Err(Error::Invalid(
                        "restored ledger reuses a reserved or exposed cluster across epochs".into(),
                    ));
                }
            }
            match (&entry.state, entry.monetary_state) {
                (ExposureState::Reserved, MonetaryReservationState::HeldUndispatched)
                | (ExposureState::Reserved, MonetaryReservationState::ReleasedUndispatched)
                | (
                    ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. },
                    MonetaryReservationState::DispatchedCostPending
                    | MonetaryReservationState::Finalized,
                ) => {}
                _ => {
                    return Err(Error::Invalid(
                        "inconsistent restored exposure state".into(),
                    ));
                }
            }
            match entry.state {
                ExposureState::Dispatched {
                    dispatched_at_unix_ms,
                } if dispatched_at_unix_ms < 0 => {
                    return Err(Error::Invalid("negative dispatch timestamp".into()));
                }
                ExposureState::FeedbackUsed { used_at_unix_ms } if used_at_unix_ms < 0 => {
                    return Err(Error::Invalid("negative feedback timestamp".into()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn is_cluster_consumed(&self, cluster_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            !matches!(entry.state, ExposureState::Reserved)
                && entry.task_cluster_ids.iter().any(|id| id == cluster_id)
        })
    }

    pub fn query_attempts_reserved(&self) -> usize {
        self.entries.len()
    }

    fn occupied_clusters(&self) -> BTreeSet<&str> {
        self.entries
            .iter()
            .flat_map(|entry| entry.task_cluster_ids.iter().map(String::as_str))
            .collect()
    }

    fn entry_mut(&mut self, ticket_id: &str) -> Result<&mut ExposureEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.ticket_id == ticket_id)
            .ok_or(Error::NotFound)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochStatus {
    Open,
    ClosedNoEligibleSlices,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochPolicy {
    pub epoch_id: String,
    pub status: EpochStatus,
    pub eligible_slice_count: u32,
}

impl EpochPolicy {
    pub fn open(epoch_id: impl Into<String>, eligible_slice_count: u32) -> Result<Self> {
        let epoch_id = epoch_id.into();
        identifier(&epoch_id)?;
        Ok(Self {
            epoch_id,
            status: EpochStatus::Open,
            eligible_slice_count,
        })
    }

    pub fn issue_slice(&mut self) -> Result<u32> {
        if self.status != EpochStatus::Open {
            return Err(Error::Conflict("epoch closed; no new tickets".into()));
        }
        if self.eligible_slice_count == 0 {
            self.status = EpochStatus::ClosedNoEligibleSlices;
            return Err(Error::Conflict("no eligible independent slices".into()));
        }
        self.eligible_slice_count -= 1;
        Ok(self.eligible_slice_count)
    }

    pub fn cross_epoch_score_delta(_old: f64, _new: f64) -> Result<f64> {
        Err(Error::Invalid(
            "cross-epoch absolute score deltas are not gains".into(),
        ))
    }
}
