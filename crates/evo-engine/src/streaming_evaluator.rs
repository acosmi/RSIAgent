//! Persistent v4.1 independent evaluation control plane.
//! Stored role/evidence bindings are rechecked at every transition; caller JSON
//! and replay/self-report artifacts cannot mint a formal evaluation.
use crate::evidence::load_stored_source;
use crate::release_store::{
    RELEASE_CANDIDATE_SCHEMA, ReleaseCandidateRecord, TypedSourceRef,
    validate_candidate_read_in_session,
};
use crate::revocation_gate::{MAX_UPSTREAM_CLOSURE_NODES, judge_dependency};
use evo_core::evaluation::{
    BernsteinReport, ClusterObservation, ExperimentPlan, FormalExperimentPlanV41,
    OptimizationBudgetPlan, OptimizerComparisonContract, ProfileKind, Verdict, decide,
    empirical_bernstein, parse_decimal,
};
use evo_core::holdout::{
    AnchorCoverageMatrix, AnchorResult, ExposureLedger, ExposureState, FrozenCandidatePair,
    HoldoutManifest, MonetaryReservationState, SequentialMode,
};
use evo_core::sequential::{
    CompletePairedUnit, EarlyStopCertificate, EarlyStopCertificateParts, EarlyStopDecision,
    EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan, SequentialRejectOnly,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
use evo_storage::budget::{
    BudgetCallFence, BudgetCallRecord, BudgetCallState, BudgetDispatchDecision, BudgetStage,
};
use evo_storage::{Session, Store};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, Error as DeError, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};

const CONTROL_KIND: &str = "registered_evaluation_control_v41";
const HOLDOUT_KIND: &str = "protected_holdout_v41";
const LEDGER_KIND: &str = "exposure_ledger_v1";
const ALPHA_USE_KIND: &str = "alpha_claim_reservation_v1";
const TICKET_KIND: &str = "evaluation_ticket_v2";
const TICKET_RECEIPT_KIND: &str = "evaluation_ticket_issue_receipt_v1";
const EXECUTION_RECEIPT_KIND: &str = "execution_receipt_v2";
const GRADER_RECEIPT_KIND: &str = "independent_grader_receipt_v2";
const FORMAL_KIND: &str = "formal_evaluation_v2";
const OUTPUT_KIND: &str = "evaluation_execution_output_v1";
const RESOURCE_KIND: &str = "evaluation_resource_evidence_v1";
const ENVELOPE_SCHEMA: &str = "rsia.typed_artifact_envelope.v1";
const STATISTICS_CHANGED: &str = "formal_complete_batch_statistics_changed";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypedArtifactEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

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

fn valid_time(value: i64) -> Result<()> {
    if value < 0 {
        return Err(Error::Invalid("timestamp must be nonnegative".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredEvaluationControl {
    pub schema_version: String,
    pub id: String,
    pub v1_plan_snapshot: ExperimentPlan,
    pub formal_plan: FormalExperimentPlanV41,
    pub alpha_plan: ResearchFamilyAlphaPlan,
    pub optimizer_comparison: OptimizerComparisonContract,
    pub optimization_budget: OptimizationBudgetPlan,
    pub early_stop_plan: Option<EarlyStopPlan>,
    pub anchor_matrix: AnchorCoverageMatrix,
    pub attempt_id: String,
    pub fixed_sample_claim_id: String,
    pub billing_scope: String,
    pub executor_actor: String,
    pub evaluator_actor: String,
    pub proposer_actor: String,
    pub approver_actor: String,
    pub oracle_digest: String,
    pub grader_digest: String,
    pub fixed_grader: FixedGraderSpec,
    pub evidence_scope: EvaluationEvidenceScope,
    pub created_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationEvidenceScope {
    /// Local deterministic program fixture. It cannot authorize production.
    ProgramFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostEvidenceScope {
    /// Only calls directly attached to this acceptance ticket were reconciled.
    TicketExecutionOnly,
}

impl RegisteredEvaluationControl {
    /// Frozen controls retain the original blanket-inconclusive postprocessing.
    pub const SCHEMA_V1: &'static str = "rsia.registered_evaluation_control.v1";
    /// Explicitly selects verdict preservation without changing the core algorithm.
    pub const SCHEMA: &'static str = "rsia.registered_evaluation_control.v2";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA_V1 && self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported registered-control schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.attempt_id,
            &self.fixed_sample_claim_id,
            &self.billing_scope,
            &self.executor_actor,
            &self.evaluator_actor,
            &self.proposer_actor,
            &self.approver_actor,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.oracle_digest, "oracle_digest"),
            (&self.grader_digest, "grader_digest"),
        ] {
            digest(value, name)?;
        }
        self.fixed_grader.validate()?;
        if self.grader_digest != fingerprint(&self.fixed_grader)? {
            return Err(Error::Invalid(
                "grader_digest does not identify the frozen grader".into(),
            ));
        }
        valid_time(self.created_at_unix_seconds)?;
        let actors: BTreeSet<_> = [
            self.executor_actor.as_str(),
            self.evaluator_actor.as_str(),
            self.proposer_actor.as_str(),
            self.approver_actor.as_str(),
        ]
        .into_iter()
        .collect();
        if actors.len() != 4 {
            return Err(Error::Invalid(
                "executor, evaluator, proposer, and approver must be distinct".into(),
            ));
        }
        self.formal_plan.validate_against(
            &self.v1_plan_snapshot,
            &self.alpha_plan,
            &self.optimizer_comparison,
            &self.optimization_budget,
        )?;
        self.alpha_plan.validate()?;
        self.anchor_matrix.validate()?;
        if self.formal_plan.v1_plan_snapshot_digest != self.v1_plan_snapshot.digest()?
            || self.formal_plan.alpha_plan_digest != self.alpha_plan.digest()?
            || self.formal_plan.anchor_coverage_digest != self.anchor_matrix.digest()?
            || self.formal_plan.research_family_id != self.alpha_plan.research_family_id
        {
            return Err(Error::Invalid(
                "registered control has inconsistent plan dependencies".into(),
            ));
        }
        self.alpha_plan.allocation(
            &self.fixed_sample_claim_id,
            &self.attempt_id,
            FormalClaimKind::FixedSampleGain,
        )?;
        if let Some(early) = &self.early_stop_plan {
            early.validate_against(&self.alpha_plan)?;
            if early.v1_plan_snapshot_digest != self.formal_plan.v1_plan_snapshot_digest
                || early.anchor_coverage_digest != self.formal_plan.anchor_coverage_digest
                || early.attempt_id != self.attempt_id
                || early.n_planned != self.formal_plan.n_planned
                || early.profile != self.formal_plan.profile
            {
                return Err(Error::Invalid(
                    "early-stop plan differs from the registered formal plan".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixedGraderMethod {
    ExactJsonAnswerV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedGraderSpec {
    pub schema_version: String,
    pub version: String,
    pub method: FixedGraderMethod,
}

impl FixedGraderSpec {
    pub const SCHEMA: &'static str = "rsia.fixed_grader.v1";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid("unsupported fixed grader schema".into()));
        }
        identifier(&self.version)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenOracleEntry {
    pub target_id: String,
    pub expected_answer_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedInputRef {
    pub target_id: String,
    pub cluster_or_anchor_id: String,
    pub input_artifact_id: String,
    pub input_digest: String,
}

impl ProtectedInputRef {
    fn validate(&self) -> Result<()> {
        identifier(&self.target_id)?;
        identifier(&self.cluster_or_anchor_id)?;
        identifier(&self.input_artifact_id)?;
        digest(&self.input_digest, "input_digest")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedHoldoutRecord {
    pub schema_version: String,
    pub id: String,
    pub registration_id: String,
    pub pair: FrozenCandidatePair,
    pub manifest: HoldoutManifest,
    pub candidate_bundle_digest: String,
    pub baseline_bundle_digest: String,
    pub rotating_inputs: Vec<ProtectedInputRef>,
    pub anchor_inputs: Vec<ProtectedInputRef>,
    pub oracle_entries: Vec<FrozenOracleEntry>,
    pub oracle_payload_digest: String,
}

impl ProtectedHoldoutRecord {
    pub const SCHEMA: &'static str = "rsia.protected_holdout_record.v1";

    pub fn validate_against(&self, control: &RegisteredEvaluationControl) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported protected-holdout schema".into(),
            ));
        }
        identifier(&self.id)?;
        identifier(&self.registration_id)?;
        if self.registration_id != control.id {
            return Err(Error::Conflict(
                "holdout belongs to another registration".into(),
            ));
        }
        self.pair.validate()?;
        self.manifest.validate()?;
        for (value, name) in [
            (&self.candidate_bundle_digest, "candidate_bundle_digest"),
            (&self.baseline_bundle_digest, "baseline_bundle_digest"),
            (&self.oracle_payload_digest, "oracle_payload_digest"),
        ] {
            digest(value, name)?;
        }
        if self.pair.v1_plan_snapshot_digest != control.formal_plan.v1_plan_snapshot_digest
            || self.manifest.v1_plan_snapshot_digest != control.formal_plan.v1_plan_snapshot_digest
            || self.manifest.alpha_plan_digest != control.alpha_plan.digest()?
            || self.manifest.research_family_id != control.alpha_plan.research_family_id
            || self.manifest.candidate_pair_digest != self.pair.digest()?
            || self.manifest.anchor_coverage_digest != control.anchor_matrix.digest()?
            || self.manifest.oracle_version != control.oracle_digest
            || self.manifest.grader_digest != control.grader_digest
        {
            return Err(Error::Invalid(
                "holdout manifest does not bind the registered control".into(),
            ));
        }
        match (&control.early_stop_plan, &self.manifest.sequential_mode) {
            (None, SequentialMode::Disabled) => {}
            (Some(plan), SequentialMode::RejectOnly { .. }) => {
                self.manifest.validate_sequential_binding(plan)?;
            }
            _ => {
                return Err(Error::Invalid(
                    "holdout sequential mode differs from the registered plan".into(),
                ));
            }
        }
        if self.rotating_inputs.len() != self.manifest.ordered_task_cluster_ids.len() {
            return Err(Error::Invalid("rotating input list is incomplete".into()));
        }
        let mut target_ids = BTreeSet::new();
        for (index, input) in self.rotating_inputs.iter().enumerate() {
            input.validate()?;
            if input.cluster_or_anchor_id != self.manifest.ordered_task_cluster_ids[index]
                || !target_ids.insert(input.target_id.as_str())
            {
                return Err(Error::Invalid(
                    "rotating inputs must follow the frozen cluster order".into(),
                ));
            }
        }
        let required_anchors = control.anchor_matrix.anchor_ids();
        let mut actual_anchors = BTreeSet::new();
        for input in &self.anchor_inputs {
            input.validate()?;
            if !target_ids.insert(input.target_id.as_str())
                || !actual_anchors.insert(input.cluster_or_anchor_id.as_str())
            {
                return Err(Error::Invalid("duplicate protected input target".into()));
            }
        }
        if actual_anchors != required_anchors {
            return Err(Error::Invalid("anchor input coverage is incomplete".into()));
        }
        let expected_targets: BTreeSet<_> = self
            .rotating_inputs
            .iter()
            .chain(self.anchor_inputs.iter())
            .map(|input| input.target_id.as_str())
            .collect();
        let mut oracle_targets = BTreeSet::new();
        for entry in &self.oracle_entries {
            identifier(&entry.target_id)?;
            parse_strict_json(&entry.expected_answer_json)?;
            if !oracle_targets.insert(entry.target_id.as_str()) {
                return Err(Error::Invalid("duplicate frozen oracle entry".into()));
            }
        }
        if oracle_targets != expected_targets
            || self.oracle_payload_digest != fingerprint(&self.oracle_entries)?
        {
            return Err(Error::Invalid(
                "frozen oracle does not cover the exact ticket targets".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self, control: &RegisteredEvaluationControl) -> Result<String> {
        self.validate_against(control)?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum EvaluationTargetKind {
    Rotating { cluster_id: String },
    Anchor { anchor_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum TargetState {
    Planned,
    Running {
        candidate: Option<ExecutionSlotBinding>,
        baseline: Option<ExecutionSlotBinding>,
    },
    Completed {
        candidate_score_micros: u32,
        baseline_score_micros: u32,
        anchor_result: Option<AnchorResult>,
        candidate_receipt_id: String,
        baseline_receipt_id: String,
        grader_receipt_id: String,
        completion_sequence: u64,
    },
    PlannedFailure {
        reason: String,
    },
    EnvironmentInvalid {
        reason: String,
    },
    NotDispatched {
        released_call_ids: Vec<String>,
    },
    CancelledAfterDispatch {
        candidate: Option<ExecutionSlotBinding>,
        baseline: Option<ExecutionSlotBinding>,
    },
    RunningUsageUnknown {
        candidate: Option<ExecutionSlotBinding>,
        baseline: Option<ExecutionSlotBinding>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase", deny_unknown_fields)]
pub enum ExecutionSlotPhase {
    ReservedBound,
    Dispatched {
        dispatch_id: String,
        dispatched_at_unix_seconds: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSlotBinding {
    pub side: ExecutionSide,
    pub budget_call_id: String,
    pub request_digest: String,
    pub phase: ExecutionSlotPhase,
    pub receipt_id: Option<String>,
}

impl ExecutionSlotBinding {
    fn validate(&self) -> Result<()> {
        identifier(&self.budget_call_id)?;
        digest(&self.request_digest, "slot request_digest")?;
        if let Some(receipt_id) = &self.receipt_id {
            identifier(receipt_id)?;
            if !matches!(self.phase, ExecutionSlotPhase::Dispatched { .. }) {
                return Err(Error::Invalid(
                    "receipt cannot bind an undispatched execution slot".into(),
                ));
            }
        }
        if let ExecutionSlotPhase::Dispatched {
            dispatch_id,
            dispatched_at_unix_seconds,
        } = &self.phase
        {
            identifier(dispatch_id)?;
            valid_time(*dispatched_at_unix_seconds)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationTarget {
    pub target_id: String,
    pub kind: EvaluationTargetKind,
    pub input_artifact_id: String,
    pub input_digest: String,
    pub state: TargetState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketLifecycle {
    Reserved,
    Running,
    ReadyForFinal,
    StopRequested,
    Draining,
    EarlyStopped,
    CompleteBatch,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationTicketV2 {
    pub schema_version: String,
    pub id: String,
    pub registration_id: String,
    pub registration_digest: String,
    pub holdout_id: String,
    pub manifest_digest: String,
    pub v1_plan_snapshot_digest: String,
    pub bound_v1_plan_digest: String,
    pub formal_plan_digest: String,
    pub profile: evo_core::evaluation::ProfileKind,
    pub n_planned: u32,
    pub research_family_id: String,
    pub alpha_plan_digest: String,
    pub candidate_digest: String,
    pub baseline_digest: String,
    pub candidate_bundle_digest: String,
    pub baseline_bundle_digest: String,
    pub environment_digest: String,
    pub proposer_actor: String,
    pub evaluator_actor: String,
    pub executor_actor: String,
    pub approver_actor: String,
    pub issue_receipt_digest: String,
    pub evidence_scope: EvaluationEvidenceScope,
    pub attempt_id: String,
    pub targets: Vec<EvaluationTarget>,
    pub lifecycle: TicketLifecycle,
    pub next_sequence: u64,
    pub pending_certificate: Option<EarlyStopCertificate>,
    pub stop_member_terminal_digest: Option<String>,
    pub late_execution_receipt_ids: Vec<String>,
    pub late_grader_receipt_ids: Vec<String>,
    pub unattributed_budget_call_ids: Vec<String>,
    pub invalid_reasons: Vec<String>,
    pub issued_at_unix_seconds: i64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_candidate_binding"
    )]
    pub candidate_dependency_binding: Option<FormalCandidateDependencyBinding>,
}

impl EvaluationTicketV2 {
    pub const SCHEMA: &'static str = "rsia.evaluation_ticket.v2";

    fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported evaluation-ticket schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.registration_id,
            &self.holdout_id,
            &self.research_family_id,
            &self.proposer_actor,
            &self.evaluator_actor,
            &self.executor_actor,
            &self.approver_actor,
            &self.attempt_id,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.registration_digest, "registration_digest"),
            (&self.manifest_digest, "manifest_digest"),
            (&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest"),
            (&self.bound_v1_plan_digest, "bound_v1_plan_digest"),
            (&self.formal_plan_digest, "formal_plan_digest"),
            (&self.alpha_plan_digest, "alpha_plan_digest"),
            (&self.candidate_digest, "candidate_digest"),
            (&self.baseline_digest, "baseline_digest"),
            (&self.candidate_bundle_digest, "candidate_bundle_digest"),
            (&self.baseline_bundle_digest, "baseline_bundle_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.issue_receipt_digest, "issue_receipt_digest"),
        ] {
            digest(value, name)?;
        }
        if let Some(binding) = &self.candidate_dependency_binding {
            binding.validate()?;
        }
        valid_time(self.issued_at_unix_seconds)?;
        if self.targets.is_empty() || self.next_sequence == 0 {
            return Err(Error::Invalid("ticket has no planned targets".into()));
        }
        let mut ids = BTreeSet::new();
        for target in &self.targets {
            identifier(&target.target_id)?;
            identifier(&target.input_artifact_id)?;
            digest(&target.input_digest, "target input_digest")?;
            if !ids.insert(&target.target_id) {
                return Err(Error::Invalid("duplicate ticket target".into()));
            }
            match &target.state {
                TargetState::Running {
                    candidate,
                    baseline,
                }
                | TargetState::RunningUsageUnknown {
                    candidate,
                    baseline,
                }
                | TargetState::CancelledAfterDispatch {
                    candidate,
                    baseline,
                } => {
                    for slot in [candidate, baseline].into_iter().flatten() {
                        slot.validate()?;
                    }
                }
                TargetState::Completed {
                    candidate_score_micros,
                    baseline_score_micros,
                    candidate_receipt_id,
                    baseline_receipt_id,
                    grader_receipt_id,
                    completion_sequence,
                    ..
                } => {
                    if *candidate_score_micros > 1_000_000
                        || *baseline_score_micros > 1_000_000
                        || *completion_sequence == 0
                    {
                        return Err(Error::Invalid("invalid completed target state".into()));
                    }
                    for receipt_id in [candidate_receipt_id, baseline_receipt_id, grader_receipt_id]
                    {
                        identifier(receipt_id)?;
                    }
                }
                TargetState::NotDispatched { released_call_ids } => {
                    for call_id in released_call_ids {
                        identifier(call_id)?;
                    }
                }
                _ => {}
            }
        }
        for call_id in &self.unattributed_budget_call_ids {
            identifier(call_id)?;
        }
        if let Some(stop_digest) = &self.stop_member_terminal_digest {
            digest(stop_digest, "stop_member_terminal_digest")?;
        }
        Ok(())
    }

    fn target(&self, target_id: &str) -> Result<&EvaluationTarget> {
        self.targets
            .iter()
            .find(|target| target.target_id == target_id)
            .ok_or(Error::NotFound)
    }

    fn target_mut(&mut self, target_id: &str) -> Result<&mut EvaluationTarget> {
        self.targets
            .iter_mut()
            .find(|target| target.target_id == target_id)
            .ok_or(Error::NotFound)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketIssueReceipt {
    schema_version: String,
    ticket_id: String,
    registration_digest: String,
    manifest_digest: String,
    evaluator_actor: String,
    issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlphaClaimReservation {
    schema_version: String,
    research_family_id: String,
    claim_id: String,
    attempt_id: String,
    ticket_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSide {
    Candidate,
    Baseline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerTaskView {
    pub ticket_id: String,
    pub target_id: String,
    pub input_artifact_id: String,
    pub input_digest: String,
    pub side: ExecutionSide,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub request_digest: String,
}

#[derive(Debug, Clone)]
pub struct ExecutionReceiptRequest {
    pub receipt_id: String,
    pub ticket_id: String,
    pub target_id: String,
    pub side: ExecutionSide,
    pub request_digest: String,
    pub output_artifact_id: String,
    pub output_utf8: String,
    pub budget_call_id: String,
    pub latency_micros: u64,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct StartExecutionRequest {
    pub ticket_id: String,
    pub target_id: String,
    pub side: ExecutionSide,
    pub fence: BudgetCallFence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReceiptV2 {
    pub schema_version: String,
    pub receipt_id: String,
    pub ticket_id: String,
    pub target_id: String,
    pub side: ExecutionSide,
    pub request_digest: String,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub output_digest: String,
    pub output_artifact_id: String,
    pub budget_call_id: String,
    pub executor_actor: String,
    pub latency_micros: u64,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct GraderReceiptRequest {
    pub receipt_id: String,
    pub ticket_id: String,
    pub target_id: String,
    pub candidate_execution_receipt_id: String,
    pub baseline_execution_receipt_id: String,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionOutputArtifact {
    schema_version: String,
    id: String,
    ticket_id: String,
    target_id: String,
    side: ExecutionSide,
    output_utf8: String,
    output_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependentGraderReceiptV2 {
    pub schema_version: String,
    pub receipt_id: String,
    pub ticket_id: String,
    pub target_id: String,
    pub candidate_execution_receipt_id: String,
    pub baseline_execution_receipt_id: String,
    pub candidate_output_digest: String,
    pub baseline_output_digest: String,
    pub oracle_digest: String,
    pub grader_digest: String,
    pub scored_output_pair_digest: String,
    pub candidate_score_micros: u32,
    pub baseline_score_micros: u32,
    pub anchor_result: Option<AnchorResult>,
    pub evaluator_actor: String,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BernsteinReportWire {
    pub stats_version: String,
    pub verdict: Verdict,
    pub n: usize,
    pub mean: String,
    pub variance: String,
    pub radius: String,
    pub lcb: String,
    pub alpha_i: String,
    pub reasons: Vec<String>,
}

impl BernsteinReportWire {
    fn from_report(report: &BernsteinReport) -> Result<Self> {
        Ok(Self {
            stats_version: report.stats_version.clone(),
            verdict: report.verdict,
            n: report.n,
            mean: finite_decimal(report.mean)?,
            variance: finite_decimal(report.variance)?,
            radius: finite_decimal(report.radius)?,
            lcb: finite_decimal(report.lcb)?,
            alpha_i: finite_decimal(report.alpha_i)?,
            reasons: report.reasons.clone(),
        })
    }
}

fn finite_decimal(value: f64) -> Result<String> {
    if !value.is_finite() {
        return Err(Error::Invalid("non-finite formal statistic".into()));
    }
    let mut rendered = format!("{value:.17}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    if rendered == "-0" {
        rendered = "0".into();
    }
    Ok(rendered)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceEvidenceRecord {
    schema_version: String,
    ticket_id: String,
    billing_scope: String,
    cost_scope: CostEvidenceScope,
    calls: Vec<BudgetCallRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "variant", deny_unknown_fields)]
pub enum FormalEvaluationV2 {
    CompleteBatch {
        evidence_scope: EvaluationEvidenceScope,
        cost_evidence_scope: CostEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        manifest_digest: String,
        candidate_bundle_digest: String,
        baseline_bundle_digest: String,
        environment_digest: String,
        verdict: Verdict,
        report: BernsteinReportWire,
        anchor_evidence_digest: String,
        resource_evidence_digest: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_stored_binding"
        )]
        stored_dependency_binding: Option<FormalStoredDependencyBinding>,
    },
    EarlyStopped {
        evidence_scope: EvaluationEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        certificate: EarlyStopCertificate,
    },
    Invalid {
        evidence_scope: EvaluationEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        reasons: Vec<String>,
    },
}

impl FormalEvaluationV2 {
    pub fn is_promotable(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct IssueTicketRequest {
    pub ticket_id: String,
    pub registration_id: String,
    pub holdout_id: String,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamingEvaluationStatus {
    pub ticket: EvaluationTicketV2,
    pub formal: Option<FormalEvaluationV2>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedReportVariant {
    CompleteBatch,
    EarlyStopped,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorEvidenceStatus {
    CompletePassedStoredClosure,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyEvidenceStatus {
    NamespaceWatermarkOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedFormalReportView {
    pub report_id: String,
    pub report_digest: String,
    pub variant: VerifiedReportVariant,
    pub verdict: Option<Verdict>,
    pub ticket_id: String,
    pub ticket_digest: String,
    pub v1_plan_snapshot_digest: String,
    pub formal_plan_digest: String,
    pub manifest_digest: String,
    pub candidate_digest: String,
    pub baseline_parent_digest: String,
    pub candidate_bundle_digest: String,
    pub baseline_bundle_digest: String,
    pub environment_digest: String,
    pub profile: evo_core::evaluation::ProfileKind,
    pub proposer_actor: String,
    pub evaluator_actor: String,
    pub approver_actor: String,
    pub anchor_evidence_digest: Option<String>,
    pub resource_evidence_digest: Option<String>,
    pub anchor_status: AnchorEvidenceStatus,
    pub evidence_scope: EvaluationEvidenceScope,
    pub cost_evidence_scope: Option<CostEvidenceScope>,
    pub revoke_watermark: Option<(i64, String)>,
    pub dependency_status: DependencyEvidenceStatus,
    pub promotion_eligible: bool,
    pub ineligibility_reasons: Vec<String>,
}

pub struct IndependentEvaluationControl;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationProgress {
    pub control_digest: Option<String>,
    pub holdout_digest: Option<String>,
}

impl IndependentEvaluationControl {
    pub async fn registration_progress(
        ctx: &Context,
        store: &Store,
        control_id: &str,
        holdout_id: &str,
    ) -> Result<RegistrationProgress> {
        ctx.require(&[Role::Evaluator])?;
        identifier(control_id)?;
        identifier(holdout_id)?;
        let mut session = store.session().await?;
        let control =
            get_record::<RegisteredEvaluationControl>(&mut session, ctx, CONTROL_KIND, control_id)
                .await?;
        let holdout =
            get_record::<ProtectedHoldoutRecord>(&mut session, ctx, HOLDOUT_KIND, holdout_id)
                .await?;
        let progress = match (control, holdout) {
            (None, None) => RegistrationProgress {
                control_digest: None,
                holdout_digest: None,
            },
            (None, Some(_)) => {
                return Err(Error::Conflict(
                    "holdout exists without its registered control".into(),
                ));
            }
            (Some(control), holdout) => {
                control.validate()?;
                if control.id != control_id || control.evaluator_actor != ctx.actor() {
                    return Err(Error::Forbidden);
                }
                let control_digest = control.digest()?;
                let holdout_digest = match holdout {
                    Some(holdout) => {
                        if holdout.id != holdout_id || holdout.registration_id != control_id {
                            return Err(Error::Conflict(
                                "holdout registration binding differs".into(),
                            ));
                        }
                        holdout.validate_against(&control)?;
                        Some(holdout.digest(&control)?)
                    }
                    None => None,
                };
                RegistrationProgress {
                    control_digest: Some(control_digest),
                    holdout_digest,
                }
            }
        };
        session.commit().await?;
        Ok(progress)
    }

    pub async fn register_control(
        ctx: &Context,
        store: &Store,
        control: RegisteredEvaluationControl,
    ) -> Result<String> {
        ctx.require(&[Role::Evaluator])?;
        control.validate()?;
        if control.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        validate_root_budget(ctx, store, &control).await?;
        let mut session = store.session().await?;
        if get_record::<RegisteredEvaluationControl>(&mut session, ctx, CONTROL_KIND, &control.id)
            .await?
            .is_some()
        {
            return Err(Error::Conflict(
                "evaluation control already registered".into(),
            ));
        }
        let digest = control.digest()?;
        put_record(
            &mut session,
            ctx,
            CONTROL_KIND,
            &control.id,
            ctx.actor(),
            &control,
        )
        .await?;
        session
            .audit(ctx, "register_evaluation_control", &control.id)
            .await?;
        session.commit().await?;
        Ok(digest)
    }

    pub async fn register_holdout(
        ctx: &Context,
        store: &Store,
        holdout: ProtectedHoldoutRecord,
    ) -> Result<String> {
        ctx.require(&[Role::Evaluator])?;
        let mut session = store.session().await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &holdout.registration_id).await?;
        if control.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        holdout.validate_against(&control)?;
        if get_record::<ProtectedHoldoutRecord>(&mut session, ctx, HOLDOUT_KIND, &holdout.id)
            .await?
            .is_some()
        {
            return Err(Error::Conflict("holdout already registered".into()));
        }
        let digest = holdout.digest(&control)?;
        put_record(
            &mut session,
            ctx,
            HOLDOUT_KIND,
            &holdout.id,
            ctx.actor(),
            &holdout,
        )
        .await?;
        session
            .put_edge(
                ctx,
                "artifact",
                &storage_id(HOLDOUT_KIND, &holdout.id)?,
                "artifact",
                &storage_id(CONTROL_KIND, &control.id)?,
            )
            .await?;
        session
            .audit(ctx, "register_protected_holdout", &holdout.id)
            .await?;
        session.commit().await?;
        Ok(digest)
    }

    pub async fn issue_ticket(
        ctx: &Context,
        store: &Store,
        request: IssueTicketRequest,
    ) -> Result<EvaluationTicketV2> {
        ctx.require(&[Role::Evaluator])?;
        identifier(&request.ticket_id)?;
        identifier(&request.registration_id)?;
        identifier(&request.holdout_id)?;
        valid_time(request.issued_at_unix_seconds)?;
        let mut session = store.session().await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &request.registration_id).await?;
        let holdout: ProtectedHoldoutRecord =
            need_record(&mut session, ctx, HOLDOUT_KIND, &request.holdout_id).await?;
        if control.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        control.validate()?;
        holdout.validate_against(&control)?;
        let issued_at_unix_ms = request
            .issued_at_unix_seconds
            .checked_mul(1_000)
            .ok_or_else(|| Error::Invalid("ticket timestamp conversion overflow".into()))?;
        if request.issued_at_unix_seconds < control.created_at_unix_seconds
            || issued_at_unix_ms < holdout.manifest.issued_at_unix_ms
        {
            return Err(Error::Invalid(
                "ticket must be issued after control and manifest registration facts".into(),
            ));
        }
        let issue_control_digest = fingerprint(&control)?;
        let issue_holdout_digest = fingerprint(&holdout)?;
        session.commit().await?;
        validate_root_budget(ctx, store, &control).await?;
        let mut session = store.session().await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &request.registration_id).await?;
        let holdout: ProtectedHoldoutRecord =
            need_record(&mut session, ctx, HOLDOUT_KIND, &request.holdout_id).await?;
        control.validate()?;
        holdout.validate_against(&control)?;
        if control.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if fingerprint(&control)? != issue_control_digest
            || fingerprint(&holdout)? != issue_holdout_digest
        {
            return Err(Error::Conflict(
                "registration facts changed during ticket issue".into(),
            ));
        }
        let candidate_dependency_binding =
            bind_candidate_in_session(&mut session, ctx, &control, &holdout).await?;
        if candidate_dependency_binding.is_some() {
            let existing = collect_dependency_nodes(
                &mut session,
                ctx,
                vec![
                    ("artifact".into(), holdout.pair.candidate_digest.clone()),
                    artifact_edge(HOLDOUT_KIND, &holdout.id)?,
                ],
            )
            .await?;
            let new_claims = control
                .alpha_plan
                .allocations
                .iter()
                .filter(|allocation| allocation.attempt_id == control.attempt_id)
                .count();
            // Capacity accounting for this transaction's actual ticket, issue
            // receipt and claim writes is not a content snapshot or future proof.
            if existing.len().saturating_add(2).saturating_add(new_claims)
                > MAX_UPSTREAM_CLOSURE_NODES
            {
                return Err(Error::Conflict(
                    "formal dependency closure exceeds 10000 nodes; refused, not truncated".into(),
                ));
            }
        }
        if get_record::<EvaluationTicketV2>(&mut session, ctx, TICKET_KIND, &request.ticket_id)
            .await?
            .is_some()
        {
            return Err(Error::Conflict("ticket already issued".into()));
        }
        let mut ledger = match get_record::<ExposureLedger>(
            &mut session,
            ctx,
            LEDGER_KIND,
            &control.alpha_plan.research_family_id,
        )
        .await?
        {
            Some(ledger) => ledger,
            None => ExposureLedger::new(
                control.alpha_plan.research_family_id.clone(),
                control.alpha_plan.digest()?,
                control.v1_plan_snapshot.query_limit,
            )?,
        };
        ledger.reserve(request.ticket_id.clone(), &holdout.manifest)?;
        let claim_allocations: Vec<_> = control
            .alpha_plan
            .allocations
            .iter()
            .filter(|allocation| allocation.attempt_id == control.attempt_id)
            .collect();
        if claim_allocations.is_empty() {
            return Err(Error::Invalid(
                "attempt has no preregistered alpha claims".into(),
            ));
        }
        for allocation in claim_allocations {
            let id = format!(
                "{}:{}",
                control.alpha_plan.research_family_id, allocation.claim_id
            );
            if get_record::<AlphaClaimReservation>(&mut session, ctx, ALPHA_USE_KIND, &id)
                .await?
                .is_some()
            {
                return Err(Error::Conflict("alpha claim already reserved".into()));
            }
            let reservation = AlphaClaimReservation {
                schema_version: "rsia.alpha_claim_reservation.v1".into(),
                research_family_id: control.alpha_plan.research_family_id.clone(),
                claim_id: allocation.claim_id.clone(),
                attempt_id: allocation.attempt_id.clone(),
                ticket_id: request.ticket_id.clone(),
            };
            put_record(
                &mut session,
                ctx,
                ALPHA_USE_KIND,
                &id,
                ctx.actor(),
                &reservation,
            )
            .await?;
        }
        let registration_digest = control.digest()?;
        let manifest_digest = holdout.manifest.digest()?;
        let issue_receipt = TicketIssueReceipt {
            schema_version: "rsia.evaluation_ticket_issue_receipt.v1".into(),
            ticket_id: request.ticket_id.clone(),
            registration_digest: registration_digest.clone(),
            manifest_digest: manifest_digest.clone(),
            evaluator_actor: ctx.actor().into(),
            issued_at_unix_seconds: request.issued_at_unix_seconds,
        };
        let issue_receipt_digest = fingerprint(&issue_receipt)?;
        let mut bound_v1_plan = control.v1_plan_snapshot.clone();
        bound_v1_plan.bind_candidate(holdout.pair.candidate_digest.clone())?;
        let rotating = holdout
            .rotating_inputs
            .iter()
            .map(|input| EvaluationTarget {
                target_id: input.target_id.clone(),
                kind: EvaluationTargetKind::Rotating {
                    cluster_id: input.cluster_or_anchor_id.clone(),
                },
                input_artifact_id: input.input_artifact_id.clone(),
                input_digest: input.input_digest.clone(),
                state: TargetState::Planned,
            });
        let anchors = holdout.anchor_inputs.iter().map(|input| EvaluationTarget {
            target_id: input.target_id.clone(),
            kind: EvaluationTargetKind::Anchor {
                anchor_id: input.cluster_or_anchor_id.clone(),
            },
            input_artifact_id: input.input_artifact_id.clone(),
            input_digest: input.input_digest.clone(),
            state: TargetState::Planned,
        });
        let ticket = EvaluationTicketV2 {
            schema_version: EvaluationTicketV2::SCHEMA.into(),
            id: request.ticket_id.clone(),
            registration_id: control.id.clone(),
            registration_digest,
            holdout_id: holdout.id.clone(),
            manifest_digest,
            v1_plan_snapshot_digest: control.formal_plan.v1_plan_snapshot_digest.clone(),
            bound_v1_plan_digest: bound_v1_plan.digest()?,
            formal_plan_digest: control.formal_plan.digest()?,
            profile: control.formal_plan.profile,
            n_planned: control.formal_plan.n_planned,
            research_family_id: control.alpha_plan.research_family_id.clone(),
            alpha_plan_digest: control.alpha_plan.digest()?,
            candidate_digest: holdout.pair.candidate_digest.clone(),
            baseline_digest: holdout.pair.baseline_digest.clone(),
            candidate_bundle_digest: holdout.candidate_bundle_digest.clone(),
            baseline_bundle_digest: holdout.baseline_bundle_digest.clone(),
            environment_digest: holdout.manifest.environment_digest.clone(),
            proposer_actor: control.proposer_actor.clone(),
            evaluator_actor: control.evaluator_actor.clone(),
            executor_actor: control.executor_actor.clone(),
            approver_actor: control.approver_actor.clone(),
            issue_receipt_digest,
            evidence_scope: control.evidence_scope,
            attempt_id: control.attempt_id.clone(),
            targets: rotating.chain(anchors).collect(),
            lifecycle: TicketLifecycle::Reserved,
            next_sequence: 1,
            pending_certificate: None,
            stop_member_terminal_digest: None,
            late_execution_receipt_ids: Vec::new(),
            late_grader_receipt_ids: Vec::new(),
            unattributed_budget_call_ids: Vec::new(),
            invalid_reasons: Vec::new(),
            issued_at_unix_seconds: request.issued_at_unix_seconds,
            candidate_dependency_binding,
        };
        ticket.validate()?;
        put_record(
            &mut session,
            ctx,
            LEDGER_KIND,
            &control.alpha_plan.research_family_id,
            ctx.actor(),
            &ledger,
        )
        .await?;
        put_record(
            &mut session,
            ctx,
            TICKET_RECEIPT_KIND,
            &request.ticket_id,
            ctx.actor(),
            &issue_receipt,
        )
        .await?;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            &request.ticket_id,
            ctx.actor(),
            &ticket,
        )
        .await?;
        session
            .put_edge(
                ctx,
                "artifact",
                &storage_id(TICKET_KIND, &request.ticket_id)?,
                "artifact",
                &storage_id(HOLDOUT_KIND, &holdout.id)?,
            )
            .await?;
        if ticket.candidate_dependency_binding.is_some() {
            let edges = required_ticket_edges(&mut session, ctx, &ticket, &control).await?;
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(TICKET_KIND, &ticket.id)?,
                edges,
            )
            .await?;
        }
        session
            .audit(ctx, "issue_evaluation_ticket", &request.ticket_id)
            .await?;
        session.commit().await?;
        Ok(ticket)
    }

    pub async fn broker_task(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        target_id: &str,
        side: ExecutionSide,
    ) -> Result<BrokerTaskView> {
        ctx.require(&[Role::Worker, Role::Host])?;
        let mut session = store.session().await?;
        let ticket: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, ticket_id).await?;
        if ticket.executor_actor != ctx.actor()
            || matches!(
                ticket.lifecycle,
                TicketLifecycle::StopRequested
                    | TicketLifecycle::Draining
                    | TicketLifecycle::EarlyStopped
                    | TicketLifecycle::CompleteBatch
                    | TicketLifecycle::Invalid
            )
        {
            return Err(Error::Forbidden);
        }
        validate_status_closure(&mut session, ctx, &ticket, None).await?;
        let view = broker_view(&ticket, target_id, side)?;
        session.commit().await?;
        Ok(view)
    }

    pub async fn start_execution(
        ctx: &Context,
        store: &Store,
        request: StartExecutionRequest,
    ) -> Result<BudgetDispatchDecision> {
        ctx.require(&[Role::Worker, Role::Host])?;
        let mut session = store.session().await?;
        let mut ticket: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, &request.ticket_id).await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &ticket.registration_id).await?;
        if ticket.executor_actor != ctx.actor() || is_stopping(ticket.lifecycle) {
            return Err(Error::Forbidden);
        }
        validate_status_closure(&mut session, ctx, &ticket, None).await?;
        let view = broker_view(&ticket, &request.target_id, request.side)?;
        if request.fence.billing_scope != control.billing_scope
            || request.fence.actual_input_digest != view.request_digest
        {
            return Err(Error::Conflict(
                "execution fence differs from the frozen ticket slot".into(),
            ));
        }
        let call = session
            .budget_call(ctx, &control.billing_scope, &request.fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        if call.dispatch_group_id != ticket.id
            || call.stage != BudgetStage::TaskExecution
            || call.actual_input_digest != view.request_digest
            || call.state != BudgetCallState::Reserved
        {
            return Err(Error::Conflict(
                "budget call is not a reserved member of this ticket".into(),
            ));
        }
        bind_slot(
            ticket.target_mut(&request.target_id)?,
            ExecutionSlotBinding {
                side: request.side,
                budget_call_id: call.call_id.clone(),
                request_digest: view.request_digest,
                phase: ExecutionSlotPhase::ReservedBound,
                receipt_id: None,
            },
        )?;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            &ticket.id,
            ticket.evaluator_actor.as_str(),
            &ticket,
        )
        .await?;
        let decision = session.begin_budget_dispatch(ctx, &request.fence).await?;
        let dispatch_id =
            decision.call.dispatch_id.clone().ok_or_else(|| {
                Error::Conflict("budget call did not enter dispatched state".into())
            })?;
        mark_slot_dispatched(
            ticket.target_mut(&request.target_id)?,
            request.side,
            &call.call_id,
            &dispatch_id,
            request.fence.now,
        )?;
        ticket.lifecycle = TicketLifecycle::Running;
        mark_exposure_dispatched_if_needed(&mut session, ctx, &ticket, request.fence.now).await?;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            &ticket.id,
            ticket.evaluator_actor.as_str(),
            &ticket,
        )
        .await?;
        session.commit().await?;
        Ok(decision)
    }

    pub async fn record_execution_receipt(
        ctx: &Context,
        store: &Store,
        request: ExecutionReceiptRequest,
    ) -> Result<ExecutionReceiptV2> {
        ctx.require(&[Role::Worker, Role::Host])?;
        identifier(&request.receipt_id)?;
        identifier(&request.ticket_id)?;
        identifier(&request.target_id)?;
        identifier(&request.budget_call_id)?;
        identifier(&request.output_artifact_id)?;
        digest(&request.request_digest, "request_digest")?;
        if request.output_utf8.is_empty() || request.output_utf8.len() > 256 * 1024 {
            return Err(Error::Invalid(
                "execution output must be 1..=262144 UTF-8 bytes".into(),
            ));
        }
        valid_time(request.issued_at_unix_seconds)?;
        let ticket: EvaluationTicketV2 = load(store, ctx, TICKET_KIND, &request.ticket_id).await?;
        if ticket.executor_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let target = ticket.target(&request.target_id)?;
        let bundle_digest = match request.side {
            ExecutionSide::Candidate => ticket.candidate_bundle_digest.clone(),
            ExecutionSide::Baseline => ticket.baseline_bundle_digest.clone(),
        };
        let expected_request = request_digest(
            &ticket,
            target,
            request.side,
            &bundle_digest,
            &ticket.environment_digest,
        )?;
        if expected_request != request.request_digest {
            return Err(Error::Conflict(
                "execution request differs from the frozen ticket member".into(),
            ));
        }
        let call = store
            .budget_call(
                ctx,
                &control_billing_scope(store, ctx, &ticket).await?,
                &request.budget_call_id,
            )
            .await?
            .ok_or(Error::NotFound)?;
        if call.dispatch_group_id != ticket.id
            || call.stage != BudgetStage::TaskExecution
            || call.actual_input_digest != request.request_digest
            || !matches!(
                call.state,
                BudgetCallState::Dispatched
                    | BudgetCallState::Uncertain
                    | BudgetCallState::Finalized
            )
        {
            return Err(Error::Conflict(
                "execution receipt lacks the matching dispatched budget call".into(),
            ));
        }
        let output_digest = hash(request.output_utf8.as_bytes());
        let output = ExecutionOutputArtifact {
            schema_version: "rsia.execution_output.v1".into(),
            id: request.output_artifact_id.clone(),
            ticket_id: ticket.id.clone(),
            target_id: request.target_id.clone(),
            side: request.side,
            output_utf8: request.output_utf8,
            output_digest: output_digest.clone(),
        };
        let receipt = ExecutionReceiptV2 {
            schema_version: "rsia.execution_receipt.v2".into(),
            receipt_id: request.receipt_id.clone(),
            ticket_id: ticket.id.clone(),
            target_id: request.target_id.clone(),
            side: request.side,
            request_digest: request.request_digest,
            bundle_digest,
            environment_digest: ticket.environment_digest.clone(),
            output_digest,
            output_artifact_id: output.id.clone(),
            budget_call_id: request.budget_call_id,
            executor_actor: ctx.actor().into(),
            latency_micros: request.latency_micros,
            issued_at_unix_seconds: request.issued_at_unix_seconds,
        };
        let mut session = store.session().await?;
        let mut latest: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, &request.ticket_id).await?;
        if !is_stopping(latest.lifecycle) {
            validate_status_closure(&mut session, ctx, &latest, None).await?;
        }
        if let Some(existing) = get_record::<ExecutionReceiptV2>(
            &mut session,
            ctx,
            EXECUTION_RECEIPT_KIND,
            &request.receipt_id,
        )
        .await?
        {
            if fingerprint(&existing)? == fingerprint(&receipt)? {
                let existing_output: ExecutionOutputArtifact =
                    need_record(&mut session, ctx, OUTPUT_KIND, &existing.output_artifact_id)
                        .await?;
                if fingerprint(&existing_output)? != fingerprint(&output)? {
                    return Err(Error::Conflict(
                        "stored execution output differs from idempotent receipt".into(),
                    ));
                }
                return Ok(existing);
            }
            return Err(Error::Conflict(
                "execution receipt id reused with different evidence".into(),
            ));
        }
        if let Some(existing_output) =
            get_record::<ExecutionOutputArtifact>(&mut session, ctx, OUTPUT_KIND, &output.id)
                .await?
        {
            if fingerprint(&existing_output)? != fingerprint(&output)? {
                return Err(Error::Conflict(
                    "execution output id reused with different bytes or binding".into(),
                ));
            }
        } else {
            put_record(
                &mut session,
                ctx,
                OUTPUT_KIND,
                &output.id,
                ctx.actor(),
                &output,
            )
            .await?;
        }
        put_record(
            &mut session,
            ctx,
            EXECUTION_RECEIPT_KIND,
            &request.receipt_id,
            ctx.actor(),
            &receipt,
        )
        .await?;
        if latest.candidate_dependency_binding.is_some() && !is_stopping(latest.lifecycle) {
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(EXECUTION_RECEIPT_KIND, &receipt.receipt_id)?,
                vec![
                    artifact_edge(TICKET_KIND, &latest.id)?,
                    artifact_edge(OUTPUT_KIND, &output.id)?,
                ],
            )
            .await?;
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(TICKET_KIND, &latest.id)?,
                vec![artifact_edge(EXECUTION_RECEIPT_KIND, &receipt.receipt_id)?],
            )
            .await?;
        }
        if is_stopping(latest.lifecycle) {
            validate_started_slot(
                latest.target(&request.target_id)?,
                request.side,
                &receipt.budget_call_id,
                &receipt.request_digest,
            )?;
            latest.late_execution_receipt_ids.push(request.receipt_id);
        } else {
            validate_started_slot(
                latest.target(&request.target_id)?,
                request.side,
                &receipt.budget_call_id,
                &receipt.request_digest,
            )?;
            set_running_receipt(
                latest.target_mut(&request.target_id)?,
                request.side,
                &receipt.receipt_id,
            )?;
            latest.lifecycle = TicketLifecycle::Running;
        }
        latest.validate()?;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            &latest.id,
            latest.evaluator_actor.as_str(),
            &latest,
        )
        .await?;
        if latest.candidate_dependency_binding.is_some() && !is_stopping(latest.lifecycle) {
            collect_dependency_nodes(
                &mut session,
                ctx,
                vec![artifact_edge(TICKET_KIND, &latest.id)?],
            )
            .await?;
        }
        session.commit().await?;
        Ok(receipt)
    }

    pub async fn record_grader_receipt(
        ctx: &Context,
        store: &Store,
        request: GraderReceiptRequest,
    ) -> Result<IndependentGraderReceiptV2> {
        ctx.require(&[Role::Evaluator])?;
        valid_time(request.issued_at_unix_seconds)?;
        let mut session = store.session().await?;
        let mut ticket: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, &request.ticket_id).await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &ticket.registration_id).await?;
        let holdout: ProtectedHoldoutRecord =
            need_record(&mut session, ctx, HOLDOUT_KIND, &ticket.holdout_id).await?;
        if ticket.evaluator_actor != ctx.actor()
            || ctx.actor() == ticket.proposer_actor
            || ctx.actor() == ticket.approver_actor
        {
            return Err(Error::Forbidden);
        }
        let bind_accepted_facts =
            ticket.candidate_dependency_binding.is_some() && !is_stopping(ticket.lifecycle);
        // A delayed grading error cannot replace an already confirmed bound stop.
        let preserve_confirmed_stop = ticket.candidate_dependency_binding.is_some()
            && matches!(
                ticket.lifecycle,
                TicketLifecycle::Draining | TicketLifecycle::EarlyStopped
            )
            && ticket.pending_certificate.is_some();
        if !is_stopping(ticket.lifecycle) {
            validate_status_closure(&mut session, ctx, &ticket, None).await?;
        }
        let candidate: ExecutionReceiptV2 = need_record(
            &mut session,
            ctx,
            EXECUTION_RECEIPT_KIND,
            &request.candidate_execution_receipt_id,
        )
        .await?;
        let baseline: ExecutionReceiptV2 = need_record(
            &mut session,
            ctx,
            EXECUTION_RECEIPT_KIND,
            &request.baseline_execution_receipt_id,
        )
        .await?;
        validate_execution_pair(&ticket, &request.target_id, &candidate, &baseline)?;
        let target_kind = ticket.target(&request.target_id)?.kind.clone();
        let candidate_output: ExecutionOutputArtifact = need_record(
            &mut session,
            ctx,
            OUTPUT_KIND,
            &candidate.output_artifact_id,
        )
        .await?;
        let baseline_output: ExecutionOutputArtifact =
            need_record(&mut session, ctx, OUTPUT_KIND, &baseline.output_artifact_id).await?;
        validate_output_artifact(&candidate, &candidate_output)?;
        validate_output_artifact(&baseline, &baseline_output)?;
        let oracle = holdout
            .oracle_entries
            .iter()
            .find(|entry| entry.target_id == request.target_id)
            .ok_or_else(|| Error::Invalid("frozen oracle entry missing".into()))?;
        let candidate_score_micros = match grade_fixed_output(
            &control.fixed_grader,
            &candidate_output.output_utf8,
            &oracle.expected_answer_json,
        ) {
            Ok(score) => score,
            Err(error) => {
                if !preserve_confirmed_stop {
                    invalidate_in_session(
                        &mut session,
                        ctx,
                        &control,
                        &mut ticket,
                        "candidate_output_schema_invalid",
                        request.issued_at_unix_seconds,
                    )
                    .await?;
                    session.commit().await?;
                }
                return Err(error);
            }
        };
        let baseline_score_micros = match grade_fixed_output(
            &control.fixed_grader,
            &baseline_output.output_utf8,
            &oracle.expected_answer_json,
        ) {
            Ok(score) => score,
            Err(error) => {
                if !preserve_confirmed_stop {
                    invalidate_in_session(
                        &mut session,
                        ctx,
                        &control,
                        &mut ticket,
                        "baseline_output_schema_invalid",
                        request.issued_at_unix_seconds,
                    )
                    .await?;
                    session.commit().await?;
                }
                return Err(error);
            }
        };
        let anchor_result = match target_kind {
            EvaluationTargetKind::Rotating { .. } => None,
            EvaluationTargetKind::Anchor { .. } if baseline_score_micros != 1_000_000 => {
                if !preserve_confirmed_stop {
                    invalidate_in_session(
                        &mut session,
                        ctx,
                        &control,
                        &mut ticket,
                        "baseline_anchor_invalid",
                        request.issued_at_unix_seconds,
                    )
                    .await?;
                    session.commit().await?;
                }
                return Err(Error::Invalid(
                    "baseline failed a frozen critical anchor; comparison is invalid".into(),
                ));
            }
            EvaluationTargetKind::Anchor { .. } if candidate_score_micros == 1_000_000 => {
                Some(AnchorResult::Passed)
            }
            EvaluationTargetKind::Anchor { .. } => Some(AnchorResult::CriticalRegression),
        };
        let scored_output_pair_digest = fingerprint(&(
            &candidate.output_digest,
            &baseline.output_digest,
            &control.oracle_digest,
            &control.grader_digest,
            candidate_score_micros,
            baseline_score_micros,
            anchor_result,
        ))?;
        let receipt = IndependentGraderReceiptV2 {
            schema_version: "rsia.independent_grader_receipt.v2".into(),
            receipt_id: request.receipt_id.clone(),
            ticket_id: ticket.id.clone(),
            target_id: request.target_id.clone(),
            candidate_execution_receipt_id: candidate.receipt_id.clone(),
            baseline_execution_receipt_id: baseline.receipt_id.clone(),
            candidate_output_digest: candidate.output_digest.clone(),
            baseline_output_digest: baseline.output_digest.clone(),
            oracle_digest: control.oracle_digest.clone(),
            grader_digest: control.grader_digest.clone(),
            scored_output_pair_digest,
            candidate_score_micros,
            baseline_score_micros,
            anchor_result,
            evaluator_actor: ctx.actor().into(),
            issued_at_unix_seconds: request.issued_at_unix_seconds,
        };
        mark_feedback_used_if_needed(&mut session, ctx, &ticket, request.issued_at_unix_seconds)
            .await?;
        if let Some(existing) = get_record::<IndependentGraderReceiptV2>(
            &mut session,
            ctx,
            GRADER_RECEIPT_KIND,
            &request.receipt_id,
        )
        .await?
        {
            if fingerprint(&existing)? == fingerprint(&receipt)? {
                session.commit().await?;
                return Ok(existing);
            }
            return Err(Error::Conflict(
                "grader receipt id reused with different evidence".into(),
            ));
        }
        if !is_stopping(ticket.lifecycle) {
            validate_target_receipt_pair(
                ticket.target(&request.target_id)?,
                &candidate.receipt_id,
                &baseline.receipt_id,
            )?;
        }
        if !is_stopping(ticket.lifecycle)
            && matches!(
                ticket.target(&request.target_id)?.state,
                TargetState::Completed { .. }
            )
        {
            return Err(Error::Conflict(
                "completed target score is immutable".into(),
            ));
        }
        put_record(
            &mut session,
            ctx,
            GRADER_RECEIPT_KIND,
            &request.receipt_id,
            ctx.actor(),
            &receipt,
        )
        .await?;
        if bind_accepted_facts {
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(GRADER_RECEIPT_KIND, &receipt.receipt_id)?,
                vec![
                    artifact_edge(EXECUTION_RECEIPT_KIND, &candidate.receipt_id)?,
                    artifact_edge(EXECUTION_RECEIPT_KIND, &baseline.receipt_id)?,
                ],
            )
            .await?;
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(TICKET_KIND, &ticket.id)?,
                vec![artifact_edge(GRADER_RECEIPT_KIND, &receipt.receipt_id)?],
            )
            .await?;
        }
        if is_stopping(ticket.lifecycle) {
            ticket.late_grader_receipt_ids.push(request.receipt_id);
        } else {
            let sequence = ticket.next_sequence;
            ticket.next_sequence = ticket
                .next_sequence
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("ticket sequence overflow".into()))?;
            ticket.target_mut(&request.target_id)?.state = TargetState::Completed {
                candidate_score_micros,
                baseline_score_micros,
                anchor_result,
                candidate_receipt_id: candidate.receipt_id,
                baseline_receipt_id: baseline.receipt_id,
                grader_receipt_id: receipt.receipt_id.clone(),
                completion_sequence: sequence,
            };
            if anchor_result == Some(AnchorResult::CriticalRegression) {
                stop_ticket(
                    &mut session,
                    ctx,
                    &control,
                    &mut ticket,
                    EarlyStopDecision::CriticalRegression,
                    &receipt,
                    request.issued_at_unix_seconds,
                )
                .await?;
            } else if let Some(plan) = &control.early_stop_plan {
                let (decision, stream) = rebuild_stream(&ticket, plan, &control.alpha_plan)?;
                if decision.is_early_stop() {
                    stop_ticket(
                        &mut session,
                        ctx,
                        &control,
                        &mut ticket,
                        decision,
                        &receipt,
                        request.issued_at_unix_seconds,
                    )
                    .await?;
                    debug_assert_eq!(stream.decision()?, decision);
                } else if decision == EarlyStopDecision::FinalFixedSample && anchors_passed(&ticket)
                {
                    ticket.lifecycle = TicketLifecycle::ReadyForFinal;
                }
            } else if rotating_complete(&ticket) && anchors_passed(&ticket) {
                ticket.lifecycle = TicketLifecycle::ReadyForFinal;
            }
        }
        ticket.validate()?;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            &ticket.id,
            ticket.evaluator_actor.as_str(),
            &ticket,
        )
        .await?;
        if bind_accepted_facts {
            collect_dependency_nodes(
                &mut session,
                ctx,
                vec![artifact_edge(TICKET_KIND, &ticket.id)?],
            )
            .await?;
        }
        maybe_store_early_formal(&mut session, ctx, &ticket).await?;
        session.commit().await?;
        Ok(receipt)
    }

    pub async fn finalize_complete(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        now: i64,
    ) -> Result<FormalEvaluationV2> {
        ctx.require(&[Role::Evaluator])?;
        valid_time(now)?;
        let ticket: EvaluationTicketV2 = load(store, ctx, TICKET_KIND, ticket_id).await?;
        let control: RegisteredEvaluationControl =
            load(store, ctx, CONTROL_KIND, &ticket.registration_id).await?;
        if ticket.evaluator_actor != ctx.actor()
            || ticket.lifecycle != TicketLifecycle::ReadyForFinal
        {
            return Err(Error::Conflict(
                "ticket is not ready for fixed-sample finalization".into(),
            ));
        }
        let registration_digest = control.digest()?;
        if registration_digest != ticket.registration_digest {
            return Err(Error::Conflict(
                "registered control digest differs from the frozen ticket".into(),
            ));
        }
        let mut calls = Vec::new();
        let mut execution_content_pins = BTreeMap::new();
        let mut candidate_cost = 0i128;
        let mut baseline_cost = 0i128;
        let mut candidate_latencies = Vec::new();
        let mut baseline_latencies = Vec::new();
        for target in &ticket.targets {
            let TargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                ..
            } = &target.state
            else {
                return Err(Error::Invalid(
                    "complete ticket contains a non-complete target".into(),
                ));
            };
            for (receipt_id, side) in [
                (candidate_receipt_id, ExecutionSide::Candidate),
                (baseline_receipt_id, ExecutionSide::Baseline),
            ] {
                let receipt: ExecutionReceiptV2 =
                    load(store, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
                execution_content_pins.insert(receipt_id.clone(), fingerprint(&receipt)?);
                let call = store
                    .budget_call(ctx, &control.billing_scope, &receipt.budget_call_id)
                    .await?
                    .ok_or(Error::NotFound)?;
                if call.dispatch_group_id != ticket.id
                    || call.state != BudgetCallState::Finalized
                    || call.actual_cost_micros.is_none()
                    || !call.execution_closed
                {
                    return Err(Error::Conflict(
                        "formal completion requires finalized execution usage".into(),
                    ));
                }
                let amount = i128::from(call.actual_cost_micros.unwrap_or_default());
                match side {
                    ExecutionSide::Candidate => {
                        candidate_cost += amount;
                        candidate_latencies.push(receipt.latency_micros);
                    }
                    ExecutionSide::Baseline => {
                        baseline_cost += amount;
                        baseline_latencies.push(receipt.latency_micros);
                    }
                }
                calls.push(call);
            }
        }
        if baseline_cost <= 0 || candidate_cost < 0 {
            return Err(Error::Invalid(
                "baseline cost must be positive and observable".into(),
            ));
        }
        let baseline_p95 = p95(&mut baseline_latencies)?;
        let candidate_p95 = p95(&mut candidate_latencies)?;
        if baseline_p95 == 0 {
            return Err(Error::Invalid("baseline p95 must be observable".into()));
        }
        let cost_ratio = candidate_cost as f64 / baseline_cost as f64;
        let p95_ratio = candidate_p95 as f64 / baseline_p95 as f64;
        let rows: Vec<_> = ticket
            .targets
            .iter()
            .filter_map(|target| match (&target.kind, &target.state) {
                (
                    EvaluationTargetKind::Rotating { cluster_id },
                    TargetState::Completed {
                        candidate_score_micros,
                        baseline_score_micros,
                        ..
                    },
                ) => Some(ClusterObservation {
                    cluster_id: cluster_id.clone(),
                    d: (i64::from(*candidate_score_micros) - i64::from(*baseline_score_micros))
                        as f64
                        / 1_000_000.0,
                    weight: 1.0,
                }),
                _ => None,
            })
            .collect();
        let report = derive_complete_report(&ticket, &control, &rows, cost_ratio, p95_ratio)?;
        let anchor_evidence_digest = anchor_evidence_digest(&ticket)?;
        let resource_evidence = ResourceEvidenceRecord {
            schema_version: "rsia.evaluation_resource_evidence.v1".into(),
            ticket_id: ticket.id.clone(),
            billing_scope: control.billing_scope.clone(),
            cost_scope: CostEvidenceScope::TicketExecutionOnly,
            calls,
        };
        let resource_evidence_digest = fingerprint(&resource_evidence)?;
        let mut session = store.session().await?;
        let mut latest: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, ticket_id).await?;
        if latest.lifecycle != TicketLifecycle::ReadyForFinal
            || fingerprint(&latest)? != fingerprint(&ticket)?
        {
            return Err(Error::Conflict("ticket changed during finalization".into()));
        }
        let latest_control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &latest.registration_id).await?;
        if latest_control.digest()? != registration_digest {
            return Err(Error::Conflict(
                "registered control digest changed during finalization".into(),
            ));
        }
        // Revalidate actual facts after external cost/statistical precomputation,
        // before stop, settlement, resource, ticket or formal writes.
        validate_status_closure(&mut session, ctx, &latest, None).await?;
        let latest_holdout: ProtectedHoldoutRecord =
            need_record(&mut session, ctx, HOLDOUT_KIND, &latest.holdout_id).await?;
        for target in &latest.targets {
            validate_completed_target_evidence(
                &mut session,
                ctx,
                &latest,
                &latest_control,
                &latest_holdout,
                target,
            )
            .await?;
        }
        for (receipt_id, expected_digest) in execution_content_pins {
            let actual: ExecutionReceiptV2 =
                need_record(&mut session, ctx, EXECUTION_RECEIPT_KIND, &receipt_id).await?;
            if fingerprint(&actual)? != expected_digest {
                return Err(Error::Conflict(
                    "execution receipt changed during finalization".into(),
                ));
            }
        }
        let mut verified_calls = Vec::new();
        for expected in &resource_evidence.calls {
            let actual = session
                .budget_call(ctx, &control.billing_scope, &expected.call_id)
                .await?
                .ok_or(Error::NotFound)?;
            if fingerprint(&actual)? != fingerprint(expected)? {
                return Err(Error::Conflict(
                    "execution usage changed during finalization".into(),
                ));
            }
            if latest.candidate_dependency_binding.is_some() {
                verified_calls.push(actual);
            }
        }
        if latest.candidate_dependency_binding.is_some() {
            let expected_report = recompute_bound_complete_report_in_session(
                &mut session,
                ctx,
                &latest,
                &latest_control,
                &latest_holdout,
                &verified_calls,
            )
            .await?;
            if expected_report != BernsteinReportWire::from_report(&report)?
                || expected_report.verdict != report.verdict
                || resource_evidence.cost_scope != CostEvidenceScope::TicketExecutionOnly
                || latest.evidence_scope != latest_control.evidence_scope
            {
                return Err(Error::Conflict(STATISTICS_CHANGED.into()));
            }
        }
        session
            .stop_dispatch_group(
                ctx,
                &control.billing_scope,
                ticket_id,
                "complete_batch",
                now,
            )
            .await?;
        let group_calls = session
            .budget_calls_for_group(ctx, &control.billing_scope, ticket_id)
            .await?;
        let expected_call_ids: BTreeSet<_> = resource_evidence
            .calls
            .iter()
            .map(|call| call.call_id.as_str())
            .collect();
        let actual_call_ids: BTreeSet<_> = group_calls
            .iter()
            .map(|call| call.call_id.as_str())
            .collect();
        if expected_call_ids != actual_call_ids {
            invalidate_in_session(
                &mut session,
                ctx,
                &control,
                &mut latest,
                "unattributed_dispatch_group_call",
                now,
            )
            .await?;
            session.commit().await?;
            return Err(Error::Invalid(
                "ticket dispatch group contains unattributed calls".into(),
            ));
        }
        settle_exposure_money(&mut session, ctx, &latest, false).await?;
        latest.lifecycle = TicketLifecycle::CompleteBatch;
        put_record(
            &mut session,
            ctx,
            RESOURCE_KIND,
            ticket_id,
            ctx.actor(),
            &resource_evidence,
        )
        .await?;
        if latest.candidate_dependency_binding.is_some() {
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(RESOURCE_KIND, ticket_id)?,
                vec![artifact_edge(TICKET_KIND, ticket_id)?],
            )
            .await?;
        }
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            ticket_id,
            ctx.actor(),
            &latest,
        )
        .await?;
        let stored_dependency_binding =
            bind_stored_facts_in_session(&mut session, ctx, &latest).await?;
        let formal = FormalEvaluationV2::CompleteBatch {
            evidence_scope: control.evidence_scope,
            cost_evidence_scope: CostEvidenceScope::TicketExecutionOnly,
            ticket_id: ticket_id.into(),
            ticket_digest: fingerprint(&latest)?,
            manifest_digest: latest.manifest_digest.clone(),
            candidate_bundle_digest: latest.candidate_bundle_digest.clone(),
            baseline_bundle_digest: latest.baseline_bundle_digest.clone(),
            environment_digest: latest.environment_digest.clone(),
            verdict: report.verdict,
            report: BernsteinReportWire::from_report(&report)?,
            anchor_evidence_digest,
            resource_evidence_digest,
            stored_dependency_binding,
        };
        put_record(
            &mut session,
            ctx,
            FORMAL_KIND,
            ticket_id,
            ctx.actor(),
            &formal,
        )
        .await?;
        if latest.candidate_dependency_binding.is_some() {
            put_fact_edges(
                &mut session,
                ctx,
                &storage_id(FORMAL_KIND, ticket_id)?,
                vec![
                    artifact_edge(TICKET_KIND, ticket_id)?,
                    artifact_edge(RESOURCE_KIND, ticket_id)?,
                ],
            )
            .await?;
        }
        session
            .audit(ctx, "complete_formal_evaluation", ticket_id)
            .await?;
        session.commit().await?;
        Ok(formal)
    }

    pub async fn invalidate(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<FormalEvaluationV2> {
        ctx.require(&[Role::Evaluator])?;
        identifier(reason)?;
        valid_time(now)?;
        let mut session = store.session().await?;
        let mut ticket: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, ticket_id).await?;
        let control: RegisteredEvaluationControl =
            need_record(&mut session, ctx, CONTROL_KIND, &ticket.registration_id).await?;
        if ticket.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let formal =
            invalidate_in_session(&mut session, ctx, &control, &mut ticket, reason, now).await?;
        session.commit().await?;
        Ok(formal)
    }

    pub async fn settle_after_stop(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        now_unix_seconds: i64,
    ) -> Result<StreamingEvaluationStatus> {
        ctx.require(&[Role::Evaluator])?;
        valid_time(now_unix_seconds)?;
        let ticket: EvaluationTicketV2 = load(store, ctx, TICKET_KIND, ticket_id).await?;
        if ticket.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if ticket.lifecycle != TicketLifecycle::Draining {
            return Self::status(ctx, store, ticket_id).await;
        }
        let control: RegisteredEvaluationControl =
            load(store, ctx, CONTROL_KIND, &ticket.registration_id).await?;
        for target in &ticket.targets {
            if let TargetState::RunningUsageUnknown {
                candidate,
                baseline,
            } = &target.state
            {
                for slot in [candidate, baseline].into_iter().flatten() {
                    let call = store
                        .budget_call(ctx, &control.billing_scope, &slot.budget_call_id)
                        .await?
                        .ok_or(Error::NotFound)?;
                    if matches!(
                        call.state,
                        BudgetCallState::Dispatched | BudgetCallState::Uncertain
                    ) || (call.state == BudgetCallState::Finalized && !call.execution_closed)
                    {
                        return Self::status(ctx, store, ticket_id).await;
                    }
                }
            }
        }
        for call_id in &ticket.unattributed_budget_call_ids {
            let call = store
                .budget_call(ctx, &control.billing_scope, call_id)
                .await?
                .ok_or(Error::NotFound)?;
            if matches!(
                call.state,
                BudgetCallState::Dispatched | BudgetCallState::Uncertain
            ) || (call.state == BudgetCallState::Finalized && !call.execution_closed)
            {
                return Self::status(ctx, store, ticket_id).await;
            }
        }
        let mut session = store.session().await?;
        let mut latest: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, ticket_id).await?;
        if latest.lifecycle != TicketLifecycle::Draining {
            return Err(Error::Conflict(
                "ticket changed while reconciling usage".into(),
            ));
        }
        if latest
            .invalid_reasons
            .iter()
            .any(|reason| reason == "unattributed_dispatch_group_call")
        {
            invalidate_in_session(
                &mut session,
                ctx,
                &control,
                &mut latest,
                "unattributed_dispatch_group_call",
                now_unix_seconds,
            )
            .await?;
            session.commit().await?;
            return Self::status(ctx, store, ticket_id).await;
        }
        for target in &mut latest.targets {
            if let TargetState::RunningUsageUnknown {
                candidate,
                baseline,
            } = &target.state
            {
                target.state = TargetState::CancelledAfterDispatch {
                    candidate: candidate.clone(),
                    baseline: baseline.clone(),
                };
            }
        }
        settle_exposure_money(&mut session, ctx, &latest, false).await?;
        latest.pending_certificate = Some(
            latest
                .pending_certificate
                .take()
                .ok_or_else(|| Error::Invalid("draining stop lacks certificate".into()))?
                .with_usage_reconciled()?,
        );
        latest.lifecycle = TicketLifecycle::EarlyStopped;
        put_record(
            &mut session,
            ctx,
            TICKET_KIND,
            ticket_id,
            ctx.actor(),
            &latest,
        )
        .await?;
        maybe_store_early_formal(&mut session, ctx, &latest).await?;
        session.commit().await?;
        Self::status(ctx, store, ticket_id).await
    }

    pub async fn status(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
    ) -> Result<StreamingEvaluationStatus> {
        ctx.require(&[Role::Evaluator, Role::Admin])?;
        let mut session = store.session().await?;
        let ticket: EvaluationTicketV2 =
            need_record(&mut session, ctx, TICKET_KIND, ticket_id).await?;
        let formal =
            get_record::<FormalEvaluationV2>(&mut session, ctx, FORMAL_KIND, ticket_id).await?;
        validate_status_closure(&mut session, ctx, &ticket, formal.as_ref()).await?;
        session.commit().await?;
        Ok(StreamingEvaluationStatus { ticket, formal })
    }

    pub async fn verified_report_view(
        ctx: &Context,
        store: &Store,
        report_id: &str,
    ) -> Result<VerifiedFormalReportView> {
        let mut session = store.session().await?;
        let view = verified_report_view_in_session(ctx, &mut session, report_id).await?;
        session.commit().await?;
        Ok(view)
    }
}

pub async fn verified_report_view_in_session(
    ctx: &Context,
    session: &mut Session,
    report_id: &str,
) -> Result<VerifiedFormalReportView> {
    ctx.require(&[Role::Evaluator, Role::Admin])?;
    verified_report_view_core(ctx, session, report_id).await
}

pub(crate) async fn verified_report_view_for_host_in_session(
    ctx: &Context,
    session: &mut Session,
    report_id: &str,
) -> Result<VerifiedFormalReportView> {
    ctx.require(&[Role::Host])?;
    verified_report_view_core(ctx, session, report_id).await
}

async fn verified_report_view_core(
    ctx: &Context,
    session: &mut Session,
    report_id: &str,
) -> Result<VerifiedFormalReportView> {
    identifier(report_id)?;
    let ticket: EvaluationTicketV2 = need_record(session, ctx, TICKET_KIND, report_id).await?;
    let formal: FormalEvaluationV2 = need_record(session, ctx, FORMAL_KIND, report_id).await?;
    validate_status_closure(session, ctx, &ticket, Some(&formal)).await?;
    let revoke_watermark = session.watermark(ctx).await?;
    let (
        variant,
        verdict,
        anchor_evidence_digest,
        resource_evidence_digest,
        anchor_status,
        evidence_scope,
        cost_evidence_scope,
    ) = match &formal {
        FormalEvaluationV2::CompleteBatch {
            evidence_scope,
            cost_evidence_scope,
            verdict,
            anchor_evidence_digest: report_anchor_digest,
            resource_evidence_digest,
            ..
        } => (
            VerifiedReportVariant::CompleteBatch,
            Some(*verdict),
            Some(report_anchor_digest.clone()),
            Some(resource_evidence_digest.clone()),
            AnchorEvidenceStatus::CompletePassedStoredClosure,
            *evidence_scope,
            Some(*cost_evidence_scope),
        ),
        FormalEvaluationV2::EarlyStopped { evidence_scope, .. } => (
            VerifiedReportVariant::EarlyStopped,
            None,
            None,
            None,
            AnchorEvidenceStatus::NotApplicable,
            *evidence_scope,
            None,
        ),
        FormalEvaluationV2::Invalid { evidence_scope, .. } => (
            VerifiedReportVariant::Invalid,
            Some(Verdict::Invalid),
            None,
            None,
            AnchorEvidenceStatus::NotApplicable,
            *evidence_scope,
            None,
        ),
    };
    let mut reasons = vec![
        "program_fixture_not_production_authority".into(),
        "independent_process_isolation_not_verified".into(),
        "dependency_revocation_closure_unverified".into(),
    ];
    if variant != VerifiedReportVariant::CompleteBatch {
        reasons.push("report_variant_not_complete_batch".into());
    }
    if cost_evidence_scope != Some(CostEvidenceScope::TicketExecutionOnly) {
        reasons.push("resource_evidence_absent".into());
    } else {
        reasons.push("complete_optimization_cost_not_verified".into());
    }
    if !matches!(verdict, Some(Verdict::Improved | Verdict::Noninferior)) {
        reasons.push("formal_verdict_not_approvable".into());
    }
    Ok(VerifiedFormalReportView {
        report_id: report_id.into(),
        report_digest: fingerprint(&formal)?,
        variant,
        verdict,
        ticket_id: ticket.id.clone(),
        ticket_digest: fingerprint(&ticket)?,
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        formal_plan_digest: ticket.formal_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_parent_digest: ticket.baseline_digest.clone(),
        candidate_bundle_digest: ticket.candidate_bundle_digest.clone(),
        baseline_bundle_digest: ticket.baseline_bundle_digest.clone(),
        environment_digest: ticket.environment_digest.clone(),
        profile: ticket.profile,
        proposer_actor: ticket.proposer_actor.clone(),
        evaluator_actor: ticket.evaluator_actor.clone(),
        approver_actor: ticket.approver_actor.clone(),
        anchor_evidence_digest,
        resource_evidence_digest,
        anchor_status,
        evidence_scope,
        cost_evidence_scope,
        revoke_watermark,
        dependency_status: DependencyEvidenceStatus::NamespaceWatermarkOnly,
        promotion_eligible: false,
        ineligibility_reasons: reasons,
    })
}

async fn validate_status_closure(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    formal: Option<&FormalEvaluationV2>,
) -> Result<()> {
    ticket.validate()?;
    let control: RegisteredEvaluationControl =
        need_record(session, ctx, CONTROL_KIND, &ticket.registration_id).await?;
    let holdout: ProtectedHoldoutRecord =
        need_record(session, ctx, HOLDOUT_KIND, &ticket.holdout_id).await?;
    validate_candidate_binding_in_session(session, ctx, ticket, &control, &holdout).await?;
    if ticket.candidate_dependency_binding.is_some() && formal.is_some() {
        // The report stays outside stored content pins to avoid self hashing.
        // Judge its own revocation before traversing the remaining facts.
        let formal_id = storage_id(FORMAL_KIND, &ticket.id)?;
        if judge_dependency(ctx, session, "artifact", &formal_id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    control.validate()?;
    holdout.validate_against(&control)?;
    let issue: TicketIssueReceipt =
        need_record(session, ctx, TICKET_RECEIPT_KIND, &ticket.id).await?;
    let mut expected_bound_plan = control.v1_plan_snapshot.clone();
    expected_bound_plan.bind_candidate(ticket.candidate_digest.clone())?;
    if ticket.registration_id != control.id
        || ticket.holdout_id != holdout.id
        || ticket.proposer_actor != control.proposer_actor
        || ticket.evaluator_actor != control.evaluator_actor
        || ticket.executor_actor != control.executor_actor
        || ticket.approver_actor != control.approver_actor
        || ticket.evidence_scope != control.evidence_scope
        || ticket.attempt_id != control.attempt_id
        || ticket.research_family_id != control.alpha_plan.research_family_id
        || ticket.alpha_plan_digest != control.alpha_plan.digest()?
        || ticket.formal_plan_digest != control.formal_plan.digest()?
        || ticket.v1_plan_snapshot_digest != control.v1_plan_snapshot.digest()?
        || ticket.candidate_bundle_digest != holdout.candidate_bundle_digest
        || ticket.baseline_bundle_digest != holdout.baseline_bundle_digest
        || issue.schema_version != "rsia.evaluation_ticket_issue_receipt.v1"
        || issue.registration_digest != ticket.registration_digest
        || issue.manifest_digest != ticket.manifest_digest
        || issue.issued_at_unix_seconds != ticket.issued_at_unix_seconds
        || ticket.registration_digest != control.digest()?
        || ticket.manifest_digest != holdout.manifest.digest()?
        || ticket.issue_receipt_digest != fingerprint(&issue)?
        || issue.ticket_id != ticket.id
        || issue.evaluator_actor != ticket.evaluator_actor
        || ticket.candidate_digest != holdout.pair.candidate_digest
        || ticket.baseline_digest != holdout.pair.baseline_digest
        || ticket.environment_digest != holdout.manifest.environment_digest
        || ticket.bound_v1_plan_digest != expected_bound_plan.digest()?
        || ticket.profile != control.formal_plan.profile
        || ticket.n_planned != control.formal_plan.n_planned
    {
        return Err(Error::Conflict(
            "stored ticket dependency closure does not match".into(),
        ));
    }
    if ticket.candidate_dependency_binding.is_some() {
        let expected_inputs = holdout
            .rotating_inputs
            .iter()
            .map(|input| (input, false))
            .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
            .collect::<Vec<_>>();
        if ticket.targets.len() != expected_inputs.len()
            || ticket
                .targets
                .iter()
                .zip(expected_inputs)
                .any(|(target, (input, is_anchor))| {
                    target.target_id != input.target_id
                        || target.input_artifact_id != input.input_artifact_id
                        || target.input_digest != input.input_digest
                        || match &target.kind {
                            EvaluationTargetKind::Rotating { cluster_id } => {
                                is_anchor || cluster_id != &input.cluster_or_anchor_id
                            }
                            EvaluationTargetKind::Anchor { anchor_id } => {
                                !is_anchor || anchor_id != &input.cluster_or_anchor_id
                            }
                        }
                })
        {
            return Err(Error::Conflict(
                "frozen ticket input identity differs".into(),
            ));
        }
        validate_bound_fact_edges(session, ctx, ticket, &control).await?;
        collect_dependency_nodes(session, ctx, vec![artifact_edge(TICKET_KIND, &ticket.id)?])
            .await?;
    }
    match formal {
        Some(FormalEvaluationV2::EarlyStopped {
            evidence_scope,
            ticket_id,
            ticket_digest,
            certificate,
            ..
        }) => {
            certificate.validate_shape()?;
            if ticket.lifecycle != TicketLifecycle::EarlyStopped
                || ticket_id != &ticket.id
                || ticket_digest != &fingerprint(ticket)?
                || certificate.ticket_id() != ticket.id
                || certificate.manifest_digest() != ticket.manifest_digest
                || certificate.v1_plan_snapshot_digest() != ticket.v1_plan_snapshot_digest
                || certificate.alpha_plan_digest() != ticket.alpha_plan_digest
                || certificate.candidate_digest() != ticket.candidate_digest
                || certificate.baseline_digest() != ticket.baseline_digest
                || certificate.grader_digest() != control.grader_digest
                || certificate.slice_id() != holdout.manifest.slice_id
                || ticket.stop_member_terminal_digest.as_deref()
                    != Some(certificate.member_terminal_digest())
            {
                return Err(Error::Conflict(
                    "early-stop certificate dependency closure mismatch".into(),
                ));
            }
            let mut issuer_found = false;
            for target in &ticket.targets {
                if matches!(target.state, TargetState::Completed { .. }) {
                    let receipt = validate_completed_target_evidence(
                        session, ctx, ticket, &control, &holdout, target,
                    )
                    .await?;
                    if fingerprint(&receipt)? == certificate.issuer_receipt_digest() {
                        issuer_found = receipt.ticket_id == ticket.id
                            && receipt.evaluator_actor == ticket.evaluator_actor
                            && receipt.grader_digest == control.grader_digest;
                    }
                }
            }
            if !issuer_found {
                return Err(Error::Conflict(
                    "early-stop issuer receipt is absent from the stored ticket".into(),
                ));
            }
            if ticket.candidate_dependency_binding.is_some() {
                if *evidence_scope != ticket.evidence_scope {
                    return Err(Error::Conflict(EARLY_CERTIFICATE_CHANGED.into()));
                }
                validate_bound_early_certificate_in_session(
                    session,
                    ctx,
                    ticket,
                    &control,
                    &holdout,
                    certificate,
                )
                .await?;
            }
        }
        Some(FormalEvaluationV2::CompleteBatch {
            evidence_scope,
            cost_evidence_scope,
            ticket_id,
            ticket_digest,
            manifest_digest,
            candidate_bundle_digest,
            baseline_bundle_digest,
            environment_digest,
            anchor_evidence_digest: report_anchor_digest,
            resource_evidence_digest,
            stored_dependency_binding,
            verdict,
            report,
        }) => {
            match (
                &ticket.candidate_dependency_binding,
                stored_dependency_binding,
            ) {
                (Some(_), Some(binding)) => {
                    binding.validate()?;
                    require_exact_fact_edges(
                        session,
                        ctx,
                        &storage_id(RESOURCE_KIND, &ticket.id)?,
                        vec![artifact_edge(TICKET_KIND, &ticket.id)?],
                    )
                    .await?;
                    require_exact_fact_edges(
                        session,
                        ctx,
                        &storage_id(FORMAL_KIND, &ticket.id)?,
                        vec![
                            artifact_edge(TICKET_KIND, &ticket.id)?,
                            artifact_edge(RESOURCE_KIND, &ticket.id)?,
                        ],
                    )
                    .await?;
                    if bind_stored_facts_in_session(session, ctx, ticket)
                        .await?
                        .as_ref()
                        != Some(binding)
                    {
                        return Err(Error::Conflict(STORED_CHANGED.into()));
                    }
                }
                (None, None) => {}
                _ => return Err(Error::Conflict(STORED_CHANGED.into())),
            }
            if ticket.lifecycle != TicketLifecycle::CompleteBatch
                || ticket_id != &ticket.id
                || ticket_digest != &fingerprint(ticket)?
                || manifest_digest != &ticket.manifest_digest
                || candidate_bundle_digest != &ticket.candidate_bundle_digest
                || baseline_bundle_digest != &ticket.baseline_bundle_digest
                || environment_digest != &ticket.environment_digest
            {
                return Err(Error::Conflict(
                    "complete formal evaluation dependency closure mismatch".into(),
                ));
            }
            for target in &ticket.targets {
                validate_completed_target_evidence(
                    session, ctx, ticket, &control, &holdout, target,
                )
                .await?;
            }
            let mut expected_calls = BTreeMap::new();
            for target in &ticket.targets {
                let TargetState::Completed {
                    candidate_receipt_id,
                    baseline_receipt_id,
                    ..
                } = &target.state
                else {
                    return Err(Error::Invalid(
                        "complete ticket has non-complete target".into(),
                    ));
                };
                for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                    let receipt: ExecutionReceiptV2 =
                        need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
                    if expected_calls
                        .insert(receipt.budget_call_id, receipt.output_digest)
                        .is_some()
                    {
                        return Err(Error::Conflict(
                            "budget call reused across execution receipts".into(),
                        ));
                    }
                }
            }
            if anchor_evidence_digest(ticket)? != *report_anchor_digest {
                return Err(Error::Conflict("anchor evidence digest mismatch".into()));
            }
            let resource: ResourceEvidenceRecord =
                need_record(session, ctx, RESOURCE_KIND, &ticket.id).await?;
            if resource.schema_version != "rsia.evaluation_resource_evidence.v1"
                || resource.ticket_id != ticket.id
                || resource.billing_scope != control.billing_scope
                || resource.cost_scope != CostEvidenceScope::TicketExecutionOnly
                || fingerprint(&resource)? != *resource_evidence_digest
                || resource.calls.len() != ticket.targets.len() * 2
                || resource.calls.iter().any(|call| {
                    call.dispatch_group_id != ticket.id
                        || call.state != BudgetCallState::Finalized
                        || !call.execution_closed
                        || call.actual_cost_micros.is_none()
                        || expected_calls.get(&call.call_id) != call.output_digest.as_ref()
                })
                || resource
                    .calls
                    .iter()
                    .map(|call| call.call_id.as_str())
                    .collect::<BTreeSet<_>>()
                    != expected_calls.keys().map(String::as_str).collect()
            {
                return Err(Error::Conflict("resource evidence closure mismatch".into()));
            }
            if ticket.candidate_dependency_binding.is_some() {
                let actual = session
                    .budget_calls_for_group(ctx, &control.billing_scope, &ticket.id)
                    .await?;
                let expected_by_id = resource
                    .calls
                    .iter()
                    .map(|call| Ok((call.call_id.clone(), fingerprint(call)?)))
                    .collect::<Result<BTreeMap<_, _>>>()?;
                let actual_by_id = actual
                    .iter()
                    .map(|call| Ok((call.call_id.clone(), fingerprint(call)?)))
                    .collect::<Result<BTreeMap<_, _>>>()?;
                if expected_by_id != actual_by_id {
                    return Err(Error::Conflict("actual resource evidence changed".into()));
                }
                let expected_report = recompute_bound_complete_report_in_session(
                    session, ctx, ticket, &control, &holdout, &actual,
                )
                .await?;
                if report != &expected_report
                    || *verdict != expected_report.verdict
                    || *evidence_scope != ticket.evidence_scope
                    || *evidence_scope != control.evidence_scope
                    || *cost_evidence_scope != CostEvidenceScope::TicketExecutionOnly
                {
                    return Err(Error::Conflict(STATISTICS_CHANGED.into()));
                }
            }
        }
        Some(FormalEvaluationV2::Invalid {
            evidence_scope,
            ticket_id,
            ticket_digest,
            reasons,
        }) => {
            if ticket.lifecycle != TicketLifecycle::Invalid
                || ticket_id != &ticket.id
                || ticket_digest != &fingerprint(ticket)?
            {
                return Err(Error::Conflict(
                    "invalid formal evaluation dependency closure mismatch".into(),
                ));
            }
            if ticket.candidate_dependency_binding.is_some()
                && (reasons.is_empty()
                    || reasons != &ticket.invalid_reasons
                    || *evidence_scope != ticket.evidence_scope
                    || *evidence_scope != control.evidence_scope)
            {
                return Err(Error::Conflict("formal_invalid_outcome_changed".into()));
            }
        }
        None if matches!(
            ticket.lifecycle,
            TicketLifecycle::EarlyStopped
                | TicketLifecycle::CompleteBatch
                | TicketLifecycle::Invalid
        ) =>
        {
            return Err(Error::Invalid(
                "terminal ticket lacks formal evaluation".into(),
            ));
        }
        None => {}
    }
    Ok(())
}

fn derive_complete_report(
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
    rows: &[ClusterObservation],
    cost_ratio: f64,
    p95_ratio: f64,
) -> Result<BernsteinReport> {
    let alpha = control.alpha_plan.allocation(
        &control.fixed_sample_claim_id,
        &control.attempt_id,
        FormalClaimKind::FixedSampleGain,
    )?;
    let alpha = alpha
        .parse::<f64>()
        .map_err(|_| Error::Invalid("invalid fixed-sample alpha".into()))?;
    let raw = empirical_bernstein(rows, alpha)?;
    let mut bound_v1_plan = control.v1_plan_snapshot.clone();
    bound_v1_plan.bind_candidate(ticket.candidate_digest.clone())?;
    if bound_v1_plan.digest()? != ticket.bound_v1_plan_digest {
        return Err(Error::Conflict("bound v1 plan digest mismatch".into()));
    }
    postprocess_complete_report(
        &control.schema_version,
        ticket.profile,
        decide(&bound_v1_plan, raw, cost_ratio, p95_ratio, true)?,
    )
}

// The caller first validates the full candidate/content/dependency closure.
// Only current facts from its Session supply statistics; the formal report
// stays outside its own upstream content pins and supplies no numeric input.
async fn recompute_bound_complete_report_in_session(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
    calls: &[BudgetCallRecord],
) -> Result<BernsteinReportWire> {
    let changed = || Error::Conflict(STATISTICS_CHANGED.into());
    let planned_n = usize::try_from(ticket.n_planned).map_err(|_| changed())?;
    if ticket.candidate_dependency_binding.is_none()
        || !matches!(
            ticket.lifecycle,
            TicketLifecycle::ReadyForFinal | TicketLifecycle::CompleteBatch
        )
        || planned_n != control.v1_plan_snapshot.n_planned
        || ticket.n_planned != control.formal_plan.n_planned
        || planned_n != holdout.manifest.ordered_task_cluster_ids.len()
        || !anchors_passed(ticket)
    {
        return Err(changed());
    }
    let actual_by_id: BTreeMap<_, _> = calls
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect();
    if actual_by_id.len() != calls.len() {
        return Err(changed());
    }
    let mut sequences = BTreeSet::new();
    let mut clusters = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut used_call_ids = BTreeSet::new();
    let mut rows = Vec::new();
    let mut candidate_cost = 0i128;
    let mut baseline_cost = 0i128;
    let mut candidate_latencies = Vec::new();
    let mut baseline_latencies = Vec::new();
    for target in &ticket.targets {
        let TargetState::Completed {
            candidate_receipt_id,
            baseline_receipt_id,
            grader_receipt_id,
            completion_sequence,
            ..
        } = &target.state
        else {
            return Err(changed());
        };
        if !sequences.insert(*completion_sequence) || !grader_ids.insert(grader_receipt_id.as_str())
        {
            return Err(changed());
        }
        let grader =
            validate_completed_target_evidence(session, ctx, ticket, control, holdout, target)
                .await?;
        if grader.schema_version != "rsia.independent_grader_receipt.v2"
            || grader.receipt_id != *grader_receipt_id
        {
            return Err(changed());
        }
        if let EvaluationTargetKind::Rotating { cluster_id } = &target.kind {
            if !clusters.insert(cluster_id.as_str()) {
                return Err(changed());
            }
            rows.push(ClusterObservation {
                cluster_id: cluster_id.clone(),
                d: (i64::from(grader.candidate_score_micros)
                    - i64::from(grader.baseline_score_micros)) as f64
                    / 1_000_000.0,
                weight: 1.0,
            });
        }
        for (receipt_id, side) in [
            (candidate_receipt_id, ExecutionSide::Candidate),
            (baseline_receipt_id, ExecutionSide::Baseline),
        ] {
            if !receipt_ids.insert(receipt_id.as_str()) {
                return Err(changed());
            }
            let receipt: ExecutionReceiptV2 =
                need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
            let bundle_digest = match side {
                ExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                ExecutionSide::Baseline => &ticket.baseline_bundle_digest,
            };
            let expected_request = request_digest(
                ticket,
                target,
                side,
                bundle_digest,
                &ticket.environment_digest,
            )?;
            if receipt.schema_version != "rsia.execution_receipt.v2"
                || receipt.receipt_id != *receipt_id
                || receipt.executor_actor != ticket.executor_actor
                || receipt.request_digest != expected_request
            {
                return Err(changed());
            }
            let call = actual_by_id
                .get(receipt.budget_call_id.as_str())
                .ok_or_else(changed)?;
            let amount = call.actual_cost_micros.ok_or_else(changed)?;
            if !used_call_ids.insert(call.call_id.as_str())
                || call.billing_scope != control.billing_scope
                || call.namespace != ctx.namespace()
                || call.dispatch_group_id != ticket.id
                || call.stage != BudgetStage::TaskExecution
                || call.actual_input_digest != expected_request
                || call.output_digest.as_deref() != Some(receipt.output_digest.as_str())
                || call.state != BudgetCallState::Finalized
                || !call.execution_closed
                || call.dispatch_id.is_none()
                || call.dispatched_at.is_none()
                || call.finalized_at.is_none()
                || call.execution_closed_at.is_none()
                || amount < 0
                || call.actual_currency.as_deref()
                    != Some(control.optimization_budget.root_total.currency.as_str())
                || call.actual_pricing_version.as_deref()
                    != Some(
                        control
                            .optimization_budget
                            .root_total
                            .pricing_version
                            .as_str(),
                    )
            {
                return Err(changed());
            }
            // Historical closure does not require a live lease or open root.
            // A late usage settlement may occur after execution has closed.
            let amount = i128::from(amount);
            match side {
                ExecutionSide::Candidate => {
                    candidate_cost = candidate_cost.checked_add(amount).ok_or_else(changed)?;
                    candidate_latencies.push(receipt.latency_micros);
                }
                ExecutionSide::Baseline => {
                    baseline_cost = baseline_cost.checked_add(amount).ok_or_else(changed)?;
                    baseline_latencies.push(receipt.latency_micros);
                }
            }
        }
    }
    let target_count = u64::try_from(ticket.targets.len()).map_err(|_| changed())?;
    if !sequences.iter().copied().eq(1..=target_count)
        || ticket.next_sequence != target_count.checked_add(1).ok_or_else(changed)?
        || rows.len() != planned_n
        || !rows
            .iter()
            .map(|row| &row.cluster_id)
            .eq(holdout.manifest.ordered_task_cluster_ids.iter())
        || used_call_ids.len() != actual_by_id.len()
        || baseline_cost <= 0
        || candidate_cost < 0
    {
        return Err(changed());
    }
    if let Some(early) = &control.early_stop_plan {
        early.validate_against(&control.alpha_plan)?;
        if early.ordered_cluster_ids != holdout.manifest.ordered_task_cluster_ids {
            return Err(changed());
        }
        // Replay actual callback order. The existing pending map consumes the
        // frozen continuous prefix, including valid reverse callback arrival.
        let (decision, stream) = rebuild_stream(ticket, early, &control.alpha_plan)?;
        if decision != EarlyStopDecision::FinalFixedSample || stream.k() != ticket.n_planned {
            return Err(changed());
        }
    }
    let baseline_p95 = p95(&mut baseline_latencies)?;
    let candidate_p95 = p95(&mut candidate_latencies)?;
    if baseline_p95 == 0 {
        return Err(changed());
    }
    let report = derive_complete_report(
        ticket,
        control,
        &rows,
        candidate_cost as f64 / baseline_cost as f64,
        candidate_p95 as f64 / baseline_p95 as f64,
    )?;
    if report.n != planned_n {
        return Err(changed());
    }
    BernsteinReportWire::from_report(&report)
}

async fn validate_completed_target_evidence(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
    target: &EvaluationTarget,
) -> Result<IndependentGraderReceiptV2> {
    let TargetState::Completed {
        candidate_score_micros,
        baseline_score_micros,
        anchor_result,
        candidate_receipt_id,
        baseline_receipt_id,
        grader_receipt_id,
        ..
    } = &target.state
    else {
        return Err(Error::Invalid(
            "formal evidence target is not completed".into(),
        ));
    };
    let candidate: ExecutionReceiptV2 =
        need_record(session, ctx, EXECUTION_RECEIPT_KIND, candidate_receipt_id).await?;
    let baseline: ExecutionReceiptV2 =
        need_record(session, ctx, EXECUTION_RECEIPT_KIND, baseline_receipt_id).await?;
    validate_execution_pair(ticket, &target.target_id, &candidate, &baseline)?;
    let candidate_output: ExecutionOutputArtifact =
        need_record(session, ctx, OUTPUT_KIND, &candidate.output_artifact_id).await?;
    let baseline_output: ExecutionOutputArtifact =
        need_record(session, ctx, OUTPUT_KIND, &baseline.output_artifact_id).await?;
    validate_output_artifact(&candidate, &candidate_output)?;
    validate_output_artifact(&baseline, &baseline_output)?;
    let oracle = holdout
        .oracle_entries
        .iter()
        .find(|entry| entry.target_id == target.target_id)
        .ok_or_else(|| Error::Invalid("stored oracle target missing".into()))?;
    let expected_candidate = grade_fixed_output(
        &control.fixed_grader,
        &candidate_output.output_utf8,
        &oracle.expected_answer_json,
    )?;
    let expected_baseline = grade_fixed_output(
        &control.fixed_grader,
        &baseline_output.output_utf8,
        &oracle.expected_answer_json,
    )?;
    let expected_anchor = match &target.kind {
        EvaluationTargetKind::Rotating { .. } => None,
        EvaluationTargetKind::Anchor { .. }
            if expected_baseline == 1_000_000 && expected_candidate == 1_000_000 =>
        {
            Some(AnchorResult::Passed)
        }
        EvaluationTargetKind::Anchor { .. } if expected_baseline == 1_000_000 => {
            Some(AnchorResult::CriticalRegression)
        }
        EvaluationTargetKind::Anchor { .. } => {
            return Err(Error::Invalid("stored anchor baseline is invalid".into()));
        }
    };
    let grader: IndependentGraderReceiptV2 =
        need_record(session, ctx, GRADER_RECEIPT_KIND, grader_receipt_id).await?;
    let expected_pair_digest = fingerprint(&(
        &candidate.output_digest,
        &baseline.output_digest,
        &control.oracle_digest,
        &control.grader_digest,
        expected_candidate,
        expected_baseline,
        expected_anchor,
    ))?;
    if *candidate_score_micros != expected_candidate
        || *baseline_score_micros != expected_baseline
        || *anchor_result != expected_anchor
        || grader.ticket_id != ticket.id
        || grader.target_id != target.target_id
        || grader.candidate_execution_receipt_id != candidate.receipt_id
        || grader.baseline_execution_receipt_id != baseline.receipt_id
        || grader.candidate_output_digest != candidate.output_digest
        || grader.baseline_output_digest != baseline.output_digest
        || grader.oracle_digest != control.oracle_digest
        || grader.grader_digest != control.grader_digest
        || grader.evaluator_actor != ticket.evaluator_actor
        || grader.scored_output_pair_digest != expected_pair_digest
        || grader.candidate_score_micros != expected_candidate
        || grader.baseline_score_micros != expected_baseline
        || grader.anchor_result != expected_anchor
    {
        return Err(Error::Conflict(
            "stored grader evidence closure mismatch".into(),
        ));
    }
    Ok(grader)
}

const EARLY_CERTIFICATE_CHANGED: &str = "formal_early_stop_certificate_changed";

// Recompute the first stop from this Session's actual accepted receipts.
// The formal/pending copies supply no numeric or decision input.
async fn validate_bound_early_certificate_in_session(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
    certificate: &EarlyStopCertificate,
) -> Result<()> {
    let changed = || Error::Conflict(EARLY_CERTIFICATE_CHANGED.into());
    if ticket.candidate_dependency_binding.is_none()
        || ticket.lifecycle != TicketLifecycle::EarlyStopped
        || certificate.usage_uncertain()
    {
        return Err(changed());
    }
    ticket.validate()?;
    control.validate()?;
    holdout.validate_against(control)?;
    let pending = ticket.pending_certificate.as_ref().ok_or_else(changed)?;
    let mut bound_plan = control.v1_plan_snapshot.clone();
    bound_plan.bind_candidate(ticket.candidate_digest.clone())?;
    pending.validate_shape()?;
    if fingerprint(pending)? != fingerprint(certificate)?
        || ticket.registration_id != control.id
        || ticket.holdout_id != holdout.id
        || ticket.registration_digest != control.digest()?
        || ticket.manifest_digest != holdout.manifest.digest()?
        || ticket.v1_plan_snapshot_digest != control.v1_plan_snapshot.digest()?
        || ticket.bound_v1_plan_digest != bound_plan.digest()?
        || ticket.formal_plan_digest != control.formal_plan.digest()?
        || ticket.alpha_plan_digest != control.alpha_plan.digest()?
        || ticket.research_family_id != control.alpha_plan.research_family_id
        || ticket.attempt_id != control.attempt_id
        || ticket.profile != control.formal_plan.profile
        || ticket.candidate_digest != holdout.pair.candidate_digest
        || ticket.baseline_digest != holdout.pair.baseline_digest
        || ticket.candidate_bundle_digest != holdout.candidate_bundle_digest
        || ticket.baseline_bundle_digest != holdout.baseline_bundle_digest
        || ticket.environment_digest != holdout.manifest.environment_digest
        || ticket.executor_actor != control.executor_actor
        || ticket.evaluator_actor != control.evaluator_actor
        || ticket.evidence_scope != control.evidence_scope
        || ticket.n_planned != control.formal_plan.n_planned
        || ticket.n_planned as usize != holdout.manifest.ordered_task_cluster_ids.len()
    {
        return Err(changed());
    }
    let inputs = holdout
        .rotating_inputs
        .iter()
        .map(|input| (input, false))
        .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
        .collect::<Vec<_>>();
    if ticket.targets.len() != inputs.len()
        || ticket
            .targets
            .iter()
            .zip(inputs)
            .any(|(target, (input, anchor))| {
                target.target_id != input.target_id
                    || target.input_artifact_id != input.input_artifact_id
                    || target.input_digest != input.input_digest
                    || match &target.kind {
                        EvaluationTargetKind::Rotating { cluster_id } => {
                            anchor || cluster_id != &input.cluster_or_anchor_id
                        }
                        EvaluationTargetKind::Anchor { anchor_id } => {
                            !anchor || anchor_id != &input.cluster_or_anchor_id
                        }
                    }
            })
    {
        return Err(changed());
    }
    let mut stream = match &control.early_stop_plan {
        Some(plan) => {
            plan.validate_against(&control.alpha_plan)?;
            if plan.ordered_cluster_ids != holdout.manifest.ordered_task_cluster_ids {
                return Err(changed());
            }
            Some(SequentialRejectOnly::open(
                plan.clone(),
                &control.alpha_plan,
            )?)
        }
        None => None,
    };
    let calls = session
        .budget_calls_for_group(ctx, &control.billing_scope, &ticket.id)
        .await?;
    if session
        .budget_call_scopes_for_group(ctx, &ticket.id)
        .await?
        != vec![control.billing_scope.clone()]
    {
        return Err(changed());
    }
    let by_id = calls
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != calls.len() {
        return Err(changed());
    }
    for call in &calls {
        if call.billing_scope != control.billing_scope
            || call.namespace != ctx.namespace()
            || call.dispatch_group_id != ticket.id
            || call.stage != BudgetStage::TaskExecution
        {
            return Err(changed());
        }
        match call.state {
            BudgetCallState::Finalized => {
                if !call.execution_closed
                    || call.dispatch_id.is_none()
                    || call.dispatched_at.is_none()
                    || call.finalized_at.is_none()
                    || call.execution_closed_at.is_none()
                    || !call.actual_cost_micros.is_some_and(|amount| amount >= 0)
                    || call.actual_currency.as_deref()
                        != Some(control.optimization_budget.root_total.currency.as_str())
                    || call.actual_pricing_version.as_deref()
                        != Some(
                            control
                                .optimization_budget
                                .root_total
                                .pricing_version
                                .as_str(),
                        )
                {
                    return Err(changed());
                }
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => {
                if call.dispatch_id.is_some() || call.dispatched_at.is_some() {
                    return Err(changed());
                }
            }
            _ => return Err(changed()),
        }
    }
    let mut used_calls = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    let mut events = Vec::new();
    for target in &ticket.targets {
        match &target.state {
            TargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                completion_sequence,
                ..
            } => {
                if !sequences.insert(*completion_sequence)
                    || !grader_ids.insert(grader_receipt_id.as_str())
                {
                    return Err(changed());
                }
                let grader = validate_completed_target_evidence(
                    session, ctx, ticket, control, holdout, target,
                )
                .await?;
                if grader.schema_version != "rsia.independent_grader_receipt.v2"
                    || grader.receipt_id != *grader_receipt_id
                {
                    return Err(changed());
                }
                valid_time(grader.issued_at_unix_seconds)?;
                for (id, side) in [
                    (candidate_receipt_id, ExecutionSide::Candidate),
                    (baseline_receipt_id, ExecutionSide::Baseline),
                ] {
                    if !receipt_ids.insert(id.as_str()) {
                        return Err(changed());
                    }
                    let receipt: ExecutionReceiptV2 =
                        need_record(session, ctx, EXECUTION_RECEIPT_KIND, id).await?;
                    let bundle = match side {
                        ExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        ExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request =
                        request_digest(ticket, target, side, bundle, &ticket.environment_digest)?;
                    let call = by_id
                        .get(receipt.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    if receipt.schema_version != "rsia.execution_receipt.v2"
                        || receipt.receipt_id != *id
                        || receipt.executor_actor != ticket.executor_actor
                        || receipt.request_digest != request
                        || !used_calls.insert(call.call_id.as_str())
                        || call.state != BudgetCallState::Finalized
                        || call.actual_input_digest != request
                        || call.output_digest.as_deref() != Some(receipt.output_digest.as_str())
                    {
                        return Err(changed());
                    }
                }
                events.push((*completion_sequence, target, grader));
            }
            TargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    let call = by_id
                        .get(slot.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    let bundle = match slot.side {
                        ExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        ExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request = request_digest(
                        ticket,
                        target,
                        slot.side,
                        bundle,
                        &ticket.environment_digest,
                    )?;
                    if !used_calls.insert(call.call_id.as_str())
                        || slot.request_digest != request
                        || call.actual_input_digest != request
                    {
                        return Err(changed());
                    }
                    match &slot.phase {
                        ExecutionSlotPhase::Dispatched {
                            dispatch_id,
                            dispatched_at_unix_seconds,
                        } if call.state == BudgetCallState::Finalized
                            && call.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                            && call.dispatched_at == Some(*dispatched_at_unix_seconds) => {}
                        ExecutionSlotPhase::ReservedBound
                            if matches!(
                                call.state,
                                BudgetCallState::Released | BudgetCallState::Cancelled
                            ) => {}
                        _ => return Err(changed()),
                    }
                }
            }
            TargetState::NotDispatched { released_call_ids } => {
                for id in released_call_ids {
                    let call = by_id.get(id.as_str()).ok_or_else(changed)?;
                    if !used_calls.insert(call.call_id.as_str())
                        || !matches!(
                            call.state,
                            BudgetCallState::Released | BudgetCallState::Cancelled
                        )
                    {
                        return Err(changed());
                    }
                }
            }
            _ => return Err(changed()),
        }
    }
    // Already recorded unattributed calls remain paid/audit facts; they do not
    // become accepted statistical units or disappear from terminal accounting.
    for id in &ticket.unattributed_budget_call_ids {
        if !by_id.contains_key(id.as_str()) || !used_calls.insert(id.as_str()) {
            return Err(changed());
        }
    }
    let count = u64::try_from(events.len()).map_err(|_| changed())?;
    if count == 0
        || !sequences.iter().copied().eq(1..=count)
        || ticket.next_sequence != count.checked_add(1).ok_or_else(changed)?
        || used_calls.len() != by_id.len()
    {
        return Err(changed());
    }
    events.sort_by_key(|(sequence, _, _)| *sequence);
    let ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    ledger.validate_restore()?;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .ok_or_else(changed)?;
    let first_feedback_at = events[0]
        .2
        .issued_at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(changed)?;
    if ledger.research_family_id != ticket.research_family_id
        || ledger.alpha_plan_digest != ticket.alpha_plan_digest
        || entry.manifest_digest != ticket.manifest_digest
        || entry.candidate_pair_digest != holdout.manifest.candidate_pair_digest
        || entry.epoch_id != holdout.manifest.epoch_id
        || entry.slice_id != holdout.manifest.slice_id
        || entry.task_cluster_ids != holdout.manifest.ordered_task_cluster_ids
        || entry.monetary_state != MonetaryReservationState::Finalized
        || !matches!(
            &entry.state,
            ExposureState::FeedbackUsed { used_at_unix_ms } if *used_at_unix_ms == first_feedback_at
        )
    {
        return Err(changed());
    }
    let mut first_stop = None;
    for (position, (sequence, target, grader)) in events.iter().enumerate() {
        let decision = match &target.kind {
            EvaluationTargetKind::Anchor { .. }
                if grader.anchor_result == Some(AnchorResult::CriticalRegression) =>
            {
                EarlyStopDecision::CriticalRegression
            }
            EvaluationTargetKind::Rotating { cluster_id } => match stream.as_mut() {
                Some(stream) => stream.record_complete_unit(CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: grader.baseline_score_micros,
                    candidate_score_micros: grader.candidate_score_micros,
                })?,
                None => EarlyStopDecision::Continue,
            },
            _ => EarlyStopDecision::Continue,
        };
        if decision.is_early_stop() {
            if position + 1 != events.len() {
                return Err(changed());
            }
            let (k, lcb, ucb, prefix) = if decision == EarlyStopDecision::CriticalRegression {
                (0, None, None, fingerprint(&Vec::<String>::new())?)
            } else {
                let stream = stream.as_ref().ok_or_else(changed)?;
                let bound = stream.boundary()?.ok_or_else(changed)?;
                let wire = bound.to_wire()?;
                (
                    bound.k,
                    Some(wire.lcb),
                    Some(wire.ucb),
                    stream.completed_prefix_digest()?,
                )
            };
            first_stop = Some((
                decision,
                k,
                lcb,
                ucb,
                prefix,
                grader,
                sequence.checked_add(1).ok_or_else(changed)?,
            ));
            break;
        }
    }
    let (decision, k, lcb, ucb, prefix, issuer, stop_sequence) = first_stop.ok_or_else(changed)?;
    let expected = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
        ticket_id: ticket.id.clone(),
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        early_stop_plan_digest: control
            .early_stop_plan
            .as_ref()
            .map(EarlyStopPlan::digest)
            .transpose()?
            .unwrap_or(control.formal_plan.digest()?),
        alpha_plan_digest: ticket.alpha_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        slice_id: holdout.manifest.slice_id.clone(),
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_digest: ticket.baseline_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        issuer_receipt_digest: fingerprint(issuer)?,
        k,
        lcb,
        ucb,
        completed_prefix_digest: prefix,
        // This is the frozen stop snapshot, not the later reconciled targets.
        member_terminal_digest: ticket
            .stop_member_terminal_digest
            .clone()
            .ok_or_else(changed)?,
        stop_reason: decision,
        stopped_at_unix_ms: issuer
            .issued_at_unix_seconds
            .checked_mul(1_000)
            .ok_or_else(changed)?,
        stop_sequence,
        usage_uncertain: false,
    })?;
    if fingerprint(&expected)? != fingerprint(certificate)? {
        return Err(changed());
    }
    Ok(())
}

fn anchor_evidence_digest(ticket: &EvaluationTicketV2) -> Result<String> {
    fingerprint(
        &ticket
            .targets
            .iter()
            .filter(|target| matches!(target.kind, EvaluationTargetKind::Anchor { .. }))
            .collect::<Vec<_>>(),
    )
}

async fn load<T: for<'de> Deserialize<'de>>(
    store: &Store,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Result<T> {
    let mut session = store.session().await?;
    let value = need_record(&mut session, ctx, kind, id).await?;
    session.commit().await?;
    Ok(value)
}

fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e05-{}", fingerprint(&(record_kind, id))?))
}

async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = session
        .get::<TypedArtifactEnvelope<T>>(ctx, "artifact", &storage_id)
        .await?;
    match envelope {
        Some(envelope)
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == record_kind =>
        {
            Ok(Some(envelope.payload))
        }
        Some(_) => Err(Error::Conflict("typed artifact envelope mismatch".into())),
        None => Ok(None),
    }
}

async fn need_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    get_record(session, ctx, record_kind, id)
        .await?
        .ok_or(Error::NotFound)
}

async fn put_record<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
    owner: &str,
    payload: &T,
) -> Result<()> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = TypedArtifactEnvelope {
        schema_version: ENVELOPE_SCHEMA.into(),
        id: storage_id.clone(),
        record_kind: record_kind.into(),
        payload,
    };
    session
        .put(ctx, "artifact", &storage_id, owner, &envelope)
        .await
}

// These inner schemas add read and derivation pins, never production authority.
const CANDIDATE_BINDING_SCHEMA: &str = "rsia.formal_candidate_dependency_binding.v1";
const STORED_BINDING_SCHEMA: &str = "rsia.formal_stored_dependency_binding.v1";
const CANDIDATE_CHANGED: &str = "formal_candidate_dependency_binding_changed";
const STORED_CHANGED: &str = "formal_stored_dependency_binding_changed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalDependencyNode {
    pub kind: String,
    /// Actual objects key, including e05-<fingerprint((record_kind,logical_id))>.
    pub id: String,
    pub schema_version: String,
    pub record_kind: Option<String>,
    pub logical_id: String,
    /// Full typed record fingerprint; for a run this pins StoredTraceAuthority,
    /// separately from TypedSourceRef.content_digest (the trace body digest).
    pub content_digest: String,
    pub outgoing_edge_digest: String,
    pub outgoing_edge_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalCandidateDependencyBinding {
    pub schema_version: String,
    pub namespace: String,
    pub candidate_id: String,
    pub candidate_record_digest: String,
    pub bundle_digest: String,
    pub parent_digest: String,
    pub profile_id: String,
    pub environment_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark_seq: i64,
    pub revoke_watermark_digest: String,
    pub nodes: Vec<FormalDependencyNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalStoredDependencyBinding {
    pub schema_version: String,
    pub namespace: String,
    pub candidate_binding_digest: String,
    pub ticket_id: String,
    pub ticket_physical_id: String,
    pub ticket_digest: String,
    pub resource_physical_id: String,
    /// Complete actual fact slice, excluding the report itself. The final ticket
    /// has already been written when this is derived; no future placeholders or
    /// whole shared ExposureLedger are part of the snapshot.
    pub nodes: Vec<FormalDependencyNode>,
}

fn validate_node_shape(nodes: &[FormalDependencyNode]) -> Result<()> {
    if nodes.is_empty() || nodes.len() > MAX_UPSTREAM_CLOSURE_NODES {
        return Err(Error::Invalid(
            "invalid formal dependency node count".into(),
        ));
    }
    let mut previous = None;
    for node in nodes {
        identifier(&node.id)?;
        identifier(&node.logical_id)?;
        identifier(&node.schema_version)?;
        digest(&node.content_digest, "dependency content_digest")?;
        digest(&node.outgoing_edge_digest, "outgoing_edge_digest")?;
        if !matches!(node.kind.as_str(), "run" | "artifact")
            || node.outgoing_edge_count > MAX_UPSTREAM_CLOSURE_NODES
        {
            return Err(Error::Invalid("invalid formal dependency identity".into()));
        }
        match (&node.record_kind, node.schema_version.as_str()) {
            (None, "rsia.optimization.source.v1") if node.kind == "run" => {}
            (None, RELEASE_CANDIDATE_SCHEMA) if node.kind == "artifact" => {}
            (Some(kind), ENVELOPE_SCHEMA)
                if node.kind == "artifact" && dependency_record_schema(kind).is_some() =>
            {
                if storage_id(kind, &node.logical_id)? != node.id {
                    return Err(Error::Invalid(
                        "dependency physical identity differs".into(),
                    ));
                }
            }
            _ => {
                return Err(Error::Invalid(
                    "unsupported formal dependency schema".into(),
                ));
            }
        }
        if node.record_kind.is_none() && node.logical_id != node.id {
            return Err(Error::Invalid("dependency logical identity differs".into()));
        }
        let key = (&node.kind, &node.id);
        if previous.is_some_and(|prior| prior >= key) {
            return Err(Error::Invalid(
                "formal dependency nodes must be canonical".into(),
            ));
        }
        previous = Some(key);
    }
    Ok(())
}

impl FormalCandidateDependencyBinding {
    fn validate(&self) -> Result<()> {
        if self.schema_version != CANDIDATE_BINDING_SCHEMA {
            return Err(Error::Invalid(
                "unsupported formal candidate binding schema".into(),
            ));
        }
        for id in [
            &self.namespace,
            &self.candidate_id,
            &self.profile_id,
            &self.proposer_actor,
        ] {
            identifier(id)?;
        }
        for value in [
            &self.candidate_record_digest,
            &self.bundle_digest,
            &self.parent_digest,
            &self.environment_digest,
            &self.revoke_watermark_digest,
        ] {
            digest(value, "candidate dependency digest")?;
        }
        if self.revoke_watermark_seq < 0 || self.sources.is_empty() {
            return Err(Error::Invalid(
                "candidate binding lacks source or watermark identity".into(),
            ));
        }
        let mut previous = None;
        for source in &self.sources {
            identifier(&source.id)?;
            digest(&source.content_digest, "source content_digest")?;
            let key = (&source.kind, &source.id);
            if source.kind != "run" || previous.is_some_and(|prior| prior >= key) {
                return Err(Error::Invalid(
                    "candidate binding sources must be canonical and distinct".into(),
                ));
            }
            previous = Some(key);
        }
        validate_node_shape(&self.nodes)?;
        if !self.nodes.iter().any(|node| {
            node.kind == "artifact"
                && node.id == self.candidate_id
                && node.schema_version == RELEASE_CANDIDATE_SCHEMA
                && node.content_digest == self.candidate_record_digest
        }) || self.sources.iter().any(|source| {
            !self
                .nodes
                .iter()
                .any(|node| node.kind == source.kind && node.id == source.id)
        }) {
            return Err(Error::Invalid(
                "candidate binding lacks complete source identity".into(),
            ));
        }
        Ok(())
    }
}

impl FormalStoredDependencyBinding {
    fn validate(&self) -> Result<()> {
        if self.schema_version != STORED_BINDING_SCHEMA {
            return Err(Error::Invalid(
                "unsupported formal stored binding schema".into(),
            ));
        }
        identifier(&self.namespace)?;
        identifier(&self.ticket_id)?;
        digest(&self.candidate_binding_digest, "candidate_binding_digest")?;
        digest(&self.ticket_digest, "stored ticket_digest")?;
        if self.ticket_physical_id != storage_id(TICKET_KIND, &self.ticket_id)?
            || self.resource_physical_id != storage_id(RESOURCE_KIND, &self.ticket_id)?
        {
            return Err(Error::Invalid(
                "stored binding physical identity differs".into(),
            ));
        }
        validate_node_shape(&self.nodes)?;
        for (id, kind) in [
            (&self.ticket_physical_id, TICKET_KIND),
            (&self.resource_physical_id, RESOURCE_KIND),
        ] {
            if !self.nodes.iter().any(|node| {
                node.kind == "artifact"
                    && &node.id == id
                    && node.record_kind.as_deref() == Some(kind)
            }) {
                return Err(Error::Invalid(
                    "stored binding lacks required actual fact".into(),
                ));
            }
        }
        Ok(())
    }
}

fn deserialize_candidate_binding<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<FormalCandidateDependencyBinding>, D::Error> {
    // Deserialize T, not Option<T>: a present null must fail. #[serde(default)]
    // alone handles the historical absent field, preserving its serialized bytes.
    let binding = FormalCandidateDependencyBinding::deserialize(deserializer)?;
    binding.validate().map_err(D::Error::custom)?;
    Ok(Some(binding))
}

fn deserialize_stored_binding<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<FormalStoredDependencyBinding>, D::Error> {
    let binding = FormalStoredDependencyBinding::deserialize(deserializer)?;
    binding.validate().map_err(D::Error::custom)?;
    Ok(Some(binding))
}

fn dependency_record_schema(kind: &str) -> Option<&'static str> {
    match kind {
        CONTROL_KIND => Some(RegisteredEvaluationControl::SCHEMA),
        HOLDOUT_KIND => Some(ProtectedHoldoutRecord::SCHEMA),
        TICKET_KIND => Some(EvaluationTicketV2::SCHEMA),
        TICKET_RECEIPT_KIND => Some("rsia.evaluation_ticket_issue_receipt.v1"),
        ALPHA_USE_KIND => Some("rsia.alpha_claim_reservation.v1"),
        EXECUTION_RECEIPT_KIND => Some("rsia.execution_receipt.v2"),
        GRADER_RECEIPT_KIND => Some("rsia.independent_grader_receipt.v2"),
        OUTPUT_KIND => Some("rsia.execution_output.v1"),
        RESOURCE_KIND => Some("rsia.evaluation_resource_evidence.v1"),
        // Formal reports and shared exposure ledgers cannot be upstream facts of
        // their own report. No arbitrary schema or SQL budget row is accepted.
        _ => None,
    }
}

fn validate_dependency_payload_identity(kind: &str, payload: &Value) -> Result<()> {
    let (ids, digests, timestamp): (&[&str], &[&str], Option<&str>) = match kind {
        TICKET_RECEIPT_KIND => (
            &["ticket_id", "evaluator_actor"],
            &["registration_digest", "manifest_digest"],
            Some("issued_at_unix_seconds"),
        ),
        ALPHA_USE_KIND => (
            &["research_family_id", "claim_id", "attempt_id", "ticket_id"],
            &[],
            None,
        ),
        EXECUTION_RECEIPT_KIND => (
            &[
                "receipt_id",
                "ticket_id",
                "target_id",
                "output_artifact_id",
                "budget_call_id",
                "executor_actor",
            ],
            &[
                "request_digest",
                "bundle_digest",
                "environment_digest",
                "output_digest",
            ],
            Some("issued_at_unix_seconds"),
        ),
        GRADER_RECEIPT_KIND => (
            &[
                "receipt_id",
                "ticket_id",
                "target_id",
                "candidate_execution_receipt_id",
                "baseline_execution_receipt_id",
                "evaluator_actor",
            ],
            &[
                "candidate_output_digest",
                "baseline_output_digest",
                "oracle_digest",
                "grader_digest",
                "scored_output_pair_digest",
            ],
            Some("issued_at_unix_seconds"),
        ),
        OUTPUT_KIND => (&["id", "ticket_id", "target_id"], &["output_digest"], None),
        RESOURCE_KIND => (&["ticket_id", "billing_scope"], &[], None),
        // These full native validators also validate the rest of their contracts.
        CONTROL_KIND | HOLDOUT_KIND | TICKET_KIND => (&[], &[], None),
        _ => {
            return Err(Error::Invalid(
                "unsupported formal dependency record kind".into(),
            ));
        }
    };
    for field in ids {
        identifier(
            payload
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
        )?;
    }
    for field in digests {
        digest(
            payload
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
            field,
        )?;
    }
    if let Some(field) = timestamp {
        valid_time(
            payload
                .get(field)
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
        )?;
    }
    if kind == GRADER_RECEIPT_KIND {
        for field in ["candidate_score_micros", "baseline_score_micros"] {
            if payload
                .get(field)
                .and_then(Value::as_u64)
                .is_none_or(|value| value > 1_000_000)
            {
                return Err(Error::Invalid(
                    "formal dependency score is out of bounds".into(),
                ));
            }
        }
    }
    Ok(())
}

async fn typed_node_fingerprint<T: DeserializeOwned + Serialize>(
    session: &mut Session,
    ctx: &Context,
    physical_id: &str,
    kind: &str,
    logical_id: &str,
) -> Result<String> {
    let envelope: TypedArtifactEnvelope<T> = session.need(ctx, "artifact", physical_id).await?;
    if envelope.schema_version != ENVELOPE_SCHEMA
        || envelope.id != physical_id
        || envelope.record_kind != kind
        || storage_id(kind, logical_id)? != physical_id
    {
        return Err(Error::Conflict(
            "formal dependency typed identity mismatch".into(),
        ));
    }
    fingerprint(&envelope)
}

async fn dependency_node_identity(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Result<(String, Option<String>, String, String)> {
    if judge_dependency(ctx, session, kind, id).await?.is_some() {
        return Err(Error::Forbidden);
    }
    if kind == "run" {
        let authority = load_stored_source(session, ctx, id).await?;
        if authority.record.id != id {
            return Err(Error::Conflict("source authority id differs".into()));
        }
        if serde_json::to_vec(&authority)
            .map_err(|_| Error::Internal)?
            .len()
            > 4 * 1024 * 1024
        {
            return Err(Error::Invalid("object exceeds 4 MiB".into()));
        }
        return Ok((
            authority.schema_version.clone(),
            None,
            id.into(),
            fingerprint(&authority)?,
        ));
    }
    if kind != "artifact" {
        return Err(Error::Invalid("unsupported formal dependency kind".into()));
    }
    let value: Value = session.need(ctx, kind, id).await?;
    if serde_json::to_vec(&value)
        .map_err(|_| Error::Internal)?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(Error::Invalid("object exceeds 4 MiB".into()));
    }
    let schema = value
        .get("schema_version")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("formal dependency has no schema".into()))?;
    if schema == RELEASE_CANDIDATE_SCHEMA {
        let candidate: ReleaseCandidateRecord = session.need(ctx, kind, id).await?;
        if candidate.id != id {
            return Err(Error::Conflict(
                "candidate physical identity differs".into(),
            ));
        }
        validate_candidate_read_in_session(session, ctx, &candidate).await?;
        return Ok((schema.into(), None, id.into(), fingerprint(&candidate)?));
    }
    if schema != ENVELOPE_SCHEMA {
        return Err(Error::Invalid(
            "unsupported formal dependency schema".into(),
        ));
    }
    let envelope: TypedArtifactEnvelope<Value> = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("invalid formal typed dependency".into()))?;
    let record_kind = envelope.record_kind.as_str();
    let expected_schema = dependency_record_schema(record_kind)
        .ok_or_else(|| Error::Invalid("unsupported formal dependency record kind".into()))?;
    let payload = &envelope.payload;
    let actual_schema = payload.get("schema_version").and_then(Value::as_str);
    if actual_schema != Some(expected_schema)
        && !(record_kind == CONTROL_KIND
            && actual_schema == Some(RegisteredEvaluationControl::SCHEMA_V1))
    {
        return Err(Error::Invalid(
            "unsupported formal dependency inner schema".into(),
        ));
    }
    validate_dependency_payload_identity(record_kind, payload)?;
    let logical_field = match record_kind {
        CONTROL_KIND | HOLDOUT_KIND | TICKET_KIND | OUTPUT_KIND => "id",
        EXECUTION_RECEIPT_KIND | GRADER_RECEIPT_KIND => "receipt_id",
        _ => "ticket_id",
    };
    let logical_id = if record_kind == ALPHA_USE_KIND {
        let family = payload
            .get("research_family_id")
            .and_then(Value::as_str)
            .ok_or(Error::Internal)?;
        let claim = payload
            .get("claim_id")
            .and_then(Value::as_str)
            .ok_or(Error::Internal)?;
        identifier(family)?;
        identifier(claim)?;
        format!("{family}:{claim}")
    } else {
        payload
            .get(logical_field)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("formal dependency lacks logical identity".into()))?
            .into()
    };
    let content = match record_kind {
        CONTROL_KIND => {
            let control: RegisteredEvaluationControl =
                need_record(session, ctx, CONTROL_KIND, &logical_id).await?;
            control.validate()?;
            typed_node_fingerprint::<RegisteredEvaluationControl>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        HOLDOUT_KIND => {
            let holdout: ProtectedHoldoutRecord =
                need_record(session, ctx, HOLDOUT_KIND, &logical_id).await?;
            let control: RegisteredEvaluationControl =
                need_record(session, ctx, CONTROL_KIND, &holdout.registration_id).await?;
            holdout.validate_against(&control)?;
            typed_node_fingerprint::<ProtectedHoldoutRecord>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        TICKET_KIND => {
            let ticket: EvaluationTicketV2 =
                need_record(session, ctx, TICKET_KIND, &logical_id).await?;
            ticket.validate()?;
            typed_node_fingerprint::<EvaluationTicketV2>(session, ctx, id, record_kind, &logical_id)
                .await?
        }
        TICKET_RECEIPT_KIND => {
            typed_node_fingerprint::<TicketIssueReceipt>(session, ctx, id, record_kind, &logical_id)
                .await?
        }
        ALPHA_USE_KIND => {
            typed_node_fingerprint::<AlphaClaimReservation>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        EXECUTION_RECEIPT_KIND => {
            typed_node_fingerprint::<ExecutionReceiptV2>(session, ctx, id, record_kind, &logical_id)
                .await?
        }
        GRADER_RECEIPT_KIND => {
            typed_node_fingerprint::<IndependentGraderReceiptV2>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        OUTPUT_KIND => {
            let output: ExecutionOutputArtifact =
                need_record(session, ctx, OUTPUT_KIND, &logical_id).await?;
            if output.output_digest != hash(output.output_utf8.as_bytes())
                || output.output_utf8.is_empty()
                || output.output_utf8.len() > 256 * 1024
            {
                return Err(Error::Conflict(
                    "formal dependency output bytes changed".into(),
                ));
            }
            typed_node_fingerprint::<ExecutionOutputArtifact>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        RESOURCE_KIND => {
            let resource: ResourceEvidenceRecord =
                need_record(session, ctx, RESOURCE_KIND, &logical_id).await?;
            if resource.cost_scope != CostEvidenceScope::TicketExecutionOnly
                || resource.calls.is_empty()
            {
                return Err(Error::Invalid(
                    "formal resource identity is incomplete".into(),
                ));
            }
            let mut call_ids = BTreeSet::new();
            for expected in &resource.calls {
                identifier(&expected.call_id)?;
                if expected.dispatch_group_id != resource.ticket_id
                    || expected.billing_scope != resource.billing_scope
                    || !call_ids.insert(&expected.call_id)
                {
                    return Err(Error::Conflict(
                        "formal resource call identity differs".into(),
                    ));
                }
                let actual = session
                    .budget_call(ctx, &resource.billing_scope, &expected.call_id)
                    .await?
                    .ok_or(Error::NotFound)?;
                if fingerprint(&actual)? != fingerprint(expected)? {
                    return Err(Error::Conflict("actual resource evidence changed".into()));
                }
            }
            typed_node_fingerprint::<ResourceEvidenceRecord>(
                session,
                ctx,
                id,
                record_kind,
                &logical_id,
            )
            .await?
        }
        _ => {
            return Err(Error::Invalid(
                "unsupported formal dependency record kind".into(),
            ));
        }
    };
    Ok((
        ENVELOPE_SCHEMA.into(),
        Some(envelope.record_kind),
        logical_id,
        content,
    ))
}

async fn collect_dependency_nodes(
    session: &mut Session,
    ctx: &Context,
    roots: Vec<(String, String)>,
) -> Result<Vec<FormalDependencyNode>> {
    let mut seen = BTreeSet::new();
    let mut pending: BTreeSet<_> = roots.into_iter().collect();
    let mut nodes = Vec::new();
    while let Some((kind, id)) = pending.pop_first() {
        if !seen.insert((kind.clone(), id.clone())) {
            continue;
        }
        // Unknown and missing typed nodes count too, before their refusal. Cycles
        // and diamonds deduplicate over kind+physical id, across all roots.
        if seen.len() > MAX_UPSTREAM_CLOSURE_NODES {
            return Err(Error::Conflict(
                "formal dependency closure exceeds 10000 nodes; refused, not truncated".into(),
            ));
        }
        let edges = session
            .outgoing_edges_bounded(ctx, &kind, &id, MAX_UPSTREAM_CLOSURE_NODES)
            .await?;
        for edge in &edges {
            if !seen.contains(edge) {
                pending.insert(edge.clone());
            }
        }
        if seen.len() + pending.len() > MAX_UPSTREAM_CLOSURE_NODES {
            return Err(Error::Conflict(
                "formal dependency closure exceeds 10000 nodes; refused, not truncated".into(),
            ));
        }
        let (schema_version, record_kind, logical_id, content_digest) =
            dependency_node_identity(session, ctx, &kind, &id).await?;
        nodes.push(FormalDependencyNode {
            kind,
            id,
            schema_version,
            record_kind,
            logical_id,
            content_digest,
            outgoing_edge_digest: fingerprint(&edges)?,
            outgoing_edge_count: edges.len(),
        });
    }
    nodes.sort_by(|left, right| (&left.kind, &left.id).cmp(&(&right.kind, &right.id)));
    Ok(nodes)
}

fn candidate_matches_frozen(
    candidate: &ReleaseCandidateRecord,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
) -> Result<()> {
    if candidate.id != holdout.pair.candidate_digest
        || candidate.bundle_digest != holdout.candidate_bundle_digest
        || candidate.parent_digest != holdout.pair.baseline_digest
        || candidate.environment_digest != holdout.manifest.environment_digest
        || candidate.proposer_actor != control.proposer_actor
    {
        return Err(Error::Conflict(
            "formal candidate differs from the frozen pair".into(),
        ));
    }
    Ok(())
}

async fn bind_candidate_in_session(
    session: &mut Session,
    ctx: &Context,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
) -> Result<Option<FormalCandidateDependencyBinding>> {
    // Presence is decided once at issue, never by a subsequent read of an old
    // unbound ticket. A malformed present candidate cannot become legacy None.
    let Some(candidate) = session
        .get::<ReleaseCandidateRecord>(ctx, "artifact", &holdout.pair.candidate_digest)
        .await?
    else {
        return Ok(None);
    };
    candidate_matches_frozen(&candidate, control, holdout)?;
    // Tombstones are checked before watermark/body/edge traversal, including
    // begin_revoke where the source body and all dependent objects still exist.
    for source in &candidate.sources {
        if judge_dependency(ctx, session, &source.kind, &source.id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    validate_candidate_read_in_session(session, ctx, &candidate).await?;
    let watermark = session
        .watermark(ctx)
        .await?
        .ok_or_else(|| Error::Conflict("missing source revoke watermark".into()))?;
    digest(&watermark.1, "revoke watermark digest")?;
    let edges = session
        .outgoing_edges_bounded(ctx, "artifact", &candidate.id, MAX_UPSTREAM_CLOSURE_NODES)
        .await?;
    for source in &candidate.sources {
        if !edges.contains(&(source.kind.clone(), source.id.clone())) {
            return Err(Error::Conflict(
                "candidate lacks an actual direct source edge".into(),
            ));
        }
    }
    let binding = FormalCandidateDependencyBinding {
        schema_version: CANDIDATE_BINDING_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        candidate_id: candidate.id.clone(),
        candidate_record_digest: fingerprint(&candidate)?,
        bundle_digest: candidate.bundle_digest.clone(),
        parent_digest: candidate.parent_digest.clone(),
        profile_id: candidate.profile_id.clone(),
        environment_digest: candidate.environment_digest.clone(),
        proposer_actor: candidate.proposer_actor.clone(),
        sources: candidate.sources.clone(),
        revoke_watermark_seq: watermark.0,
        revoke_watermark_digest: watermark.1,
        nodes: collect_dependency_nodes(session, ctx, vec![("artifact".into(), candidate.id)])
            .await?,
    };
    binding.validate()?;
    Ok(Some(binding))
}

async fn validate_candidate_binding_in_session(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
    holdout: &ProtectedHoldoutRecord,
) -> Result<()> {
    let Some(binding) = &ticket.candidate_dependency_binding else {
        return Ok(());
    };
    binding.validate()?;
    let candidate: ReleaseCandidateRecord =
        session.need(ctx, "artifact", &binding.candidate_id).await?;
    // Full immutable pin FIRST: stripping S2 while keeping its edges, watermark
    // and bundle cannot silently change the frozen source set or its authority.
    if fingerprint(&candidate)? != binding.candidate_record_digest {
        return Err(Error::Conflict(CANDIDATE_CHANGED.into()));
    }
    for source in &binding.sources {
        if judge_dependency(ctx, session, &source.kind, &source.id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    if binding.namespace != ctx.namespace()
        || binding.candidate_id != ticket.candidate_digest
        || binding.bundle_digest != ticket.candidate_bundle_digest
        || binding.parent_digest != ticket.baseline_digest
        || binding.environment_digest != ticket.environment_digest
        || binding.proposer_actor != ticket.proposer_actor
    {
        return Err(Error::Conflict(CANDIDATE_CHANGED.into()));
    }
    let current = bind_candidate_in_session(session, ctx, control, holdout).await?;
    if current.as_ref() != Some(binding) {
        return Err(Error::Conflict(CANDIDATE_CHANGED.into()));
    }
    Ok(())
}

async fn put_fact_edges(
    session: &mut Session,
    ctx: &Context,
    src_id: &str,
    edges: Vec<(String, String)>,
) -> Result<()> {
    for (kind, id) in edges {
        session
            .put_edge(ctx, "artifact", src_id, &kind, &id)
            .await?;
    }
    Ok(())
}

fn artifact_edge(kind: &str, logical_id: &str) -> Result<(String, String)> {
    Ok(("artifact".into(), storage_id(kind, logical_id)?))
}

async fn require_exact_fact_edges(
    session: &mut Session,
    ctx: &Context,
    src_id: &str,
    mut expected: Vec<(String, String)>,
) -> Result<()> {
    expected.sort();
    expected.dedup();
    if session
        .outgoing_edges_bounded(ctx, "artifact", src_id, MAX_UPSTREAM_CLOSURE_NODES)
        .await?
        != expected
    {
        return Err(Error::Conflict(
            "formal fact dependency edges differ".into(),
        ));
    }
    Ok(())
}

async fn required_ticket_edges(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
) -> Result<Vec<(String, String)>> {
    let mut edges = vec![
        ("artifact".into(), ticket.candidate_digest.clone()),
        artifact_edge(HOLDOUT_KIND, &ticket.holdout_id)?,
        artifact_edge(TICKET_RECEIPT_KIND, &ticket.id)?,
    ];
    for allocation in control
        .alpha_plan
        .allocations
        .iter()
        .filter(|item| item.attempt_id == ticket.attempt_id)
    {
        let id = format!(
            "{}:{}",
            control.alpha_plan.research_family_id, allocation.claim_id
        );
        let actual: AlphaClaimReservation = need_record(session, ctx, ALPHA_USE_KIND, &id).await?;
        if actual.schema_version != "rsia.alpha_claim_reservation.v1"
            || actual.research_family_id != ticket.research_family_id
            || actual.claim_id != allocation.claim_id
            || actual.attempt_id != ticket.attempt_id
            || actual.ticket_id != ticket.id
        {
            return Err(Error::Conflict(
                "formal alpha reservation identity differs".into(),
            ));
        }
        edges.push(artifact_edge(ALPHA_USE_KIND, &id)?);
    }
    for target in &ticket.targets {
        match &target.state {
            TargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } => {
                for id in [candidate_receipt_id, baseline_receipt_id] {
                    edges.push(artifact_edge(EXECUTION_RECEIPT_KIND, id)?);
                }
                edges.push(artifact_edge(GRADER_RECEIPT_KIND, grader_receipt_id)?);
            }
            TargetState::Running {
                candidate,
                baseline,
            }
            | TargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | TargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    if let Some(id) = &slot.receipt_id {
                        edges.push(artifact_edge(EXECUTION_RECEIPT_KIND, id)?);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(edges)
}

async fn validate_bound_fact_edges(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    control: &RegisteredEvaluationControl,
) -> Result<()> {
    if ticket.candidate_dependency_binding.is_none() {
        return Ok(());
    }
    let ticket_edges = required_ticket_edges(session, ctx, ticket, control).await?;
    require_exact_fact_edges(
        session,
        ctx,
        &storage_id(TICKET_KIND, &ticket.id)?,
        ticket_edges,
    )
    .await?;
    require_exact_fact_edges(
        session,
        ctx,
        &storage_id(HOLDOUT_KIND, &ticket.holdout_id)?,
        vec![artifact_edge(CONTROL_KIND, &ticket.registration_id)?],
    )
    .await?;
    for target in &ticket.targets {
        let mut receipt_ids = Vec::new();
        match &target.state {
            TargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } => {
                receipt_ids.extend([candidate_receipt_id, baseline_receipt_id]);
                require_exact_fact_edges(
                    session,
                    ctx,
                    &storage_id(GRADER_RECEIPT_KIND, grader_receipt_id)?,
                    vec![
                        artifact_edge(EXECUTION_RECEIPT_KIND, candidate_receipt_id)?,
                        artifact_edge(EXECUTION_RECEIPT_KIND, baseline_receipt_id)?,
                    ],
                )
                .await?;
            }
            TargetState::Running {
                candidate,
                baseline,
            }
            | TargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | TargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                receipt_ids.extend(
                    [candidate, baseline]
                        .into_iter()
                        .flatten()
                        .filter_map(|slot| slot.receipt_id.as_ref()),
                );
            }
            _ => {}
        }
        for id in receipt_ids {
            let receipt: ExecutionReceiptV2 =
                need_record(session, ctx, EXECUTION_RECEIPT_KIND, id).await?;
            require_exact_fact_edges(
                session,
                ctx,
                &storage_id(EXECUTION_RECEIPT_KIND, id)?,
                vec![
                    artifact_edge(TICKET_KIND, &ticket.id)?,
                    artifact_edge(OUTPUT_KIND, &receipt.output_artifact_id)?,
                ],
            )
            .await?;
        }
    }
    Ok(())
}

async fn bind_stored_facts_in_session(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
) -> Result<Option<FormalStoredDependencyBinding>> {
    let Some(candidate) = &ticket.candidate_dependency_binding else {
        return Ok(None);
    };
    let ticket_physical_id = storage_id(TICKET_KIND, &ticket.id)?;
    let resource_physical_id = storage_id(RESOURCE_KIND, &ticket.id)?;
    let binding = FormalStoredDependencyBinding {
        schema_version: STORED_BINDING_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        candidate_binding_digest: fingerprint(candidate)?,
        ticket_id: ticket.id.clone(),
        ticket_digest: fingerprint(ticket)?,
        nodes: collect_dependency_nodes(
            session,
            ctx,
            vec![
                ("artifact".into(), ticket_physical_id.clone()),
                ("artifact".into(), resource_physical_id.clone()),
            ],
        )
        .await?,
        ticket_physical_id,
        resource_physical_id,
    };
    binding.validate()?;
    Ok(Some(binding))
}

async fn control_billing_scope(
    store: &Store,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
) -> Result<String> {
    let control: RegisteredEvaluationControl =
        load(store, ctx, CONTROL_KIND, &ticket.registration_id).await?;
    Ok(control.billing_scope)
}

async fn validate_root_budget(
    ctx: &Context,
    store: &Store,
    control: &RegisteredEvaluationControl,
) -> Result<()> {
    if control.billing_scope != control.optimization_budget.root_budget_scope_id {
        return Err(Error::Invalid(
            "registered billing scope differs from the formal root budget".into(),
        ));
    }
    let root = store
        .root_budget(ctx, &control.billing_scope)
        .await?
        .ok_or(Error::Budget)?;
    let receipt = control
        .optimization_budget
        .admin_authorization_receipt_digest
        .as_deref()
        .ok_or(Error::Budget)?;
    let (negative, units) = parse_decimal(&control.optimization_budget.root_total.amount)?;
    if negative || units == 0 || units % 100 != 0 {
        return Err(Error::Invalid(
            "formal root budget must be positive and exactly representable in micros".into(),
        ));
    }
    let planned_micros = i64::try_from(units / 100)
        .map_err(|_| Error::Invalid("formal root budget exceeds i64 micros".into()))?;
    if root.authorizing_namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    if root.stopped
        || root.currency != control.optimization_budget.root_total.currency
        || root.pricing_version != control.optimization_budget.root_total.pricing_version
        || root.total_limit_micros != planned_micros
        || root.authorization_receipt_digest != receipt
    {
        return Err(Error::Conflict(
            "persistent root budget differs from the registered formal budget".into(),
        ));
    }
    Ok(())
}

fn validate_target_receipt_pair(
    target: &EvaluationTarget,
    candidate_receipt_id: &str,
    baseline_receipt_id: &str,
) -> Result<()> {
    match &target.state {
        TargetState::Running {
            candidate: Some(candidate),
            baseline: Some(baseline),
        } if candidate.receipt_id.as_deref() == Some(candidate_receipt_id)
            && baseline.receipt_id.as_deref() == Some(baseline_receipt_id) =>
        {
            Ok(())
        }
        TargetState::Completed { .. } => Err(Error::Conflict(
            "completed target score is immutable".into(),
        )),
        _ => Err(Error::Invalid(
            "grader requires both execution receipts recorded on the target".into(),
        )),
    }
}

fn validate_output_artifact(
    receipt: &ExecutionReceiptV2,
    output: &ExecutionOutputArtifact,
) -> Result<()> {
    if output.schema_version != "rsia.execution_output.v1"
        || output.id != receipt.output_artifact_id
        || output.ticket_id != receipt.ticket_id
        || output.target_id != receipt.target_id
        || output.side != receipt.side
        || output.output_digest != receipt.output_digest
        || hash(output.output_utf8.as_bytes()) != receipt.output_digest
    {
        return Err(Error::Conflict(
            "execution output artifact does not match its receipt".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactJsonAnswer {
    answer: Value,
}

struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: DeError,
    {
        Number::from_f64(value)
            .map(|number| StrictJsonValue(Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate JSON key: {key}")));
            }
            let value = object.next_value::<StrictJsonValue>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

pub(crate) fn grade_fixed_output(
    grader: &FixedGraderSpec,
    output_utf8: &str,
    expected_answer_json: &str,
) -> Result<u32> {
    grader.validate()?;
    let output = parse_strict_json(output_utf8)?;
    let Value::Object(mut object) = output.0 else {
        return Err(Error::Invalid(
            "execution output violates fixed grader schema".into(),
        ));
    };
    if object.len() != 1 || !object.contains_key("answer") {
        return Err(Error::Invalid(
            "execution output violates fixed grader schema".into(),
        ));
    }
    let output = ExactJsonAnswer {
        answer: object.remove("answer").ok_or(Error::Internal)?,
    };
    let expected_answer = parse_strict_json(expected_answer_json)?.0;
    match grader.method {
        FixedGraderMethod::ExactJsonAnswerV1 => Ok(if output.answer == expected_answer {
            1_000_000
        } else {
            0
        }),
    }
}

fn parse_strict_json(value: &str) -> Result<StrictJsonValue> {
    let mut deserializer = serde_json::Deserializer::from_str(value);
    let parsed = StrictJsonValue::deserialize(&mut deserializer)
        .map_err(|_| Error::Invalid("JSON contains invalid or duplicate fields".into()))?;
    deserializer
        .end()
        .map_err(|_| Error::Invalid("JSON contains trailing content".into()))?;
    Ok(parsed)
}

fn request_digest(
    ticket: &EvaluationTicketV2,
    target: &EvaluationTarget,
    side: ExecutionSide,
    bundle_digest: &str,
    environment_digest: &str,
) -> Result<String> {
    fingerprint(&(
        "rsia.evaluation_execution_request.v1",
        &ticket.id,
        &target.target_id,
        &target.input_digest,
        side,
        bundle_digest,
        environment_digest,
        &ticket.manifest_digest,
    ))
}

fn broker_view(
    ticket: &EvaluationTicketV2,
    target_id: &str,
    side: ExecutionSide,
) -> Result<BrokerTaskView> {
    let target = ticket.target(target_id)?;
    let bundle_digest = match side {
        ExecutionSide::Candidate => ticket.candidate_bundle_digest.clone(),
        ExecutionSide::Baseline => ticket.baseline_bundle_digest.clone(),
    };
    let request_digest = request_digest(
        ticket,
        target,
        side,
        &bundle_digest,
        &ticket.environment_digest,
    )?;
    Ok(BrokerTaskView {
        ticket_id: ticket.id.clone(),
        target_id: target_id.into(),
        input_artifact_id: target.input_artifact_id.clone(),
        input_digest: target.input_digest.clone(),
        side,
        bundle_digest,
        environment_digest: ticket.environment_digest.clone(),
        request_digest,
    })
}

fn bind_slot(target: &mut EvaluationTarget, binding: ExecutionSlotBinding) -> Result<()> {
    let (mut candidate, mut baseline) = match &target.state {
        TargetState::Planned => (None, None),
        TargetState::Running {
            candidate,
            baseline,
        } => (candidate.clone(), baseline.clone()),
        _ => {
            return Err(Error::Conflict(
                "target cannot bind another execution slot".into(),
            ));
        }
    };
    let slot = match binding.side {
        ExecutionSide::Candidate => &mut candidate,
        ExecutionSide::Baseline => &mut baseline,
    };
    if slot.is_some() {
        return Err(Error::Conflict("execution side is already bound".into()));
    }
    *slot = Some(binding);
    target.state = TargetState::Running {
        candidate,
        baseline,
    };
    Ok(())
}

fn mark_slot_dispatched(
    target: &mut EvaluationTarget,
    side: ExecutionSide,
    call_id: &str,
    dispatch_id: &str,
    now: i64,
) -> Result<()> {
    let TargetState::Running {
        candidate,
        baseline,
    } = &mut target.state
    else {
        return Err(Error::Conflict("target execution slot is not bound".into()));
    };
    let slot = match side {
        ExecutionSide::Candidate => candidate,
        ExecutionSide::Baseline => baseline,
    }
    .as_mut()
    .ok_or_else(|| Error::Conflict("execution side is not bound".into()))?;
    if slot.budget_call_id != call_id || !matches!(slot.phase, ExecutionSlotPhase::ReservedBound) {
        return Err(Error::Conflict("execution slot binding changed".into()));
    }
    slot.phase = ExecutionSlotPhase::Dispatched {
        dispatch_id: dispatch_id.into(),
        dispatched_at_unix_seconds: now,
    };
    Ok(())
}

fn validate_started_slot(
    target: &EvaluationTarget,
    side: ExecutionSide,
    call_id: &str,
    request_digest: &str,
) -> Result<()> {
    let (candidate, baseline) = match &target.state {
        TargetState::Running {
            candidate,
            baseline,
        }
        | TargetState::RunningUsageUnknown {
            candidate,
            baseline,
        }
        | TargetState::CancelledAfterDispatch {
            candidate,
            baseline,
        } => (candidate, baseline),
        _ => {
            return Err(Error::Conflict(
                "execution target has no started slot".into(),
            ));
        }
    };
    let slot = match side {
        ExecutionSide::Candidate => candidate,
        ExecutionSide::Baseline => baseline,
    }
    .as_ref()
    .ok_or_else(|| Error::Conflict("execution side has no started slot".into()))?;
    if slot.budget_call_id != call_id
        || slot.request_digest != request_digest
        || !matches!(slot.phase, ExecutionSlotPhase::Dispatched { .. })
    {
        return Err(Error::Conflict(
            "execution receipt differs from the persisted dispatch slot".into(),
        ));
    }
    Ok(())
}

fn set_running_receipt(
    target: &mut EvaluationTarget,
    side: ExecutionSide,
    receipt_id: &str,
) -> Result<()> {
    let (mut candidate, mut baseline) = match &target.state {
        TargetState::Running {
            candidate,
            baseline,
        } => (candidate.clone(), baseline.clone()),
        _ => {
            return Err(Error::Conflict(
                "target cannot accept a new execution receipt".into(),
            ));
        }
    };
    let slot = match side {
        ExecutionSide::Candidate => &mut candidate,
        ExecutionSide::Baseline => &mut baseline,
    }
    .as_mut()
    .ok_or_else(|| Error::Conflict("execution side is not bound".into()))?;
    if slot.receipt_id.is_some() || !matches!(slot.phase, ExecutionSlotPhase::Dispatched { .. }) {
        return Err(Error::Conflict(
            "execution side cannot accept a receipt".into(),
        ));
    }
    slot.receipt_id = Some(receipt_id.into());
    target.state = TargetState::Running {
        candidate,
        baseline,
    };
    Ok(())
}

fn validate_execution_pair(
    ticket: &EvaluationTicketV2,
    target_id: &str,
    candidate: &ExecutionReceiptV2,
    baseline: &ExecutionReceiptV2,
) -> Result<()> {
    if candidate.ticket_id != ticket.id
        || baseline.ticket_id != ticket.id
        || candidate.target_id != target_id
        || baseline.target_id != target_id
        || candidate.side != ExecutionSide::Candidate
        || baseline.side != ExecutionSide::Baseline
        || candidate.bundle_digest != ticket.candidate_bundle_digest
        || baseline.bundle_digest != ticket.baseline_bundle_digest
        || candidate.environment_digest != ticket.environment_digest
        || baseline.environment_digest != ticket.environment_digest
    {
        return Err(Error::Conflict(
            "execution receipts do not form the frozen candidate/baseline pair".into(),
        ));
    }
    Ok(())
}

fn is_stopping(state: TicketLifecycle) -> bool {
    matches!(
        state,
        TicketLifecycle::StopRequested
            | TicketLifecycle::Draining
            | TicketLifecycle::EarlyStopped
            | TicketLifecycle::CompleteBatch
            | TicketLifecycle::Invalid
    )
}

fn rotating_complete(ticket: &EvaluationTicketV2) -> bool {
    ticket.targets.iter().all(|target| {
        !matches!(target.kind, EvaluationTargetKind::Rotating { .. })
            || matches!(target.state, TargetState::Completed { .. })
    })
}

fn anchors_passed(ticket: &EvaluationTicketV2) -> bool {
    ticket
        .targets
        .iter()
        .all(|target| match (&target.kind, &target.state) {
            (EvaluationTargetKind::Anchor { .. }, TargetState::Completed { anchor_result, .. }) => {
                *anchor_result == Some(AnchorResult::Passed)
            }
            (EvaluationTargetKind::Anchor { .. }, _) => false,
            _ => true,
        })
}

fn rebuild_stream(
    ticket: &EvaluationTicketV2,
    plan: &EarlyStopPlan,
    alpha: &ResearchFamilyAlphaPlan,
) -> Result<(EarlyStopDecision, SequentialRejectOnly)> {
    let mut stream = SequentialRejectOnly::open(plan.clone(), alpha)?;
    let mut completed: Vec<_> = ticket
        .targets
        .iter()
        .filter_map(|target| match (&target.kind, &target.state) {
            (
                EvaluationTargetKind::Rotating { cluster_id },
                TargetState::Completed {
                    candidate_score_micros,
                    baseline_score_micros,
                    completion_sequence,
                    ..
                },
            ) => Some((
                *completion_sequence,
                CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: *baseline_score_micros,
                    candidate_score_micros: *candidate_score_micros,
                },
            )),
            _ => None,
        })
        .collect();
    completed.sort_by_key(|(sequence, _)| *sequence);
    for (_, unit) in completed {
        let decision = stream.record_complete_unit(unit)?;
        if decision.is_terminal() {
            return Ok((decision, stream));
        }
    }
    Ok((stream.decision()?, stream))
}

async fn mark_exposure_dispatched_if_needed(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    at_unix_seconds: i64,
) -> Result<()> {
    let at_unix_ms = at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(|| Error::Invalid("dispatch timestamp conversion overflow".into()))?;
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let state = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .map(|entry| entry.state.clone())
        .ok_or(Error::NotFound)?;
    match state {
        ExposureState::Reserved => {
            ledger.mark_dispatched(&ticket.id, at_unix_ms)?;
            put_record(
                session,
                ctx,
                LEDGER_KIND,
                &ticket.research_family_id,
                ticket.evaluator_actor.as_str(),
                &ledger,
            )
            .await?;
        }
        ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. } => {}
    }
    Ok(())
}

async fn mark_feedback_used_if_needed(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    at_unix_seconds: i64,
) -> Result<()> {
    let at_unix_ms = at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(|| Error::Invalid("feedback timestamp conversion overflow".into()))?;
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .ok_or(Error::NotFound)?;
    match entry.state.clone() {
        ExposureState::Dispatched { .. } => {
            ledger.mark_feedback_used(&ticket.id, at_unix_ms)?;
            put_record(
                session,
                ctx,
                LEDGER_KIND,
                &ticket.research_family_id,
                ticket.evaluator_actor.as_str(),
                &ledger,
            )
            .await?;
        }
        ExposureState::FeedbackUsed { .. } => {}
        ExposureState::Reserved => {
            return Err(Error::Conflict(
                "grader feedback cannot precede real dispatch".into(),
            ));
        }
    }
    Ok(())
}

async fn settle_exposure_money(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
    usage_uncertain: bool,
) -> Result<()> {
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .ok_or(Error::NotFound)?;
    let mut changed = false;
    let exposure_state = entry.state.clone();
    let monetary_state = entry.monetary_state;
    match (&exposure_state, monetary_state) {
        (ExposureState::Reserved, MonetaryReservationState::HeldUndispatched)
            if !usage_uncertain =>
        {
            ledger.release_undispatched_money(&ticket.id)?;
            changed = true;
        }
        (ExposureState::Reserved, MonetaryReservationState::HeldUndispatched) => {
            return Err(Error::Conflict(
                "cannot release exposure money while dispatched usage is unknown".into(),
            ));
        }
        (
            ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. },
            MonetaryReservationState::DispatchedCostPending,
        ) if !usage_uncertain => {
            ledger.finalize_dispatched_cost(&ticket.id)?;
            changed = true;
        }
        (_, MonetaryReservationState::DispatchedCostPending) if usage_uncertain => {}
        (
            _,
            MonetaryReservationState::Finalized | MonetaryReservationState::ReleasedUndispatched,
        ) => {}
        _ => {
            return Err(Error::Conflict(
                "exposure monetary state is inconsistent at terminal settlement".into(),
            ));
        }
    }
    if changed {
        put_record(
            session,
            ctx,
            LEDGER_KIND,
            &ticket.research_family_id,
            ticket.evaluator_actor.as_str(),
            &ledger,
        )
        .await?;
    }
    Ok(())
}

async fn close_execution_slots(
    session: &mut Session,
    ctx: &Context,
    control: &RegisteredEvaluationControl,
    ticket: &mut EvaluationTicketV2,
    reason: &str,
    now: i64,
) -> Result<bool> {
    session
        .stop_dispatch_group(ctx, &control.billing_scope, &ticket.id, reason, now)
        .await?;
    let calls = session
        .budget_calls_for_group(ctx, &control.billing_scope, &ticket.id)
        .await?;
    let earliest_dispatch = calls
        .iter()
        .filter(|call| {
            matches!(
                call.state,
                BudgetCallState::Dispatched
                    | BudgetCallState::Uncertain
                    | BudgetCallState::Finalized
            )
        })
        .map(|call| {
            call.dispatched_at.ok_or_else(|| {
                Error::Conflict("dispatched budget call lacks dispatch timestamp".into())
            })
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .min();
    if let Some(dispatched_at) = earliest_dispatch {
        mark_exposure_dispatched_if_needed(session, ctx, ticket, dispatched_at).await?;
    }
    let mut by_id: BTreeMap<String, BudgetCallRecord> = calls
        .into_iter()
        .map(|call| (call.call_id.clone(), call))
        .collect();
    let mut attributed = BTreeSet::new();
    for target in &ticket.targets {
        match &target.state {
            TargetState::Running {
                candidate,
                baseline,
            }
            | TargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | TargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    attributed.insert(slot.budget_call_id.clone());
                }
            }
            TargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                ..
            } => {
                for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                    let receipt: ExecutionReceiptV2 =
                        need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
                    attributed.insert(receipt.budget_call_id);
                }
            }
            _ => {}
        }
    }
    let mut usage_uncertain = false;
    for call in by_id.values_mut() {
        if attributed.contains(&call.call_id) {
            continue;
        }
        ticket
            .unattributed_budget_call_ids
            .push(call.call_id.clone());
        if !ticket
            .invalid_reasons
            .iter()
            .any(|value| value == "unattributed_dispatch_group_call")
        {
            ticket
                .invalid_reasons
                .push("unattributed_dispatch_group_call".into());
        }
        match call.state {
            BudgetCallState::Reserved => {
                let fence = call_fence(call, now);
                *call = session
                    .release_undispatched_budget_call(ctx, &fence, "unattributed_ticket_stop")
                    .await?;
            }
            BudgetCallState::Dispatched | BudgetCallState::Uncertain => usage_uncertain = true,
            BudgetCallState::Finalized if !call.execution_closed => usage_uncertain = true,
            BudgetCallState::Finalized => {
                // Cost is known, but this unregistered dispatch permanently
                // invalidates the evaluation even after accounting closes.
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => {}
        }
    }
    for target in &mut ticket.targets {
        let current = target.state.clone();
        match current {
            TargetState::Planned => {
                target.state = TargetState::NotDispatched {
                    released_call_ids: Vec::new(),
                };
            }
            TargetState::Running {
                candidate,
                baseline,
            } => {
                let mut released = Vec::new();
                let mut dispatched = false;
                let mut target_uncertain = false;
                for slot in [&candidate, &baseline].into_iter().flatten() {
                    let call = by_id
                        .get_mut(&slot.budget_call_id)
                        .ok_or_else(|| Error::Conflict("bound budget call is missing".into()))?;
                    match call.state {
                        BudgetCallState::Reserved => {
                            let fence = call_fence(call, now);
                            *call = session
                                .release_undispatched_budget_call(
                                    ctx,
                                    &fence,
                                    "ticket_stopped_before_dispatch",
                                )
                                .await?;
                            released.push(call.call_id.clone());
                        }
                        BudgetCallState::Released | BudgetCallState::Cancelled => {
                            released.push(call.call_id.clone());
                        }
                        BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                            dispatched = true;
                            target_uncertain = true;
                        }
                        BudgetCallState::Finalized => {
                            dispatched = true;
                            target_uncertain |= !call.execution_closed;
                        }
                    }
                }
                usage_uncertain |= target_uncertain;
                target.state = if target_uncertain {
                    TargetState::RunningUsageUnknown {
                        candidate,
                        baseline,
                    }
                } else if dispatched {
                    TargetState::CancelledAfterDispatch {
                        candidate,
                        baseline,
                    }
                } else {
                    TargetState::NotDispatched {
                        released_call_ids: released,
                    }
                };
            }
            _ => {}
        }
    }
    ticket.unattributed_budget_call_ids.sort();
    ticket.unattributed_budget_call_ids.dedup();
    Ok(usage_uncertain)
}

fn call_fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

async fn stop_ticket(
    session: &mut Session,
    ctx: &Context,
    control: &RegisteredEvaluationControl,
    ticket: &mut EvaluationTicketV2,
    decision: EarlyStopDecision,
    issuer: &IndependentGraderReceiptV2,
    now: i64,
) -> Result<()> {
    ticket.lifecycle = TicketLifecycle::StopRequested;
    let usage_uncertain =
        close_execution_slots(session, ctx, control, ticket, stop_reason(decision), now).await?;
    settle_exposure_money(session, ctx, ticket, usage_uncertain).await?;
    let (k, lcb, ucb, prefix_digest) = if decision == EarlyStopDecision::CriticalRegression {
        (0, None, None, fingerprint(&Vec::<String>::new())?)
    } else {
        let plan = control
            .early_stop_plan
            .as_ref()
            .ok_or_else(|| Error::Invalid("statistical stop lacks early-stop plan".into()))?;
        let (_, stream) = rebuild_stream(ticket, plan, &control.alpha_plan)?;
        let bound = stream
            .boundary()?
            .ok_or_else(|| Error::Invalid("statistical stop lacks a completed prefix".into()))?;
        let wire = bound.to_wire()?;
        (
            bound.k,
            Some(wire.lcb),
            Some(wire.ucb),
            stream.completed_prefix_digest()?,
        )
    };
    let member_terminal_digest = fingerprint(&ticket.targets)?;
    ticket.stop_member_terminal_digest = Some(member_terminal_digest.clone());
    let certificate = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
        ticket_id: ticket.id.clone(),
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        early_stop_plan_digest: control
            .early_stop_plan
            .as_ref()
            .map(EarlyStopPlan::digest)
            .transpose()?
            .unwrap_or(control.formal_plan.digest()?),
        alpha_plan_digest: ticket.alpha_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        slice_id: load_slice_id(session, ctx, &ticket.holdout_id).await?,
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_digest: ticket.baseline_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        issuer_receipt_digest: fingerprint(issuer)?,
        k,
        lcb,
        ucb,
        completed_prefix_digest: prefix_digest,
        member_terminal_digest,
        stop_reason: decision,
        stopped_at_unix_ms: now
            .checked_mul(1_000)
            .ok_or_else(|| Error::Invalid("stop timestamp conversion overflow".into()))?,
        stop_sequence: ticket.next_sequence,
        usage_uncertain,
    })?;
    ticket.pending_certificate = Some(certificate);
    ticket.lifecycle = if usage_uncertain {
        TicketLifecycle::Draining
    } else {
        TicketLifecycle::EarlyStopped
    };
    Ok(())
}

async fn invalidate_in_session(
    session: &mut Session,
    ctx: &Context,
    control: &RegisteredEvaluationControl,
    ticket: &mut EvaluationTicketV2,
    reason: &str,
    now: i64,
) -> Result<FormalEvaluationV2> {
    identifier(reason)?;
    valid_time(now)?;
    ticket.lifecycle = TicketLifecycle::Invalid;
    if !ticket
        .invalid_reasons
        .iter()
        .any(|existing| existing == reason)
    {
        ticket.invalid_reasons.push(reason.into());
    }
    let usage_uncertain = close_execution_slots(session, ctx, control, ticket, reason, now).await?;
    settle_exposure_money(session, ctx, ticket, usage_uncertain).await?;
    let formal = FormalEvaluationV2::Invalid {
        evidence_scope: control.evidence_scope,
        ticket_id: ticket.id.clone(),
        ticket_digest: fingerprint(ticket)?,
        reasons: ticket.invalid_reasons.clone(),
    };
    put_record(session, ctx, TICKET_KIND, &ticket.id, ctx.actor(), ticket).await?;
    put_record(session, ctx, FORMAL_KIND, &ticket.id, ctx.actor(), &formal).await?;
    Ok(formal)
}

async fn load_slice_id(session: &mut Session, ctx: &Context, holdout_id: &str) -> Result<String> {
    let holdout: ProtectedHoldoutRecord =
        need_record(session, ctx, HOLDOUT_KIND, holdout_id).await?;
    Ok(holdout.manifest.slice_id)
}

async fn maybe_store_early_formal(
    session: &mut Session,
    ctx: &Context,
    ticket: &EvaluationTicketV2,
) -> Result<()> {
    if ticket.lifecycle == TicketLifecycle::EarlyStopped {
        let certificate = ticket
            .pending_certificate
            .clone()
            .ok_or_else(|| Error::Invalid("early-stopped ticket lacks certificate".into()))?;
        if ticket.candidate_dependency_binding.is_some() {
            let control: RegisteredEvaluationControl =
                need_record(session, ctx, CONTROL_KIND, &ticket.registration_id).await?;
            let holdout: ProtectedHoldoutRecord =
                need_record(session, ctx, HOLDOUT_KIND, &ticket.holdout_id).await?;
            validate_bound_early_certificate_in_session(
                session,
                ctx,
                ticket,
                &control,
                &holdout,
                &certificate,
            )
            .await?;
        }
        let formal = FormalEvaluationV2::EarlyStopped {
            evidence_scope: ticket.evidence_scope,
            ticket_id: ticket.id.clone(),
            ticket_digest: fingerprint(ticket)?,
            certificate,
        };
        put_record(session, ctx, FORMAL_KIND, &ticket.id, ctx.actor(), &formal).await?;
    }
    Ok(())
}

fn stop_reason(decision: EarlyStopDecision) -> &'static str {
    match decision {
        EarlyStopDecision::CriticalRegression => "critical_regression",
        EarlyStopDecision::FutilityQualityGain => "futility_quality_gain",
        EarlyStopDecision::FutilityNoninferiority => "futility_noninferiority",
        EarlyStopDecision::RegressedSequential => "sequential_regression",
        _ => "invalid_early_stop",
    }
}

fn postprocess_complete_report(
    control_schema: &str,
    profile: ProfileKind,
    mut report: BernsteinReport,
) -> Result<BernsteinReport> {
    match control_schema {
        RegisteredEvaluationControl::SCHEMA_V1 => {
            // Preserve frozen v1 reports, including their original reason order.
            if profile == ProfileKind::NoninferiorSavings {
                report.verdict = Verdict::Inconclusive;
                report
                    .reasons
                    .push("cost_savings_statistical_proof_unsupported".into());
            }
            report.verdict = Verdict::Inconclusive;
            report
                .reasons
                .push("complete_optimization_cost_not_verified".into());
        }
        RegisteredEvaluationControl::SCHEMA => {
            if profile == ProfileKind::NoninferiorSavings && report.verdict == Verdict::Noninferior
            {
                report.verdict = Verdict::Inconclusive;
                report
                    .reasons
                    .push("cost_savings_statistical_proof_unsupported".into());
            } else if report.verdict == Verdict::Regressed
                && report.reasons.iter().any(|reason| reason == "negative_lcb")
            {
                report.verdict = Verdict::Inconclusive;
                report
                    .reasons
                    .push("regression_not_confirmed_ucb_boundary_not_crossed".into());
            }
        }
        _ => {
            return Err(Error::Invalid(
                "unsupported registered-control schema".into(),
            ));
        }
    }
    Ok(report)
}

fn p95(values: &mut [u64]) -> Result<u64> {
    if values.is_empty() {
        return Err(Error::Invalid("p95 requires observations".into()));
    }
    values.sort_unstable();
    let rank = 95usize
        .checked_mul(values.len())
        .ok_or_else(|| Error::Invalid("p95 rank overflow".into()))?
        .div_ceil(100);
    Ok(values[rank.saturating_sub(1)])
}

#[cfg(test)]
mod complete_verdict_tests {
    use super::*;

    #[test]
    fn v2_postprocessing_preserves_safety_regression_without_negative_lcb() {
        // Complete-batch finalization always passes safety_ok=true; this checks
        // only the private defensive rule, not a reachable batch safety path.
        for profile in [ProfileKind::QualityGain, ProfileKind::NoninferiorSavings] {
            let mut plan = ExperimentPlan::first_low_risk("safety-fixture", 60).unwrap();
            plan.profile = profile;
            plan.freeze(1).unwrap();
            plan.bind_candidate("candidate").unwrap();
            let rows = vec![
                ClusterObservation {
                    cluster_id: "one".into(),
                    d: 1.0,
                    weight: 1.0,
                },
                ClusterObservation {
                    cluster_id: "two".into(),
                    d: 1.0,
                    weight: 1.0,
                },
            ];
            let report = decide(
                &plan,
                empirical_bernstein(&rows, 0.04).unwrap(),
                1.0,
                1.0,
                false,
            )
            .unwrap();
            assert_eq!(report.verdict, Verdict::Regressed);
            assert_eq!(report.reasons, vec!["safety_failed"]);
            let kept =
                postprocess_complete_report(RegisteredEvaluationControl::SCHEMA, profile, report)
                    .unwrap();
            assert_eq!(kept.verdict, Verdict::Regressed);
            assert_eq!(kept.reasons, vec!["safety_failed"]);
        }
    }

    #[test]
    fn private_postprocessing_rejects_an_unknown_control_version() {
        let report = empirical_bernstein(&[], 0.04).unwrap();
        assert!(matches!(
            postprocess_complete_report("unknown", ProfileKind::QualityGain, report),
            Err(Error::Invalid(_))
        ));
    }
}
