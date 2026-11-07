//! Registered instance and ownership bounds from plan §§3.3.1–3.3.2.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityScope {
    Instance,
    Claim,
    OptimizationJob,
    ExplorationNode,
    Root,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CapacityItem {
    #[serde(rename = "runs")]
    Run,
    #[serde(rename = "events")]
    Event,
    #[serde(rename = "skills")]
    Skill,
    #[serde(rename = "inflight_prepare")]
    InflightPrepare,
    #[serde(rename = "exploration_nodes")]
    ExplorationNode,
    #[serde(rename = "replay_worlds")]
    ReplayWorld,
    #[serde(rename = "active_leases")]
    ActiveLease,
    #[serde(rename = "staged_packages")]
    StagingPackage,
    #[serde(rename = "concurrent_dispatches")]
    ConcurrentManagementDispatch,
    #[serde(rename = "development_cycles")]
    DevelopmentCycle,
    #[serde(rename = "consolidation_claims")]
    ConsolidationClaim,
    #[serde(rename = "proposals_per_claim")]
    ProposalPerClaim,
    #[serde(rename = "staging_bindings_per_claim")]
    StagingBindingPerClaim,
    #[serde(rename = "practice_attempt_sets")]
    PracticeAttemptSet,
    #[serde(rename = "meta_trials")]
    MetaTrial,
    #[serde(rename = "exploration_worlds")]
    ExplorationWorld,
    #[serde(rename = "optimization_history")]
    OptimizationHistory,
    #[serde(rename = "concurrent_optimization_jobs")]
    ConcurrentOptimizationJob,
    #[serde(rename = "skill_groups_per_job")]
    SkillGroupPerJob,
    #[serde(rename = "curriculum_probe_jobs")]
    CurriculumProbeJob,
    #[serde(rename = "replay_economic_experiments")]
    ReplayEconomicExperiment,
    #[serde(rename = "management_job_records")]
    ManagementJobRecord,
    #[serde(rename = "failure_records")]
    FailureRecord,
    #[serde(rename = "selection_records")]
    SelectionRecord,
    #[serde(rename = "repair_templates")]
    RepairTemplate,
    #[serde(rename = "reset_receipts_per_node")]
    ResetReceiptPerNode,
    #[serde(rename = "external_services")]
    ExternalService,
    #[serde(rename = "root_stage_plan_versions_per_root")]
    RootStagePlanVersionPerRoot,
    #[serde(rename = "host_surfaces")]
    HostSurface,
    #[serde(rename = "direction_hints")]
    DirectionHint,
    #[serde(rename = "trace_exports")]
    TraceExport,
    #[serde(rename = "improver_releases")]
    ImproverRelease,
}

impl CapacityItem {
    pub const ALL: [Self; 32] = [
        Self::Run,
        Self::Event,
        Self::Skill,
        Self::InflightPrepare,
        Self::ExplorationNode,
        Self::ReplayWorld,
        Self::ActiveLease,
        Self::StagingPackage,
        Self::ConcurrentManagementDispatch,
        Self::DevelopmentCycle,
        Self::ConsolidationClaim,
        Self::ProposalPerClaim,
        Self::StagingBindingPerClaim,
        Self::PracticeAttemptSet,
        Self::MetaTrial,
        Self::ExplorationWorld,
        Self::OptimizationHistory,
        Self::ConcurrentOptimizationJob,
        Self::SkillGroupPerJob,
        Self::CurriculumProbeJob,
        Self::ReplayEconomicExperiment,
        Self::ManagementJobRecord,
        Self::FailureRecord,
        Self::SelectionRecord,
        Self::RepairTemplate,
        Self::ResetReceiptPerNode,
        Self::ExternalService,
        Self::RootStagePlanVersionPerRoot,
        Self::HostSurface,
        Self::DirectionHint,
        Self::TraceExport,
        Self::ImproverRelease,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Run => "runs",
            Self::Event => "events",
            Self::Skill => "skills",
            Self::InflightPrepare => "inflight_prepare",
            Self::ExplorationNode => "exploration_nodes",
            Self::ReplayWorld => "replay_worlds",
            Self::ActiveLease => "active_leases",
            Self::StagingPackage => "staged_packages",
            Self::ConcurrentManagementDispatch => "concurrent_dispatches",
            Self::DevelopmentCycle => "development_cycles",
            Self::ConsolidationClaim => "consolidation_claims",
            Self::ProposalPerClaim => "proposals_per_claim",
            Self::StagingBindingPerClaim => "staging_bindings_per_claim",
            Self::PracticeAttemptSet => "practice_attempt_sets",
            Self::MetaTrial => "meta_trials",
            Self::ExplorationWorld => "exploration_worlds",
            Self::OptimizationHistory => "optimization_history",
            Self::ConcurrentOptimizationJob => "concurrent_optimization_jobs",
            Self::SkillGroupPerJob => "skill_groups_per_job",
            Self::CurriculumProbeJob => "curriculum_probe_jobs",
            Self::ReplayEconomicExperiment => "replay_economic_experiments",
            Self::ManagementJobRecord => "management_job_records",
            Self::FailureRecord => "failure_records",
            Self::SelectionRecord => "selection_records",
            Self::RepairTemplate => "repair_templates",
            Self::ResetReceiptPerNode => "reset_receipts_per_node",
            Self::ExternalService => "external_services",
            Self::RootStagePlanVersionPerRoot => "root_stage_plan_versions_per_root",
            Self::HostSurface => "host_surfaces",
            Self::DirectionHint => "direction_hints",
            Self::TraceExport => "trace_exports",
            Self::ImproverRelease => "improver_releases",
        }
    }

    pub const fn limit(self) -> u64 {
        match self {
            Self::Run => 1000,
            Self::Event => 10000,
            Self::Skill => 1000,
            Self::InflightPrepare => 5,
            Self::ExplorationNode => 500,
            Self::ReplayWorld => 100,
            Self::ActiveLease => 10,
            Self::StagingPackage => 20,
            Self::ConcurrentManagementDispatch => 1,
            Self::DevelopmentCycle => 1000,
            Self::ConsolidationClaim => 250,
            Self::ProposalPerClaim => 1,
            Self::StagingBindingPerClaim => 1,
            Self::PracticeAttemptSet => 200,
            Self::MetaTrial => 10,
            Self::ExplorationWorld => 100,
            Self::OptimizationHistory => 1000,
            Self::ConcurrentOptimizationJob => 1,
            Self::SkillGroupPerJob => 2,
            Self::CurriculumProbeJob => 100,
            Self::ReplayEconomicExperiment => 100,
            Self::ManagementJobRecord => 10000,
            Self::FailureRecord => 10000,
            Self::SelectionRecord => 1000,
            Self::RepairTemplate => 100,
            Self::ResetReceiptPerNode => 1,
            Self::ExternalService => 50,
            Self::RootStagePlanVersionPerRoot => 100,
            Self::HostSurface => 100,
            Self::DirectionHint => 100,
            Self::TraceExport => 1000,
            Self::ImproverRelease => 100,
        }
    }

    pub const fn scope(self) -> CapacityScope {
        match self {
            Self::Run => CapacityScope::Instance,
            Self::Event => CapacityScope::Instance,
            Self::Skill => CapacityScope::Instance,
            Self::InflightPrepare => CapacityScope::Instance,
            Self::ExplorationNode => CapacityScope::Instance,
            Self::ReplayWorld => CapacityScope::Instance,
            Self::ActiveLease => CapacityScope::Instance,
            Self::StagingPackage => CapacityScope::Instance,
            Self::ConcurrentManagementDispatch => CapacityScope::Instance,
            Self::DevelopmentCycle => CapacityScope::Instance,
            Self::ConsolidationClaim => CapacityScope::Instance,
            Self::ProposalPerClaim => CapacityScope::Claim,
            Self::StagingBindingPerClaim => CapacityScope::Claim,
            Self::PracticeAttemptSet => CapacityScope::Instance,
            Self::MetaTrial => CapacityScope::Instance,
            Self::ExplorationWorld => CapacityScope::Instance,
            Self::OptimizationHistory => CapacityScope::Instance,
            Self::ConcurrentOptimizationJob => CapacityScope::Instance,
            Self::SkillGroupPerJob => CapacityScope::OptimizationJob,
            Self::CurriculumProbeJob => CapacityScope::Instance,
            Self::ReplayEconomicExperiment => CapacityScope::Instance,
            Self::ManagementJobRecord => CapacityScope::Instance,
            Self::FailureRecord => CapacityScope::Instance,
            Self::SelectionRecord => CapacityScope::Instance,
            Self::RepairTemplate => CapacityScope::Instance,
            Self::ResetReceiptPerNode => CapacityScope::ExplorationNode,
            Self::ExternalService => CapacityScope::Instance,
            Self::RootStagePlanVersionPerRoot => CapacityScope::Root,
            Self::HostSurface => CapacityScope::Instance,
            Self::DirectionHint => CapacityScope::Instance,
            Self::TraceExport => CapacityScope::Instance,
            Self::ImproverRelease => CapacityScope::Instance,
        }
    }

    /// A zero-delta replay or terminal close remains legal at a full bound.
    pub fn admit(self, used: u64, delta: u64) -> Result<()> {
        self.admit_within(self.limit(), used, delta)
    }

    /// The same gate against an explicit bound, which only the legacy pure
    /// capacity API and its event writer still supply.
    pub fn admit_within(self, limit: u64, used: u64, delta: u64) -> Result<()> {
        if delta != 0 && (used > limit || delta > limit - used) {
            return Err(Error::Capacity(CapacityConflict {
                item: self,
                used,
                limit,
                delta,
            }));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(deny_unknown_fields)]
#[error("capacity_conflict item={item} used={used} limit={limit} delta={delta}")]
pub struct CapacityConflict {
    pub item: CapacityItem,
    pub used: u64,
    pub limit: u64,
    pub delta: u64,
}

impl std::fmt::Display for CapacityItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

pub const MAX_RUNS: u64 = CapacityItem::Run.limit();
pub const MAX_EVENTS: u64 = CapacityItem::Event.limit();
pub const MAX_SKILLS: u64 = CapacityItem::Skill.limit();
pub const MAX_PREPARE: u64 = CapacityItem::InflightPrepare.limit();
pub const MAX_EXPLORATION_NODES: u64 = CapacityItem::ExplorationNode.limit();
pub const MAX_REPLAY_WORLDS: u64 = CapacityItem::ReplayWorld.limit();
pub const MAX_ACTIVE_LEASES: u64 = CapacityItem::ActiveLease.limit();
pub const MAX_STAGED_PACKAGES: u64 = CapacityItem::StagingPackage.limit();
pub const MAX_CONCURRENT_DISPATCH: u64 = CapacityItem::ConcurrentManagementDispatch.limit();
pub const MAX_SKILL_GROUPS_PER_JOB: u64 = CapacityItem::SkillGroupPerJob.limit();
