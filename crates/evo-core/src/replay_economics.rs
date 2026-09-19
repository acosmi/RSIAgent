//! Contracts for a paired online economic comparison of fixed vs replay-selected exploration.
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const EXPERIMENT_SCHEMA: &str = "rsia.replay_economic_experiment.v1";
pub const COST_RECEIPT_SCHEMA: &str = "rsia.replay_economic_cost_receipt.v1";
pub const REPORT_SCHEMA: &str = "rsia.replay_economic_report.v1";
pub const REPLAY_SELECTION_RULE_V1: &str = "rsia.pareto_attainment.v2";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicClaim {
    QualityGain,
    NoninferiorSavings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmOrder {
    FixedThenReplaySelected,
    ReplaySelectedThenFixed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTaskRef {
    pub ordinal: u32,
    pub task_id: String,
    pub task_digest: String,
    pub cluster_id: String,
    pub arm_order: ArmOrder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationOnlyDiagnosticRef {
    pub w_sim: u8,
    pub report_id: String,
    pub report_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySelectionRef {
    pub report_artifact_id: String,
    pub report_digest: String,
    pub semantic_digest: String,
    pub world_pool_digest: String,
    pub policy_digest: String,
    pub profile_digest: String,
    pub caps_digest: String,
    pub selection_rule_version: String,
    pub selected_with_w_sim: u8,
    pub target_w_online: u8,
    pub simulation_only: Vec<SimulationOnlyDiagnosticRef>,
}

impl ReplaySelectionRef {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.report_artifact_id)?;
        identifier(&self.selection_rule_version)?;
        for (value, name) in [
            (&self.report_digest, "replay report_digest"),
            (&self.semantic_digest, "replay semantic_digest"),
            (&self.world_pool_digest, "world_pool_digest"),
            (&self.policy_digest, "replay policy_digest"),
            (&self.profile_digest, "replay profile_digest"),
            (&self.caps_digest, "replay caps_digest"),
        ] {
            digest(value, name)?;
        }
        if self.selection_rule_version != REPLAY_SELECTION_RULE_V1
            || self.selected_with_w_sim != 1
            || self.target_w_online != 1
        {
            return Err(Error::Invalid(
                "online candidate selection must be frozen for W=1".into(),
            ));
        }
        let mut widths = BTreeSet::new();
        for diagnostic in &self.simulation_only {
            if !matches!(diagnostic.w_sim, 2 | 4) || !widths.insert(diagnostic.w_sim) {
                return Err(Error::Invalid(
                    "simulation-only diagnostics require unique W_sim 2 or 4".into(),
                ));
            }
            identifier(&diagnostic.report_id)?;
            digest(&diagnostic.report_digest, "simulation report_digest")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchedRuntimeContract {
    pub w_online: u8,
    pub fixed_policy_digest: String,
    pub replay_selected_policy_digest: String,
    pub generation_strategy_digest: String,
    pub candidate_bundle_digest: String,
    pub baseline_bundle_digest: String,
    pub environment_digest: String,
    pub model_digest: String,
    pub tools_digest: String,
    pub runner_digest: String,
    pub grader_digest: String,
    pub guidance_digest: String,
    pub rules_digest: String,
    pub context_signature: String,
    pub target_runtime_profile: String,
    pub per_arm_node_budget: u8,
    pub per_arm_root_budget_micros: i64,
}

impl MatchedRuntimeContract {
    pub fn validate(&self) -> Result<()> {
        if self.w_online != 1
            || self.per_arm_node_budget == 0
            || self.per_arm_node_budget > 12
            || self.per_arm_root_budget_micros < 0
        {
            return Err(Error::Invalid(
                "matched runtime requires W_online=1 and equal bounded opportunity".into(),
            ));
        }
        for (value, name) in [
            (&self.fixed_policy_digest, "fixed_policy_digest"),
            (
                &self.replay_selected_policy_digest,
                "replay_selected_policy_digest",
            ),
            (
                &self.generation_strategy_digest,
                "generation_strategy_digest",
            ),
            (&self.candidate_bundle_digest, "candidate_bundle_digest"),
            (&self.baseline_bundle_digest, "baseline_bundle_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.model_digest, "model_digest"),
            (&self.tools_digest, "tools_digest"),
            (&self.runner_digest, "runner_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.guidance_digest, "guidance_digest"),
            (&self.rules_digest, "rules_digest"),
            (&self.context_signature, "context_signature"),
        ] {
            digest(value, name)?;
        }
        identifier(&self.target_runtime_profile)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostComponentKind {
    HistoryCollection,
    PolicyGeneration,
    ReplayCpu,
    ReplayStorage,
    OnlineDevelopmentFixed,
    OnlineDevelopmentReplaySelected,
    IndependentAcceptance,
    CanaryOperations,
    HumanReview,
}

impl CostComponentKind {
    pub const ALL: [Self; 9] = [
        Self::HistoryCollection,
        Self::PolicyGeneration,
        Self::ReplayCpu,
        Self::ReplayStorage,
        Self::OnlineDevelopmentFixed,
        Self::OnlineDevelopmentReplaySelected,
        Self::IndependentAcceptance,
        Self::CanaryOperations,
        Self::HumanReview,
    ];

    fn expected_scope(self) -> CostScope {
        match self {
            Self::OnlineDevelopmentFixed | Self::OnlineDevelopmentReplaySelected => {
                CostScope::PerTask
            }
            Self::CanaryOperations => CostScope::Operations,
            _ => CostScope::OneTime,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullCostPlanV1 {
    pub component_kinds: Vec<CostComponentKind>,
    pub currency: String,
    pub pricing_version: String,
    pub payment_subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicBudgetStage {
    HistoryCollection,
    CandidateGeneration,
    StorageCpu,
    DevelopmentExecution,
    FormalEvaluation,
    GrayOperations,
    HumanReview,
}

impl EconomicBudgetStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HistoryCollection => "history_collection",
            Self::CandidateGeneration => "candidate_generation",
            Self::StorageCpu => "storage_cpu",
            Self::DevelopmentExecution => "development_execution",
            Self::FormalEvaluation => "formal_evaluation",
            Self::GrayOperations => "gray_operations",
            Self::HumanReview => "human_review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source", deny_unknown_fields)]
pub enum ComponentBillingSourceV1 {
    BudgetCall {
        dispatch_group_id: String,
        stage: EconomicBudgetStage,
    },
    AdminMeasurement {
        source_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentBillingBindingV1 {
    pub component: CostComponentKind,
    pub source: ComponentBillingSourceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EconomicBudgetBindingV1 {
    pub billing_scope: String,
    pub root_budget_id: String,
    pub currency: String,
    pub pricing_version: String,
    pub payment_subject: String,
    pub components: Vec<ComponentBillingBindingV1>,
}

impl EconomicBudgetBindingV1 {
    pub fn validate_against(&self, plan: &FullCostPlanV1) -> Result<()> {
        plan.validate()?;
        for value in [
            &self.billing_scope,
            &self.root_budget_id,
            &self.currency,
            &self.pricing_version,
            &self.payment_subject,
        ] {
            identifier(value)?;
        }
        if self.currency != plan.currency
            || self.pricing_version != plan.pricing_version
            || self.payment_subject != plan.payment_subject
        {
            return Err(Error::Conflict(
                "economic budget identity differs from the cost plan".into(),
            ));
        }
        let mut components = BTreeSet::new();
        for binding in &self.components {
            if !components.insert(binding.component) {
                return Err(Error::Invalid("duplicate component budget binding".into()));
            }
            match &binding.source {
                ComponentBillingSourceV1::BudgetCall {
                    dispatch_group_id, ..
                } => identifier(dispatch_group_id)?,
                ComponentBillingSourceV1::AdminMeasurement { source_id } => identifier(source_id)?,
            }
        }
        if components != CostComponentKind::ALL.into_iter().collect() {
            return Err(Error::Invalid(
                "every cost component requires one frozen billing source".into(),
            ));
        }
        Ok(())
    }

    pub fn component(&self, component: CostComponentKind) -> Result<&ComponentBillingSourceV1> {
        self.components
            .iter()
            .find(|binding| binding.component == component)
            .map(|binding| &binding.source)
            .ok_or_else(|| Error::Invalid("missing component budget binding".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EconomicSourceRefV1 {
    pub id: String,
    pub digest: String,
}

impl FullCostPlanV1 {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.currency)?;
        identifier(&self.pricing_version)?;
        identifier(&self.payment_subject)?;
        let actual: BTreeSet<_> = self.component_kinds.iter().copied().collect();
        let required: BTreeSet<_> = CostComponentKind::ALL.into_iter().collect();
        if actual != required || actual.len() != self.component_kinds.len() {
            return Err(Error::Invalid(
                "full cost plan must declare every component exactly once".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEconomicExperimentV1 {
    pub schema_version: String,
    pub id: String,
    pub profile_id: String,
    pub evaluator_actor: String,
    pub executor_actor: String,
    pub proposer_actor: String,
    pub approver_actor: String,
    pub primary_claim: EconomicClaim,
    pub min_gain_micros: u32,
    pub noninferiority_margin_micros: u32,
    pub preregistration_digest: String,
    pub paired_ticket_id: String,
    pub paired_ticket_digest: String,
    pub dataset_epoch: String,
    pub tasks: Vec<PairedTaskRef>,
    pub runtime: MatchedRuntimeContract,
    pub replay_selection: ReplaySelectionRef,
    pub cost_plan: FullCostPlanV1,
    pub budget_binding: EconomicBudgetBindingV1,
    pub sources: Vec<EconomicSourceRefV1>,
    pub source_watermark: u64,
}

impl ReplayEconomicExperimentV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EXPERIMENT_SCHEMA || self.tasks.is_empty() {
            return Err(Error::Invalid(
                "unsupported or empty replay-economic experiment".into(),
            ));
        }
        for value in [
            &self.id,
            &self.profile_id,
            &self.evaluator_actor,
            &self.executor_actor,
            &self.proposer_actor,
            &self.approver_actor,
            &self.paired_ticket_id,
            &self.dataset_epoch,
        ] {
            identifier(value)?;
        }
        let actors: BTreeSet<_> = [
            self.evaluator_actor.as_str(),
            self.executor_actor.as_str(),
            self.proposer_actor.as_str(),
            self.approver_actor.as_str(),
        ]
        .into_iter()
        .collect();
        if actors.len() != 4
            || self.source_watermark == 0
            || self.min_gain_micros == 0
            || self.min_gain_micros > 1_000_000
            || self.noninferiority_margin_micros > 1_000_000
        {
            return Err(Error::Invalid(
                "experiment actors must be independent and watermark present".into(),
            ));
        }
        digest(&self.preregistration_digest, "preregistration_digest")?;
        digest(&self.paired_ticket_digest, "paired_ticket_digest")?;
        self.runtime.validate()?;
        self.replay_selection.validate()?;
        self.cost_plan.validate()?;
        self.budget_binding.validate_against(&self.cost_plan)?;
        if self.runtime.replay_selected_policy_digest != self.replay_selection.policy_digest {
            return Err(Error::Conflict(
                "runtime and replay selection policy differ".into(),
            ));
        }
        let mut task_ids = BTreeSet::new();
        let mut clusters = BTreeSet::new();
        for (index, task) in self.tasks.iter().enumerate() {
            if task.ordinal != index as u32 + 1 {
                return Err(Error::Invalid(
                    "paired task ordinals must be contiguous".into(),
                ));
            }
            identifier(&task.task_id)?;
            identifier(&task.cluster_id)?;
            digest(&task.task_digest, "paired task_digest")?;
            if !task_ids.insert(task.task_id.as_str()) || !clusters.insert(task.cluster_id.as_str())
            {
                return Err(Error::Invalid(
                    "paired tasks require unique independent clusters".into(),
                ));
            }
        }
        if self.sources.is_empty() {
            return Err(Error::Invalid(
                "economic experiment requires a source closure".into(),
            ));
        }
        let mut sources = BTreeSet::new();
        for source in &self.sources {
            identifier(&source.id)?;
            digest(&source.digest, "economic source digest")?;
            if !sources.insert(source.id.as_str()) {
                return Err(Error::Invalid("duplicate experiment source".into()));
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
pub enum CostScope {
    OneTime,
    PerTask,
    PerSuccess,
    Operations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSourceKind {
    BudgetCall,
    AdminMeasurement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementState {
    KnownFinal,
    KnownNonmonetary,
    UsageUncertain,
    NotIncurred,
    UnsupportedMeasurement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostComponentReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub experiment_id: String,
    pub component: CostComponentKind,
    pub scope: CostScope,
    pub source_kind: CostSourceKind,
    pub source_id: String,
    pub source_digest: String,
    pub billing_scope: Option<String>,
    pub budget_call_id: Option<String>,
    pub amount_micros: Option<i64>,
    pub currency: Option<String>,
    pub pricing_version: Option<String>,
    pub payment_subject: Option<String>,
    pub tokens: Option<u64>,
    pub latency_micros: Option<u64>,
    pub storage_bytes: Option<u64>,
    pub cpu_nanos: Option<u64>,
    pub human_minutes: Option<u32>,
    pub measurement_state: MeasurementState,
    pub proof_digest: Option<String>,
    pub reason: Option<String>,
    pub created_seq: u64,
}

impl CostComponentReceiptV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != COST_RECEIPT_SCHEMA || self.created_seq == 0 {
            return Err(Error::Invalid(
                "invalid cost receipt schema/sequence".into(),
            ));
        }
        for value in [&self.id, &self.experiment_id, &self.source_id] {
            identifier(value)?;
        }
        digest(&self.source_digest, "cost source_digest")?;
        if self.scope != self.component.expected_scope() {
            return Err(Error::Conflict(
                "cost component scope differs from the frozen plan".into(),
            ));
        }
        if let Some(call) = &self.budget_call_id {
            identifier(call)?;
        }
        if let Some(scope) = &self.billing_scope {
            identifier(scope)?;
        }
        if let Some(proof) = &self.proof_digest {
            digest(proof, "cost proof_digest")?;
        }
        if let Some(reason) = &self.reason {
            text(reason, "cost reason", 512)?;
        }
        match self.measurement_state {
            MeasurementState::KnownFinal => {
                if self.amount_micros.is_none_or(|amount| amount < 0)
                    || self
                        .currency
                        .as_deref()
                        .is_none_or(|value| identifier(value).is_err())
                    || self
                        .pricing_version
                        .as_deref()
                        .is_none_or(|value| identifier(value).is_err())
                    || self
                        .payment_subject
                        .as_deref()
                        .is_none_or(|value| identifier(value).is_err())
                {
                    return Err(Error::Invalid(
                        "known-final receipt requires nonnegative priced money".into(),
                    ));
                }
            }
            MeasurementState::KnownNonmonetary => {
                if self.amount_micros.is_some()
                    || self.currency.is_some()
                    || self.pricing_version.is_some()
                    || self.payment_subject.is_some()
                    || [
                        self.tokens,
                        self.latency_micros,
                        self.storage_bytes,
                        self.cpu_nanos,
                        self.human_minutes.map(u64::from),
                    ]
                    .into_iter()
                    .all(|value| value.is_none())
                {
                    return Err(Error::Invalid(
                        "nonmonetary receipt needs a real nonmonetary measure".into(),
                    ));
                }
            }
            MeasurementState::UsageUncertain => {
                if self.amount_micros.is_some() || self.reason.is_none() {
                    return Err(Error::Invalid(
                        "usage-uncertain receipt cannot claim final money".into(),
                    ));
                }
            }
            MeasurementState::NotIncurred => {
                if self.amount_micros.is_some() || self.proof_digest.is_none() {
                    return Err(Error::Invalid(
                        "not-incurred requires proof and no amount".into(),
                    ));
                }
            }
            MeasurementState::UnsupportedMeasurement => {
                if self.amount_micros.is_some() || self.reason.is_none() {
                    return Err(Error::Invalid(
                        "unsupported measurement requires a reason".into(),
                    ));
                }
            }
        }
        match self.source_kind {
            CostSourceKind::BudgetCall
                if self.budget_call_id.as_deref() != Some(&self.source_id)
                    || self.billing_scope.is_none() =>
            {
                return Err(Error::Conflict(
                    "budget-call receipt source and call id differ".into(),
                ));
            }
            CostSourceKind::AdminMeasurement
                if self.proof_digest.is_none() || self.billing_scope.is_some() =>
            {
                return Err(Error::Invalid(
                    "admin measurement requires an audited proof digest".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum EconomicComputation {
    Complete {
        one_time_increment_micros: i64,
        fixed_arm_total_micros: i64,
        replay_arm_total_micros: i64,
        marginal_saving_per_task_micros: i64,
        projected_tasks: u64,
        net_saving_micros: i64,
        break_even_tasks: Option<u64>,
    },
    UsageUncertain {
        receipt_ids: Vec<String>,
    },
    Blocked {
        reasons: Vec<String>,
    },
}

pub fn compute_economics(
    plan: &FullCostPlanV1,
    receipts: &[CostComponentReceiptV1],
    completed_paired_tasks: u32,
    projected_tasks: u64,
) -> Result<EconomicComputation> {
    plan.validate()?;
    if completed_paired_tasks == 0 {
        return Err(Error::Invalid("paired task count must be positive".into()));
    }
    let mut kinds = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut call_ids = BTreeSet::new();
    let mut uncertain = Vec::new();
    let mut blocked = Vec::new();
    let mut one_time = 0i64;
    let mut fixed = 0i64;
    let mut replay = 0i64;
    for receipt in receipts {
        receipt.validate()?;
        if !ids.insert(receipt.id.as_str()) {
            return Err(Error::Conflict("duplicate cost receipt id".into()));
        }
        if let Some(call_id) = &receipt.budget_call_id
            && !call_ids.insert(call_id.as_str())
        {
            return Err(Error::Conflict(
                "one actual budget call cannot be counted twice".into(),
            ));
        }
        kinds.insert(receipt.component);
        match receipt.measurement_state {
            MeasurementState::KnownFinal => {
                if receipt.currency.as_deref() != Some(plan.currency.as_str())
                    || receipt.pricing_version.as_deref() != Some(plan.pricing_version.as_str())
                    || receipt.payment_subject.as_deref() != Some(plan.payment_subject.as_str())
                {
                    blocked.push(format!("incomparable_pricing: {}", receipt.id));
                    continue;
                }
                let amount = receipt.amount_micros.ok_or(Error::Internal)?;
                match receipt.component {
                    CostComponentKind::OnlineDevelopmentFixed => {
                        fixed = fixed.checked_add(amount).ok_or(Error::Budget)?;
                    }
                    CostComponentKind::OnlineDevelopmentReplaySelected => {
                        replay = replay.checked_add(amount).ok_or(Error::Budget)?;
                    }
                    _ => one_time = one_time.checked_add(amount).ok_or(Error::Budget)?,
                }
            }
            MeasurementState::NotIncurred => {}
            MeasurementState::UsageUncertain => uncertain.push(receipt.id.clone()),
            MeasurementState::KnownNonmonetary => {
                blocked.push(format!("nonmonetary_only: {}", receipt.id));
            }
            MeasurementState::UnsupportedMeasurement => {
                blocked.push(format!("unsupported_measurement: {}", receipt.id));
            }
        }
    }
    for required in CostComponentKind::ALL {
        if !kinds.contains(&required) {
            blocked.push(format!("missing_component: {required:?}"));
        }
    }
    if !uncertain.is_empty() {
        uncertain.sort();
        return Ok(EconomicComputation::UsageUncertain {
            receipt_ids: uncertain,
        });
    }
    if !blocked.is_empty() {
        blocked.sort();
        blocked.dedup();
        return Ok(EconomicComputation::Blocked { reasons: blocked });
    }
    let completed = i64::from(completed_paired_tasks);
    let difference = fixed.checked_sub(replay).ok_or(Error::Budget)?;
    let marginal = difference / completed;
    let projected = i128::from(projected_tasks);
    let net = projected
        .checked_mul(i128::from(difference))
        .and_then(|value| value.checked_div(i128::from(completed)))
        .and_then(|value| value.checked_sub(i128::from(one_time)))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(Error::Budget)?;
    let break_even_tasks = if difference > 0 {
        let numerator = i128::from(one_time)
            .checked_mul(i128::from(completed))
            .ok_or(Error::Budget)?;
        let value = numerator
            .checked_add(i128::from(difference) - 1)
            .and_then(|value| value.checked_div(i128::from(difference)))
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Error::Budget)?;
        Some(value)
    } else {
        None
    };
    Ok(EconomicComputation::Complete {
        one_time_increment_micros: one_time,
        fixed_arm_total_micros: fixed,
        replay_arm_total_micros: replay,
        marginal_saving_per_task_micros: marginal,
        projected_tasks,
        net_saving_micros: net,
        break_even_tasks,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedQualityOutcome {
    PassedGain,
    PassedNoninferior,
    ZeroOrInconclusive,
    Negative,
    EarlyRejected,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicReportTerminal {
    CompletePositive,
    CompleteZeroOrInconclusive,
    CompleteNegative,
    EarlyRejected,
    Invalid,
    Cancelled,
    UsageUncertain,
    BlockedSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedUnitTerminal {
    Complete,
    Failed,
    Cancelled,
    UsageUncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEconomicUnitReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub experiment_id: String,
    pub ordinal: u32,
    pub task_digest: String,
    pub cluster_id: String,
    pub fixed_score_micros: Option<u32>,
    pub replay_selected_score_micros: Option<u32>,
    pub critical_capabilities_preserved: Option<bool>,
    pub fixed_execution_receipt_id: Option<String>,
    pub replay_execution_receipt_id: Option<String>,
    pub fixed_latency_micros: Option<u64>,
    pub replay_latency_micros: Option<u64>,
    pub budget_call_ids: Vec<String>,
    pub terminal: PairedUnitTerminal,
}

impl ReplayEconomicUnitReceiptV1 {
    pub fn validate_against(&self, experiment: &ReplayEconomicExperimentV1) -> Result<()> {
        if self.schema_version != "rsia.replay_economic_unit_receipt.v1"
            || self.experiment_id != experiment.id
        {
            return Err(Error::Conflict("paired unit experiment differs".into()));
        }
        identifier(&self.id)?;
        let planned = experiment
            .tasks
            .get(self.ordinal.saturating_sub(1) as usize)
            .ok_or_else(|| Error::Invalid("paired unit ordinal is not planned".into()))?;
        if planned.ordinal != self.ordinal
            || planned.task_digest != self.task_digest
            || planned.cluster_id != self.cluster_id
        {
            return Err(Error::Conflict(
                "paired unit differs from the frozen manifest".into(),
            ));
        }
        digest(&self.task_digest, "unit task_digest")?;
        identifier(&self.cluster_id)?;
        let mut calls = BTreeSet::new();
        for call in &self.budget_call_ids {
            identifier(call)?;
            if !calls.insert(call.as_str()) {
                return Err(Error::Invalid("duplicate unit budget call".into()));
            }
        }
        if self.terminal == PairedUnitTerminal::Complete {
            if self
                .fixed_score_micros
                .is_none_or(|value| value > 1_000_000)
                || self
                    .replay_selected_score_micros
                    .is_none_or(|value| value > 1_000_000)
                || self.critical_capabilities_preserved.is_none()
                || self.fixed_latency_micros.is_none()
                || self.replay_latency_micros.is_none()
            {
                return Err(Error::Invalid(
                    "complete paired unit lacks actual score/gate/latency evidence".into(),
                ));
            }
            for receipt in [
                self.fixed_execution_receipt_id.as_deref(),
                self.replay_execution_receipt_id.as_deref(),
            ] {
                identifier(receipt.ok_or_else(|| {
                    Error::Invalid("complete paired unit lacks execution receipt".into())
                })?)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmLatencySummaryV1 {
    pub fixed_p50_micros: u64,
    pub fixed_p95_micros: u64,
    pub replay_selected_p50_micros: u64,
    pub replay_selected_p95_micros: u64,
}

pub fn summarize_unit_latencies(
    units: &[ReplayEconomicUnitReceiptV1],
) -> Result<ArmLatencySummaryV1> {
    let mut fixed = Vec::with_capacity(units.len());
    let mut replay = Vec::with_capacity(units.len());
    for unit in units {
        if unit.terminal != PairedUnitTerminal::Complete {
            return Err(Error::Invalid(
                "latency summary requires complete paired units".into(),
            ));
        }
        fixed.push(unit.fixed_latency_micros.ok_or(Error::Internal)?);
        replay.push(unit.replay_latency_micros.ok_or(Error::Internal)?);
    }
    if fixed.is_empty() {
        return Err(Error::Invalid("latency summary has no units".into()));
    }
    fixed.sort_unstable();
    replay.sort_unstable();
    Ok(ArmLatencySummaryV1 {
        fixed_p50_micros: percentile(&fixed, 50),
        fixed_p95_micros: percentile(&fixed, 95),
        replay_selected_p50_micros: percentile(&replay, 50),
        replay_selected_p95_micros: percentile(&replay, 95),
    })
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let rank = values.len().saturating_mul(percentile).div_ceil(100);
    values[rank.saturating_sub(1).min(values.len() - 1)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEconomicReportV1 {
    pub schema_version: String,
    pub id: String,
    pub experiment_id: String,
    pub experiment_digest: String,
    pub paired_ticket_id: String,
    pub paired_ticket_digest: String,
    pub paired_receipt_closure_digest: Option<String>,
    pub completed_paired_tasks: u32,
    pub paired_units: Vec<ReplayEconomicUnitReceiptV1>,
    pub latency: Option<ArmLatencySummaryV1>,
    pub quality_outcome: PairedQualityOutcome,
    pub cost_receipt_ids: Vec<String>,
    pub economics: EconomicComputation,
    pub terminal: EconomicReportTerminal,
    pub reasons: Vec<String>,
}

impl ReplayEconomicReportV1 {
    pub fn validate_against(&self, experiment: &ReplayEconomicExperimentV1) -> Result<()> {
        experiment.validate()?;
        if self.schema_version != REPORT_SCHEMA
            || self.experiment_id != experiment.id
            || self.experiment_digest != experiment.digest()?
            || self.paired_ticket_id != experiment.paired_ticket_id
            || self.paired_ticket_digest != experiment.paired_ticket_digest
            || self.completed_paired_tasks > experiment.tasks.len() as u32
        {
            return Err(Error::Conflict(
                "economic report differs from preregistered paired experiment".into(),
            ));
        }
        identifier(&self.id)?;
        let mut receipt_ids = BTreeSet::new();
        for receipt_id in &self.cost_receipt_ids {
            identifier(receipt_id)?;
            if !receipt_ids.insert(receipt_id.as_str()) {
                return Err(Error::Invalid("duplicate report cost receipt".into()));
            }
        }
        for reason in &self.reasons {
            text(reason, "economic report reason", 512)?;
        }
        if let Some(receipts) = &self.paired_receipt_closure_digest {
            digest(receipts, "paired_receipt_closure_digest")?;
        }
        if self.completed_paired_tasks != self.paired_units.len() as u32 {
            return Err(Error::Conflict(
                "paired report count differs from embedded unit receipts".into(),
            ));
        }
        for unit in &self.paired_units {
            unit.validate_against(experiment)?;
        }
        if self.paired_units.is_empty() {
            if self.latency.is_some() {
                return Err(Error::Invalid(
                    "blocked report cannot invent latency quantiles".into(),
                ));
            }
        } else if self.latency.as_ref() != Some(&summarize_unit_latencies(&self.paired_units)?) {
            return Err(Error::Conflict(
                "reported latency quantiles differ from paired units".into(),
            ));
        }
        let mean_delta = if self.paired_units.is_empty() {
            None
        } else {
            let total = self.paired_units.iter().try_fold(0i64, |sum, unit| {
                let fixed = i64::from(unit.fixed_score_micros.ok_or(Error::Internal)?);
                let replay = i64::from(unit.replay_selected_score_micros.ok_or(Error::Internal)?);
                sum.checked_add(replay - fixed).ok_or(Error::Budget)
            })?;
            Some(total / self.paired_units.len() as i64)
        };
        let critical_preserved = self
            .paired_units
            .iter()
            .all(|unit| unit.critical_capabilities_preserved == Some(true));
        match self.quality_outcome {
            PairedQualityOutcome::PassedGain
                if !critical_preserved
                    || mean_delta
                        .is_none_or(|delta| delta < i64::from(experiment.min_gain_micros)) =>
            {
                return Err(Error::Conflict(
                    "quality-gain outcome differs from paired receipts".into(),
                ));
            }
            PairedQualityOutcome::PassedNoninferior
                if !critical_preserved
                    || mean_delta.is_none_or(|delta| {
                        delta < -i64::from(experiment.noninferiority_margin_micros)
                    }) =>
            {
                return Err(Error::Conflict(
                    "noninferiority outcome differs from paired receipts".into(),
                ));
            }
            PairedQualityOutcome::Negative
                if critical_preserved
                    && mean_delta.is_some_and(|delta| {
                        delta >= -i64::from(experiment.noninferiority_margin_micros)
                    }) =>
            {
                return Err(Error::Conflict(
                    "negative outcome differs from paired receipts".into(),
                ));
            }
            _ => {}
        }
        let expected = match (
            experiment.primary_claim,
            &self.quality_outcome,
            &self.economics,
        ) {
            (
                EconomicClaim::QualityGain,
                PairedQualityOutcome::PassedGain,
                EconomicComputation::Complete { .. },
            ) if self.completed_paired_tasks == experiment.tasks.len() as u32
                && self.paired_receipt_closure_digest.is_some() =>
            {
                EconomicReportTerminal::CompletePositive
            }
            (
                EconomicClaim::NoninferiorSavings,
                PairedQualityOutcome::PassedNoninferior,
                EconomicComputation::Complete {
                    marginal_saving_per_task_micros,
                    ..
                },
            ) if *marginal_saving_per_task_micros > 0
                && self.completed_paired_tasks == experiment.tasks.len() as u32
                && self.paired_receipt_closure_digest.is_some() =>
            {
                EconomicReportTerminal::CompletePositive
            }
            (_, PairedQualityOutcome::ZeroOrInconclusive, EconomicComputation::Complete { .. }) => {
                EconomicReportTerminal::CompleteZeroOrInconclusive
            }
            (_, PairedQualityOutcome::Negative, EconomicComputation::Complete { .. }) => {
                EconomicReportTerminal::CompleteNegative
            }
            (_, PairedQualityOutcome::EarlyRejected, _) => EconomicReportTerminal::EarlyRejected,
            (_, PairedQualityOutcome::Invalid, _) => EconomicReportTerminal::Invalid,
            (_, _, EconomicComputation::UsageUncertain { .. }) => {
                EconomicReportTerminal::UsageUncertain
            }
            _ => EconomicReportTerminal::BlockedSupport,
        };
        if self.terminal != expected {
            return Err(Error::Conflict(
                "economic report terminal overstates its evidence".into(),
            ));
        }
        Ok(())
    }
}

pub fn economic_report_is_not_formal(_: &ReplayEconomicReportV1) -> Result<()> {
    Err(Error::Invalid(
        "ReplayEconomicReport cannot become FormalEvaluation or approval evidence".into(),
    ))
}
